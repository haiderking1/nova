#!/usr/bin/env python3
"""Repackage linux-firmware's GSP .bin files into the TLV images nova-core loads.

nova-core is built against GSP firmware r570.144, the same version linux-firmware
ships as nvidia/<chip>/gsp/*-570.144.bin, but it wants it in the TLV format from
Documentation/gpu/nova/core/tlv.rst. Layouts below follow nouveau, which loads
the .bin files directly (include/nvfw/{fw,hs}.h, nvkm/falcon/fw.c, gsp/rm/r535).

Only Ampere (GA102+) and Ada are handled: Turing/GA100 also need
gen_bootloader.tlv, Hopper+ need fmc.tlv.

usage: mktlv.py <linux-firmware lib/firmware> <out lib/firmware> <version>
"""

import os
import struct
import sys

# chip prefix -> GSP ELF signature section (nouveau gsp/ga102.c, ad102.c)
SIG_SECTIONS = {"ga10": ".fwsignature_ga10x", "ad10": ".fwsignature_ad10x"}


def u32(data, off):
    return struct.unpack_from("<I", data, off)[0]


def tlv(blocks):
    out = bytearray(b"NVFW")
    for tag, value in blocks:
        if isinstance(value, int):
            value = struct.pack("<I", value)
        elif isinstance(value, str):
            value = value.encode("ascii")
        out += tag.encode("ascii") + struct.pack("<I", len(value)) + value
        out += b"\0" * (-len(value) % 4)
    return bytes(out)


def bin_hdr(data):
    """struct nvfw_bin_hdr -> (header_offset, data_offset, data_size)"""
    magic, _ver, _size, header_off, data_off, data_size = struct.unpack_from("<6I", data)
    assert magic == 0x10DE, f"bad bin magic {magic:#x}"
    assert data_off + data_size <= len(data)
    return header_off, data_off, data_size


def elf_sections(data):
    assert data[:4] == b"\x7fELF" and data[4] == 2, "not a 64-bit ELF"
    shoff = struct.unpack_from("<Q", data, 0x28)[0]
    shentsize, shnum, shstrndx = struct.unpack_from("<3H", data, 0x3A)
    headers = [struct.unpack_from("<IIQQQQ", data, shoff + i * shentsize) for i in range(shnum)]
    names_off = headers[shstrndx][4]
    sections = {}
    for name_idx, _type, _flags, _addr, off, size in headers:
        end = data.index(b"\0", names_off + name_idx)
        sections[data[names_off + name_idx:end].decode()] = data[off:off + size]
    return sections


def build_id(sections):
    note = sections.get(".note.gnu.build-id")
    if not note:
        return None
    namesz, descsz, _type = struct.unpack_from("<3I", note)
    desc_off = 12 + namesz + (-namesz % 4)
    return note[desc_off:desc_off + descsz].hex()


def gsp(rm, chip, version):
    """gsp.tlv + gsp.bin from the GSP-RM ELF (nouveau r535_gsp_elf_section)."""
    sections = elf_sections(rm)
    image = sections[".fwimage"]
    sigs = sections[SIG_SECTIONS[chip[:4]]]
    blocks = [("VERS", version), ("FILE", "gsp.bin"), ("SIZE", len(image)), ("SIGN", sigs)]
    if bid := build_id(sections):
        blocks.append(("BLID", bid))
    return tlv(blocks), image


def gsp_bootloader(bl, version):
    """RISC-V bootloader: nvfw_bin_hdr + RM_RISCV_UCODE_DESC (r535_gsp_rm_boot_ctor)."""
    header_off, data_off, data_size = bin_hdr(bl)
    (_ver, _bl_off, _bl_size, _blp_off, _blp_size, _elf_off, _elf_size, app_version,
     manifest_off, _manifest_size, mon_data_off, _mon_data_size, mon_code_off,
     _mon_code_size) = struct.unpack_from("<14I", bl, header_off)
    return tlv([
        ("VERS", version),
        ("CDOF", mon_code_off),
        ("DAOF", mon_data_off),
        ("MFOF", manifest_off),
        ("APPV", app_version),
        ("BLOB", bl[data_off:data_off + data_size]),
    ])


def booter(fw, version):
    """HS v2 booter: nvfw_bin_hdr + nvfw_hs_header_v2 (nvkm_falcon_fw_ctor_hs_v2)."""
    header_off, data_off, data_size = bin_hdr(fw)
    (sig_prod_off, sig_prod_size, patch_loc_off, patch_sig_off, meta_off, _meta_size,
     num_sig_off, load_hdr_off, _load_hdr_size) = struct.unpack_from("<9I", fw, header_off)

    patch_loc = u32(fw, patch_loc_off)
    sig_start = sig_prod_off + u32(fw, patch_sig_off)
    num_sig = u32(fw, num_sig_off)
    fuse_ver, engine_id, ucode_id = struct.unpack_from("<3I", fw, meta_off)
    os_code_off, os_code_size, os_data_off, os_data_size, num_apps = \
        struct.unpack_from("<5I", fw, load_hdr_off)
    assert num_apps >= 1, "booter has no apps"
    app0_off, app0_size = struct.unpack_from("<2I", fw, load_hdr_off + 20)
    assert num_sig >= 1 and sig_prod_size % num_sig == 0
    assert os_data_off <= patch_loc < data_size

    return tlv([
        ("VERS", version),
        ("DAOF", os_data_off),
        ("DASZ", os_data_size),
        ("CDOF", os_code_off),
        ("CDSZ", os_code_size),
        ("PLOC", patch_loc),
        ("FUSE", fuse_ver),
        ("ENID", engine_id),
        ("UCID", ucode_id),
        ("A0CO", app0_off),
        ("A0CS", app0_size),
        ("NSIG", num_sig),
        ("SIGN", fw[sig_start:sig_start + sig_prod_size]),
        ("BLOB", fw[data_off:data_off + data_size]),
    ])


def main(src_root, out_root, version):
    def read(path):
        with open(path, "rb") as f:
            return f.read()

    def write(path, data):
        with open(path, "wb") as f:
            f.write(data)

    nvidia = os.path.join(src_root, "nvidia")
    chips = sorted(c for c in os.listdir(nvidia) if c[:4] in SIG_SECTIONS and c != "ga100")
    done = []
    for chip in chips:
        src = os.path.join(nvidia, chip, "gsp")
        if not os.path.exists(os.path.join(src, f"gsp-{version}.bin")):
            continue
        out = os.path.join(out_root, "nvidia", chip, "gsp")
        os.makedirs(out, exist_ok=True)

        gsp_tlv, gsp_bin = gsp(read(os.path.join(src, f"gsp-{version}.bin")), chip, version)
        write(os.path.join(out, "gsp.tlv"), gsp_tlv)
        write(os.path.join(out, "gsp.bin"), gsp_bin)
        write(os.path.join(out, "gsp_bootloader.tlv"),
              gsp_bootloader(read(os.path.join(src, f"bootloader-{version}.bin")), version))
        for kind in ("booter_load", "booter_unload"):
            write(os.path.join(out, f"{kind}.tlv"),
                  booter(read(os.path.join(src, f"{kind}-{version}.bin")), version))
        done.append(chip)

    if not done:
        sys.exit(f"no {version} GSP firmware found under {nvidia}")
    print("nova firmware written for:", " ".join(done))


if __name__ == "__main__":
    main(*sys.argv[1:])

// SPDX-License-Identifier: GPL-2.0

//! Self-test of compute: the GPU runs shaders on the graphics engine, the second one storing a
//! value to memory, which the CPU checks.

use kernel::{
    device,
    prelude::*,
    ptr::Alignment,
    sizes::SZ_4K, //
};

use crate::{
    driver::Bar0,
    gpu::Chipset,
    gsp::{
        cmdq::Cmdq,
        fifo::compute_c7c0::Qmd,
        rm::device::RmDevice, //
    },
    mm::GpuMm,
};

use super::{
    gr::{
        ComputeChannel,
        GrContext, //
    },
    COMPUTE_CHID,
};

/// Value the storing shader stores.
const VALUE: u32 = 0x600d_cafe;
/// Value the compute engine writes once the storing shader is done, without running instructions:
/// tells a wrong shader from writes not reaching memory.
const RELEASED: u32 = 0x0d07_e000;

// Layout of the test's memory page. Programs are 128-byte aligned, work descriptors 256-byte
// aligned.

/// Offset of the program that only exits; the storing program follows, 128 bytes after.
const EXIT_PROGRAM: u64 = 0x000;
/// Offset of the work descriptor running the program that only exits; that running the storing
/// program follows, 256 bytes after.
const EXIT_QMD: u64 = 0x400;
/// Offset of the word the storing program stores [`VALUE`] to.
const RESULT: u64 = 0xa00;
/// Offset of the word [`RELEASED`] is written to.
const RELEASE: u64 = 0xb00;

/// What the test's memory page is filled with first, to tell the words written since.
const UNWRITTEN: u32 = 0xa5a5_a5a5;

/// Registers the programs use: up to R5, plus 2 the hardware reserves.
const REGISTERS: u8 = 8;

/// Scheduling bits of an instruction (bits 105 to 121, in its last word): stall `delay` cycles
/// before the next instruction, possibly yielding to other warps, without scoreboard barriers.
/// Field positions are those of Mesa's NAK compiler (`set_instr_deps()`).
const fn deps(delay: u32, yld: bool) -> u32 {
    // Delay in bits 108:105, yield in bit 109, no write (112:110) or read (115:113) barrier.
    delay << 9 | (yld as u32) << 13 | 7 << 14 | 7 << 17
}

/// Encodes the GA10x (SM86) instruction `MOV Rd, imm32`, which sets register `rd`.
const fn mov(rd: u32, imm: u32, deps: u32) -> [u32; 4] {
    [0x0000_7802 | rd << 16, imm, 0x0000_0f00, deps]
}

/// Encodes `STG.E [R2.64], Rdata`: stores register `data` at the address in registers R2 and R3.
const fn stg(data: u32, deps: u32) -> [u32; 4] {
    [0x0200_7986, data, 0x0c10_093f, deps]
}

/// `EXIT`: ends the thread.
const EXIT: [u32; 4] = [0x0000_794d, 0, 0x0380_0000, 0x000f_ea00];
/// `BRA .`: loops on itself, after `EXIT`, as compilers end programs.
const BRA_SELF: [u32; 4] = [0x0000_7947, 0xffff_fff0, 0x0383_ffff, 0x000f_c000];

/// The program storing [`VALUE`] at `addr`, scheduled like Mesa's NAK compiler does: the store
/// waits 5 cycles for the `MOV` setting its value, without yielding, which would make the store
/// not wait for it.
// CAST: the address is split into its low and high 32 bits.
fn store_program(addr: u64) -> [[u32; 4]; 6] {
    [
        mov(2, addr as u32, deps(1, false)),
        mov(3, (addr >> 32) as u32, deps(1, false)),
        mov(5, VALUE, deps(5, false)),
        stg(5, deps(1, false)),
        EXIT,
        BRA_SELF,
    ]
}

/// Runs the self-test on a compute channel on the graphics engine of `gr`.
pub(crate) fn run(
    dev: &device::Device<device::Bound>,
    cmdq: &Cmdq<'_>,
    bar: Bar0<'_>,
    mm: &mut GpuMm<'_>,
    chipset: Chipset,
    rm: Option<&RmDevice>,
    gr: &GrContext,
) -> Result {
    let rm = rm.ok_or(ENODEV)?;
    let mut channel = ComputeChannel::new(dev, cmdq, mm, chipset, rm, gr, COMPUTE_CHID)?;
    let result = run_on(dev, bar, mm, &mut channel);
    channel.free(dev, cmdq, mm);

    result
}

fn run_on(
    dev: &device::Device,
    bar: Bar0<'_>,
    mm: &mut GpuMm<'_>,
    channel: &mut ComputeChannel<'_>,
) -> Result {
    channel.init(bar, mm)?;

    let page = mm.alloc_contiguous(SZ_4K, Alignment::new::<SZ_4K>())?;
    let mut fill = KVec::new();
    fill.resize(SZ_4K / size_of::<u32>(), UNWRITTEN, GFP_KERNEL)?;
    mm.write_vram(page.range().start, &fill)?;
    let mapped = channel.map_vram(mm, page.range())?;
    let va = mapped.start().into_raw();
    dev_info!(
        dev,
        "compute test: memory at GPU virtual address {:#x}\n",
        va
    );

    let result = (|| -> Result {
        let base = page.range().start;
        let write =
            |mm: &mut GpuMm<'_>, offset: u64, words: &[u32]| mm.write_vram(base + offset, words);
        let qmd = |program, registers| Qmd::new(va + program, [1, 1, 1], [1, 1, 1], registers);
        let mut program = KVec::new();

        // Exit right away.
        program.extend_from_slice(&EXIT, GFP_KERNEL)?;
        program.extend_from_slice(&BRA_SELF, GFP_KERNEL)?;
        write(mm, EXIT_PROGRAM, &program)?;
        write(mm, EXIT_QMD, qmd(EXIT_PROGRAM, REGISTERS).words())?;
        channel.launch(bar, mm, va + EXIT_QMD)?;
        dev_info!(dev, "compute test: a shader ran\n");

        // Store `VALUE`, then have the engine release `RELEASED` once done.
        let program_offset = EXIT_PROGRAM + 0x80;
        let qmd_offset = EXIT_QMD + 0x100;
        program.clear();
        for instruction in store_program(va + RESULT) {
            program.extend_from_slice(&instruction, GFP_KERNEL)?;
        }
        write(mm, program_offset, &program)?;
        write(
            mm,
            qmd_offset,
            qmd(program_offset, REGISTERS)
                .release(va + RELEASE, RELEASED)?
                .words(),
        )?;

        channel.launch(bar, mm, va + qmd_offset)?;
        let stored = mm.read_vram(base + RESULT)?;
        let released = mm.read_vram(base + RELEASE)?;
        if stored != VALUE || released != RELEASED {
            dev_err!(
                dev,
                "compute test: the shader stored {:#010x} (expected {:#010x}), the engine released {:#010x} (expected {:#010x})\n",
                stored,
                VALUE,
                released,
                RELEASED
            );
            return Err(EIO);
        }
        dev_info!(dev, "compute test: a shader stored {:#010x}\n", stored);

        Ok(())
    })();

    let _ = channel.unmap(mm, mapped);
    result
}

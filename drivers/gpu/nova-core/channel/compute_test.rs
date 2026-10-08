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

/// Value the storing shaders store.
const VALUE: u32 = 0x600d_cafe;
/// Value the compute engine writes once the last shader is done, without running instructions:
/// tells a wrong shader from writes not reaching memory.
const RELEASED: u32 = 0x0d07_e000;

// Layout of the test's memory page. Programs are 128-byte aligned, work descriptors 256-byte
// aligned.

/// Offset of the program that only exits; the storing programs follow, 128 bytes apart.
const EXIT_PROGRAM: u64 = 0x000;
/// Offset of the work descriptor running the program that only exits; those running the storing
/// programs follow, 256 bytes apart.
const EXIT_QMD: u64 = 0x400;
/// Offset of the word the first storing program stores to; the others follow, 16 bytes apart.
const RESULTS: u64 = 0xa00;
/// Offset of the word [`RELEASED`] is written to.
const RELEASE: u64 = 0xb00;

/// What the test's memory page is filled with first, to tell the words written since.
const UNWRITTEN: u32 = 0xa5a5_a5a5;

/// Registers the programs use: up to R5, plus 2 the hardware reserves.
const REGISTERS: u8 = 8;

/// Encodes the GA10x (SM86) instruction `MOV Rd, imm32`, which sets register `rd`.
const fn mov(rd: u32, imm: u32) -> [u32; 4] {
    [0x0000_7802 | rd << 16, imm, 0x0000_0f00, 0x000f_fe00]
}

/// Encodes `STG.E [R2.64], Rdata`: stores register `data` at the address in registers R2 and R3.
const fn stg(data: u32) -> [u32; 4] {
    [0x0200_7986, data, 0x0c10_093f, 0x000f_e200]
}

/// `EXIT`: ends the thread.
const EXIT: [u32; 4] = [0x0000_794d, 0, 0x0380_0000, 0x000f_ea00];
/// `NOP`: does nothing.
const NOP: [u32; 4] = [0x0000_7918, 0, 0, 0x000f_c000];
/// `BRA .`: loops on itself, after `EXIT`, as compilers end programs.
const BRA_SELF: [u32; 4] = [0x0000_7947, 0xffff_fff0, 0x0383_ffff, 0x000f_c000];

/// A storing program the test runs.
struct Variant {
    /// What the variant tells.
    name: &'static str,
    /// The instructions storing, given the address to store to; `EXIT` and `BRA .` follow.
    instructions: fn(u64) -> [[u32; 4]; 4],
    /// Value the program stores, given the address it stores to.
    expected: fn(u64) -> u32,
    /// Registers the program runs with.
    registers: u8,
}

// CAST: addresses are split into their low and high 32 bits.
const VARIANTS: [Variant; 5] = [
    Variant {
        name: "store R2, the address register",
        instructions: |addr| {
            [
                mov(2, addr as u32),
                mov(3, (addr >> 32) as u32),
                stg(2),
                NOP,
            ]
        },
        expected: |addr| addr as u32,
        registers: REGISTERS,
    },
    Variant {
        name: "store R4",
        instructions: |addr| {
            [
                mov(2, addr as u32),
                mov(3, (addr >> 32) as u32),
                mov(4, VALUE),
                stg(4),
            ]
        },
        expected: |_| VALUE,
        registers: REGISTERS,
    },
    Variant {
        name: "store R5, set first",
        instructions: |addr| {
            [
                mov(5, VALUE),
                mov(2, addr as u32),
                mov(3, (addr >> 32) as u32),
                stg(5),
            ]
        },
        expected: |_| VALUE,
        registers: REGISTERS,
    },
    Variant {
        name: "store R5",
        instructions: |addr| {
            [
                mov(2, addr as u32),
                mov(3, (addr >> 32) as u32),
                mov(5, VALUE),
                stg(5),
            ]
        },
        expected: |_| VALUE,
        registers: REGISTERS,
    },
    Variant {
        name: "store R5, with 16 registers",
        instructions: |addr| {
            [
                mov(2, addr as u32),
                mov(3, (addr >> 32) as u32),
                mov(5, VALUE),
                stg(5),
            ]
        },
        expected: |_| VALUE,
        registers: 16,
    },
];

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

        let mut passed = true;
        for (i, variant) in (1u64..).zip(VARIANTS.iter()) {
            let program_offset = EXIT_PROGRAM + 0x80 * i;
            let qmd_offset = EXIT_QMD + 0x100 * i;
            let result = RESULTS + 0x10 * (i - 1);

            program.clear();
            for instruction in (variant.instructions)(va + result) {
                program.extend_from_slice(&instruction, GFP_KERNEL)?;
            }
            program.extend_from_slice(&EXIT, GFP_KERNEL)?;
            program.extend_from_slice(&BRA_SELF, GFP_KERNEL)?;
            write(mm, program_offset, &program)?;
            write(
                mm,
                qmd_offset,
                qmd(program_offset, variant.registers)
                    .release(va + RELEASE, RELEASED + u32::try_from(i)?)?
                    .words(),
            )?;

            channel.launch(bar, mm, va + qmd_offset)?;
            let stored = mm.read_vram(base + result)?;
            let released = mm.read_vram(base + RELEASE)?;
            let expected = (variant.expected)(va + result);
            dev_info!(
                dev,
                "compute test: {}: stored {:#010x} (expected {:#010x}), released {:#010x} (expected {:#010x})\n",
                variant.name,
                stored,
                expected,
                released,
                RELEASED + u32::try_from(i)?
            );
            passed &= stored == expected;
        }

        if !passed {
            return Err(EIO);
        }
        Ok(())
    })();

    let _ = channel.unmap(mm, mapped);
    result
}

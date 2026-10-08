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
    gsp::fifo::compute_c7c0::Qmd,
    mm::GpuMm,
    shader::{
        self,
        Instr, //
    },
};

use super::gr::ComputeChannel;

/// Value the storing shader stores.
const VALUE: u32 = 0x600d_cafe;
/// Value the compute engine writes once the storing shader is done, without running instructions:
/// tells a wrong shader from writes not reaching memory.
const RELEASED: u32 = 0x0d07_e000;

// Layout of the memory page shaders run from. Programs are 128-byte aligned, work descriptors
// 256-byte aligned.

/// Offset of the program.
const PROGRAM: u64 = 0x000;
/// Offset of the work descriptor.
const QMD: u64 = 0x400;
/// Offset of the word the storing program stores [`VALUE`] to.
const RESULT: u64 = 0xa00;
/// Offset of the word [`RELEASED`] is written to.
const RELEASE: u64 = 0xb00;

/// Largest program the page holds, in instructions.
// CAST: the offsets are those of the 4 KiB page.
const MAX_PROGRAM_LEN: usize = (QMD - PROGRAM) as usize / size_of::<Instr>();

/// What the page is filled with first, to tell the words written since.
const UNWRITTEN: u32 = 0xa5a5_a5a5;

/// Size and shape of the work of a program.
pub(crate) struct Grid {
    /// Number of blocks of threads.
    pub(crate) blocks: [u32; 3],
    /// Number of threads of each block.
    pub(crate) threads: [u16; 3],
    /// Number of registers each thread uses.
    pub(crate) registers: u8,
}

impl Grid {
    /// A single thread using `registers` registers.
    const fn single(registers: u8) -> Self {
        Self {
            blocks: [1, 1, 1],
            threads: [1, 1, 1],
            registers,
        }
    }
}

/// Runs the self-test on `channel`, after setting it up for compute work.
pub(crate) fn run(
    dev: &device::Device,
    bar: Bar0<'_>,
    mm: &mut GpuMm<'_>,
    channel: &mut ComputeChannel<'_>,
) -> Result {
    channel.init(bar, mm)?;

    // Exit right away.
    run_program(
        bar,
        mm,
        channel,
        |_| {
            Ok([
                shader::exit().deps(shader::Deps::stall(5)),
                shader::bra_self(),
            ])
        },
        Grid::single(shader::registers(0)),
        |_, _| Ok(()),
    )?;
    dev_info!(dev, "compute test: a shader ran\n");

    // Store `VALUE`, then have the engine release `RELEASED` once done.
    let mut stored = UNWRITTEN;
    run_program(
        bar,
        mm,
        channel,
        |va| Ok(shader::store(va + RESULT, VALUE)),
        Grid::single(shader::STORE_REGISTERS),
        |mm, base| {
            stored = mm.read_vram(base + RESULT)?;
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

            Ok(())
        },
    )?;
    dev_info!(dev, "compute test: a shader stored {:#010x}\n", stored);

    Ok(())
}

/// Runs a program on `channel` as `grid` says, and returns once it is done.
///
/// The program is built by `program` from the GPU virtual address of the memory page it runs
/// from. Once it is done, `check` gets that page's VRAM address.
pub(crate) fn run_program<const N: usize>(
    bar: Bar0<'_>,
    mm: &mut GpuMm<'_>,
    channel: &mut ComputeChannel<'_>,
    program: impl FnOnce(u64) -> Result<[Instr; N]>,
    grid: Grid,
    check: impl FnOnce(&mut GpuMm<'_>, crate::mm::VramAddress) -> Result,
) -> Result {
    const { assert!(N <= MAX_PROGRAM_LEN) };

    let page = mm.alloc_contiguous(SZ_4K, Alignment::new::<SZ_4K>())?;
    let base = page.range().start;
    let mut fill = KVec::new();
    fill.resize(SZ_4K / size_of::<u32>(), UNWRITTEN, GFP_KERNEL)?;
    mm.write_vram(base, &fill)?;
    let mapped = channel.map_vram(mm, page.range())?;
    let va = mapped.start().into_raw();

    let result = (|| -> Result {
        let mut words = KVec::with_capacity(N * 4, GFP_KERNEL)?;
        for instr in program(va)? {
            words.extend_from_slice(instr.words(), GFP_KERNEL)?;
        }
        mm.write_vram(base + PROGRAM, &words)?;

        let qmd = Qmd::new(va + PROGRAM, grid.blocks, grid.threads, grid.registers)
            .release(va + RELEASE, RELEASED)?;
        mm.write_vram(base + QMD, qmd.words())?;

        channel.launch(bar, mm, va + QMD)?;
        check(mm, base)
    })();

    let _ = channel.unmap(mm, mapped);
    result
}

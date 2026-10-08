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
    num,
};

use super::{
    gr::{
        ComputeChannel,
        GrContext, //
    },
    COMPUTE_CHID,
};

/// Value the second shader stores.
const VALUE: u32 = 0x600d_cafe;
/// Value the compute engine writes once the second shader is done, without running instructions:
/// tells a wrong shader from writes not reaching memory.
const RELEASED: u32 = 0x0d07_e000;

// Layout of the test's memory page. Programs are 128-byte aligned, work descriptors 256-byte
// aligned.

/// Offset of the program that only exits.
const EXIT_PROGRAM: u64 = 0x000;
/// Offset of the program storing [`VALUE`].
const STORE_PROGRAM: u64 = 0x080;
/// Offset of the work descriptor running the program that only exits.
const EXIT_QMD: u64 = 0x100;
/// Offset of the work descriptor running the program storing [`VALUE`].
const STORE_QMD: u64 = 0x200;
/// Offset of the word [`VALUE`] is stored to.
const RESULT: u64 = 0x300;
/// Offset of the word [`RELEASED`] is written to.
const RELEASE: u64 = 0x310;

/// What the test's memory page is filled with first, to tell the words written since.
const UNWRITTEN: u32 = 0xa5a5_a5a5;

/// Registers the programs use: up to R5, plus 2 the hardware reserves.
const REGISTERS: u8 = 8;

/// Encodes the GA10x (SM86) instruction `MOV Rd, imm32`, which sets register `rd`.
const fn mov(rd: u32, imm: u32) -> [u32; 4] {
    [0x0000_7802 | rd << 16, imm, 0x0000_0f00, 0x000f_fe00]
}

/// `STG.E [R2.64], R5`: stores register R5 at the address in registers R2 and R3.
const STG_R5_TO_R2: [u32; 4] = [0x0200_7986, 0x0000_0005, 0x0c10_093f, 0x000f_e200];
/// `EXIT`: ends the thread.
const EXIT: [u32; 4] = [0x0000_794d, 0, 0x0380_0000, 0x000f_ea00];
/// `BRA .`: loops on itself, after `EXIT`, as compilers end programs.
const BRA_SELF: [u32; 4] = [0x0000_7947, 0xffff_fff0, 0x0383_ffff, 0x000f_c000];

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

        // Exit right away.
        let mut program = KVec::new();
        program.extend_from_slice(&EXIT, GFP_KERNEL)?;
        program.extend_from_slice(&BRA_SELF, GFP_KERNEL)?;
        write(mm, EXIT_PROGRAM, &program)?;

        // Store `VALUE` at the result's address, then exit.
        let result_va = va + RESULT;
        // CAST: the address is split into its low and high 32 bits.
        program.clear();
        for instruction in [
            mov(2, result_va as u32),
            mov(3, (result_va >> 32) as u32),
            mov(5, VALUE),
            STG_R5_TO_R2,
            EXIT,
            BRA_SELF,
        ] {
            program.extend_from_slice(&instruction, GFP_KERNEL)?;
        }
        write(mm, STORE_PROGRAM, &program)?;

        // A single thread.
        let qmd = |program| Qmd::new(va + program, [1, 1, 1], [1, 1, 1], REGISTERS);
        write(mm, EXIT_QMD, qmd(EXIT_PROGRAM).words())?;
        write(
            mm,
            STORE_QMD,
            qmd(STORE_PROGRAM).release(va + RELEASE, RELEASED)?.words(),
        )?;

        channel.launch(bar, mm, va + EXIT_QMD)?;
        dev_info!(dev, "compute test: a shader ran\n");

        channel.launch(bar, mm, va + STORE_QMD)?;
        let stored = mm.read_vram(base + RESULT)?;
        let released = mm.read_vram(base + RELEASE)?;
        dev_info!(
            dev,
            "compute test: the shader stored {:#010x} (expected {:#010x}), the engine released {:#010x} (expected {:#010x})\n",
            stored,
            VALUE,
            released,
            RELEASED
        );
        if stored != VALUE || released != RELEASED {
            // Where the shader wrote, if it did: the words written past the work descriptors.
            let mut offset = RESULT;
            while offset < num::usize_as_u64(SZ_4K) {
                let word = mm.read_vram(base + offset)?;
                if word != UNWRITTEN {
                    dev_info!(
                        dev,
                        "compute test: word at {:#x} written: {:#010x}\n",
                        offset,
                        word
                    );
                }
                offset += num::usize_as_u64(size_of::<u32>());
            }
            return Err(EIO);
        }

        Ok(())
    })();

    let _ = channel.unmap(mm, mapped);
    result
}

// SPDX-License-Identifier: GPL-2.0

//! Self-test of Nova's own scanout: shows a surface Nova allocated, then goes back to the boot
//! display's framebuffer.
//!
//! With a compute channel, a compute shader draws a pattern on the surface, one thread per pixel.
//! Otherwise, with a copy channel, the copy engine fills it with green, and without either, the
//! CPU fills it with red. What shows on screen thus tells what works.

use kernel::{
    device,
    prelude::*,
    ptr::Alignment,
    sizes::SZ_64K,
    time::{
        delay::fsleep,
        Delta, //
    }, //
};

use crate::{
    boot_display::BootDisplay,
    channel::{compute_test, copy::CopyChannel, gr::ComputeChannel, hal::Grid},
    driver::Bar0,
    mm::{
        GpuMm,
        VramAddress, //
    },
    num,
    shader::{
        self,
        PixelSurface, //
    }, //
};

use super::{
    DisplayEngine,
    Surface, //
};

/// How long the test surface stays on screen.
const SHOW_TIME: Delta = Delta::from_secs(1);

/// Color of the test surface filled by the CPU, in XRGB8888: a full screen of it can't be
/// mistaken for anything else.
const RED: u32 = 0xff0000;
/// Color of the test surface filled by the copy engine.
const GREEN: u32 = 0x00ff00;

/// Shows the test surface for a few seconds, then the boot display's framebuffer again.
pub(crate) fn run(
    dev: &device::Device,
    bar: Bar0<'_>,
    mm: &mut GpuMm<'_>,
    engine: &mut DisplayEngine<'_>,
    boot: &BootDisplay,
    copy: Option<&mut CopyChannel<'_>>,
    compute: Option<&mut ComputeChannel<'_>>,
) -> Result {
    if boot.format.bits_per_pixel != 32 {
        return Err(ENOTSUPP);
    }

    let (width, height) = (boot.width, boot.height);
    let back = Surface {
        addr: boot.vram.start,
        width,
        height,
        pitch: boot.stride,
    };
    // Don't take the screen away unless it can be given back.
    back.check()?;

    let pitch = (width * 4).next_multiple_of(64);
    let fb = mm.alloc_contiguous(
        num::u32_as_usize(pitch) * num::u32_as_usize(height),
        Alignment::new::<SZ_64K>(),
    )?;
    let test = Surface {
        addr: fb.range().start,
        width,
        height,
        pitch,
    };
    test.check()?;

    let drawn = compute.is_some_and(|channel| {
        draw_pattern(bar, mm, channel, fb.range(), &test)
            .inspect_err(|e| {
                dev_err!(
                    dev,
                    "display: a shader can't draw the test surface: {:?}\n",
                    e
                )
            })
            .is_ok()
    });
    let copied = !drawn
        && copy.is_some_and(|channel| {
            copy_fill(bar, mm, channel, fb.range(), &test, GREEN)
                .inspect_err(|e| {
                    dev_err!(
                        dev,
                        "display: the copy engine can't fill the test surface: {:?}\n",
                        e
                    )
                })
                .is_ok()
        });
    if !drawn && !copied {
        let mut line = KVec::with_capacity(num::u32_as_usize(width), GFP_KERNEL)?;
        line.resize(num::u32_as_usize(width), RED, GFP_KERNEL)?;
        for y in 0..height {
            mm.write_vram(test.addr + u64::from(y) * u64::from(pitch), &line)?;
        }
    }

    dev_info!(
        dev,
        "display: showing the scanout test surface ({})\n",
        if drawn {
            "a pattern drawn by a compute shader"
        } else if copied {
            "solid green, filled by the copy engine"
        } else {
            "solid red, filled by the CPU"
        }
    );
    let shown = engine.show(bar, &test);
    if shown.is_ok() {
        fsleep(SHOW_TIME);
    }
    let restored = engine.show(bar, &back);
    // The test surface may be read until the next frame starts.
    fsleep(Delta::from_millis(100));

    match (&shown, &restored) {
        (Ok(()), Ok(())) => dev_info!(dev, "display: scanout test done\n"),
        _ => dev_err!(
            dev,
            "display: scanout test failed: show {:?}, restore {:?}\n",
            shown,
            restored
        ),
    }

    shown.and(restored)
}

/// Draws [`shader::pattern`] on `surface`, which is in the VRAM `range`, with a compute shader on
/// `channel`.
fn draw_pattern(
    bar: Bar0<'_>,
    mm: &mut GpuMm<'_>,
    channel: &mut ComputeChannel<'_>,
    range: core::ops::Range<VramAddress>,
    surface: &Surface,
) -> Result {
    let mapped = channel.map_vram(mm, range)?;
    let target = PixelSurface {
        addr: mapped.start().into_raw(),
        width: surface.width,
        height: surface.height,
        pitch: surface.pitch,
    };
    // One thread per pixel.
    let grid = Grid {
        blocks: [
            surface.width.div_ceil(shader::PATTERN_BLOCK_WIDTH.into()),
            surface.height.div_ceil(shader::PATTERN_BLOCK_HEIGHT.into()),
            1,
        ],
        threads: [shader::PATTERN_BLOCK_WIDTH, shader::PATTERN_BLOCK_HEIGHT, 1],
        registers: shader::PATTERN_REGISTERS,
    };
    let drawn = compute_test::run_program(
        bar,
        mm,
        channel,
        |_| Ok(shader::pattern(&target)),
        grid,
        |_, _| Ok(()),
    );
    let unmapped = channel.unmap(mm, mapped);

    drawn.and(unmapped)
}

/// Fills `surface`, which is in the VRAM `range`, with `color`, using the copy engine of
/// `channel`.
fn copy_fill(
    bar: Bar0<'_>,
    mm: &mut GpuMm<'_>,
    channel: &mut CopyChannel<'_>,
    range: core::ops::Range<VramAddress>,
    surface: &Surface,
    color: u32,
) -> Result {
    let mapped = channel.map_vram(mm, range)?;
    let filled = channel.fill(
        bar,
        mm,
        mapped.start(),
        surface.pitch,
        surface.width,
        surface.height,
        color,
    );
    let unmapped = channel.unmap(mm, mapped);

    filled.and(unmapped)
}

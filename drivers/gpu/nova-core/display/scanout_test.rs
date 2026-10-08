// SPDX-License-Identifier: GPL-2.0

//! Self-test of Nova's own scanout: shows a solid red surface Nova allocated, then goes back to
//! the boot display's framebuffer.

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
    driver::Bar0,
    mm::GpuMm,
    num, //
};

use super::{
    DisplayEngine,
    Surface, //
};

/// How long the test surface stays on screen.
const SHOW_TIME: Delta = Delta::from_secs(10);

/// Color of the test surface, in XRGB8888: a full screen of it can't be mistaken for anything else.
const RED: u32 = 0xff0000;

/// Shows the test surface for a few seconds, then the boot display's framebuffer again.
pub(crate) fn run(
    dev: &device::Device,
    bar: Bar0<'_>,
    mm: &mut GpuMm<'_>,
    engine: &mut DisplayEngine<'_>,
    boot: &BootDisplay,
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

    let mut line = KVec::with_capacity(num::u32_as_usize(width), GFP_KERNEL)?;
    line.resize(num::u32_as_usize(width), RED, GFP_KERNEL)?;
    for y in 0..height {
        mm.write_vram(test.addr + u64::from(y) * u64::from(pitch), &line)?;
    }

    dev_info!(
        dev,
        "display: showing the scanout test surface (solid red)\n"
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

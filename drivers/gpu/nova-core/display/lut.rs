// SPDX-License-Identifier: GPL-2.0

//! Input LUTs of windows.
//!
//! Since nvdisplay 3 (Turing), windows are blended in FP16: a window showing a surface in any other
//! format needs an input LUT converting its colors to FP16, or the display engine rejects its
//! updates (see `EvoFlipC5Common()` in NVIDIA's `nvkms-evo3.c`).

use kernel::{
    prelude::*,
    ptr::Alignment,
    sizes::SZ_4K, //
};

use crate::{
    gsp::disp::window_c67e::ILUT_MODE_DIRECT10,
    mm::{
        ContiguousVram,
        GpuMm,
        VramAddress, //
    },
    num, //
};

/// Number of entries mapping input values in DIRECT10 mode: one per 10-bit value.
const ENTRIES: u32 = 1024;
/// Size of the header in front of the entries, in entries. It's only used in segmented mode.
const HEADER_ENTRIES: u32 = 4;
/// Size of the LUT in entries: the header, the entries mapping input values, and a copy of the
/// last one for interpolation to use.
const SIZE_ENTRIES: u32 = HEADER_ENTRIES + ENTRIES + 1;
/// Size of an entry in bytes: red, green and blue in FP16, and 16 unused bits.
const ENTRY_SIZE: usize = 8;

/// An input LUT in VRAM.
pub(super) struct InputLut {
    vram: ContiguousVram,
}

impl InputLut {
    /// Allocates an input LUT leaving colors as they are.
    pub(super) fn identity(mm: &mut GpuMm<'_>) -> Result<Self> {
        let size = num::u32_as_usize(SIZE_ENTRIES) * ENTRY_SIZE;
        let vram = mm.alloc_contiguous(size.next_multiple_of(SZ_4K), Alignment::new::<SZ_4K>())?;

        let mut words = KVec::with_capacity(size / size_of::<u32>(), GFP_KERNEL)?;
        words.resize(
            num::u32_as_usize(HEADER_ENTRIES) * ENTRY_SIZE / size_of::<u32>(),
            0,
            GFP_KERNEL,
        )?;
        for input in Iterator::chain(0..ENTRIES, [ENTRIES - 1]) {
            let value = u32::from(unorm10_to_f16(input));
            // Red and green, then blue.
            words.push(value | value << 16, GFP_KERNEL)?;
            words.push(value, GFP_KERNEL)?;
        }
        mm.write_vram(vram.range().start, &words)?;

        Ok(Self { vram })
    }

    /// Returns the VRAM address of the LUT, which is 256-byte aligned.
    pub(super) fn addr(&self) -> VramAddress {
        self.vram.range().start
    }

    /// Returns the value of the window's `SET_ILUT_CONTROL` method for the LUT: direct mode,
    /// without interpolation.
    pub(super) fn control(&self) -> u32 {
        SIZE_ENTRIES << 8 | ILUT_MODE_DIRECT10 << 2
    }
}

/// Converts `value`, a 10-bit value standing for `value / 1023`, to FP16, rounding to the nearest.
fn unorm10_to_f16(value: u32) -> u16 {
    const MAX: u32 = 1023;

    if value == 0 {
        return 0;
    }

    // `value / MAX` is `2^-shift * (1 + mantissa / 1024)` with `value << shift` in `MAX..2 * MAX`,
    // and so `1024 * (value << shift) / MAX` in `1024..=2048`.
    let mut shift = 0;
    while value << shift < MAX {
        shift += 1;
    }
    let scaled = (2 * 1024 * (value << shift) + MAX) / (2 * MAX);
    // Rounding up to 2048 makes the mantissa overflow into the exponent, which the addition does.
    let bits = (15 - shift) << 10;

    // CAST: `bits + scaled - 1024` is at most `15 << 10 | 1023`.
    (bits + scaled - 1024) as u16
}

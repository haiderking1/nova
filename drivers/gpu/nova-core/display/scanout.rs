// SPDX-License-Identifier: GPL-2.0

//! Nova's own scanout: two framebuffers in VRAM that the window shows in turns.
//!
//! The CPU draws into the framebuffer that isn't shown, the back buffer, through BAR1, then
//! flips: the window shows the back buffer from the next frame on, and the framebuffer shown
//! before becomes the back buffer. Frames are thus never shown half-drawn.

use kernel::{
    prelude::*,
    ptr::Alignment,
    sizes::SZ_64K, //
};

use crate::{
    driver::Bar0,
    mm::{
        bar_user::BarUser,
        vmm::MappedRange,
        ContiguousVram,
        GpuMm,
        PAGE_SIZE, //
    },
    num, //
};

use super::{
    DisplayEngine,
    Surface, //
};

/// Bytes per pixel of the framebuffers, which are XRGB8888.
const CPP: u32 = 4;

/// A framebuffer of the scanout, mapped into BAR1 for the CPU to draw into.
struct Framebuffer {
    _vram: ContiguousVram,
    mapped: MappedRange,
    surface: Surface,
}

/// Size and layout of the framebuffers of Nova's own scanout, which are XRGB8888.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScanoutMode {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Distance between the starts of two lines, in bytes.
    pub stride: u32,
}

/// Two framebuffers that the window shows in turns.
///
/// The framebuffers must be unmapped with [`Scanout::unmap`].
pub(crate) struct Scanout {
    framebuffers: [Framebuffer; 2],
    /// Index of the back buffer, the framebuffer that isn't shown.
    back: usize,
    mode: ScanoutMode,
}

impl Scanout {
    /// Allocates the framebuffers of a `width` by `height` scanout and maps them into BAR1.
    ///
    /// Nothing is shown until the first [`Scanout::flip`].
    pub(crate) fn new(
        mm: &mut GpuMm<'_>,
        bar_user: &BarUser<'_>,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        // The window reads lines in units of 64 bytes.
        let stride = width
            .checked_mul(CPP)
            .and_then(|line| line.checked_next_multiple_of(64))
            .ok_or(EINVAL)?;
        let mode = ScanoutMode {
            width,
            height,
            stride,
        };

        let first = Self::alloc_framebuffer(mm, bar_user, mode)?;
        let second = match Self::alloc_framebuffer(mm, bar_user, mode) {
            Ok(framebuffer) => framebuffer,
            Err(err) => {
                let _ = bar_user.unmap(mm, first.mapped);
                return Err(err);
            }
        };

        Ok(Self {
            framebuffers: [first, second],
            back: 0,
            mode,
        })
    }

    /// Allocates a framebuffer for `mode` and maps it into BAR1.
    fn alloc_framebuffer(
        mm: &mut GpuMm<'_>,
        bar_user: &BarUser<'_>,
        mode: ScanoutMode,
    ) -> Result<Framebuffer> {
        let size = num::u32_as_usize(mode.stride)
            .checked_mul(num::u32_as_usize(mode.height))
            .and_then(|size| size.checked_next_multiple_of(PAGE_SIZE))
            .ok_or(EINVAL)?;
        // Large pages, like nouveau's framebuffers.
        let vram = mm.alloc_contiguous(size, Alignment::new::<SZ_64K>())?;
        let surface = Surface {
            addr: vram.range().start,
            width: mode.width,
            height: mode.height,
            pitch: mode.stride,
        };
        surface.check()?;

        let mapped = bar_user.map_vram(mm, vram.range())?;

        Ok(Framebuffer {
            _vram: vram,
            mapped,
            surface,
        })
    }

    /// Returns the size and layout of the framebuffers.
    pub(crate) fn mode(&self) -> ScanoutMode {
        self.mode
    }

    /// Writes `data` at `offset` into the back buffer.
    ///
    /// `offset` is in bytes from the start of the framebuffer (see [`ScanoutMode`] for its
    /// layout). `offset` and the length of `data` must be multiples of 4.
    pub(crate) fn write(&self, bar_user: &BarUser<'_>, offset: usize, data: &[u8]) -> Result {
        bar_user.write(&self.framebuffers[self.back].mapped, offset, data)
    }

    /// Shows the back buffer from the next frame on, and returns once the framebuffer shown
    /// before isn't read anymore: it is the back buffer from then on.
    pub(crate) fn flip(&mut self, engine: &mut DisplayEngine<'_>, bar: Bar0<'_>) -> Result {
        engine.show(bar, &self.framebuffers[self.back].surface)?;
        self.back = 1 - self.back;

        Ok(())
    }

    /// Unmaps the framebuffers from BAR1 and frees them: the window must not show them anymore,
    /// e.g. because its channel was freed.
    pub(crate) fn unmap(self, mm: &mut GpuMm<'_>, bar_user: &BarUser<'_>) -> Result {
        let mut result = Ok(());
        for framebuffer in self.framebuffers {
            if let Err(err) = bar_user.unmap(mm, framebuffer.mapped) {
                result = Err(err);
            }
        }

        result
    }
}

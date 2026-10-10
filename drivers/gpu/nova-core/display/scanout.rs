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
    PresentMode,
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
    /// The head that shows the scanout, whose vblanks pace it.
    pub head: u32,
}

/// A rectangle of `width` by `height` XRGB8888 pixels, copied from `(src_x, src_y)` of a buffer to
/// `(dst_x, dst_y)` of what the display shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayCopy {
    /// Column of the first pixel copied.
    pub src_x: u32,
    /// Line of the first pixel copied.
    pub src_y: u32,
    /// Column the first pixel is copied to.
    pub dst_x: u32,
    /// Line the first pixel is copied to.
    pub dst_y: u32,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// A buffer mapped for the copy engine, which copies it to the display.
///
/// Made by [`NovaCoreApi::map_display_source()`], it must be given back to
/// [`NovaCoreApi::unmap_display_source()`] before the buffer is freed.
///
/// [`NovaCoreApi::map_display_source()`]: crate::api::NovaCoreApi::map_display_source
/// [`NovaCoreApi::unmap_display_source()`]: crate::api::NovaCoreApi::unmap_display_source
pub struct DisplaySource {
    /// The buffer's mapping in the address space of the kernel's copy channel.
    pub(crate) mapped: MappedRange,
    /// Size of the buffer in bytes.
    pub(crate) size: u64,
}

impl DisplaySource {
    /// Drops the mapping without tearing it down, for when nova-core is gone, and the address
    /// space of its copy channel with it.
    pub fn forget(self) {
        self.mapped.forget();
    }
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
    /// Allocates the framebuffers of a `width` by `height` scanout, cleared to black, and maps them
    /// into BAR1.
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
            head: super::engine::HEAD,
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

        // Black until drawn into.
        let mut line = KVec::new();
        line.resize(num::u32_as_usize(mode.stride), 0u8, GFP_KERNEL)?;
        for y in 0..num::u32_as_usize(mode.height) {
            if let Err(err) = bar_user.write(&mapped, y * line.len(), &line) {
                let _ = bar_user.unmap(mm, mapped);
                return Err(err);
            }
        }

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

    /// Returns the surface of the back buffer, the framebuffer that isn't shown.
    pub(crate) fn back(&self) -> &Surface {
        &self.framebuffers[self.back].surface
    }

    /// Writes `data` at `offset` into the back buffer.
    ///
    /// `offset` is in bytes from the start of the framebuffer (see [`ScanoutMode`] for its
    /// layout). `offset` and the length of `data` must be multiples of 4.
    pub(crate) fn write(&self, bar_user: &BarUser<'_>, offset: usize, data: &[u8]) -> Result {
        bar_user.write(&self.framebuffers[self.back].mapped, offset, data)
    }

    /// Shows the back buffer from the next frame on, or right away as `present` says, and returns
    /// once the framebuffer shown before isn't read anymore: it is the back buffer from then on.
    pub(crate) fn flip(
        &mut self,
        engine: &mut DisplayEngine<'_>,
        bar: Bar0<'_>,
        present: PresentMode,
    ) -> Result {
        self.show_back(|surface| engine.show(bar, surface, present))
    }

    /// Has `show` show the back buffer, which it returns once the framebuffer shown before isn't
    /// read anymore: it is the back buffer from then on.
    pub(crate) fn show_back<R>(&mut self, show: impl FnOnce(&Surface) -> Result<R>) -> Result<R> {
        let shown = show(&self.framebuffers[self.back].surface)?;
        self.back = 1 - self.back;

        Ok(shown)
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

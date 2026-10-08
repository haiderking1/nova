// SPDX-License-Identifier: GPL-2.0

//! The display the system firmware set up at boot.
//!
//! The firmware (e.g. the UEFI graphics output protocol) sets a mode and leaves a linear
//! framebuffer in VRAM. The display engine keeps scanning it out after nova-core takes the GPU
//! over and boots the GSP, until something sets another mode, so drawing into it shows on screen.

use core::ops::Range;

use kernel::{
    device::Bound,
    pci,
    prelude::*,
    screen_info::{
        PixelFormat,
        ScreenInfo,
        VideoType, //
    },
};

use crate::{
    driver::bar1_resource_index,
    mm::VramAddress, //
};

/// The display the system firmware set up on this GPU.
#[derive(Clone)]
pub(crate) struct BootDisplay {
    /// VRAM holding the framebuffer.
    pub(crate) vram: Range<VramAddress>,
    /// Width of the mode, in pixels.
    pub(crate) width: u32,
    /// Height of the mode, in pixels.
    pub(crate) height: u32,
    /// Distance between the starts of two lines of the framebuffer, in bytes.
    pub(crate) stride: u32,
    /// Pixel layout of the framebuffer.
    pub(crate) format: PixelFormat,
}

impl BootDisplay {
    /// Returns the display the firmware set up on `pdev`, if it set one up there.
    ///
    /// Must be called before the GSP boots, while BAR1 still maps VRAM 1:1.
    pub(crate) fn find(pdev: &pci::Device<Bound>) -> Result<Option<Self>> {
        let si = ScreenInfo::primary();
        if si.video_type() != VideoType::EFI || !si.is_on(pdev) {
            return Ok(None);
        }
        let Some(lfb_base) = si.lfb_base() else {
            return Ok(None);
        };

        // The firmware framebuffer is reached through BAR1, which maps VRAM 1:1 until the GSP
        // boots, so its offset into BAR1 is its VRAM address.
        let bar1_idx = bar1_resource_index(pdev)?;
        let bar1_start = pdev.resource_start(bar1_idx)?;
        let bar1_len = pdev.resource_len(bar1_idx)?;
        let start = lfb_base.checked_sub(bar1_start).ok_or(EINVAL)?;

        let size = u64::from(si.stride())
            .checked_mul(si.height().into())
            .ok_or(EINVAL)?;
        let end = start.checked_add(size).ok_or(EINVAL)?;
        if size == 0 || size > si.lfb_size() || end > bar1_len {
            return Err(EINVAL);
        }

        Ok(Some(Self {
            vram: VramAddress::from_raw(start)..VramAddress::from_raw(end),
            width: si.width(),
            height: si.height(),
            stride: si.stride(),
            format: si.pixel_format()?,
        }))
    }
}

// SPDX-License-Identifier: GPL-2.0

//! The display the system firmware set up.
//!
//! Until a native driver takes over, the boot firmware (e.g. the UEFI graphics output protocol)
//! leaves a mode set and a linear framebuffer being scanned out. [`ScreenInfo`] describes it.
//!
//! C header: [`include/linux/screen_info.h`](srctree/include/linux/screen_info.h)

use core::ptr;

use crate::{
    bindings,
    device::Bound,
    error::{
        from_err_ptr,
        to_result, //
    },
    pci,
    prelude::*, //
};

/// Kind of display the firmware set up (`VIDEO_TYPE_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoType(u32);

impl VideoType {
    /// Framebuffer from the EFI graphics output protocol.
    pub const EFI: Self = Self(bindings::VIDEO_TYPE_EFI);
    /// VESA framebuffer.
    pub const VLFB: Self = Self(bindings::VIDEO_TYPE_VLFB);

    /// Returns whether this kind of display has a linear framebuffer.
    pub fn has_lfb(self) -> bool {
        self == Self::EFI || self == Self::VLFB
    }
}

/// Position and size of a color channel within a pixel, in bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Channel {
    /// Bit offset of the channel.
    pub offset: u8,
    /// Number of bits in the channel; 0 if the pixel doesn't have it.
    pub length: u8,
}

/// Layout of a direct-color pixel in a firmware framebuffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelFormat {
    /// Size of a pixel in bits.
    pub bits_per_pixel: u8,
    /// Alpha channel.
    pub alpha: Channel,
    /// Red channel.
    pub red: Channel,
    /// Green channel.
    pub green: Channel,
    /// Blue channel.
    pub blue: Channel,
}

/// Description of a display set up by the system firmware.
#[derive(Clone, Copy)]
pub struct ScreenInfo(bindings::screen_info);

impl ScreenInfo {
    /// Returns the primary display, the one the firmware set up for the boot console.
    pub fn primary() -> Self {
        // SAFETY: `sysfb_primary_display` is filled in during early boot, before any driver runs,
        // and only read afterwards.
        Self(unsafe { ptr::read(&raw const bindings::sysfb_primary_display.screen) })
    }

    /// Returns the kind of display.
    pub fn video_type(&self) -> VideoType {
        // SAFETY: `self.0` is a valid `struct screen_info`.
        VideoType(unsafe { bindings::screen_info_video_type(&self.0) })
    }

    /// Returns the physical address of the linear framebuffer, if the display has one.
    pub fn lfb_base(&self) -> Option<u64> {
        if !self.video_type().has_lfb() {
            return None;
        }

        // SAFETY: `self.0` is a valid `struct screen_info`.
        Some(unsafe { bindings::__screen_info_lfb_base(&self.0) })
    }

    /// Returns the size of the linear framebuffer in bytes, 0 if the display doesn't have one.
    pub fn lfb_size(&self) -> u64 {
        // SAFETY: `self.0` is a valid `struct screen_info`.
        unsafe { bindings::__screen_info_lfb_size(&self.0, self.video_type().0) }
    }

    /// Returns the width of the display in pixels.
    pub fn width(&self) -> u32 {
        self.0.lfb_width.into()
    }

    /// Returns the height of the display in pixels.
    pub fn height(&self) -> u32 {
        self.0.lfb_height.into()
    }

    /// Returns the distance between the starts of two lines of the framebuffer, in bytes.
    pub fn stride(&self) -> u32 {
        self.0.lfb_linelength.into()
    }

    /// Returns the pixel layout of the framebuffer.
    ///
    /// Fails with `EINVAL` if the firmware didn't describe one, and with `ENOTSUPP` for indexed
    /// (palette) formats.
    pub fn pixel_format(&self) -> Result<PixelFormat> {
        let mut f = bindings::pixel_format::default();

        // SAFETY: `self.0` is a valid `struct screen_info` and `f` is valid for writes.
        to_result(unsafe { bindings::screen_info_pixel_format(&self.0, &mut f) })?;

        if f.indexed {
            return Err(ENOTSUPP);
        }

        // SAFETY: For direct-color formats (`indexed == false`), the channel struct is the active
        // member of the union.
        let channels = unsafe { f.__bindgen_anon_1.__bindgen_anon_1 };
        let channel = |c: bindings::pixel_format__bindgen_ty_1__bindgen_ty_1__bindgen_ty_1| {
            Channel {
                offset: c.offset,
                length: c.length,
            }
        };

        Ok(PixelFormat {
            bits_per_pixel: f.bits_per_pixel,
            alpha: channel(channels.alpha),
            red: channel(channels.red),
            green: channel(channels.green),
            blue: channel(channels.blue),
        })
    }

    /// Returns whether the framebuffer lies in the memory BARs of `pdev`.
    ///
    /// A driver for `pdev` uses this to find out whether it takes over the firmware's display.
    pub fn is_on(&self, pdev: &pci::Device<Bound>) -> bool {
        // SAFETY: `self.0` is a valid `struct screen_info`.
        let owner = unsafe { bindings::screen_info_pci_dev(&self.0) };
        let Ok(owner) = from_err_ptr(owner) else {
            return false;
        };
        if owner.is_null() {
            return false;
        }

        let is_on = ptr::eq(owner, pdev.as_raw());

        // SAFETY: `screen_info_pci_dev()` returned `owner` with a reference held, which we drop.
        unsafe { bindings::pci_dev_put(owner) };

        is_on
    }
}

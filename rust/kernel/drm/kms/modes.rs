// SPDX-License-Identifier: GPL-2.0

//! DRM display modes.
//!
//! TODO: Docs
use bindings;

use crate::types::Opaque;

/// DRM kernel-internal display mode structure.
///
/// This structure contains various resolution and timing information for a given display mode in
/// DRM.
///
/// # Invariants
///
/// - The data layout of this structure is guaranteed to be equivalent to that of `struct
///   drm_display_mode`.
/// - We ensure through our bindings that rust's data aliasing rules are maintained, ensuring it is
///   safe to read any fields inside of `self.inner`.
#[repr(transparent)]
pub struct DisplayMode {
    inner: Opaque<bindings::drm_display_mode>,
}

// SAFETY: Our bindings are thread-safe via our type invariants.
unsafe impl Send for DisplayMode {}
// SAFETY: Our bindings are thread-safe via our type invariants.
unsafe impl Sync for DisplayMode {}

impl DisplayMode {
    /// Convert a raw pointer to a `struct drm_display_mode` into an immutable [`DisplayMode`] ref.
    ///
    /// # SAFETY
    ///
    /// - The caller guarantees that `self_ptr` points to a valid initialized `struct
    ///   drm_display_mode`.
    /// - The caller must ensure that rust's data aliasing rules will not be broken for the lifetime
    ///   of `'a`, e.g. no mutable references may exist while immutable references exist to Self.
    #[inline]
    pub(crate) unsafe fn as_ref<'a>(self_ptr: *const bindings::drm_display_mode) -> &'a Self {
        // SAFETY: The pointer is valid via our safety contract, and the data layout of this struct
        // is equivalent to `Self` via our type invariants.
        unsafe { &*self_ptr.cast() }
    }

    /// Return a raw pointer to the `struct drm_display_mode` contained within this [`DisplayMode`].
    #[inline]
    pub(crate) fn as_raw(&self) -> *const bindings::drm_display_mode {
        self.inner.get().cast_const()
    }

    /// Retrieve the pixel clock for the adjusted display mode in kHz.
    #[inline]
    pub fn crtc_clock(&self) -> i32 {
        // SAFETY: Reading these fields is safe via our type invariants.
        unsafe { (*self.as_raw()).crtc_clock }
    }

    /// Retrieve the start of the vertical sync period for the adjusted display mode.
    #[inline]
    pub fn crtc_vblank_start(&self) -> u16 {
        // SAFETY: Reading these fields is safe via our type invariants.
        unsafe { (*self.as_raw()).crtc_vblank_start }
    }

    /// Retrieve the end of the vertical sync period for the adjusted display mode.
    #[inline]
    pub fn crtc_vblank_end(&self) -> u16 {
        // SAFETY: Reading these fields is safe via our type invariants.
        unsafe { (*self.as_raw()).crtc_vblank_end }
    }

    /// Retrieve the number of vertical scanlines for a full scanout frame in this adjusted display
    /// mode.
    #[inline]
    pub fn crtc_vtotal(&self) -> u16 {
        // SAFETY: Reading these fields is safe via our type invariants.
        unsafe { (*self.as_raw()).crtc_vtotal }
    }

    /// Retrieve the width of the visible area of this display mode, in pixels.
    #[inline]
    pub fn hdisplay(&self) -> u16 {
        // SAFETY: Reading these fields is safe via our type invariants.
        unsafe { (*self.as_raw()).hdisplay }
    }

    /// Retrieve the height of the visible area of this display mode, in pixels.
    #[inline]
    pub fn vdisplay(&self) -> u16 {
        // SAFETY: Reading these fields is safe via our type invariants.
        unsafe { (*self.as_raw()).vdisplay }
    }

    /// Retrieve the pixel clock of this display mode, in kHz.
    #[inline]
    pub fn clock(&self) -> i32 {
        // SAFETY: Reading these fields is safe via our type invariants.
        unsafe { (*self.as_raw()).clock }
    }

    /// Retrieve the first pixel of the horizontal sync pulse of this display mode.
    #[inline]
    pub fn hsync_start(&self) -> u16 {
        // SAFETY: Reading these fields is safe via our type invariants.
        unsafe { (*self.as_raw()).hsync_start }
    }

    /// Retrieve the pixel after the horizontal sync pulse of this display mode.
    #[inline]
    pub fn hsync_end(&self) -> u16 {
        // SAFETY: Reading these fields is safe via our type invariants.
        unsafe { (*self.as_raw()).hsync_end }
    }

    /// Retrieve the total width of a line of this display mode, in pixels.
    #[inline]
    pub fn htotal(&self) -> u16 {
        // SAFETY: Reading these fields is safe via our type invariants.
        unsafe { (*self.as_raw()).htotal }
    }

    /// Retrieve the first line of the vertical sync pulse of this display mode.
    #[inline]
    pub fn vsync_start(&self) -> u16 {
        // SAFETY: Reading these fields is safe via our type invariants.
        unsafe { (*self.as_raw()).vsync_start }
    }

    /// Retrieve the line after the vertical sync pulse of this display mode.
    #[inline]
    pub fn vsync_end(&self) -> u16 {
        // SAFETY: Reading these fields is safe via our type invariants.
        unsafe { (*self.as_raw()).vsync_end }
    }

    /// Retrieve the total number of lines of a frame of this display mode.
    #[inline]
    pub fn vtotal(&self) -> u16 {
        // SAFETY: Reading these fields is safe via our type invariants.
        unsafe { (*self.as_raw()).vtotal }
    }

    /// Return whether the horizontal sync pulse of this display mode is negative.
    #[inline]
    pub fn negative_hsync(&self) -> bool {
        // SAFETY: Reading these fields is safe via our type invariants.
        let flags = unsafe { (*self.as_raw()).flags };

        flags & bindings::DRM_MODE_FLAG_NHSYNC != 0
    }

    /// Return whether the vertical sync pulse of this display mode is negative.
    #[inline]
    pub fn negative_vsync(&self) -> bool {
        // SAFETY: Reading these fields is safe via our type invariants.
        let flags = unsafe { (*self.as_raw()).flags };

        flags & bindings::DRM_MODE_FLAG_NVSYNC != 0
    }

    /// Return whether this display mode is progressive: neither interlaced nor doublescan.
    #[inline]
    pub fn is_progressive(&self) -> bool {
        // SAFETY: Reading these fields is safe via our type invariants.
        let flags = unsafe { (*self.as_raw()).flags };

        flags & (bindings::DRM_MODE_FLAG_INTERLACE | bindings::DRM_MODE_FLAG_DBLSCAN) == 0
    }

    /// Return the refresh rate of this display mode in Hz, rounded, like [`drm_mode_vrefresh()`].
    ///
    /// [`drm_mode_vrefresh()`]: srctree/drivers/gpu/drm/drm_modes.c
    #[inline]
    pub fn vrefresh(&self) -> i32 {
        // SAFETY: `self.as_raw()` points to a valid mode, which `drm_mode_vrefresh()` only reads.
        unsafe { bindings::drm_mode_vrefresh(self.as_raw()) }
    }
}

/// Whether a display pipeline can drive a [`DisplayMode`], see [`enum drm_mode_status`].
///
/// [`enum drm_mode_status`]: srctree/include/drm/drm_modes.h
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModeStatus(bindings::drm_mode_status);

impl ModeStatus {
    /// The mode can be driven.
    pub const OK: Self = Self(bindings::drm_mode_status_MODE_OK);

    /// The mode can't be driven, for an unspecified reason.
    pub const BAD: Self = Self(bindings::drm_mode_status_MODE_BAD);

    /// The pixel clock of the mode is too high.
    pub const CLOCK_HIGH: Self = Self(bindings::drm_mode_status_MODE_CLOCK_HIGH);

    /// The mode is interlaced, which isn't supported.
    pub const NO_INTERLACE: Self = Self(bindings::drm_mode_status_MODE_NO_INTERLACE);

    /// Wrap a status returned by C.
    pub(crate) fn from_raw(status: bindings::drm_mode_status) -> Self {
        Self(status)
    }

    /// Return the status for C.
    pub(crate) fn into_raw(self) -> bindings::drm_mode_status {
        self.0
    }
}

/// A display mode built by a driver, e.g. the fixed mode of a display it can't reprogram.
///
/// Dereferences to [`DisplayMode`].
pub struct OwnedDisplayMode(bindings::drm_display_mode);

// SAFETY: `OwnedDisplayMode` is plain data that is never shared with C while mutable.
unsafe impl Send for OwnedDisplayMode {}
// SAFETY: `OwnedDisplayMode` is only read through shared references.
unsafe impl Sync for OwnedDisplayMode {}

impl OwnedDisplayMode {
    /// Create a mode with only a size, for a display whose timings the driver doesn't control.
    ///
    /// This is the equivalent of [`DRM_SIMPLE_MODE()`]. `width_mm` and `height_mm` are the
    /// physical size of the display, 0 if unknown.
    ///
    /// [`DRM_SIMPLE_MODE()`]: srctree/include/drm/drm_modes.h
    pub fn simple(width: u16, height: u16, width_mm: u16, height_mm: u16) -> Self {
        Self(bindings::drm_display_mode {
            type_: bindings::DRM_MODE_TYPE_DRIVER as u8,
            // Nonzero to pass validation; the timings are not real.
            clock: 1,
            hdisplay: width,
            hsync_start: width,
            hsync_end: width,
            htotal: width,
            vdisplay: height,
            vsync_start: height,
            vsync_end: height,
            vtotal: height,
            width_mm,
            height_mm,
            ..pin_init::zeroed()
        })
    }
}

impl core::ops::Deref for OwnedDisplayMode {
    type Target = DisplayMode;

    fn deref(&self) -> &DisplayMode {
        // SAFETY: `self.0` is a valid, initialized `struct drm_display_mode`, which is only
        // mutated through `&mut self`, so no mutable references exist while `&self` is borrowed.
        unsafe { DisplayMode::as_ref(&self.0) }
    }
}

// SPDX-License-Identifier: GPL-2.0 OR MIT

//! DRM EDID.
//!
//! An EDID (Extended Display Identification Data) is the description a display gives of itself:
//! its name, physical size, supported modes and so on. DRM reads it from the display (see
//! [`ConnectorGuard::read_edid`]) and hands it to the connector the display is plugged into.
//!
//! C header: [`include/drm/drm_edid.h`](srctree/include/drm/drm_edid.h)
//!
//! [`ConnectorGuard::read_edid`]: crate::drm::kms::connector::ConnectorGuard::read_edid

use crate::bindings;
use core::ptr::NonNull;

/// An EDID, as read from a display.
///
/// # Invariants
///
/// `ptr` points to a valid [`struct drm_edid`] allocated by DRM's EDID functions, which this
/// object owns.
///
/// [`struct drm_edid`]: srctree/include/drm/drm_edid.h
pub struct Edid {
    ptr: NonNull<bindings::drm_edid>,
}

impl Edid {
    /// Takes ownership of the [`struct drm_edid`] `ptr`, as returned by DRM's EDID read and
    /// allocation functions, or returns `None` if `ptr` is NULL.
    ///
    /// # Safety
    ///
    /// `ptr` must be NULL or point to a valid [`struct drm_edid`], to be freed with
    /// `drm_edid_free()`, that nothing else owns.
    ///
    /// [`struct drm_edid`]: srctree/include/drm/drm_edid.h
    pub(crate) unsafe fn from_raw(ptr: *const bindings::drm_edid) -> Option<Self> {
        // INVARIANT: By the safety requirements, a non-NULL `ptr` is a valid `struct drm_edid`
        // that we now own.
        NonNull::new(ptr.cast_mut()).map(|ptr| Self { ptr })
    }

    /// Returns the raw `struct drm_edid`.
    pub(crate) fn as_raw(&self) -> *const bindings::drm_edid {
        self.ptr.as_ptr()
    }
}

impl Drop for Edid {
    fn drop(&mut self) {
        // SAFETY: `self.ptr` is a valid `struct drm_edid` owned by `self`, by the type invariants.
        unsafe { bindings::drm_edid_free(self.ptr.as_ptr()) };
    }
}

// SAFETY: A `struct drm_edid` is an immutable copy of the EDID data, not tied to any thread.
unsafe impl Send for Edid {}

// SAFETY: A `struct drm_edid` is never modified after allocation, so it can be shared between
// threads.
unsafe impl Sync for Edid {}

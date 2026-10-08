// SPDX-License-Identifier: GPL-2.0 OR MIT

//! DRM EDID.
//!
//! An EDID (Extended Display Identification Data) is the description a display gives of itself:
//! its name, physical size, supported modes and so on. Drivers read it from the display (usually
//! over DDC), wrap it in an [`Edid`] and hand it to the connector the display is plugged into.
//!
//! C header: [`include/drm/drm_edid.h`](srctree/include/drm/drm_edid.h)

use crate::{
    bindings,
    prelude::*, //
};
use core::ptr::NonNull;

/// An EDID, as read from a display.
///
/// # Invariants
///
/// `ptr` points to a valid [`struct drm_edid`] allocated with `drm_edid_alloc()`, which this
/// object owns.
///
/// [`struct drm_edid`]: srctree/include/drm/drm_edid.h
pub struct Edid {
    ptr: NonNull<bindings::drm_edid>,
}

impl Edid {
    /// Creates an [`Edid`] holding a copy of the raw EDID `data`.
    ///
    /// The data is not validated here: DRM checks it when the EDID is used, and [`Edid::is_valid`]
    /// tells whether it would accept it.
    pub fn new(data: &[u8]) -> Result<Self> {
        // SAFETY: `data` is valid for reads of `data.len()` bytes, which `drm_edid_alloc()` copies.
        let ptr = unsafe { bindings::drm_edid_alloc(data.as_ptr().cast(), data.len()) };

        // INVARIANT: `drm_edid_alloc()` returns either a valid `struct drm_edid` or NULL.
        Ok(Self {
            ptr: NonNull::new(ptr.cast_mut()).ok_or(ENOMEM)?,
        })
    }

    /// Returns whether the EDID is valid: well formed, with valid checksums.
    pub fn is_valid(&self) -> bool {
        // SAFETY: `self.ptr` is a valid `struct drm_edid` by the type invariants.
        unsafe { bindings::drm_edid_valid(self.ptr.as_ptr()) }
    }

    /// Returns the raw `struct drm_edid`.
    pub(crate) fn as_raw(&self) -> *const bindings::drm_edid {
        self.ptr.as_ptr()
    }
}

impl Drop for Edid {
    fn drop(&mut self) {
        // SAFETY: `self.ptr` was allocated with `drm_edid_alloc()` and is owned by `self`, by the
        // type invariants.
        unsafe { bindings::drm_edid_free(self.ptr.as_ptr()) };
    }
}

// SAFETY: A `struct drm_edid` is an immutable copy of the EDID data, not tied to any thread.
unsafe impl Send for Edid {}

// SAFETY: A `struct drm_edid` is never modified after allocation, so it can be shared between
// threads.
unsafe impl Sync for Edid {}

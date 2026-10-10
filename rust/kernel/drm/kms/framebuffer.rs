// SPDX-License-Identifier: GPL-2.0 OR MIT

//! DRM framebuffers.
//!
//! C header: [`include/drm/drm_framebuffer.h`](srctree/include/drm/drm_framebuffer.h)

use crate::{
    drm::{
        gem::IntoGEMObject,
        Device,
        kms::{
            DeviceContext,
            Normal,
            KmsDriver,
            ModeObject,
            Sealed, //
        }, //
    },
    types::*, //
};
use bindings;
use core::{
    marker::*,
    ptr, //
};

/// The main interface for [`struct drm_framebuffer`].
///
/// # Invariants
///
/// - `self.0` is initialized for as long as this object is exposed to users.
/// - This type has an identical data layout to [`struct drm_framebuffer`]
///
/// [`struct drm_framebuffer`]: srctree/include/drm/drm_framebuffer.h
#[repr(transparent)]
pub struct Framebuffer<T: KmsDriver, C: DeviceContext = Normal>(
    Opaque<bindings::drm_framebuffer>,
    PhantomData<(T, C)>,
);

// SAFETY:
// - `self.0` is initialized for as long as this object is exposed to users
// - `base` is initialized by DRM when `self.0` is initialized, thus `raw_mode_obj()` always returns
//   a valid pointer.
unsafe impl<T: KmsDriver, C: DeviceContext> ModeObject<C> for Framebuffer<T, C> {
    type Driver = T;

    fn drm_dev(&self) -> &Device<Self::Driver, C> {
        // SAFETY: `dev` points to an initialized `struct drm_device` for as long as this type is
        // initialized
        unsafe { Device::from_raw((*self.0.get()).dev) }
    }

    fn raw_mode_obj(&self) -> *mut bindings::drm_mode_object {
        // SAFETY: We don't expose Framebuffer<T> to users before its initialized, so `base` is
        // always initialized
        unsafe { &raw mut (*self.0.get()).base }
    }
}

// SAFETY: References to framebuffers are safe to be accessed from any thread after device
// initialization.
unsafe impl<T: KmsDriver> Send for Framebuffer<T> {}
// SAFETY: References to framebuffers are safe to be accessed from any thread after device
// initialization.
unsafe impl<T: KmsDriver> Sync for Framebuffer<T> {}

// For implementing ModeObject
impl<T: KmsDriver, C: DeviceContext> Sealed for Framebuffer<T, C> {}

impl<T: KmsDriver, C: DeviceContext> PartialEq for Framebuffer<T, C> {
    fn eq(&self, other: &Self) -> bool {
        ptr::eq(self.0.get(), other.0.get())
    }
}
impl<T: KmsDriver, C: DeviceContext> Eq for Framebuffer<T, C> {}

impl<T: KmsDriver, C: DeviceContext> Framebuffer<T, C> {
    /// Convert a raw pointer to a `struct drm_framebuffer` into a [`Framebuffer`]
    ///
    /// # Safety
    ///
    /// The caller guarantews that `ptr` points to a initialized `struct drm_framebuffer` for at
    /// least the entire lifetime of `'a`.
    #[inline]
    pub(super) unsafe fn from_raw<'a>(ptr: *const bindings::drm_framebuffer) -> &'a Self {
        // SAFETY: Our data layout is identical to drm_framebuffer
        unsafe { &*ptr.cast() }
    }

    /// Returns the number of color planes of the framebuffer's format.
    fn plane_count(&self) -> usize {
        // SAFETY: `format` is set when the framebuffer is initialized, and points to static data.
        let format = unsafe { &*(*self.0.get()).format };

        usize::from(format.num_planes)
    }

    /// Returns the distance between the starts of two lines of color plane `index`, in bytes, or
    /// [`None`] if the framebuffer's format has no such plane.
    pub fn pitch(&self, index: usize) -> Option<u32> {
        // SAFETY: `pitches` is set when the framebuffer is initialized, and never changes.
        let pitches = unsafe { &(*self.0.get()).pitches };

        pitches.get(index).filter(|_| index < self.plane_count()).copied()
    }

    /// Returns the offset of color plane `index` in its GEM object, in bytes, or [`None`] if the
    /// framebuffer's format has no such plane.
    pub fn offset(&self, index: usize) -> Option<u32> {
        // SAFETY: `offsets` is set when the framebuffer is initialized, and never changes.
        let offsets = unsafe { &(*self.0.get()).offsets };

        offsets.get(index).filter(|_| index < self.plane_count()).copied()
    }

    /// Returns the GEM object holding color plane `index`, or [`None`] if the framebuffer's format
    /// has no such plane.
    pub fn gem_object(&self, index: usize) -> Option<&T::Object> {
        // SAFETY: `obj` is set when the framebuffer is initialized, and holds a reference to each
        // object for as long as the framebuffer lives.
        let objects = unsafe { &(*self.0.get()).obj };
        let obj = *objects.get(index).filter(|_| index < self.plane_count())?;
        if obj.is_null() {
            return None;
        }

        // SAFETY:
        // - Framebuffers are created from the handles of files of `T`, whose objects are all
        //   `T::Object`s: `obj` is embedded within a `T::Object`.
        // - The framebuffer holds a reference to `obj`, so it lives as long as `self`.
        Some(unsafe { <T::Object as IntoGEMObject>::from_raw(obj) })
    }
}

super::impl_aref_for_mode_object!(impl<T: KmsDriver> for Framebuffer<T>);

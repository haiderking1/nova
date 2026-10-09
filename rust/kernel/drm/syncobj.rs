// SPDX-License-Identifier: GPL-2.0 OR MIT

//! DRM synchronization objects.
//!
//! A synchronization object is a container for a fence, which userspace refers to by a handle of
//! its file. Drivers that set [`Driver::FEAT_SYNCOBJ`] give userspace the core ioctls to create,
//! wait for and transfer them; drivers take fences to wait for from them, and give them the
//! fences of the work they run.
//!
//! [`Driver::FEAT_SYNCOBJ`]: crate::drm::Driver::FEAT_SYNCOBJ
//!
//! C header: [`include/drm/drm_syncobj.h`](srctree/include/drm/drm_syncobj.h)

use crate::{
    bindings,
    dma_buf::{
        Fence,
        FenceChain, //
    },
    drm::{
        file::DriverFile,
        File, //
    },
    error::to_result,
    prelude::*,
    sync::aref::{
        ARef,
        AlwaysRefCounted, //
    },
    types::Opaque,
};

use core::ptr::{
    self,
    NonNull, //
};

/// A DRM synchronization object.
///
/// # Invariants
///
/// The inner `struct drm_syncobj` is valid, and refcounted.
#[repr(transparent)]
pub struct SyncObj(Opaque<bindings::drm_syncobj>);

impl SyncObj {
    fn as_raw(&self) -> *mut bindings::drm_syncobj {
        self.0.get()
    }

    /// Looks up the synchronization object of `file` with handle `handle`.
    ///
    /// Fails with `ENOENT` if `file` has none.
    pub fn lookup<T: DriverFile>(file: &File<T>, handle: u32) -> Result<ARef<Self>> {
        // SAFETY: `file.as_raw()` is a valid `struct drm_file`.
        let ptr = unsafe { bindings::drm_syncobj_find(file.as_raw(), handle) };
        let ptr = NonNull::new(ptr.cast::<Self>()).ok_or(ENOENT)?;

        // SAFETY: `drm_syncobj_find()` returned a reference to a valid synchronization object,
        // which `Self` wraps transparently, and whose ownership passes to the `ARef`.
        Ok(unsafe { ARef::from_raw(ptr) })
    }

    /// Returns the fence of the synchronization object of `file` with handle `handle`, at `point`
    /// of its timeline, or its fence if `point` is 0.
    ///
    /// Fails with `ENOENT` if `file` has no such object, or if it has no fence at `point`, and
    /// with `EINVAL` if it has no fence.
    pub fn find_fence<T: DriverFile>(
        file: &File<T>,
        handle: u32,
        point: u64,
    ) -> Result<ARef<Fence>> {
        let mut fence: *mut bindings::dma_fence = ptr::null_mut();

        // SAFETY: `file.as_raw()` is a valid `struct drm_file`, and `fence` a valid location to
        // return the fence to.
        to_result(unsafe {
            bindings::drm_syncobj_find_fence(file.as_raw(), handle, point, 0, &mut fence)
        })?;
        let fence = NonNull::new(fence.cast::<Fence>()).ok_or(EINVAL)?;

        // SAFETY: On success, `drm_syncobj_find_fence()` returned a reference to a valid fence,
        // which `Fence` wraps transparently, and whose ownership passes to the `ARef`.
        Ok(unsafe { ARef::from_raw(fence) })
    }

    /// Replaces the fence of the synchronization object with `fence`, or removes it if `None`.
    pub fn replace_fence(&self, fence: Option<&Fence>) {
        let fence = fence.map_or(ptr::null_mut(), Fence::as_raw);

        // SAFETY: `self.as_raw()` is a valid synchronization object, and `fence` either a valid
        // fence or null, which `drm_syncobj_replace_fence()` takes its own reference to.
        unsafe { bindings::drm_syncobj_replace_fence(self.as_raw(), fence) };
    }

    /// Adds `fence` as `point` of the timeline of the synchronization object, in `chain`.
    pub fn add_point(&self, chain: FenceChain, fence: &Fence, point: u64) {
        // SAFETY: `self.as_raw()` is a valid synchronization object, `chain` an unused node whose
        // ownership passes to `drm_syncobj_add_point()`, and `fence` a valid fence, which it takes
        // its own reference to.
        unsafe {
            bindings::drm_syncobj_add_point(self.as_raw(), chain.into_raw(), fence.as_raw(), point)
        };
    }
}

// SAFETY: The type invariants guarantee that `SyncObj` is always refcounted.
unsafe impl AlwaysRefCounted for SyncObj {
    fn inc_ref(&self) {
        // SAFETY: `self.as_raw()` is a valid synchronization object.
        unsafe { bindings::drm_syncobj_get(self.as_raw()) };
    }

    unsafe fn dec_ref(obj: NonNull<Self>) {
        // SAFETY: The caller holds a reference to the valid synchronization object `obj`, which it
        // gives up.
        unsafe { bindings::drm_syncobj_put(obj.as_ref().as_raw()) };
    }
}

// SAFETY: Synchronization objects are refcounted and locked internally, so they can be used and
// released from any thread.
unsafe impl Send for SyncObj {}
// SAFETY: As for `Send`.
unsafe impl Sync for SyncObj {}

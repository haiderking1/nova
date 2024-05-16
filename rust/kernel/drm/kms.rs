// SPDX-License-Identifier: GPL-2.0 OR MIT

//! KMS driver abstractions for rust.

use crate::{
    drm::{
        Device,
        DeviceContext,
        Early,
        device::EarlyInternal,
        driver::Driver,
        private::Sealed, //
    }, //
    error::to_result,
    prelude::*,
    sync::aref::ARef, //
};
use bindings;
use core::{
    ops::Deref,
    marker::PhantomData,
};

/// The C vtable for a [`Device`].
///
/// This is created internally by DRM.
pub struct ModeConfigOps {
    pub(crate) kms_vtable: bindings::drm_mode_config_funcs,
    pub(crate) kms_helper_vtable: bindings::drm_mode_config_helper_funcs,
}

/// Various device-wide information for a [`Device`] that is provided during initialization.
#[derive(Copy, Clone)]
pub struct ModeConfigInfo {
    /// The minimum (w, h) resolution this driver can support
    pub min_resolution: (u32, u32),
    /// The maximum (w, h) resolution this driver can support
    pub max_resolution: (u32, u32),
    /// The maximum (w, h) cursor size this driver can support
    pub max_cursor: (u32, u32),
    /// The preferred depth for dumb ioctls
    pub preferred_depth: u32,
    /// An optional default fourcc format code to be preferred for clients.
    pub preferred_fourcc: Option<u32>,
}

/// A trait representing a type that can be used for setting up KMS, or a stub.
///
/// For drivers which don't have KMS support, the methods provided by this trait may be stubs. It is
/// implemented internally by DRM.
pub trait KmsImpl: private::KmsImpl {}

pub(crate) mod private {
    use super::*;

    /// Private callback implemented internally by DRM for setting up KMS on a device, or stubbing
    /// the KMS setup for devices which don't have KMS support.
    #[allow(private_interfaces)]
    #[allow(unreachable_pub)]
    pub trait KmsImpl {
        /// The parent driver for this KMS implementation
        type Driver: Driver;

        /// The optional KMS callback operations for this driver.
        const MODE_CONFIG_OPS: Option<ModeConfigOps>;

        /// The callback for using a Driver's callbacks to probe the KMS capabilities and object
        /// topology.
        ///
        /// # Safety
        ///
        /// This function may only be called once.
        #[allow(private_interfaces)]
        unsafe fn setup_kms(
            _drm: &Device<Self::Driver, EarlyInternal>
        ) -> Result<(KmsResources<Self::Driver>, ModeConfigInfo)>
        where
            Self: Sized,
        {
            // TODO before submission: There is definitely a proper way of doing this, but I don't
            // remember what it is.
            build_error!("This should never be reachable")
        }
    }
}

/// The [`DeviceContext`] of a KMS [`Device`] that is currently creating static mode objects.
///
/// Static mode objects are defined as mode objects which can only be created while setting up KMS
/// on a device, as such this is the only [`DeviceContext`] in which such objects may be created.
///
/// # Invariants
///
/// - A [`Device`] in this context is guaranteed to be in the [`probe`] callback.
/// - This [`DeviceContext`] shares all invariants of [`Early`].
///
/// [`probe`]: KmsDriver::probe
pub struct KmsProbe;

impl Sealed for KmsProbe {}
impl DeviceContext for KmsProbe {}

impl<T: KmsDriver> Deref for Device<T, KmsProbe> {
    type Target = Device<T, Early>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: `KmsProbe` shares all invariants of `Early`.
        unsafe { self.assume_ctx() }
    }
}

/// An object in charge of releasing KMS-related resources on [`Drop`].
///
/// # Invariants
///
/// `self.0` is a valid [`Device`] which has KMS fully initialized for the lifetime of this
/// object.
pub struct KmsResources<T: Driver>(ARef<Device<T, EarlyInternal>>);

impl<T: Driver> Drop for KmsResources<T> {
    fn drop(&mut self) {
        // SAFETY: Via our type invariants, we are guaranteed that KMS was fully setup for this
        // device.
        unsafe { bindings::drm_atomic_helper_shutdown(self.0.as_raw()) };
    }
}

// SAFETY: KMS driver teardown can happen from any thread, as long as it is single threaded.
unsafe impl<T: Driver> Send for KmsResources<T> {}

/// A trait which must be implemented by drivers that wish to support KMS
///
/// It should be implemented for the same type that implements [`Driver`]. Drivers which don't
/// support KMS should use [`NoKms<Self>`].
///
/// [`NoKms<Self>`]: NoKms
#[vtable]
pub trait KmsDriver: Driver {
    /// Return a [`ModeConfigInfo`] structure for this [`device::Device`].
    fn mode_config_info(dev: &Device<Self, Early>) -> Result<ModeConfigInfo>
    where
        Self: Sized;

    /// Create mode objects like [`crtc::Crtc`], [`plane::Plane`], etc. for this device
    fn probe(drm: &Device<Self, KmsProbe>) -> Result
    where
        Self: Sized;
}

impl<T: KmsDriver> private::KmsImpl for T {
    type Driver = Self;

    const MODE_CONFIG_OPS: Option<ModeConfigOps> = Some(ModeConfigOps {
        kms_vtable: bindings::drm_mode_config_funcs {
            atomic_check: Some(bindings::drm_atomic_helper_check),
            fb_create: Some(bindings::drm_gem_fb_create),
            mode_valid: None,
            atomic_commit: Some(bindings::drm_atomic_helper_commit),
            get_format_info: None,
            atomic_state_free: None,
            atomic_state_alloc: None,
            atomic_state_clear: None,
        },

        kms_helper_vtable: bindings::drm_mode_config_helper_funcs {
            atomic_commit_setup: None,
            atomic_commit_tail: None,
        },
    });

    unsafe fn setup_kms(
        drm: &Device<Self::Driver, EarlyInternal>
    ) -> Result<(KmsResources<Self::Driver>, ModeConfigInfo)>
    where
        Self: Sized
    {
        let mode_config_info = T::mode_config_info(drm)?;

        // PANIC: `MODE_CONFIG_OPS` is always `Some` in this implementation.
        let ops = T::MODE_CONFIG_OPS.as_ref().unwrap();

        // Fill out the relevant vtables and fill information we got from ModeConfigInfo
        //
        // SAFETY:
        // We do not expose any KMS related DRM methods in the `Uninit` context, ensuring that
        // we have exclusive access to `mode_config` until we call `drmm_mode_config_init()` below.
        unsafe {
            (*drm.as_raw()).mode_config = bindings::drm_mode_config {
                funcs: &ops.kms_vtable,
                helper_private: &ops.kms_helper_vtable,
                min_width: mode_config_info.min_resolution.0,
                min_height: mode_config_info.min_resolution.1,
                max_width: mode_config_info.max_resolution.0,
                max_height: mode_config_info.max_resolution.1,
                cursor_width: mode_config_info.max_cursor.0,
                cursor_height: mode_config_info.max_cursor.1,
                preferred_depth: mode_config_info.preferred_depth,
                ..Default::default()
            };
        };

        // SAFETY: We just setup all of the info required to call this function above.
        to_result(unsafe { bindings::drmm_mode_config_init(drm.as_raw()) })?;

        // INVARIANT:
        // - This is the only location that we cast into the `KmsProbe` context, and the cast only
        //   lasts for as long as `T::probe()`.
        // - We are already in the `EarlyInternal` context which fulfills all invariants of `Early`,
        //   ensuring `KmsProbe` also fulfills all invariants of `Early`.
        // SAFETY: This cast is safe via our type invariants.
        T::probe(unsafe { drm.assume_ctx() })?;

        // TODO: In the future, we should add a hook here for initializing the state of each
        // modesetting object that was created in T::probe(). This must happen _after_ all objects
        // are created, as it is normal for the atomic modesetting state of one object to reference
        // another object.
        //
        // Until we add such a hook, just call drm_mode_config_reset(). This is required to ensure
        // that all atomic states are initialized, otherwise attempting modesets is UB.
        //
        // SAFETY: It is always safe to call this function on a device after drmm_mode_config_init()
        // has been called
        unsafe { bindings::drm_mode_config_reset(drm.as_raw()) };

        Ok((KmsResources(drm.into()), mode_config_info))
    }
}

impl<T: KmsDriver> KmsImpl for T {}

/// A stub implementation of KMS for drivers without KMS support.
///
/// This type can be used with DRM drivers which lack KMS support in order to satisfy the associated
/// `Driver::Kms` trait.
pub struct NoKms<T: Driver>(PhantomData<T>);

impl<T: Driver> private::KmsImpl for NoKms<T> {
    type Driver = T;

    const MODE_CONFIG_OPS: Option<ModeConfigOps> = None;
}

impl<T: Driver> KmsImpl for NoKms<T> {}

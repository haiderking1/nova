// SPDX-License-Identifier: GPL-2.0 OR MIT

//! KMS driver abstractions for rust.

pub mod connector;
pub mod crtc;
pub mod encoder;
pub mod plane;

use crate::{
    container_of,
    drm::{
        DeviceContext,
        Early,
        Normal,
        device::EarlyInternal,
        device::Device,
        driver::Driver,
        private::Sealed, //
    }, //
    error::to_result,
    prelude::*,
    sync::{
        aref::{
            AlwaysRefCounted,
            ARef, //
        },
        Mutex,
        MutexGuard, //
    },
};
use bindings;
use core::{
    marker::PhantomData,
    ptr::{
        self,
        NonNull, //
    }, //
    ops::Deref, //
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
    /// The driver's [`DriverConnector`] implementation.
    ///
    /// TODO: This will be unneeded in the future once we support multiple [`DriverConnector`]
    /// implementations.
    ///
    /// [`DriverConnector`]: connector::DriverConnector
    type Connector: connector::DriverConnector;

    /// The driver's [`DriverPlane`] implementation.
    ///
    /// TODO: This will be unneeded in the future once we support multiple [`DriverPlane`]
    /// implementations.
    ///
    type Plane: plane::DriverPlane;

    /// The driver's [`DriverCrtc`] implementation.
    ///
    /// TODO: This will be unneeded in the future once we support multiple [`DriverCrtc`]
    /// implementations.
    ///
    /// [`DriverCrtc`]: crtc::DriverCrtc
    type Crtc: crtc::DriverCrtc;

    /// The driver's [`DriverEncoder`] implementation.
    ///
    /// TODO: This will be unneeded in the future once we support multiple [`DriverEncoder`]
    /// implementations.
    ///
    /// [`DriverEncoder`]: encoder::DriverEncoder
    type Encoder: encoder::DriverEncoder;

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

impl<T: KmsDriver> Device<T> {
    /// Retrieve a pointer to the mode_config mutex
    #[inline]
    pub(crate) fn mode_config_mutex(&self) -> &Mutex<()> {
        // SAFETY: This lock is initialized for as long as `Device<T>` is exposed to users
        unsafe { Mutex::from_raw(&raw mut (*self.as_raw()).mode_config.mutex) }
    }

    /// Acquire the [`mode_config.mutex`] for this [`Device`].
    #[inline]
    pub fn mode_config_lock(&self) -> ModeConfigGuard<'_, T> {
        // INVARIANT: We're locking mode_config.mutex, fulfilling our invariant that this lock is
        // held throughout ModeConfigGuard's lifetime.
        ModeConfigGuard(self.mode_config_mutex().lock(), PhantomData)
    }
}

/// A modesetting object in DRM.
///
/// This is any type of object where the underlying C object contains a [`struct drm_mode_object`].
///
/// This type is only implemented by the DRM crate itself.
///
/// # Safety
///
/// [`raw_mode_obj()`] must always return a valid pointer to an initialized
/// [`struct drm_mode_object`].
///
/// [`struct drm_mode_object`]: srctree/include/drm/drm_mode_object.h
/// [`raw_mode_obj()`]: ModeObject::raw_mode_obj()
pub unsafe trait ModeObject<C: DeviceContext = Normal>: Sealed {
    /// The parent driver for this [`ModeObject`].
    type Driver: KmsDriver;

    /// Return the [`Device`] for this [`ModeObject`].
    fn drm_dev(&self) -> &Device<Self::Driver, C>;

    /// Return a pointer to the [`struct drm_mode_object`] for this [`ModeObject`].
    ///
    /// [`struct drm_mode_object`]: (srctree/include/drm/drm_mode_object.h)
    fn raw_mode_obj(&self) -> *mut bindings::drm_mode_object;
}

/// A trait for modesetting objects which don't come with their own reference-counting.
///
/// Some [`ModeObject`] types in DRM do not have a reference count. These types are considered
/// "static" and share the lifetime of their parent [`Device`]. To retrieve an owned reference to
/// such types, see [`KmsRef`].
///
/// # Safety
///
/// This trait must only be implemented for modesetting objects which do not have a refcount within
/// their [`struct drm_mode_object`], otherwise [`KmsRef`] can't guarantee the object will stay
/// alive.
///
/// [`struct drm_mode_object`]: (srctree/include/drm/drm_mode_object.h)
pub unsafe trait StaticModeObject<C: DeviceContext = Normal>: ModeObject<C> {}

/// An owned reference to a [`StaticModeObject`].
///
/// Note that since [`StaticModeObject`] types share the lifetime of their parent [`Device`], the
/// parent [`Device`] will stay alive as long as this type exists. Thus, users should be aware that
/// storing a [`KmsRef`] within a [`ModeObject`] is a circular reference.
///
/// # Invariants
///
/// `self.0` points to a valid instance of `T` throughout the lifetime of this type.
pub struct KmsRef<T: StaticModeObject + Send + Sync, C: DeviceContext = Normal>
(
    NonNull<T>,
    PhantomData<C>,
);

// SAFETY: Owned references to DRM device are thread-safe as long as the device is initialized.
unsafe impl<T: StaticModeObject + Send + Sync, C: DeviceContext> Send for KmsRef<T, C> {}

// SAFETY: Owned references to DRM device are thread-safe as long as the device is initialized.
unsafe impl<T: StaticModeObject + Send + Sync, C: DeviceContext> Sync for KmsRef<T, C> {}

impl<T: StaticModeObject + Send + Sync, C: DeviceContext> From<&T> for KmsRef<T, C> {
    fn from(value: &T) -> Self {
        // We will drop the reference we leak here in Drop
        value.drm_dev().inc_ref();

        Self(value.into(), PhantomData)
    }
}

impl<T: StaticModeObject + Send + Sync, C: DeviceContext> Drop for KmsRef<T, C> {
    fn drop(&mut self) {
        // SAFETY: We're reclaiming the reference we leaked in From<&T>
        drop(unsafe { ARef::from_raw(self.drm_dev().into()) })
    }
}

impl<T: StaticModeObject + Send + Sync, C: DeviceContext> Deref for KmsRef<T, C> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: We're guaranteed object will point to a valid object as long as we hold dev
        unsafe { self.0.as_ref() }
    }
}

impl<T: StaticModeObject + Send + Sync, C: DeviceContext> Clone for KmsRef<T, C> {
    fn clone(&self) -> Self {
        self.drm_dev().inc_ref();

        Self(self.0, PhantomData)
    }
}

impl<T: StaticModeObject + Send + Sync, C: DeviceContext> KmsRef<T, C> {
    /// Cast a reference to a static modeset object into an owned [`KmsRef`].
    ///
    /// # Safety
    ///
    /// - The caller guarantees that at least one owned reference is taken out for the respective
    ///   [`Device`] this modesetting object is from.
    /// - The caller guarantees that `ptr` points to a valid instance of `T`.
    #[expect(unused)]
    pub(super) unsafe fn from_raw(ptr: *mut T) -> Self {
        // SAFETY: The caller guarantees that `ptr` points to a valid instance of `T`.
        KmsRef(unsafe { NonNull::new_unchecked(ptr) }, PhantomData)
    }
}

macro_rules! impl_aref_for_mode_object {
    (impl $( < $( $param:ident: $bound:ident ),+ > )? for $type:ty) => {
        // SAFETY: drm_mode_object_get()/put() ensure the type is ref-counted according to the
        // safety contract
        unsafe impl $( < $( $param: $bound ),+ > )? kernel::sync::aref::AlwaysRefCounted for $type {
            #[inline]
            fn inc_ref(&self) {
                // SAFETY: We're guaranteed by the safety contract of `ModeObject` that
                // `raw_mode_obj()` always returns a pointer to an initialized `drm_mode_object`.
                unsafe { kernel::bindings::drm_mode_object_get(self.raw_mode_obj()) }
            }

            #[inline]
            unsafe fn dec_ref(obj: core::ptr::NonNull<Self>) {
                // SAFETY: We're guaranteed by the safety contract of `ModeObject` that
                // `raw_mode_obj()` always returns a pointer to an initialized `drm_mode_object`.
                unsafe { kernel::bindings::drm_mode_object_put(obj.as_ref().raw_mode_obj()) }
            }
        }
    };
}

pub(super) use impl_aref_for_mode_object;

/// A mode config guard.
///
/// This is an exclusive primitive that represents when [`drm_device.mode_config.mutex`] is held - as
/// some modesetting operations (particularly ones related to [`connectors`](connector)) are still
/// protected under this single lock. The lock will be dropped once this object is dropped.
///
/// # Invariants
///
/// - `self.0` is contained within a [`struct drm_mode_config`], which is contained within a
///   [`struct drm_device`].
/// - The [`KmsDriver`] implementation of that [`struct drm_device`] is always `T`.
/// - This type proves that [`drm_device.mode_config.mutex`] is acquired.
///
/// [`struct drm_mode_config`]: (srctree/include/drm/drm_device.h)
/// [`drm_device.mode_config.mutex`]: (srctree/include/drm/drm_device.h)
/// [`struct drm_device`]: (srctree/include/drm/drm_device.h)
pub struct ModeConfigGuard<'a, T: KmsDriver>(MutexGuard<'a, ()>, PhantomData<T>);

impl<'a, T: KmsDriver> ModeConfigGuard<'a, T> {
    /// Construct a new [`ModeConfigGuard`].
    ///
    /// # Safety
    ///
    /// The caller must ensure that [`drm_device.mode_config.mutex`] is acquired.
    ///
    /// [`drm_device.mode_config.mutex`]: (srctree/include/drm/drm_device.h)
    pub(crate) unsafe fn new(drm: &'a Device<T>) -> Self {
        // SAFETY: Our safety contract fulfills the requirements of `MutexGuard::new()`
        // INVARIANT: And our safety contract ensures that this type proves that
        // `drm_device.mode_config.mutex` is acquired.
        Self(
            unsafe { MutexGuard::new(drm.mode_config_mutex(), ()) },
            PhantomData,
        )
    }

    /// Return the [`Device`] that this [`ModeConfigGuard`] belongs to.
    pub fn drm_dev(&self) -> &'a Device<T> {
        // TODO: We should probably consider just adding some sort of as_raw() function to locks for
        // situations like this. For now, just do this by hand.
        // LYUDE SHOULD HAVE DONE THIS BEFORE PATCH SUBMISSION
        // IF SHE DIDN'T, BUG HER.
        let lock: *mut bindings::mutex = ptr::from_ref(self.0.lock_ref()).cast_mut().cast();

        // SAFETY:
        // - `self` is embedded within a `drm_mode_config` via our type invariants
        // - `self.0.lock` has an equivalent data type to `mutex` via its type invariants.
        let mode_config = unsafe { container_of!(lock, bindings::drm_mode_config, mutex) };

        // SAFETY: And that `drm_mode_config` lives in a `drm_device` via type invariants.
        unsafe {
            Device::from_raw(container_of!(
                mode_config,
                bindings::drm_device,
                mode_config
            ))
        }
    }

    /// Assert that the given device is the owner of this mode config guard.
    ///
    /// # Panics
    ///
    /// Panics if `dev` is different from the owning device for this mode config guard.
    #[inline]
    pub(crate) fn assert_owner(&self, dev: &Device<T>) {
        assert!(ptr::eq(self.drm_dev(), dev));
    }
}

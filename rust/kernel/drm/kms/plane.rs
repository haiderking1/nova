// SPDX-License-Identifier: GPL-2.0 OR MIT

//! DRM display planes.
//!
//! C header: [`include/drm/drm_plane.h`](srctree/include/drm/drm_plane.h)

use super::{
    KmsDriver,
    ModeObject,
    Sealed,
    StaticModeObject, //
};
use crate::{
    alloc::KBox,
    bindings,
    container_of,
    drm::{
        device::Device,
        fourcc::*,
        Normal,
        DeviceContext,
        kms::{
            atomic::*,
            crtc::*,
            framebuffer::*,
            KmsProbe,
            ModeObjectVtable, //
        },
    },
    error::{
        from_result,
        to_result,
        Error, //
    },
    prelude::*,
    types::Opaque, //
};
use core::{
    cell::Cell,
    marker::*,
    mem::ManuallyDrop,
    ops::*,
    pin::Pin,
    ptr::{
        null,
        null_mut,
        NonNull, //
    },
};

/// The main trait for implementing the [`struct drm_plane`] API for [`Plane`].
///
/// Any KMS driver should have at least one implementation of this type, which allows them to create
/// [`Plane`] objects. Additionally, a driver may store driver-private data within the type that
/// implements [`DriverPlane`] - and it will be made available when using a fully typed [`Plane`]
/// object.
///
/// # Invariants
///
/// - Any C FFI callbacks generated using this trait are guaranteed that passed-in
///   [`struct drm_plane`] pointers are contained within a [`Plane<Self>`].
/// - Any C FFI callbacks generated using this trait are guaranteed that passed-in
///   [`struct drm_plane_state`] pointers are contained within a [`PlaneState<Self::State>`].
///
/// [`struct drm_plane`]: srctree/include/drm/drm_plane.h
/// [`struct drm_plane_state`]: srctree/include/drm/drm_plane.h
#[vtable]
pub trait DriverPlane: Send + Sync + Sized {
    /// The generated C vtable for this [`DriverPlane`] implementation.
    const OPS: &'static DriverPlaneOps = &DriverPlaneOps {
        funcs: bindings::drm_plane_funcs {
            // This tree still has `reset`; `drm_mode_config_reset()` uses `atomic_create_state`
            // when it's unset.
            reset: None,
            atomic_create_state: Some(atomic_create_state_callback::<Self::State>),
            update_plane: Some(bindings::drm_atomic_helper_update_plane),
            disable_plane: Some(bindings::drm_atomic_helper_disable_plane),
            destroy: Some(plane_destroy_callback::<Self>),
            set_property: None,
            atomic_duplicate_state: Some(atomic_duplicate_state_callback::<Self::State>),
            atomic_destroy_state: Some(atomic_destroy_state_callback::<Self::State>),
            atomic_set_property: None,
            atomic_get_property: None,
            late_register: None,
            early_unregister: None,
            atomic_print_state: None,
            format_mod_supported: None,
            format_mod_supported_async: None,
        },

        helper_funcs: bindings::drm_plane_helper_funcs {
            prepare_fb: None,
            cleanup_fb: None,
            begin_fb_access:
                <<Self::State as DriverPlaneState>::Kind as PlaneStateKind>::Raw::BEGIN_FB_ACCESS,
            end_fb_access:
                <<Self::State as DriverPlaneState>::Kind as PlaneStateKind>::Raw::END_FB_ACCESS,
            atomic_check: if Self::HAS_ATOMIC_CHECK {
                Some(atomic_check_callback::<Self>)
            } else {
                None
            },
            atomic_update: if Self::HAS_ATOMIC_UPDATE {
                Some(atomic_update_callback::<Self>)
            } else {
                None
            },
            atomic_enable: None,
            atomic_disable: None,
            atomic_async_check: None,
            atomic_async_update: None,
            panic_flush: None,
            get_scanout_buffer: None,
        },
    };

    /// The type to pass to the `args` field of [`UnregisteredPlane::new`].
    ///
    /// This type will be made available in in the `args` argument of [`Self::new`]. Drivers which
    /// don't need this can simply pass [`()`] here.
    type Args;

    /// The parent [`KmsDriver`] implementation.
    type Driver: KmsDriver;

    /// The [`DriverPlaneState`] implementation for this [`DriverPlane`].
    ///
    /// See [`DriverPlaneState`] for more info.
    type State: DriverPlaneState;

    /// The constructor for creating a [`Plane`] using this [`DriverPlane`] implementation.
    ///
    /// Drivers may use this to instantiate their [`DriverPlane`] object.
    fn new(device: &Device<Self::Driver, KmsProbe>, args: Self::Args) -> impl PinInit<Self, Error>;

    /// The optional [`drm_plane_helper_funcs.atomic_update`] hook for this plane.
    ///
    /// Drivers may use this to customize the atomic update phase of their [`Plane`] objects. If not
    /// specified, this function is a no-op.
    ///
    /// [`drm_plane_helper_funcs.atomic_update`]: srctree/include/drm/drm_modeset_helper_vtables.h
    fn atomic_update(_commit: PlaneAtomicCommit<'_, Self>) {
        build_error::build_error("This should not be reachable")
    }

    /// The optional [`drm_plane_helper_funcs.atomic_check`] hook for this plane.
    ///
    /// Drivers may use this to customize the atomic check phase of their [`Plane`] objects. The
    /// result of this function determines whether the atomic check passed or failed.
    ///
    /// [`drm_plane_helper_funcs.atomic_check`]: srctree/include/drm/drm_modeset_helper_vtables.h
    fn atomic_check(_check: PlaneAtomicCheck<'_, Self>) -> Result {
        build_error::build_error("This should not be reachable")
    }
}

/// The generated C vtable for a [`DriverPlane`].
///
/// This type is created internally by DRM.
pub struct DriverPlaneOps {
    funcs: bindings::drm_plane_funcs,
    helper_funcs: bindings::drm_plane_helper_funcs,
}

/// An enumerator describing a type of [`Plane`].
///
/// This is mainly just relevant for DRM legacy drivers.
///
/// # Invariants
///
/// This type is identical to [`enum drm_plane_type`].
///
/// [`enum drm_plane_type`]: srctree/include/drm/drm_plane.h
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Type {
    /// Overlay planes represent all non-primary, non-cursor planes. Some drivers refer to these
    /// types of planes as "sprites" internally.
    Overlay = bindings::drm_plane_type_DRM_PLANE_TYPE_OVERLAY,

    /// A primary plane attached to a CRTC that is the most likely to be able to light up the CRTC
    /// when no scaling/cropping is used, and the plane covers the whole CRTC.
    Primary = bindings::drm_plane_type_DRM_PLANE_TYPE_PRIMARY,

    /// A cursor plane attached to a CRTC that is more likely to be enabled when no scaling/cropping
    /// is used, and the framebuffer has the size indicated by [`ModeConfigInfo::max_cursor`].
    ///
    /// [`ModeConfigInfo::max_cursor`]: crate::drm::kms::ModeConfigInfo
    Cursor = bindings::drm_plane_type_DRM_PLANE_TYPE_CURSOR,
}

/// The main interface for a [`struct drm_plane`].
///
/// This type is the main interface for dealing with DRM planes. In addition, it also allows
/// immutable access to whatever private data is contained within an implementor's [`DriverPlane`]
/// type.
///
/// # Invariants
///
/// - `plane` and `inner` are initialized for as long as this object is made available to users.
/// - The data layout of this structure begins with [`struct drm_plane`].
/// - The atomic state for this type can always be assumed to be of type [`PlaneState<T::State>`].
///
/// [`struct drm_plane`]: srctree/include/drm/drm_plane.h
#[repr(C)]
#[pin_data]
pub struct Plane<T: DriverPlane, C: DeviceContext = Normal> {
    /// The FFI drm_plane object
    plane: Opaque<bindings::drm_plane>,
    /// The driver's private inner data
    #[pin]
    inner: T,
    _ctx: PhantomData<C>,
}

impl<T: DriverPlane, C: DeviceContext> Sealed for Plane<T, C> {}

impl<T: DriverPlane, C: DeviceContext> Deref for Plane<T, C> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

// SAFETY: `funcs` is initialized when the plane is allocated
unsafe impl<T: DriverPlane, C: DeviceContext> ModeObjectVtable for Plane<T, C> {
    type Vtable = bindings::drm_plane_funcs;

    fn vtable(&self) -> *const Self::Vtable {
        // SAFETY: `as_raw()` always returns a valid plane pointer
        unsafe { *self.as_raw() }.funcs
    }
}

impl<T: DriverPlane, C: DeviceContext> Plane<T, C> {
    super::impl_from_opaque_mode_obj! {
        fn <'a, D>(&'a OpaquePlane<D, C>) -> &'a Self;
        use
            T as DriverPlane,
            D as KmsDriver<Plane = ...>
    }
}

impl<T: DriverPlane> Plane<T, KmsProbe> {
    /// Construct a new [`UnregisteredPlane`].
    ///
    /// A driver may use this from their [`KmsDriver::create_objects`] callback in order to
    /// construct new [`UnregisteredPlane`] objects.
    ///
    /// [`KmsDriver::create_objects`]: kernel::drm::kms::KmsDriver::create_objects
    pub fn new<'a, 'b: 'a>(
        dev: &'a Device<T::Driver, KmsProbe>,
        possible_crtcs: u32,
        formats: &[u32],
        format_modifiers: Option<&[u64]>,
        type_: Type,
        name: Option<&CStr>,
        args: T::Args,
    ) -> Result<&'b Self> {
        let this: Pin<KBox<Plane<T, _>>> = KBox::try_pin_init(
            try_pin_init!(Plane {
                plane: Opaque::new(bindings::drm_plane {
                    helper_private: &T::OPS.helper_funcs,
                    ..Default::default()
                }),
                inner <- T::new(dev, args),
                _ctx: PhantomData::<KmsProbe>,
            }),
            GFP_KERNEL,
        )?;

        // TODO: Move this over to using collect() someday
        // Create a modifiers array with the sentinel for passing to DRM
        let format_modifiers_raw;
        if let Some(modifiers) = format_modifiers {
            let mut raw = KVec::with_capacity(modifiers.len() + 1, GFP_KERNEL)?;
            for modifier in modifiers {
                raw.push(*modifier, GFP_KERNEL)?;
            }
            raw.push(FORMAT_MOD_INVALID, GFP_KERNEL)?;

            format_modifiers_raw = Some(raw);
        } else {
            format_modifiers_raw = None;
        }

        // SAFETY:
        // - `dev` handles destroying the plane, and thus will outlive us and always be valid.
        // - We just allocated `this`, and we won't move it since it's pinned
        // - We just allocated the `format_modifiers_raw` vec and added the sentinel DRM expects
        //   above
        // - `drm_universal_plane_init` will memcpy() the following parameters into its own storage,
        //   so it's safe for them to become inaccessible after this call returns:
        //   - `formats`
        //   - `format_modifiers_raw`
        //   - `name`
        // - `type_` is equivalent to `drm_plane_type` via its type invariants.
        to_result(unsafe {
            bindings::drm_universal_plane_init(
                dev.as_raw(),
                this.as_raw(),
                possible_crtcs,
                &T::OPS.funcs,
                formats.as_ptr(),
                formats.len().try_into().map_err(|_| EINVAL)?,
                format_modifiers_raw.map_or(null(), |f| f.as_ptr()),
                type_ as u32,
                name.map_or(null(), |n| n.as_char_ptr()),
            )
        })?;

        // SAFETY: We don't move anything
        let this = unsafe { Pin::into_inner_unchecked(this) };

        // We'll re-assemble the box in plane_destroy_callback()
        let this = KBox::into_raw(this);

        // UnregisteredPlane has an equivalent data layout
        // TODO: fix this stop using this as the thing just use container_of
        let this: *mut Self = this.cast();

        // SAFETY: We just allocated the plane above, so this pointer must be valid
        Ok(unsafe { &*this })
    }
}

/// A trait implemented by any type that acts as a [`struct drm_plane`] interface.
///
/// This is implemented internally by DRM.
///
/// # Safety
///
/// [`as_raw()`] must always return a valid pointer to an initialized [`struct drm_plane`].
///
/// [`struct drm_plane`]: srctree/include/drm/drm_plane.h
/// [`as_raw()`]: AsRawPlane::as_raw()
pub unsafe trait AsRawPlane<C: DeviceContext = Normal> {
    /// The type used to represent an atomic state for this [`Plane`].
    type State: AsRawPlaneState + FromRawPlaneState;

    /// Return the raw `bindings::drm_plane` for this DRM plane.
    ///
    /// Drivers should never use this directly.
    fn as_raw(&self) -> *mut bindings::drm_plane;

    /// Convert a raw `bindings::drm_plane` pointer into an object of this type.
    ///
    /// # Safety
    ///
    /// Callers promise that `ptr` points to a valid instance of this type
    unsafe fn from_raw<'a>(ptr: *mut bindings::drm_plane) -> &'a Self;
}

// SAFETY:
// - Via our type variants our data layout starts with `drm_plane`
// - Since we don't expose `plane` to users before it has been initialized, this and our data
//   layout ensure that `as_raw()` always returns a valid pointer to a `drm_plane`.
unsafe impl<T: DriverPlane, C: DeviceContext> AsRawPlane<C> for Plane<T, C> {
    type State = PlaneState<T::State>;

    #[inline]
    fn as_raw(&self) -> *mut bindings::drm_plane {
        self.plane.get()
    }

    #[inline]
    unsafe fn from_raw<'a>(ptr: *mut bindings::drm_plane) -> &'a Self {
        // Our data layout start with `bindings::drm_plane`.
        let ptr: *mut Self = ptr.cast();

        // SAFETY: Our safety contract requires that `ptr` point to a valid intance of `Self`.
        unsafe { &*ptr }
    }
}

// SAFETY: We don't expose Plane<T> to users before `base` is initialized in ::new(), so
// `raw_mode_obj` always returns a valid pointer to a bindings::drm_mode_object.
unsafe impl<T: DriverPlane, C: DeviceContext> ModeObject<C> for Plane<T, C> {
    type Driver = T::Driver;

    #[inline]
    fn drm_dev(&self) -> &Device<Self::Driver, C> {
        // SAFETY: DRM planes exist for as long as the device does, so this pointer is always valid
        unsafe { Device::from_raw((*self.as_raw()).dev) }
    }

    #[inline]
    fn raw_mode_obj(&self) -> *mut bindings::drm_mode_object {
        // SAFETY: We don't expose DRM planes to users before `base` is initialized
        unsafe { &raw mut (*self.as_raw()).base }
    }
}

// SAFETY: `Plane`s do not have a reference count of their own.
unsafe impl<T: DriverPlane, C: DeviceContext> StaticModeObject<C> for Plane<T, C> {}

// SAFETY: Our interface is thread-safe once it is fully initialized.
unsafe impl<T: DriverPlane> Send for Plane<T> {}

// SAFETY: Our interface is thread-safe once it is fully initialized.
unsafe impl<T: DriverPlane> Sync for Plane<T> {}

/// Common methods available on any type which implements [`AsRawPlane`].
///
/// This is implemented internally by DRM, and provides many of the basic methods for working with
/// planes.
pub trait RawPlane<C: DeviceContext = Normal>: AsRawPlane<C> {
    /// Return the index of this DRM plane
    #[inline]
    fn index(&self) -> u32 {
        // SAFETY:
        // - The index is initialized by the time we expose planes to users, and does not change
        //   throughout its lifetime
        // - `.as_raw()` always returns a valid poiinter.
        unsafe { *self.as_raw() }.index
    }

    /// Return the index of this DRM plane in the form of a bitmask
    #[inline]
    fn mask(&self) -> u32 {
        1 << self.index()
    }
}
impl<T: AsRawPlane<C>, C: DeviceContext> RawPlane<C> for T {}

/// A [`struct drm_plane`] without a known [`DriverPlane`] implementation.
///
/// This is mainly for situations where our bindings can't infer the [`DriverPlane`] implementation
/// for a [`struct drm_plane`] automatically. It is identical to [`Plane`], except that it does not
/// provide access to the driver's private data.
///
/// It may be upcasted to a full [`Plane`] using [`Plane::from_opaque`] or
/// [`Plane::try_from_opaque`].
///
/// # Invariants
///
/// - `plane` is initialized for as long as this object is made available to users.
/// - The data layout of this structure is equivalent to [`struct drm_plane`].
///
/// [`struct drm_plane`]: srctree/include/drm/drm_plane.h
#[repr(transparent)]
pub struct OpaquePlane<T: KmsDriver, C: DeviceContext = Normal> {
    plane: Opaque<bindings::drm_plane>,
    _p: PhantomData<(T, C)>,
}

impl<T: KmsDriver, C: DeviceContext> Sealed for OpaquePlane<T, C> {}

// SAFETY:
// * Via our type variants our data layout is identical to `drm_plane`
// * Since we don't expose `plane` to users before it has been initialized, this and our data
//   layout ensure that `as_raw()` always returns a valid pointer to a `drm_plane`.
unsafe impl<T: KmsDriver, C: DeviceContext> AsRawPlane<C> for OpaquePlane<T, C> {
    type State = OpaquePlaneState<T>;

    fn as_raw(&self) -> *mut bindings::drm_plane {
        self.plane.get()
    }

    unsafe fn from_raw<'a>(ptr: *mut bindings::drm_plane) -> &'a Self {
        // SAFETY: Our data layout is identical to `bindings::drm_plane`
        unsafe { &*ptr.cast() }
    }
}

// SAFETY: We don't expose OpaquePlane<T> to users before `base` is initialized in
// Plane::<T>::new(), so `raw_mode_obj` always returns a valid pointer to a
// bindings::drm_mode_object.
unsafe impl<T: KmsDriver, C: DeviceContext> ModeObject<C> for OpaquePlane<T, C> {
    type Driver = T;

    fn drm_dev(&self) -> &Device<Self::Driver, C> {
        // SAFETY: DRM planes exist for as long as the device does, so this pointer is always valid
        unsafe { Device::from_raw((*self.as_raw()).dev) }
    }

    fn raw_mode_obj(&self) -> *mut bindings::drm_mode_object {
        // SAFETY: We don't expose DRM planes to users before `base` is initialized
        unsafe { &raw mut (*self.as_raw()).base }
    }
}

// SAFETY: `OpaquePlane`s do not have a reference count of their own.
unsafe impl<T: KmsDriver, C: DeviceContext> StaticModeObject<C> for OpaquePlane<T, C> {}

// SAFETY: `funcs` is initialized when the plane is allocated
unsafe impl<T: KmsDriver, C: DeviceContext> ModeObjectVtable for OpaquePlane<T, C> {
    type Vtable = bindings::drm_plane_funcs;

    fn vtable(&self) -> *const Self::Vtable {
        // SAFETY: `as_raw()` always returns a valid pointer to a plane
        unsafe { *self.as_raw() }.funcs
    }
}

// SAFETY: Our plane interfaces are guaranteed to be thread-safe once fully initialized.
unsafe impl<T: KmsDriver> Send for OpaquePlane<T> {}
// SAFETY: Our plane interfaces are guaranteed to be thread-safe once fully initialized.
unsafe impl<T: KmsDriver> Sync for OpaquePlane<T> {}

/// A trait implemented by any type which can produce a reference to a [`struct drm_plane_state`].
///
/// This is implemented internally by DRM.
///
/// [`struct drm_plane_state`]: srctree/include/drm/drm_plane.h
pub trait AsRawPlaneState: private::AsRawPlaneState {
    /// The type that this plane state interface returns to represent the parent DRM plane
    type Plane: AsRawPlane;
}

pub(crate) mod private {
    /// Trait for retrieving references to the base plane state contained within any plane state
    /// compatible type
    #[allow(unreachable_pub)]
    pub trait AsRawPlaneState {
        /// Return an immutable reference to the raw plane state
        fn as_raw(&self) -> &bindings::drm_plane_state;

        /// Get a mutable reference to the raw `bindings::drm_plane_state` contained within this
        /// type.
        ///
        /// # Safety
        ///
        /// The caller promises this mutable reference will not be used to modify any contents of
        /// `bindings::drm_plane_state` which DRM would consider to be static - like the backpointer
        /// to the DRM plane that owns this state. This also means the mutable reference should
        /// never be exposed outside of this crate.
        unsafe fn as_raw_mut(&mut self) -> &mut bindings::drm_plane_state;
    }
}

/// A trait implemented for any type which can be constructed directly from a
/// [`struct drm_plane_state`] pointer.
///
/// This is implemented internally by DRM.
///
/// [`struct drm_plane_state`]: srctree/include/drm/drm_plane.h
pub trait FromRawPlaneState: AsRawPlaneState {
    /// Get an immutable reference to this type from the given raw [`struct drm_plane_state`]
    /// pointer.
    ///
    /// # Safety
    ///
    /// - The caller guarantees `ptr` is contained within a valid instance of `Self`
    /// - The caller guarantees that `ptr` cannot not be modified for the lifetime of `'a`.
    ///
    /// [`struct drm_plane_state`]: srctree/include/drm/drm_plane.h
    unsafe fn from_raw<'a>(ptr: *const bindings::drm_plane_state) -> &'a Self;

    /// Get a mutable reference to this type from the given raw [`struct drm_plane_state`] pointer.
    ///
    /// # Safety
    ///
    /// - The caller guarantees that `ptr` is contained within a valid instance of `Self`
    /// - The caller guarantees that `ptr` cannot have any other references taken out for the
    ///   lifetime of `'a`.
    ///
    /// [`struct drm_plane_state`]: srctree/include/drm/drm_plane.h
    unsafe fn from_raw_mut<'a>(ptr: *mut bindings::drm_plane_state) -> &'a mut Self;
}

/// Common methods available on any type which implements [`AsRawPlane`].
///
/// This is implemented internally by DRM, and provides many of the basic methods for working with
/// the atomic state of [`Plane`]s.
pub trait RawPlaneState: AsRawPlaneState {
    /// Return the plane that this plane state belongs to.
    fn plane(&self) -> &Self::Plane {
        // SAFETY: The index is initialized by the time we expose Plane objects to users, and is
        // invariant throughout the lifetime of the Plane
        unsafe { Self::Plane::from_raw(self.as_raw().plane) }
    }

    /// Return the current [`OpaqueCrtc`] assigned to this plane, if there is one.
    fn crtc<'a, 'b: 'a, D>(&'a self) -> Option<&'b OpaqueCrtc<D>>
    where
        Self::Plane: ModeObject<Driver = D>,
        D: KmsDriver,
    {
        // SAFETY: This cast is guaranteed safe by `OpaqueCrtc`s invariants.
        NonNull::new(self.as_raw().crtc).map(|c| unsafe { OpaqueCrtc::from_raw(c.as_ptr()) })
    }

    /// Run the atomic check helper for this plane and the given CRTC state.
    fn atomic_helper_check<S, D>(
        &mut self,
        crtc_state: &CrtcStateMutator<'_, S>,
        can_position: bool,
        can_update_disabled: bool,
    ) -> Result
    where
        D: KmsDriver,
        S: FromRawCrtcState,
        S::Crtc: AsRawCrtc + ModeObject<Driver = D>,
        Self::Plane: ModeObject<Driver = D>,
    {
        // SAFETY: We're passing the mutable reference from `self.as_raw_mut()` directly to DRM,
        // which is safe.
        to_result(unsafe {
            bindings::drm_atomic_helper_check_plane_state(
                self.as_raw_mut(),
                crtc_state.as_raw(),
                bindings::DRM_PLANE_NO_SCALING as c_int, // TODO: add parameters for scaling
                bindings::DRM_PLANE_NO_SCALING as c_int,
                can_position,
                can_update_disabled,
            )
        })
    }

    /// Return the framebuffer currently set for this plane state
    #[inline]
    fn framebuffer<D>(&self) -> Option<&Framebuffer<D>>
    where
        Self::Plane: ModeObject<Driver = D>,
        D: KmsDriver,
    {
        // SAFETY: The layout of Framebuffer<T> is identical to `fb`
        unsafe {
            self.as_raw()
                .fb
                .as_ref()
                .map(|fb| Framebuffer::from_raw(fb))
        }
    }
}
impl<T: AsRawPlaneState + ?Sized> RawPlaneState for T {}

/// The main interface for a [`struct drm_plane_state`].
///
/// This type is the main interface for dealing with the atomic state of DRM planes. In addition, it
/// allows access to whatever private data is contained within an implementor's [`DriverPlaneState`]
/// type.
///
/// # Invariants
///
/// - The DRM C API and our interface guarantees that only the user has mutable access to `state`,
///   up until [`drm_atomic_helper_commit_hw_done`] is called. Therefore, `plane` follows rust's
///   data aliasing rules and does not need to be behind an [`Opaque`] type.
/// - `state` and `inner` initialized for as long as this object is exposed to users.
/// - The data layout of this structure begins with [`struct drm_plane_state`].
/// - The plane for this atomic state can always be assumed to be of type [`Plane<T::Plane>`].
///
/// [`struct drm_plane_state`]: srctree/include/drm/drm_plane.h
/// [`drm_atomic_helper_commit_hw_done`]: srctree/include/drm/drm_atomic_helper.h
#[derive(Default)]
#[repr(C)]
pub struct PlaneState<T: DriverPlaneState> {
    state: <T::Kind as PlaneStateKind>::Raw,
    inner: T,
}

/// The kind of C state a [`PlaneState`] is built on, chosen with [`DriverPlaneState::Kind`].
///
/// - [`Plain`]: a [`struct drm_plane_state`].
/// - [`Shadow`]: a [`struct drm_shadow_plane_state`], for planes the CPU copies to the hardware.
///   The framebuffer is then mapped for the CPU during each commit, see
///   [`PlaneStateMutator::read_rows()`].
///
/// [`struct drm_plane_state`]: srctree/include/drm/drm_plane.h
/// [`struct drm_shadow_plane_state`]: srctree/include/drm/drm_gem_atomic_helper.h
pub trait PlaneStateKind: Sealed + 'static {
    /// The C state struct.
    #[doc(hidden)]
    type Raw: RawPlaneStateBase;
}

/// A plane state built on a plain [`struct drm_plane_state`].
///
/// [`struct drm_plane_state`]: srctree/include/drm/drm_plane.h
pub enum Plain {}

impl Sealed for Plain {}

impl PlaneStateKind for Plain {
    type Raw = bindings::drm_plane_state;
}

/// A plane state built on a [`struct drm_shadow_plane_state`].
///
/// The framebuffer is mapped for the CPU between `begin_fb_access` and `end_fb_access`, which
/// cover the atomic update, using DRM's shadow-plane helpers.
///
/// [`struct drm_shadow_plane_state`]: srctree/include/drm/drm_gem_atomic_helper.h
pub enum Shadow {}

impl Sealed for Shadow {}

impl PlaneStateKind for Shadow {
    type Raw = bindings::drm_shadow_plane_state;
}

/// Type of `struct drm_plane_helper_funcs.begin_fb_access`.
#[doc(hidden)]
pub type BeginFbAccess =
    unsafe extern "C" fn(*mut bindings::drm_plane, *mut bindings::drm_plane_state) -> c_int;

/// Type of `struct drm_plane_helper_funcs.end_fb_access`.
#[doc(hidden)]
pub type EndFbAccess = unsafe extern "C" fn(*mut bindings::drm_plane, *mut bindings::drm_plane_state);

/// The C struct a plane state starts with, and the DRM helpers managing it.
///
/// # Safety
///
/// - The data layout of the implementor must start with a [`struct drm_plane_state`].
/// - `init()`, `duplicate()` and `destroy()` must manage the state like the equivalent DRM
///   helpers, and the fb access callbacks must be valid for states of this type.
///
/// [`struct drm_plane_state`]: srctree/include/drm/drm_plane.h
#[doc(hidden)]
pub unsafe trait RawPlaneStateBase: Default + Sized {
    /// The `begin_fb_access` helper for this kind of state.
    const BEGIN_FB_ACCESS: Option<BeginFbAccess>;
    /// The `end_fb_access` helper for this kind of state.
    const END_FB_ACCESS: Option<EndFbAccess>;

    /// Initialize a new state for `plane`.
    ///
    /// # Safety
    ///
    /// `plane` must be a valid `struct drm_plane` and `self` must be zeroed.
    unsafe fn init(&mut self, plane: *mut bindings::drm_plane);

    /// Duplicate the current state of `plane` into `self`.
    ///
    /// # Safety
    ///
    /// `plane` must be a valid `struct drm_plane` whose current state is of type `Self`, and
    /// `self` must be zeroed.
    unsafe fn duplicate(&mut self, plane: *mut bindings::drm_plane);

    /// Release the resources of the state.
    ///
    /// # Safety
    ///
    /// The state must have been set up with `init()` or `duplicate()`, and must not be used
    /// afterwards.
    unsafe fn destroy(&mut self);

    /// Return the [`struct drm_plane_state`] this state starts with.
    ///
    /// [`struct drm_plane_state`]: srctree/include/drm/drm_plane.h
    fn base(&self) -> &bindings::drm_plane_state;

    /// Return the [`struct drm_plane_state`] this state starts with, mutably.
    ///
    /// [`struct drm_plane_state`]: srctree/include/drm/drm_plane.h
    fn base_mut(&mut self) -> &mut bindings::drm_plane_state;
}

// SAFETY: `drm_plane_state` starts with itself, and the functions use the plain DRM state helpers.
unsafe impl RawPlaneStateBase for bindings::drm_plane_state {
    const BEGIN_FB_ACCESS: Option<BeginFbAccess> = None;
    const END_FB_ACCESS: Option<EndFbAccess> = None;

    unsafe fn init(&mut self, plane: *mut bindings::drm_plane) {
        // SAFETY: `plane` is valid and `self` is zeroed, per our safety contract.
        unsafe { bindings::__drm_atomic_helper_plane_state_init(self, plane) };
    }

    unsafe fn duplicate(&mut self, plane: *mut bindings::drm_plane) {
        // SAFETY: `plane` is valid and `self` is zeroed, per our safety contract.
        unsafe { bindings::__drm_atomic_helper_plane_duplicate_state(plane, self) };
    }

    unsafe fn destroy(&mut self) {
        // SAFETY: The state was set up by `init()` or `duplicate()`, per our safety contract.
        unsafe { bindings::__drm_atomic_helper_plane_destroy_state(self) };
    }

    fn base(&self) -> &bindings::drm_plane_state {
        self
    }

    fn base_mut(&mut self) -> &mut bindings::drm_plane_state {
        self
    }
}

// SAFETY: `drm_shadow_plane_state` starts with its `base` plane state, and the functions use
// DRM's shadow-plane helpers.
unsafe impl RawPlaneStateBase for bindings::drm_shadow_plane_state {
    const BEGIN_FB_ACCESS: Option<BeginFbAccess> = Some(bindings::drm_gem_begin_shadow_fb_access);
    const END_FB_ACCESS: Option<EndFbAccess> = Some(end_shadow_fb_access_callback);

    unsafe fn init(&mut self, plane: *mut bindings::drm_plane) {
        // SAFETY: `plane` is valid and `self` is zeroed, per our safety contract. This is what
        // `__drm_gem_reset_shadow_plane()` does, without installing the state on `plane`.
        unsafe {
            bindings::__drm_atomic_helper_plane_state_init(&mut self.base, plane);
            bindings::drm_format_conv_state_init(&mut self.fmtcnv_state);
        }
    }

    unsafe fn duplicate(&mut self, plane: *mut bindings::drm_plane) {
        // SAFETY: `plane` is valid, its current state is a shadow plane state, and `self` is
        // zeroed, per our safety contract.
        unsafe { bindings::__drm_gem_duplicate_shadow_plane_state(plane, self) };
    }

    unsafe fn destroy(&mut self) {
        // SAFETY: The state was set up by `init()` or `duplicate()`, per our safety contract.
        unsafe { bindings::__drm_gem_destroy_shadow_plane_state(self) };
    }

    fn base(&self) -> &bindings::drm_plane_state {
        &self.base
    }

    fn base_mut(&mut self) -> &mut bindings::drm_plane_state {
        &mut self.base
    }
}

/// `end_fb_access` for shadow plane states: unmap the framebuffer, then forget the mappings, so
/// a state never hands out pointers to memory that is no longer mapped.
unsafe extern "C" fn end_shadow_fb_access_callback(
    plane: *mut bindings::drm_plane,
    plane_state: *mut bindings::drm_plane_state,
) {
    // SAFETY: DRM passes a valid plane and state, mapped by `drm_gem_begin_shadow_fb_access()`.
    unsafe { bindings::drm_gem_end_shadow_fb_access(plane, plane_state) };

    // SAFETY: `plane_state` is the `base` of a `drm_shadow_plane_state`, since only shadow plane
    // states use this callback, and DRM gives us exclusive access to it here.
    let shadow = unsafe { &mut *container_of!(plane_state, bindings::drm_shadow_plane_state, base) };
    shadow.map = Default::default();
    shadow.data = Default::default();
}

/// A rectangle in a framebuffer or on the display, see [`struct drm_rect`].
///
/// `x2` and `y2` are exclusive.
///
/// [`struct drm_rect`]: srctree/include/drm/drm_rect.h
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    /// Left edge.
    pub x1: i32,
    /// Top edge.
    pub y1: i32,
    /// Right edge, exclusive.
    pub x2: i32,
    /// Bottom edge, exclusive.
    pub y2: i32,
}

impl Rect {
    /// Width of the rectangle.
    pub fn width(&self) -> i32 {
        self.x2 - self.x1
    }

    /// Height of the rectangle.
    pub fn height(&self) -> i32 {
        self.y2 - self.y1
    }
}

/// Iterator over the damaged areas of a plane in an atomic commit, see
/// [`PlaneStateMutator::damage()`].
pub struct DamageIter<'a> {
    iter: bindings::drm_atomic_helper_damage_iter,
    _p: PhantomData<&'a bindings::drm_plane_state>,
}

impl Iterator for DamageIter<'_> {
    type Item = Rect;

    fn next(&mut self) -> Option<Rect> {
        let mut rect = bindings::drm_rect::default();

        // SAFETY: `self.iter` was set up by `drm_atomic_helper_damage_iter_init()` from states
        // that outlive `'a`.
        if unsafe { bindings::drm_atomic_helper_damage_iter_next(&mut self.iter, &mut rect) } {
            Some(Rect {
                x1: rect.x1,
                y1: rect.y1,
                x2: rect.x2,
                y2: rect.y2,
            })
        } else {
            None
        }
    }
}

/// The main trait for implementing the [`struct drm_plane_state`] API for a [`Plane`].
///
/// A driver may store driver-private data within the implementor's type, which will be available
/// when using a full typed [`PlaneState`] object.
///
/// # Invariants
///
/// - Any C FFI callbacks generated using this trait are guaranteed that passed-in
///   [`struct drm_plane`] pointers are contained within a [`Plane<Self::Plane>`].
/// - Any C FFI callbacks generated using this trait are guaranteed that passed-in
///   [`struct drm_plane_state`] pointers are contained within a [`PlaneState<Self>`].
///
/// [`struct drm_plane`]: srctree/include/drm_plane.h
/// [`struct drm_plane_state`]: srctree/include/drm_plane.h
pub trait DriverPlaneState: Clone + Default + Sized {
    /// The type for this driver's drm_plane implementation
    type Plane: DriverPlane;

    /// The kind of C state this plane state is built on: [`Plain`], or [`Shadow`] for planes the
    /// CPU copies to the hardware.
    type Kind: PlaneStateKind;
}

impl<T: DriverPlaneState> Sealed for PlaneState<T> {}

impl<T: DriverPlaneState> AsRawPlaneState for PlaneState<T> {
    type Plane = Plane<T::Plane>;
}

impl<T: DriverPlaneState> private::AsRawPlaneState for PlaneState<T> {
    #[inline]
    fn as_raw(&self) -> &bindings::drm_plane_state {
        self.state.base()
    }

    #[inline]
    unsafe fn as_raw_mut(&mut self) -> &mut bindings::drm_plane_state {
        self.state.base_mut()
    }
}

impl<T: DriverPlaneState> PlaneState<T> {
    /// Iterate over the areas of the framebuffer that changed since `old_state`, clipped to the
    /// plane's source. See [`drm_atomic_helper_damage_iter_init()`].
    ///
    /// [`drm_atomic_helper_damage_iter_init()`]: srctree/drivers/gpu/drm/drm_damage_helper.c
    fn damage<'a>(&'a self, old_state: &'a Self) -> DamageIter<'a> {
        let mut iter = bindings::drm_atomic_helper_damage_iter::default();

        // SAFETY: Both states are valid plane states that outlive `'a`, and the iterator only
        // reads them.
        unsafe {
            bindings::drm_atomic_helper_damage_iter_init(
                &mut iter,
                old_state.state.base(),
                self.state.base(),
            )
        };

        DamageIter {
            iter,
            _p: PhantomData,
        }
    }
}

impl<T: DriverPlaneState<Kind = Shadow>> PlaneState<T> {
    /// See [`PlaneStateMutator::read_rows()`].
    fn read_rows(
        &self,
        index: usize,
        rect: &Rect,
        buf: &mut KVec<u8>,
        mut f: impl FnMut(u32, &[u8]) -> Result,
    ) -> Result {
        let base = self.state.base();
        if base.fb.is_null() {
            return Err(EINVAL);
        }
        // SAFETY: A plane state holds a reference on its framebuffer, which is immutable after
        // creation.
        let fb = unsafe { &*base.fb };
        // SAFETY: Every framebuffer has a valid format.
        let format = unsafe { &*fb.format };

        if index >= usize::from(format.num_planes) || index >= self.state.data.len() {
            return Err(EINVAL);
        }
        let map = &self.state.data[index];
        if map.is_iomem {
            return Err(ENOTSUPP);
        }
        // SAFETY: `is_iomem` is false, so `vaddr` is the active union member.
        let vaddr = unsafe { map.__bindgen_anon_1.vaddr };
        if vaddr.is_null() {
            // Not mapped: only valid while the framebuffer is accessed by an atomic commit.
            return Err(EINVAL);
        }

        // Rows are copied pixel-wise, which doesn't work for formats with multi-pixel blocks.
        if format.block_w[index] > 1 || format.block_h[index] > 1 {
            return Err(ENOTSUPP);
        }
        // SAFETY: `cpp` and `char_per_block` are both `[u8; 4]` aliasing the same bytes, which for
        // formats with one-pixel blocks are the bytes per pixel.
        let cpp = u32::from(unsafe { format.__bindgen_anon_1.cpp[index] });
        let pitch = fb.pitches[index];
        let vsub = if index == 0 { 1 } else { u32::from(format.vsub) };
        let (width, height) = (fb.width, fb.height.div_ceil(vsub));
        let (Ok(x1), Ok(y1), Ok(x2), Ok(y2)) = (
            u32::try_from(rect.x1),
            u32::try_from(rect.y1),
            u32::try_from(rect.x2),
            u32::try_from(rect.y2),
        ) else {
            return Err(EINVAL);
        };
        if x1 > x2 || x2 > width || y1 > y2 || y2 > height {
            return Err(EINVAL);
        }
        let start = usize::try_from(x1 * cpp)?;
        let len = usize::try_from((x2 - x1) * cpp)?;
        if start + len > usize::try_from(pitch)? {
            return Err(EINVAL);
        }

        buf.resize(len, 0, GFP_KERNEL)?;
        for y in y1..y2 {
            let offset = usize::try_from(y)? * usize::try_from(pitch)? + start;

            // SAFETY:
            // - `vaddr` maps the whole plane (`pitch * height` bytes) while the framebuffer is
            //   accessed, and `offset + len` is within that.
            // - `buf` holds `len` bytes.
            // - `memcpy()` is a volatile copy, so userspace writing the framebuffer concurrently
            //   only affects the copied values.
            unsafe { bindings::memcpy(buf.as_mut_ptr().cast(), vaddr.byte_add(offset), len) };

            f(y, buf)?;
        }

        Ok(())
    }
}

impl<T: DriverPlaneState> FromRawPlaneState for PlaneState<T> {
    #[inline]
    unsafe fn from_raw<'a>(ptr: *const bindings::drm_plane_state) -> &'a Self {
        // Our data layout starts with `bindings::drm_plane_state`.
        let ptr: *const Self = ptr.cast();

        // SAFETY:
        // - Our safety contract requires that `ptr` be contained within `Self`.
        // - Our safety contract requires the caller ensure that it is safe for us to take an
        //   immutable reference.
        unsafe { &*ptr }
    }

    #[inline]
    unsafe fn from_raw_mut<'a>(ptr: *mut bindings::drm_plane_state) -> &'a mut Self {
        // Our data layout starts with `bindings::drm_plane_state`.
        let ptr: *mut Self = ptr.cast();

        // SAFETY:
        // - Our safety contract requires that `ptr` be contained within `Self`.
        // - Our safety contract requires the caller ensure it is safe for us to take a mutable
        //   reference.
        unsafe { &mut *ptr }
    }
}

impl<T: DriverPlaneState> Deref for PlaneState<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T: DriverPlaneState> DerefMut for PlaneState<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

// SAFETY: Shares the safety guarantee of Plane<T>'s vtable impl
unsafe impl<T: DriverPlaneState> ModeObjectVtable for PlaneState<T> {
    type Vtable = bindings::drm_plane_funcs;

    fn vtable(&self) -> *const Self::Vtable {
        self.plane().vtable()
    }
}

impl<T: DriverPlaneState> PlaneState<T> {
    super::impl_from_opaque_mode_obj! {
        fn <'a, D, P>(&'a OpaquePlaneState<D>) -> &'a Self
        where
            T: DriverPlaneState<Plane = P>;
        use
            P as DriverPlane,
            D as KmsDriver<Plane = ...>
    }
}

/// A [`struct drm_plane_state`] without a known [`DriverPlaneState`] implementation.
///
/// This is mainly for situations where our bindings can't infer the [`DriverPlaneState`]
/// implementation for a [`struct drm_plane_state`] automatically. It is identical to [`Plane`],
/// except that it does not provide access to the driver's private data.
///
/// # Invariants
///
/// - The DRM C API and our interface guarantees that only the user has mutable access to `state`,
///   up until [`drm_atomic_helper_commit_hw_done`] is called. Therefore, `plane` follows rust's
///   data aliasing rules and does not need to be behind an [`Opaque`] type.
/// - `state` is initialized for as long as this object is exposed to users.
/// - The data layout of this structure is identical to [`struct drm_plane_state`].
///
/// [`struct drm_plane_state`]: srctree/include/drm/drm_plane.h
/// [`drm_atomic_helper_commit_hw_done`]: srctree/include/drm/drm_atomic_helper.h
#[repr(transparent)]
pub struct OpaquePlaneState<T: KmsDriver> {
    state: bindings::drm_plane_state,
    _p: PhantomData<T>,
}

impl<T: KmsDriver> AsRawPlaneState for OpaquePlaneState<T> {
    type Plane = OpaquePlane<T>;
}

impl<T: KmsDriver> private::AsRawPlaneState for OpaquePlaneState<T> {
    fn as_raw(&self) -> &bindings::drm_plane_state {
        &self.state
    }

    unsafe fn as_raw_mut(&mut self) -> &mut bindings::drm_plane_state {
        &mut self.state
    }
}

impl<T: KmsDriver> FromRawPlaneState for OpaquePlaneState<T> {
    unsafe fn from_raw<'a>(ptr: *const bindings::drm_plane_state) -> &'a Self {
        // SAFETY: Our data layout is identical to `ptr`
        unsafe { &*ptr.cast() }
    }

    unsafe fn from_raw_mut<'a>(ptr: *mut bindings::drm_plane_state) -> &'a mut Self {
        // SAFETY: Our data layout is identical to `ptr`
        unsafe { &mut *ptr.cast() }
    }
}

// SAFETY: Shares the safety guarantee of OpaquePlane<T>'s vtable impl
unsafe impl<T: KmsDriver> ModeObjectVtable for OpaquePlaneState<T> {
    type Vtable = bindings::drm_plane_funcs;

    fn vtable(&self) -> *const Self::Vtable {
        self.plane().vtable()
    }
}

/// An interface for mutating a [`Plane`]s atomic state.
///
/// This type is typically returned by an [`AtomicStateMutator`] within contexts where it is
/// possible to safely mutate a plane's state. In order to uphold rust's data-aliasing rules, only
/// [`PlaneStateMutator`] may exist at a time.
pub struct PlaneStateMutator<'a, T: FromRawPlaneState> {
    state: &'a mut T,
    mask: &'a Cell<u32>,
}

impl<'a, T: FromRawPlaneState> PlaneStateMutator<'a, T> {
    pub(super) fn new<D: KmsDriver>(
        mutator: &'a AtomicStateMutator<D>,
        state: NonNull<bindings::drm_plane_state>,
    ) -> Option<Self> {
        // SAFETY: `plane` is invariant throughout the lifetime of the atomic state, is
        // initialized by this point, and we're guaranteed it is of type `AsRawPlane` by type
        // invariance
        let plane = unsafe { T::Plane::from_raw((*state.as_ptr()).plane) };
        let plane_mask = plane.mask();
        let borrowed_mask = mutator.borrowed_planes.get();

        if borrowed_mask & plane_mask == 0 {
            mutator.borrowed_planes.set(borrowed_mask | plane_mask);
            Some(Self {
                mask: &mutator.borrowed_planes,
                // SAFETY: We're guaranteed `state` is of `FromRawPlaneState` by type invariance,
                // and we just confirmed by checking `borrowed_planes` that no other mutable borrows
                // have been taken out for `state`
                state: unsafe { T::from_raw_mut(state.as_ptr()) },
            })
        } else {
            None
        }
    }
}

impl<'a, T: FromRawPlaneState> Drop for PlaneStateMutator<'a, T> {
    fn drop(&mut self) {
        let mask = self.state.plane().mask();
        self.mask.set(self.mask.get() & !mask);
    }
}

impl<'a, T: FromRawPlaneState> AsRawPlaneState for PlaneStateMutator<'a, T> {
    type Plane = T::Plane;
}

impl<'a, T: FromRawPlaneState> private::AsRawPlaneState for PlaneStateMutator<'a, T> {
    fn as_raw(&self) -> &bindings::drm_plane_state {
        self.state.as_raw()
    }

    unsafe fn as_raw_mut(&mut self) -> &mut bindings::drm_plane_state {
        // SAFETY: This function is bound by the same safety contract as `self.inner.as_raw_mut()`
        unsafe { self.state.as_raw_mut() }
    }
}

impl<'a, T: FromRawPlaneState> Sealed for PlaneStateMutator<'a, T> {}

impl<'a, T: DriverPlaneState> Deref for PlaneStateMutator<'a, PlaneState<T>> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.state.inner
    }
}

impl<'a, T: DriverPlaneState> DerefMut for PlaneStateMutator<'a, PlaneState<T>> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state.inner
    }
}

impl<'a, T: DriverPlaneState> PlaneStateMutator<'a, PlaneState<T>> {
    /// Iterate over the areas of the framebuffer that changed since `old_state`, clipped to the
    /// plane's source, in framebuffer coordinates.
    ///
    /// Each area needs updating, e.g. copying to the hardware for a [`Shadow`] plane. Without
    /// damage information this is the whole visible framebuffer. See
    /// [`drm_atomic_helper_damage_iter_init()`].
    ///
    /// [`drm_atomic_helper_damage_iter_init()`]: srctree/drivers/gpu/drm/drm_damage_helper.c
    pub fn damage<'b>(&'b self, old_state: &'b PlaneState<T>) -> DamageIter<'b> {
        self.state.damage(old_state)
    }

    /// The area of the framebuffer the plane shows, in whole pixels.
    ///
    /// Set by the atomic check, e.g. [`RawPlaneState::atomic_helper_check()`].
    pub fn src(&self) -> Rect {
        // `src` is in 16.16 fixed point.
        let src = &self.state.state.base().src;
        Rect {
            x1: src.x1 >> 16,
            y1: src.y1 >> 16,
            x2: src.x2 >> 16,
            y2: src.y2 >> 16,
        }
    }

    /// Where the plane shows on the CRTC, in pixels.
    ///
    /// Set by the atomic check, e.g. [`RawPlaneState::atomic_helper_check()`].
    pub fn dst(&self) -> Rect {
        let dst = &self.state.state.base().dst;
        Rect {
            x1: dst.x1,
            y1: dst.y1,
            x2: dst.x2,
            y2: dst.y2,
        }
    }
}

impl<'a, T: DriverPlaneState<Kind = Shadow>> PlaneStateMutator<'a, PlaneState<T>> {
    /// Read the rows of `rect` from color plane `index` of the framebuffer, as mapped for the
    /// current commit.
    ///
    /// For each row from `rect.y1` to `rect.y2`, the pixels from `rect.x1` to `rect.x2` are copied
    /// into `buf`, and `f` is called with the row's `y` and the copied bytes. Userspace may write
    /// the framebuffer concurrently, which only affects the values read.
    ///
    /// The framebuffer is only mapped while the commit accesses it, e.g. in
    /// [`DriverPlane::atomic_update()`]. Fails with `EINVAL` outside of that, if `index` or `rect`
    /// don't fit the framebuffer, and with `ENOTSUPP` for framebuffers in I/O memory.
    pub fn read_rows(
        &self,
        index: usize,
        rect: &Rect,
        buf: &mut KVec<u8>,
        f: impl FnMut(u32, &[u8]) -> Result,
    ) -> Result {
        self.state.read_rows(index, rect, buf, f)
    }
}

// SAFETY: Shares the safety guarantees of `T`'s ModeObjectVtable impl
unsafe impl<'a, T: FromRawPlaneState> ModeObjectVtable for PlaneStateMutator<'a, T>
where
    T: FromRawPlaneState + ModeObjectVtable,
{
    type Vtable = T::Vtable;

    fn vtable(&self) -> *const Self::Vtable {
        self.state.vtable()
    }
}

impl<'a, T: DriverPlaneState> PlaneStateMutator<'a, PlaneState<T>> {
    super::impl_from_opaque_mode_obj! {
        fn <D, P>(PlaneStateMutator<'a, OpaquePlaneState<D>>) -> Self
        where
            T: DriverPlaneState<Plane = P>;
        use
            P as DriverPlane,
            D as KmsDriver<Plane = ...>
    }
}

/// A token provided during [`atomic_check`] callbacks for accessing the plane, atomic state, and
/// the old and new states of the plane.
///
/// [`atomic_check`]: DriverPlane::atomic_check
pub struct PlaneAtomicCheck<'a, T: DriverPlane> {
    state: &'a AtomicStateComposer<T::Driver>,
    plane: &'a Plane<T>,
}

impl<'a, T: DriverPlane> PlaneAtomicCheck<'a, T> {
    impl_atomic_state_token_ops!(
        PlaneAtomicCheck,
        AtomicStateComposer,
        Plane,
        use <'a, T>
    );
}

/// A token provided to [`DriverPlane`] callbacks during the atomic commit phase for accessing the
/// plane, atomic state, new and old states of the plane.
///
/// # Invariants
///
/// This token is proof that the old and new atomic state of `plane` are present in `state` and do
/// not have any mutators taken out.
pub struct PlaneAtomicCommit<'a, T: DriverPlane> {
    state: &'a AtomicStateMutator<T::Driver>,
    plane: &'a Plane<T>,
}

impl<'a, T: DriverPlane> PlaneAtomicCommit<'a, T> {
    impl_atomic_state_token_ops!(
        PlaneAtomicCommit,
        AtomicStateMutator,
        Plane,
        use <'a, T>
    );
}

#[allow(clippy::missing_safety_doc)]
unsafe extern "C" fn plane_destroy_callback<T: DriverPlane>(plane: *mut bindings::drm_plane) {
    // SAFETY: DRM guarantees that `plane` points to a valid initialized `drm_plane`.
    unsafe { bindings::drm_plane_cleanup(plane) };

    // SAFETY:
    // - DRM guarantees we are now the only one with access to this [`drm_plane`].
    // - We are guaranteed that `plane` is contained within a `Plane<T>`.
    drop(unsafe {
        KBox::from_raw(
            container_of!(Opaque::cast_from(plane), Plane<T>, plane).cast_mut()
        )
    });
}

#[allow(clippy::missing_safety_doc)]
unsafe extern "C" fn atomic_duplicate_state_callback<T: DriverPlaneState>(
    plane: *mut bindings::drm_plane,
) -> *mut bindings::drm_plane_state {
    // SAFETY: DRM guarantees that `plane` points to a valid initialized `drm_plane`.
    let state = unsafe { (*plane).state };
    if state.is_null() {
        return null_mut();
    }

    // SAFETY: This cast is safe via `DriverPlaneState`s type invariants.
    let state = unsafe { PlaneState::<T>::from_raw(state) };

    let new: Result<KBox<PlaneState<T>>> = KBox::try_init(
        try_init!(PlaneState {
            inner: state.inner.clone(),
            state: Default::default(),
        }),
        GFP_KERNEL,
    );

    if let Ok(mut new) = new {
        // SAFETY:
        // - `plane` is valid and its current state is a `PlaneState<T>`, so of the same kind.
        // - `new.state` was just zeroed.
        unsafe { new.state.duplicate(plane) };

        KBox::into_raw(new).cast()
    } else {
        null_mut()
    }
}

#[allow(clippy::missing_safety_doc)]
unsafe extern "C" fn atomic_destroy_state_callback<T: DriverPlaneState>(
    _plane: *mut bindings::drm_plane,
    state: *mut bindings::drm_plane_state,
) {
    // This cast is safe via our type invariants.
    let state = state.cast::<PlaneState<T>>();

    // SAFETY:
    // - DRM guarantees we are the only one with access to this state.
    // - It was set up by `atomic_create_state_callback()` or `atomic_duplicate_state_callback()`.
    unsafe { (*state).state.destroy() };

    // SAFETY:
    // * DRM guarantees we are the only one with access to this `drm_plane_state`
    // * This cast is safe via our type invariants.
    drop(unsafe { KBox::from_raw(state) });
}

#[allow(clippy::missing_safety_doc)]
unsafe extern "C" fn atomic_create_state_callback<T: DriverPlaneState>(
    plane: *mut bindings::drm_plane
) -> *mut bindings::drm_plane_state {
    let new: Result<KBox<PlaneState<T>>> = KBox::try_init(
        try_init!(PlaneState {
            state: Default::default(),
            inner: Default::default(),
        }),
        GFP_KERNEL,
    );

    match new {
        Ok(mut new) => {
            // SAFETY: DRM passes a valid `plane`, and `new.state` was just zeroed.
            unsafe { new.state.init(plane) };

            // SAFETY: We will recover this `KBox` in atomic_destroy_state_callback
            let raw = KBox::into_raw(new);

            // SAFETY: We successfully allocated `new` above, so `raw` is safe to dereference.
            unsafe { (*raw).state.base_mut() }
        },
        Err(e) => e.to_ptr()
    }
}

#[allow(clippy::missing_safety_doc)]
unsafe extern "C" fn atomic_update_callback<T: DriverPlane>(
    plane: *mut bindings::drm_plane,
    state: *mut bindings::drm_atomic_commit,
) {
    // SAFETY:
    // - We're guaranteed `plane` is of type `Plane<T>` via type invariants.
    // - We're guaranteed by DRM that `plane` is pointing to a valid initialized state.
    let plane = unsafe { Plane::from_raw(plane) };

    // SAFETY: DRM guarantees `state` points to a valid `drm_atomic_commit`
    let state = unsafe { AtomicStateMutator::new(NonNull::new_unchecked(state)) };

    // SAFETY:
    // - Since we're in the atomic_update callback, we're guaranteed by DRM that both the old and new
    //   plane state are resent in this atomic state.
    // - We just created the state mutator above, so other mutators cannot be taken out on the plane
    //   state yet.
    let commit = unsafe { PlaneAtomicCommit::new(plane, &state) };

    T::atomic_update(commit);
}

#[allow(clippy::missing_safety_doc)]
unsafe extern "C" fn atomic_check_callback<T: DriverPlane>(
    plane: *mut bindings::drm_plane,
    state: *mut bindings::drm_atomic_commit,
) -> i32 {
    // SAFETY:
    // - We're guaranteed `plane` is of type `Plane<T>` via type invariants.
    // - We're guaranteed by DRM that `plane` is pointing to a valid initialized state.
    let plane = unsafe { Plane::from_raw(plane) };

    // SAFETY: We're guaranteed by DRM that `state` points to a valid instance of `drm_atomic_commit`
    // We use ManuallyDrop here since AtomicStateComposer would otherwise drop a owned reference to
    // the atomic state upon finishing this callback.
    let state = ManuallyDrop::new(unsafe {
        AtomicStateComposer::<T::Driver>::new(NonNull::new_unchecked(state))
    });

    // SAFETY:
    // - Since we're in the atomic check callback, we're guaranteed by DRM that both the old and
    //   new plane state are present in this atomic state
    // - We just created the state composer above, so other composers cannot be taken out on the
    //   plane state yet.
    let check = unsafe { PlaneAtomicCheck::new(plane, &state) };

    from_result(|| T::atomic_check(check).map(|()| 0))
}

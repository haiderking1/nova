// SPDX-License-Identifier: GPL-2.0 OR MIT

//! DRM encoders.
//!
//! C header: [`include/drm/drm_encoder.h`](srctree/include/drm/drm_encoder.h)

use crate::{
    alloc::KBox,
    container_of,
    drm::{
        Device,
        DeviceContext,
        Normal,
        kms::{
            KmsDriver,
            KmsProbe,
            ModeObject,
            StaticModeObject,
            ModeObjectVtable,
            Sealed, //
        }
    },
    error::to_result,
    prelude::*,
    types::Opaque,
};
use bindings;
use core::{
    marker::*,
    ops::Deref,
    ptr::null, //
};
use macros::paste;

/// A macro for generating our type ID enumerator.
macro_rules! declare_encoder_types {
    ($( $oldname:ident as $newname:ident ),+) => {
        #[repr(i32)]
        #[non_exhaustive]
        #[derive(Copy, Clone, PartialEq, Eq)]
        /// An enumerator for all possible [`Encoder`] type IDs.
        pub enum Type {
            // Note: bindgen defaults the macro values to u32 and not i32, but DRM takes them as an
            // i32 - so just do the conversion here
            $(
                #[doc = concat!("The encoder type ID for a ", stringify!($newname), " encoder.")]
                $newname = paste!(crate::bindings::[<DRM_MODE_ENCODER_ $oldname>]) as i32
            ),+
        }
    };
}

declare_encoder_types! {
    NONE     as None,
    DAC      as Dac,
    TMDS     as Tmds,
    LVDS     as Lvds,
    VIRTUAL  as Virtual,
    DSI      as Dsi,
    DPMST    as DpMst,
    DPI      as Dpi
}

/// The main trait for implementing the [`struct drm_encoder`] API for [`Encoder`].
///
/// Any KMS driver should have at least one implementation of this type, which allows them to create
/// [`Encoder`] objects. Additionally, a driver may store driver-private data within the type that
/// implements [`DriverEncoder`] - and it will be made available when using a fully typed
/// [`Encoder`] object.
///
/// # Invariants
///
/// - Any C FFI callbacks generated using this trait are guaranteed that passed-in
///   [`struct drm_encoder`] pointers are contained within a [`Encoder<Self>`].
///
/// [`struct drm_encoder`]: srctree/include/drm/drm_encoder.h
#[vtable]
pub trait DriverEncoder: Send + Sync + Sized {
    /// The generated C vtable for this [`DriverEncoder`] implementation.
    const OPS: &'static DriverEncoderOps = &DriverEncoderOps {
        funcs: bindings::drm_encoder_funcs {
            reset: None,
            destroy: Some(encoder_destroy_callback::<Self>),
            late_register: None,
            early_unregister: None,
            debugfs_init: None,
        },
        helper_funcs: bindings::drm_encoder_helper_funcs {
            dpms: None,
            mode_valid: None,
            mode_fixup: None,
            prepare: None,
            mode_set: None,
            commit: None,
            detect: None,
            enable: None,
            disable: None,
            atomic_check: None,
            atomic_enable: None,
            atomic_disable: None,
            atomic_mode_set: None,
        },
    };

    /// The parent driver for this drm_encoder implementation
    type Driver: KmsDriver;

    /// The type to pass to the `args` field of [`Encoder::new`].
    ///
    /// This type will be made available in in the `args` argument of [`Self::new`]. Drivers which
    /// don't need this can simply pass [`()`] here.
    type Args;

    /// The constructor for creating a [`Encoder`] using this [`DriverEncoder`] implementation.
    ///
    /// Drivers may use this to instantiate their [`DriverEncoder`] object.
    fn new(device: &Device<Self::Driver, KmsProbe>, args: Self::Args) -> impl PinInit<Self, Error>;
}

/// The generated C vtable for a [`DriverEncoder`].
///
/// This type is created internally by DRM.
pub struct DriverEncoderOps {
    funcs: bindings::drm_encoder_funcs,
    helper_funcs: bindings::drm_encoder_helper_funcs,
}

/// A trait implemented by any type that acts as a [`struct drm_encoder`] interface.
///
/// This is implemented internally by DRM.
///
/// # Safety
///
/// [`as_raw()`] must always return a valid pointer to a [`struct drm_encoder`].
///
/// [`struct drm_encoder`]: srctree/include/drm/drm_encoder.h
/// [`as_raw()`]: AsRawEncoder::as_raw()
pub unsafe trait AsRawEncoder<C: DeviceContext = Normal> {
    /// Return the raw `bindings::drm_encoder` for this DRM encoder.
    ///
    /// Drivers should never use this directly
    fn as_raw(&self) -> *mut bindings::drm_encoder;

    /// Convert a raw `bindings::drm_encoder` pointer into an object of this type.
    ///
    /// # Safety
    ///
    /// Callers promise that `ptr` points to a valid instance of this type
    unsafe fn from_raw<'a>(ptr: *mut bindings::drm_encoder) -> &'a Self;
}

/// The main interface for a [`struct drm_encoder`].
///
/// This type is the main interface for dealing with DRM encoders. In addition, it also allows
/// immutable access to whatever private data is contained within an implementor's
/// [`DriverEncoder`] type.
///
/// # Invariants
///
/// - `encoder` and `inner` are initialized for as long as this object is made available to users.
/// - The data layout of this structure begins with [`struct drm_encoder`].
///
/// [`struct drm_encoder`]: srctree/include/drm/drm_encoder.h
#[repr(C)]
#[pin_data]
pub struct Encoder<T: DriverEncoder, C: DeviceContext = Normal> {
    /// The FFI drm_encoder object
    encoder: Opaque<bindings::drm_encoder>,
    /// The driver's private inner data
    #[pin]
    inner: T,
    _ctx: PhantomData<C>,
}

impl<T: DriverEncoder, C: DeviceContext> Sealed for Encoder<T, C> {}

// SAFETY: Our interface is thread-safe once fully initialized.
unsafe impl<T: DriverEncoder> Send for Encoder<T> {}
// SAFETY: Our interface is thread-safe once fully initialized.
unsafe impl<T: DriverEncoder> Sync for Encoder<T> {}

// SAFETY: We don't expose Encoder<T> to users before `base` is initialized in ::new(), so
// `raw_mode_obj` always returns a valid pointer to a bindings::drm_mode_object.
unsafe impl<T: DriverEncoder, C: DeviceContext> ModeObject<C> for Encoder<T, C> {
    type Driver = T::Driver;

    fn drm_dev(&self) -> &Device<Self::Driver, C> {
        // SAFETY: DRM encoders exist for as long as the device does, so this pointer is always
        // valid
        unsafe { Device::from_raw((*self.encoder.get()).dev) }
    }

    fn raw_mode_obj(&self) -> *mut bindings::drm_mode_object {
        // SAFETY: We don't expose Encoder<T> to users before it's initialized, so `base` is always
        // initialized
        unsafe { &raw mut (*self.encoder.get()).base }
    }
}

// SAFETY: `Encoder`s have no reference count of their own.
unsafe impl<T: DriverEncoder, C: DeviceContext> StaticModeObject<C> for Encoder<T, C> {}

impl<T: DriverEncoder, C: DeviceContext> Deref for Encoder<T, C> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

// SAFETY:
// - Via our type invariants our data layout starts with `drm_encoder`.
// - Since we don't expose `Encoder` to users befre it has been initialized, this and our data
//   layout ensure that `as_raw()` always returns a valid pointer to a `drm_encoder`.
unsafe impl<T: DriverEncoder, C: DeviceContext> AsRawEncoder<C> for Encoder<T, C> {
    fn as_raw(&self) -> *mut bindings::drm_encoder {
        self.encoder.get()
    }

    unsafe fn from_raw<'a>(ptr: *mut bindings::drm_encoder) -> &'a Self {
        // SAFETY: Our data layout is starts with to `bindings::drm_encoder`
        unsafe { &*ptr.cast() }
    }
}

// SAFETY: `funcs` is initialized when the encoder is allocated
unsafe impl<T: DriverEncoder> ModeObjectVtable for Encoder<T> {
    type Vtable = bindings::drm_encoder_funcs;

    fn vtable(&self) -> *const Self::Vtable {
        // SAFETY: `as_raw()` always returns a valid pointer to an encoder
        unsafe { *self.as_raw() }.funcs
    }
}

impl<T: DriverEncoder, C: DeviceContext> Encoder<T, C> {
    super::impl_from_opaque_mode_obj! {
        fn <'a, D>(&'a OpaqueEncoder<D, C>) -> &'a Self;
        use
            T as DriverEncoder,
            D as KmsDriver<Encoder = ...>
    }
}

impl<T: DriverEncoder> Encoder<T, KmsProbe> {
    /// Construct a new [`Encoder`].
    ///
    /// A driver may use this from their [`KmsDriver::create_objects`] callback in order to
    /// construct new [`Encoder`] objects.
    ///
    /// [`KmsDriver::create_objects`]: kernel::drm::kms::KmsDriver::create_objects
    pub fn new<'a, 'b: 'a>(
        dev: &'a Device<T::Driver, KmsProbe>,
        type_: Type,
        possible_crtcs: u32,
        possible_clones: u32,
        name: Option<&CStr>,
        args: T::Args,
    ) -> Result<&'b Self> {
        let this: Pin<KBox<Encoder<T, _>>> = KBox::try_pin_init(
            try_pin_init!(Encoder {
                encoder: Opaque::new(bindings::drm_encoder {
                    helper_private: &T::OPS.helper_funcs,
                    possible_crtcs,
                    possible_clones,
                    ..Default::default()
                }),
                inner <- T::new(dev, args),
                _ctx: PhantomData::<KmsProbe>,
            }),
            GFP_KERNEL,
        )?;

        // SAFETY:
        // - `dev` is responsible for destroying the encoder and thus outlives us.
        // - as_raw() returns valid pointers for each type here
        // - This initializes `this`
        // - Our type is proof that this is being called before KMS device registration
        // - `name` is optional and will be auto-generated by DRM if passed as NULL
        to_result(unsafe {
            bindings::drm_encoder_init(
                dev.as_raw(),
                this.as_raw(),
                &T::OPS.funcs,
                type_ as i32,
                name.map_or(null(), |n| n.as_char_ptr()),
            )
        })?;

        // SAFETY: We don't move anything
        let this = unsafe { Pin::into_inner_unchecked(this) };

        // We'll re-assemble the box in encoder_destroy_callback()
        let this = KBox::into_raw(this);

        // Encoder has an equivalent data layout
        let this: *mut Self = this.cast();

        // SAFETY: We just allocated the encoder above, so this pointer must be valid
        Ok(unsafe { &*this })
    }
}

/// A [`struct drm_encoder`] without a known [`DriverEncoder`] implementation.
///
/// This is mainly for situations where our bindings can't infer the [`DriverEncoder`] implementation
/// for a [`struct drm_encoder`] automatically. It is identical to [`Encoder`], except that it does not
/// provide access to the driver's private data.
///
/// # Invariants
///
/// Same as [`Encoder`].
#[repr(transparent)]
pub struct OpaqueEncoder<T: KmsDriver, C: DeviceContext = Normal> {
    encoder: Opaque<bindings::drm_encoder>,
    _p: PhantomData<(T, C)>,
}

impl<T: KmsDriver, C: DeviceContext> Sealed for OpaqueEncoder<T, C> {}

// SAFETY: All of our encoder interfaces are thread-safe once initialized.
unsafe impl<T: KmsDriver> Send for OpaqueEncoder<T> {}

// SAFETY: All of our encoder interfaces are thread-safe once initialized.
unsafe impl<T: KmsDriver> Sync for OpaqueEncoder<T> {}

// SAFETY: We don't expose OpaqueEncoder<T> to users before `base` is initialized in
// OpaqueEncoder::new(), so `raw_mode_obj` always returns a valid poiner to a
// bindings::drm_mode_object.
unsafe impl<T: KmsDriver, C: DeviceContext> ModeObject<C> for OpaqueEncoder<T, C> {
    type Driver = T;

    fn drm_dev(&self) -> &Device<Self::Driver, C> {
        // SAFETY: DRM encoders exist for as long as the device does, so this pointer is always
        // valid
        unsafe { Device::from_raw((*self.encoder.get()).dev) }
    }

    fn raw_mode_obj(&self) -> *mut bindings::drm_mode_object {
        // SAFETY: We don't expose Encoder<T> to users before it's initialized, so `base` is always
        // initialized
        unsafe { &raw mut (*self.encoder.get()).base }
    }
}

// SAFETY: `OpaqueEncoder`s do not have a reference count of their own.
unsafe impl<T: KmsDriver, C: DeviceContext> StaticModeObject<C> for OpaqueEncoder<T, C> {}

// SAFETY:
// - Via our type variants our data layout is identical to  with `drm_encoder`
// - Since we don't expose `Encoder` to users before it has been initialized, this and our data
//   layout ensure that `as_raw()` always returns a valid pointer to a `drm_encoder`.
unsafe impl<T: KmsDriver, C: DeviceContext> AsRawEncoder<C> for OpaqueEncoder<T, C> {
    fn as_raw(&self) -> *mut bindings::drm_encoder {
        self.encoder.get()
    }

    unsafe fn from_raw<'a>(ptr: *mut bindings::drm_encoder) -> &'a Self {
        // SAFETY: Our data layout is identical to `bindings::drm_encoder`
        unsafe { &*ptr.cast() }
    }
}

// SAFETY: `funcs` is initialized when the encoder is allocated
unsafe impl<T: KmsDriver, C: DeviceContext> ModeObjectVtable for OpaqueEncoder<T, C> {
    type Vtable = bindings::drm_encoder_funcs;

    fn vtable(&self) -> *const Self::Vtable {
        // SAFETY: `as_raw()` always returns a valid pointer to an encoder
        unsafe { *self.as_raw() }.funcs
    }
}

#[allow(clippy::missing_safety_doc)]
unsafe extern "C" fn encoder_destroy_callback<T: DriverEncoder>(
    encoder: *mut bindings::drm_encoder,
) {
    // SAFETY: DRM guarantees that `encoder` points to a valid initialized `drm_encoder`.
    unsafe { bindings::drm_encoder_cleanup(encoder) };

    // SAFETY:
    // - DRM guarantees we are now the only one with access to this [`drm_encoder`].
    // - This cast is safe via `DriverEncoder`s type invariants.
    drop(unsafe {
        KBox::from_raw(
            container_of!(Opaque::cast_from(encoder), Encoder<T>, encoder).cast_mut()
        )
    });
}

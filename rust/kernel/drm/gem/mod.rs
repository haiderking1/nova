// SPDX-License-Identifier: GPL-2.0 OR MIT

//! DRM GEM API
//!
//! C header: [`include/drm/drm_gem.h`](srctree/include/drm/drm_gem.h)

use crate::{
    bindings,
    drm::{
        self,
        device::{
            DeviceContext,
            Normal, //
        },
        driver::{
            AllocImpl,
            AllocOps, //
        },
    },
    error::to_result,
    mm::virt::VmaNew,
    prelude::*,
    sync::aref::{
        ARef,
        AlwaysRefCounted, //
    },
    types::Opaque,
};
use core::{
    marker::PhantomData,
    ops::Deref,
    ptr::NonNull, //
};

#[cfg(CONFIG_RUST_DRM_GEM_SHMEM_HELPER)]
pub mod shmem;

/// A macro for implementing [`AlwaysRefCounted`] for any GEM object type.
///
/// Since all GEM objects use the same refcounting scheme.
#[macro_export]
macro_rules! impl_aref_for_gem_obj {
    (
        impl $( <$( $tparam_id:ident ),+> )? for $type:ty
        $(
            where
                $( $bind_param:path : $bind_trait:path ),+
        )?
    ) => {
        // SAFETY: All GEM objects are refcounted.
        unsafe impl $( <$( $tparam_id ),+> )? $crate::sync::aref::AlwaysRefCounted for $type
        where
            Self: IntoGEMObject,
            $( $( $bind_param : $bind_trait ),+ )?
        {
            fn inc_ref(&self) {
                // SAFETY: The existence of a shared reference guarantees that the refcount is
                // non-zero.
                unsafe { bindings::drm_gem_object_get(self.as_raw()) };
            }

            unsafe fn dec_ref(obj: core::ptr::NonNull<Self>) {
                // SAFETY: `obj` is a valid pointer to an `Object<T>`.
                let obj = unsafe { obj.as_ref() }.as_raw();

                // SAFETY: The safety requirements guarantee that the refcount is non-zero.
                unsafe { bindings::drm_gem_object_put(obj) };
            }
        }
    };
}
#[cfg_attr(not(CONFIG_RUST_DRM_GEM_SHMEM_HELPER), allow(unused))]
pub(crate) use impl_aref_for_gem_obj;

/// A type alias for retrieving a [`Driver`]s [`DriverFile`] implementation from its
/// [`DriverObject`] implementation.
///
/// [`Driver`]: drm::Driver
/// [`DriverFile`]: drm::file::DriverFile
pub type DriverFile<T> = drm::File<<<T as DriverObject>::Driver as drm::Driver>::File>;

/// A type alias for retrieving the current [`AllocImpl`] for a given [`DriverObject`].
///
/// [`Driver`]: drm::Driver
pub type DriverAllocImpl<T> = <<T as DriverObject>::Driver as drm::Driver>::Object;

/// GEM object functions, which must be implemented by drivers.
pub trait DriverObject: Sync + Send + Sized + 'static {
    /// Parent `Driver` for this object.
    type Driver: drm::Driver;

    /// The data type to use for passing arguments to [`DriverObject::new`].
    type Args;

    /// Create a new driver data object for a GEM object of a given size.
    fn new(
        dev: &drm::Device<Self::Driver>,
        size: usize,
        args: Self::Args,
    ) -> impl PinInit<Self, Error>;

    /// Open a new handle to an existing object, associated with a File.
    fn open(_obj: &DriverAllocImpl<Self>, _file: &DriverFile<Self>) -> Result {
        Ok(())
    }

    /// Close a handle to an existing object, associated with a File.
    fn close(_obj: &DriverAllocImpl<Self>, _file: &DriverFile<Self>) {}

    /// Whether [`Object`]s have no shmem file backing them (`drm_gem_private_object_init()`): the
    /// driver provides their memory, and maps it with [`DriverObject::mmap`] and
    /// [`DriverObject::vmap`].
    ///
    /// Only used by [`Object`].
    const PRIVATE: bool = false;

    /// Maps `obj` into the address space of a process, with `vma`, which spans at most its size.
    ///
    /// Each mapping holds a reference to `obj`. Only used by [`Object`].
    fn mmap(_obj: &Object<Self>, _vma: &VmaNew) -> Result {
        Err(ENODEV)
    }

    /// Maps the memory of `obj` into the kernel's address space, until [`DriverObject::vunmap`].
    ///
    /// Only used by [`Object`].
    fn vmap(_obj: &Object<Self>) -> Result<KernelMapping> {
        Err(EOPNOTSUPP)
    }

    /// Ends `map`, a mapping returned by [`DriverObject::vmap`].
    ///
    /// Only used by [`Object`].
    fn vunmap(_obj: &Object<Self>, _map: KernelMapping) {}

    /// Returns the arguments of the object of a dumb buffer of `size` bytes, a buffer the CPU
    /// draws into for display, or fails if the driver has no dumb buffers.
    ///
    /// Only used by [`Object`].
    fn dumb_args(_dev: &drm::Device<Self::Driver>, _size: usize) -> Result<Self::Args> {
        Err(EOPNOTSUPP)
    }
}

/// A mapping of the memory of a GEM object into the kernel's address space.
#[derive(Clone, Copy, Debug)]
pub enum KernelMapping {
    /// Of system memory.
    System(NonNull<u8>),
    /// Of I/O memory, such as a PCI BAR showing the memory of a device.
    Io(NonNull<u8>),
}

/// The operations of the vmas mapping [`Object`]s.
struct VmOps(bindings::vm_operations_struct);

// SAFETY: The operations are never written, and their pointers are null.
unsafe impl Sync for VmOps {}

/// The operations of the vmas mapping [`Object`]s: each holds a reference to its object, like the
/// vmas of the objects of DRM's GEM helpers.
static VM_OPS: VmOps = VmOps(bindings::vm_operations_struct {
    open: Some(bindings::drm_gem_vm_open),
    close: Some(bindings::drm_gem_vm_close),
    ..pin_init::zeroed()
});

extern "C" fn mmap_callback<T: DriverObject>(
    raw_obj: *mut bindings::drm_gem_object,
    raw_vma: *mut bindings::vm_area_struct,
) -> core::ffi::c_int {
    // SAFETY: `mmap_callback` is only in the functions of `Object<T>`, so `raw_obj` is contained
    // within an `Object<T>`.
    let obj: &Object<T> = unsafe { IntoGEMObject::from_raw(raw_obj) };

    // SAFETY: DRM calls `mmap` from `drm_gem_mmap_obj()`, in the `mmap` file operation, while the
    // vma is set up.
    let vma = unsafe { VmaNew::from_raw(raw_vma) };

    // `drm_gem_mmap_obj()` checked that the vma spans at most the object, which it must keep to.
    vma.set_dontexpand();

    match T::mmap(obj, vma) {
        Ok(()) => 0,
        Err(e) => e.to_errno(),
    }
}

extern "C" fn vmap_callback<T: DriverObject>(
    raw_obj: *mut bindings::drm_gem_object,
    map: *mut bindings::iosys_map,
) -> core::ffi::c_int {
    // SAFETY: `vmap_callback` is only in the functions of `Object<T>`, so `raw_obj` is contained
    // within an `Object<T>`.
    let obj: &Object<T> = unsafe { IntoGEMObject::from_raw(raw_obj) };

    let (vaddr, is_iomem) = match T::vmap(obj) {
        Ok(KernelMapping::System(vaddr)) => (vaddr, false),
        Ok(KernelMapping::Io(vaddr)) => (vaddr, true),
        Err(e) => return e.to_errno(),
    };

    // SAFETY: DRM passes a valid `map` to fill in. Both members of its union are pointers.
    unsafe {
        map.write(bindings::iosys_map {
            is_iomem,
            __bindgen_anon_1: bindings::iosys_map__bindgen_ty_1 {
                vaddr: vaddr.as_ptr().cast(),
            },
        })
    };

    0
}

extern "C" fn vunmap_callback<T: DriverObject>(
    raw_obj: *mut bindings::drm_gem_object,
    map: *mut bindings::iosys_map,
) {
    // SAFETY: `vunmap_callback` is only in the functions of `Object<T>`, so `raw_obj` is contained
    // within an `Object<T>`.
    let obj: &Object<T> = unsafe { IntoGEMObject::from_raw(raw_obj) };

    // SAFETY: DRM passes the valid `map` `vmap_callback` filled in. Both members of its union are
    // pointers.
    let (vaddr, is_iomem) = unsafe { ((*map).__bindgen_anon_1.vaddr, (*map).is_iomem) };
    let Some(vaddr) = NonNull::new(vaddr.cast()) else {
        return;
    };

    T::vunmap(
        obj,
        if is_iomem {
            KernelMapping::Io(vaddr)
        } else {
            KernelMapping::System(vaddr)
        },
    );
}

unsafe extern "C" fn dumb_create_callback<T: DriverObject>(
    raw_file: *mut bindings::drm_file,
    raw_dev: *mut bindings::drm_device,
    args: *mut bindings::drm_mode_create_dumb,
) -> core::ffi::c_int {
    // SAFETY: DRM passes valid pointers. The device is the one of the driver of `T`, whose objects
    // are `Object<T>`s, as `dumb_create_callback` is only in their `AllocOps`.
    let dev: &drm::Device<T::Driver> = unsafe { drm::Device::from_raw(raw_dev) };

    let create = || -> Result {
        // SAFETY: DRM passes valid pointers.
        to_result(unsafe { bindings::drm_mode_size_dumb(raw_dev, args, 0, 0) })?;
        // SAFETY: `args` is valid, and `drm_mode_size_dumb()` filled in its size.
        let size = usize::try_from(unsafe { (*args).size })?;

        let obj = Object::<T>::new(dev, size, T::dumb_args(dev, size)?)?;
        let mut handle = 0;
        // SAFETY: `raw_file` and the object are valid. The handle holds its own reference.
        to_result(unsafe { bindings::drm_gem_handle_create(raw_file, obj.as_raw(), &mut handle) })?;
        // SAFETY: `args` is valid.
        unsafe { (*args).handle = handle };

        Ok(())
    };

    match create() {
        Ok(()) => 0,
        Err(e) => e.to_errno(),
    }
}

/// Trait that represents a GEM object subtype
pub trait IntoGEMObject: Sized + super::private::Sealed {
    /// Returns a reference to the raw `drm_gem_object` structure, which must be valid as long as
    /// this owning object is valid.
    fn as_raw(&self) -> *mut bindings::drm_gem_object;

    /// Converts a pointer to a `struct drm_gem_object` into a reference to `Self`.
    ///
    /// # Safety
    ///
    /// - `self_ptr` must be a valid pointer to the `struct drm_gem_object` embedded in a
    ///   valid instance of `Self`.
    /// - The caller promises that holding the immutable reference returned by this function does
    ///   not violate rust's data aliasing rules and remains valid throughout the lifetime of `'a`.
    unsafe fn from_raw<'a>(self_ptr: *mut bindings::drm_gem_object) -> &'a Self;
}

extern "C" fn open_callback<T: DriverObject>(
    raw_obj: *mut bindings::drm_gem_object,
    raw_file: *mut bindings::drm_file,
) -> core::ffi::c_int {
    // SAFETY: `open_callback` is only ever called with a valid pointer to a `struct drm_file`.
    let file = unsafe { DriverFile::<T>::from_raw(raw_file) };

    // SAFETY:
    // * `open_callback` is specified in the AllocOps structure for `DriverObject`, ensuring that
    //   `raw_obj` is contained within a `DriverAllocImpl<T>`
    // * It is only possible for `open_callback` to be called after device registration, ensuring
    //   that the object's device is in the `Registered` state.
    let obj: &DriverAllocImpl<T> = unsafe { IntoGEMObject::from_raw(raw_obj) };

    match T::open(obj, file) {
        Err(e) => e.to_errno(),
        Ok(()) => 0,
    }
}

extern "C" fn close_callback<T: DriverObject>(
    raw_obj: *mut bindings::drm_gem_object,
    raw_file: *mut bindings::drm_file,
) {
    // SAFETY: `open_callback` is only ever called with a valid pointer to a `struct drm_file`.
    let file = unsafe { DriverFile::<T>::from_raw(raw_file) };

    // SAFETY: `close_callback` is specified in the AllocOps structure for `Object<T>`, ensuring
    // that `raw_obj` is indeed contained within a `Object<T>`.
    let obj: &DriverAllocImpl<T> = unsafe { IntoGEMObject::from_raw(raw_obj) };

    T::close(obj, file);
}

impl<T: DriverObject, Ctx: DeviceContext> IntoGEMObject for Object<T, Ctx> {
    fn as_raw(&self) -> *mut bindings::drm_gem_object {
        self.obj.get()
    }

    unsafe fn from_raw<'a>(self_ptr: *mut bindings::drm_gem_object) -> &'a Self {
        // SAFETY: `obj` is guaranteed to be in an `Object<T>` via the safety contract of this
        // function
        unsafe { &*crate::container_of!(Opaque::cast_from(self_ptr), Object<T, Ctx>, obj) }
    }
}

/// Base operations shared by all GEM object classes
pub trait BaseObject: IntoGEMObject {
    /// Returns the size of the object in bytes.
    fn size(&self) -> usize {
        // SAFETY: `self.as_raw()` is guaranteed to be a pointer to a valid `struct drm_gem_object`.
        unsafe { (*self.as_raw()).size }
    }

    /// Creates a new handle for the object associated with a given `File`
    /// (or returns an existing one).
    fn create_handle<D, F>(&self, file: &drm::File<F>) -> Result<u32>
    where
        Self: AllocImpl<Driver = D>,
        D: drm::Driver<Object = Self, File = F>,
        F: drm::file::DriverFile<Driver = D>,
    {
        let mut handle: u32 = 0;
        // SAFETY: The arguments are all valid per the type invariants.
        to_result(unsafe {
            bindings::drm_gem_handle_create(file.as_raw().cast(), self.as_raw(), &mut handle)
        })?;
        Ok(handle)
    }

    /// Looks up an object by its handle for a given `File`.
    fn lookup_handle<D, F>(file: &drm::File<F>, handle: u32) -> Result<ARef<Self>>
    where
        Self: AllocImpl<Driver = D> + AlwaysRefCounted,
        D: drm::Driver<Object = Self, File = F>,
        F: drm::file::DriverFile<Driver = D>,
    {
        // SAFETY: The arguments are all valid per the type invariants.
        let ptr = unsafe { bindings::drm_gem_object_lookup(file.as_raw().cast(), handle) };
        if ptr.is_null() {
            return Err(ENOENT);
        }

        // SAFETY:
        // - A `drm::Driver` can only have a single `File` implementation.
        // - `file` uses the same `drm::Driver` as `Self`.
        // - Therefore, we're guaranteed that `ptr` must be a gem object embedded within `Self`.
        // - And we check if the pointer is null befoe calling from_raw(), ensuring that `ptr` is a
        //   valid pointer to an initialized `Self`.
        let obj = unsafe { Self::from_raw(ptr) };

        // SAFETY:
        // - We take ownership of the reference of `drm_gem_object_lookup()`.
        // - Our `NonNull` comes from an immutable reference, thus ensuring it is a valid pointer to
        //   `Self`.
        Ok(unsafe { ARef::from_raw(obj.into()) })
    }

    /// Creates an mmap offset to map the object from userspace.
    fn create_mmap_offset(&self) -> Result<u64> {
        // SAFETY: The arguments are valid per the type invariant.
        to_result(unsafe { bindings::drm_gem_create_mmap_offset(self.as_raw()) })?;

        // SAFETY: The arguments are valid per the type invariant.
        Ok(unsafe { bindings::drm_vma_node_offset_addr(&raw mut (*self.as_raw()).vma_node) })
    }
}

impl<T: IntoGEMObject> BaseObject for T {}

/// Crate-private base operations shared by all GEM object classes.
#[cfg_attr(not(CONFIG_RUST_DRM_GEM_SHMEM_HELPER), expect(unused))]
pub(crate) trait BaseObjectPrivate: IntoGEMObject {
    /// Return a pointer to this object's dma_resv.
    fn raw_dma_resv(&self) -> *mut bindings::dma_resv {
        // SAFETY: `self.as_raw()` always returns a valid pointer to the base DRM GEM object.
        unsafe { (*self.as_raw()).resv }
    }
}

impl<T: IntoGEMObject> BaseObjectPrivate for T {}

/// A base GEM object.
///
/// # Invariants
///
/// * `self.obj` is a valid instance of a `struct drm_gem_object`.
/// * Any type invariants of `Ctx` apply to the parent DRM device for this GEM object.
#[repr(C)]
#[pin_data]
pub struct Object<T: DriverObject + Send + Sync, Ctx: DeviceContext = Normal> {
    obj: Opaque<bindings::drm_gem_object>,
    #[pin]
    data: T,
    _ctx: PhantomData<Ctx>,
}

impl<T: DriverObject, Ctx: DeviceContext> Object<T, Ctx> {
    const OBJECT_FUNCS: bindings::drm_gem_object_funcs = bindings::drm_gem_object_funcs {
        free: Some(Self::free_callback),
        open: Some(open_callback::<T>),
        close: Some(close_callback::<T>),
        print_info: None,
        export: None,
        pin: None,
        unpin: None,
        get_sg_table: None,
        vmap: Some(vmap_callback::<T>),
        vunmap: Some(vunmap_callback::<T>),
        mmap: Some(mmap_callback::<T>),
        status: None,
        vm_ops: &raw const VM_OPS.0,
        evict: None,
        rss: None,
    };

    /// Returns the `Device` that owns this GEM object.
    pub fn dev(&self) -> &drm::Device<T::Driver, Ctx> {
        // SAFETY:
        // - `struct drm_gem_object.dev` is initialized and valid for as long as the GEM
        //   object lives.
        // - The device we used for creating the gem object is passed as &drm::Device<T::Driver> to
        //   Object::<T>::new(), so we know that `T::Driver` is the right generic parameter to use
        //   here.
        // - Any type invariants of `Ctx` are upheld by using the same `Ctx` for the `Device` we
        //   return.
        unsafe { drm::Device::from_raw((*self.as_raw()).dev) }
    }

    fn as_raw(&self) -> *mut bindings::drm_gem_object {
        self.obj.get()
    }

    extern "C" fn free_callback(obj: *mut bindings::drm_gem_object) {
        let ptr: *mut Opaque<bindings::drm_gem_object> = obj.cast();

        // SAFETY: All of our objects are of type `Object<T>`.
        let this = unsafe { crate::container_of!(ptr, Self, obj) };

        // SAFETY: The C code only ever calls this callback with a valid pointer to a `struct
        // drm_gem_object`.
        unsafe { bindings::drm_gem_object_release(obj) };

        // SAFETY: All of our objects are allocated via `KBox`, and we're in the
        // free callback which guarantees this object has zero remaining references,
        // so we can drop it.
        let _ = unsafe { KBox::from_raw(this) };
    }
}

impl<T: DriverObject> Object<T> {
    /// Create a new GEM object.
    pub fn new(dev: &drm::Device<T::Driver>, size: usize, args: T::Args) -> Result<ARef<Self>> {
        let obj: Pin<KBox<Self>> = KBox::pin_init(
            try_pin_init!(Self {
                obj: Opaque::new(bindings::drm_gem_object::default()),
                data <- T::new(dev, size, args),
                _ctx: PhantomData,
            }),
            GFP_KERNEL,
        )?;

        // SAFETY: `obj.as_raw()` is guaranteed to be valid by the initialization above.
        unsafe { (*obj.as_raw()).funcs = &Self::OBJECT_FUNCS };

        // INVARIANT: `dev` and the GEM object are in the same state at the moment, and upgrading
        // the typestate in `dev` will not carry over to the GEM object.
        if T::PRIVATE {
            // SAFETY: The arguments are all valid per the type invariants.
            unsafe { bindings::drm_gem_private_object_init(dev.as_raw(), obj.obj.get(), size) };
        } else if let Err(err) =
            // SAFETY: The arguments are all valid per the type invariants.
            to_result(unsafe {
                bindings::drm_gem_object_init(dev.as_raw(), obj.obj.get(), size)
            })
        {
            // SAFETY: `drm_gem_object_init()` initializes the private GEM object state before
            // failing, so `drm_gem_private_object_fini()` is the matching cleanup.
            unsafe { bindings::drm_gem_private_object_fini(obj.obj.get()) };
            return Err(err);
        }

        // SAFETY: We will never move out of `Self` as `ARef<Self>` is always treated as pinned.
        let ptr = KBox::into_raw(unsafe { Pin::into_inner_unchecked(obj) });

        // SAFETY: `ptr` comes from `KBox::into_raw` and hence can't be NULL.
        let ptr = unsafe { NonNull::new_unchecked(ptr) };

        // SAFETY: We take over the initial reference count from `drm_gem_object_init()`.
        Ok(unsafe { ARef::from_raw(ptr) })
    }
}

impl_aref_for_gem_obj! {
    impl<T> for Object<T>
    where
        T: DriverObject
}

impl<T: DriverObject, Ctx: DeviceContext> super::private::Sealed for Object<T, Ctx> {}

impl<T: DriverObject, Ctx: DeviceContext> Deref for Object<T, Ctx> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl<T: DriverObject, Ctx: DeviceContext> AllocImpl for Object<T, Ctx> {
    type Driver = T::Driver;

    const ALLOC_OPS: AllocOps = AllocOps {
        gem_create_object: None,
        prime_handle_to_fd: None,
        prime_fd_to_handle: None,
        gem_prime_import: None,
        gem_prime_import_sg_table: None,
        dumb_create: Some(dumb_create_callback::<T>),
        dumb_map_offset: None,
        // The CPU maps the objects with `vmap`, which the shadow buffer of fbdev is copied to.
        #[cfg(CONFIG_DRM_FBDEV_EMULATION = "y")]
        fbdev_probe: if drm::Device::<T::Driver>::has_kms() {
            Some(bindings::drm_fbdev_shadow_driver_fbdev_probe)
        } else {
            None
        },
        #[cfg(CONFIG_DRM_FBDEV_EMULATION = "n")]
        fbdev_probe: None,
    };
}

pub(super) const fn create_fops(owner: *mut bindings::module) -> bindings::file_operations {
    let mut fops: bindings::file_operations = pin_init::zeroed();

    fops.owner = owner;
    fops.open = Some(bindings::drm_open);
    fops.release = Some(bindings::drm_release);
    fops.unlocked_ioctl = Some(bindings::drm_ioctl);
    #[cfg(CONFIG_COMPAT)]
    {
        fops.compat_ioctl = Some(bindings::drm_compat_ioctl);
    }
    fops.poll = Some(bindings::drm_poll);
    fops.read = Some(bindings::drm_read);
    fops.llseek = Some(bindings::noop_llseek);
    fops.mmap = Some(bindings::drm_gem_mmap);
    fops.fop_flags = bindings::FOP_UNSIGNED_OFFSET;

    fops
}

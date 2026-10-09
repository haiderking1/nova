// SPDX-License-Identifier: GPL-2.0

use crate::driver::{
    DrmRegData,
    NovaDevice,
    NovaDriver, //
};
use crate::gem::NovaObject;
use kernel::{
    alloc::flags::*,
    drm::{
        self,
        gem::BaseObject,
        Registered, //
    },
    num::casts::{
        arch::IntoSafeCastArch,
        IntoSafeCast, //
    },
    prelude::*,
    transmute::AsBytes,
    uaccess::UserSlice,
    uapi,
};

pub(crate) struct File;

/// GPU information returned to userspace.
#[repr(transparent)]
struct GpuInfo(uapi::drm_nova_info_gpu);

/// Copies `name` into the zero initialised, fixed size uAPI buffer `dst`, keeping it
/// NUL-terminated.
///
/// Fails with [`ENAMETOOLONG`] if `name` does not fit in `dst` with room for the terminator.
fn copy_name(dst: &mut [u8], name: &str) -> Result {
    let bytes = name.as_bytes();

    if bytes.len() >= dst.len() {
        return Err(ENAMETOOLONG);
    }
    dst[..bytes.len()].copy_from_slice(bytes);

    Ok(())
}

impl GpuInfo {
    /// Collects the GPU information reported to userspace.
    ///
    /// This is fallible so that unexpected GSP behaviour, such as a malformed name string, is
    /// reported to userspace instead of being silently replaced with a made up value. When adding
    /// a field, take care that a value the GSP simply does not provide, for example because the
    /// firmware predates it, does not cause a failure. Such fields must fall back to zero or
    /// another documented default instead.
    fn new(reg_data: &DrmRegData<'_>) -> Result<Self> {
        let spec = reg_data.api.with(|api| api.get_ref().spec());
        let gsp_static_info = reg_data.api.with(|api| api.get_ref().gsp_static_info());

        let mut info = uapi::drm_nova_info_gpu {
            architecture: spec.chipset.arch().into(),
            chipid: spec.chipset.into(),
            vram_size: gsp_static_info.vram_size(),
            vram_bar_size: reg_data.api.with(|api| api.bar1_size())?,
            gpu_gid: gsp_static_info.gpu_gid,
            ..pin_init::zeroed()
        };

        copy_name(
            &mut info.gpu_name,
            gsp_static_info.gpu_name().map_err(|_| EINVAL)?,
        )?;

        copy_name(
            &mut info.gpu_short_name,
            gsp_static_info.gpu_short_name().map_err(|_| EINVAL)?,
        )?;

        Ok(Self(info))
    }
}

// SAFETY: `GpuInfo` has no interior mutability, and there are no uninitialized bytes.
unsafe impl AsBytes for GpuInfo {}

/// Graphics engine information returned to userspace.
#[repr(transparent)]
struct GraphicsInfo(uapi::drm_nova_info_graphics);

impl GraphicsInfo {
    /// Collects the graphics engine information reported to userspace.
    ///
    /// Fails with [`ENODEV`] if the graphics engine isn't set up.
    fn new(reg_data: &DrmRegData<'_>) -> Result<Self> {
        let info = reg_data.api.with(|api| api.graphics_info())?;

        Ok(Self(uapi::drm_nova_info_graphics {
            sm_major: info.sm_major,
            sm_minor: info.sm_minor,
            gpc_count: info.gpcs,
            tpc_count: info.tpcs,
            sm_per_tpc: info.sms_per_tpc,
            max_warps_per_sm: info.max_warps_per_sm,
            class_gpfifo: info.classes.gpfifo,
            class_copy: info.classes.copy,
            class_2d: info.classes.two_d,
            class_3d: info.classes.three_d,
            class_m2mf: info.classes.inline_to_memory,
            class_compute: info.classes.compute,
            ..pin_init::zeroed()
        }))
    }
}

// SAFETY: `GraphicsInfo` has no interior mutability, and there are no uninitialized bytes: its
// fields leave no padding.
unsafe impl AsBytes for GraphicsInfo {}

fn write_info<T: AsBytes>(info: &mut uapi::drm_nova_info, value: T) -> Result {
    // A NULL data pointer requests the size of the information structure.
    if info.data == 0 {
        info.size = size_of::<T>().try_into()?;
        return Ok(());
    }

    let mut writer = UserSlice::new(
        UserPtr::from_addr(info.data.into_safe_cast_arch()),
        info.size.into_safe_cast(),
    )
    .writer();

    info.size = writer.write_truncated(&value)?.try_into()?;

    Ok(())
}

impl drm::file::DriverFile for File {
    type Driver = NovaDriver;

    fn open(_dev: &NovaDevice) -> Result<Pin<KBox<Self>>> {
        Ok(KBox::new(Self, GFP_KERNEL)?.into())
    }
}

impl File {
    /// IOCTL: gem_create: Create a new DRM GEM object.
    pub(crate) fn gem_create(
        dev: &NovaDevice<Registered>,
        _reg_data: &DrmRegData<'_>,
        req: &mut uapi::drm_nova_gem_create,
        file: &drm::File<File>,
    ) -> Result<u32> {
        let obj = NovaObject::new(dev, req.size.try_into()?)?;

        req.handle = obj.create_handle(file)?;

        Ok(0)
    }

    /// IOCTL: gem_info: Query GEM metadata.
    pub(crate) fn gem_info(
        _dev: &NovaDevice<Registered>,
        _reg_data: &DrmRegData<'_>,
        req: &mut uapi::drm_nova_gem_info,
        file: &drm::File<File>,
    ) -> Result<u32> {
        let bo = NovaObject::lookup_handle(file, req.handle)?;

        req.size = bo.size().try_into()?;

        Ok(0)
    }

    /// IOCTL: info: Query device information.
    pub(crate) fn info(
        _dev: &NovaDevice<Registered>,
        reg_data: &DrmRegData<'_>,
        info: &mut uapi::drm_nova_info,
        _file: &drm::File<File>,
    ) -> Result<u32> {
        match info.id {
            uapi::DRM_NOVA_INFO_GPU => write_info(info, GpuInfo::new(reg_data)?)?,
            uapi::DRM_NOVA_INFO_GRAPHICS => write_info(info, GraphicsInfo::new(reg_data)?)?,
            _ => return Err(EINVAL),
        }

        Ok(0)
    }
}

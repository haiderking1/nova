// SPDX-License-Identifier: GPL-2.0
//! RVKMS's DRM File implementation.
use super::RvkmsDriver;

use kernel::{
    alloc::*,
    drm::{self, device::Device as DrmDevice},
    prelude::*,
};

pub(crate) struct File;

impl drm::file::DriverFile for File {
    type Driver = RvkmsDriver;

    fn open(_device: &DrmDevice<Self::Driver>) -> Result<Pin<KBox<Self>>> {
        Box::pin_init(init!(File {}), GFP_KERNEL)
    }
}

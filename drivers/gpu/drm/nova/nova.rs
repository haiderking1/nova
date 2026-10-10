// SPDX-License-Identifier: GPL-2.0

//! Nova DRM Driver

mod dp;
mod driver;
mod file;
mod gem;
mod kms;
mod vm;

use crate::driver::NovaDriver;

kernel::module_auxiliary_driver! {
    type: NovaDriver,
    name: "nova-drm",
    authors: ["Danilo Krummrich"],
    description: "Nova GPU driver",
    license: "GPL v2",
}

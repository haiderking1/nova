// SPDX-License-Identifier: GPL-2.0
//! RVKMS's DRM GEM implementation
use crate::{RvkmsDevice, RvkmsDriver};
use core::sync::atomic::{AtomicU64, Ordering};
use kernel::{drm::gem, prelude::*};

static GEM_ID: AtomicU64 = AtomicU64::new(0);

/// GEM Object implementation
#[pin_data]
pub(crate) struct DriverObject {
    /// ID for debugging
    id: u64,
}

pub(crate) type Object = gem::shmem::Object<DriverObject>;

impl gem::DriverObject for DriverObject {
    type Driver = RvkmsDriver;
    type Args = ();

    fn new(_dev: &RvkmsDevice, _size: usize, _args: ()) -> impl PinInit<Self, Error> {
        let id = GEM_ID.fetch_add(1, Ordering::Relaxed);

        pr_debug!("DriverObject::new id={id}\n");
        try_pin_init!(DriverObject { id })
    }
}

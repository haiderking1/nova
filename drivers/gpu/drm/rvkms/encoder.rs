// SPDX-License-Identifier: GPL-2.0
//! RVKMS's DRM Encoder implementation
use crate::RvkmsDriver;
use core::marker::PhantomPinned;
use kernel::{
    drm::{
        Device,
        Normal, //
        kms::{
            encoder,
            KmsProbe, //
        },
    },
    prelude::*,
};

#[pin_data]
pub(crate) struct DriverEncoder {
    #[pin]
    _p: PhantomPinned,
}

#[allow(unused)]
pub(crate) type Encoder<C = Normal> = encoder::Encoder<DriverEncoder, C>;

#[vtable]
impl encoder::DriverEncoder for DriverEncoder {
    type Driver = RvkmsDriver;
    type Args = ();

    fn new(
        _device: &Device<Self::Driver, KmsProbe>,
        _args: Self::Args
    ) -> impl PinInit<Self, Error> {
        try_pin_init!(Self { _p: PhantomPinned })
    }
}

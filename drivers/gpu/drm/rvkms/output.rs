// SPDX-License-Identifier: GPL-2.0
//! RVKMS's display outputs.
use crate::{
    connector::Connector,
    crtc::Crtc,
    encoder::Encoder,
    plane::Plane,
    RvkmsDriver,
};
use kernel::{
    drm::{
        Device,
        fourcc::*,
        kms::{
            connector,
            encoder,
            plane,
            KmsProbe, //
        },
    },
    prelude::*,
};

pub(crate) fn create_output(dev: &Device<RvkmsDriver, KmsProbe>, index: u8) -> Result {
    let possible_crtcs = 1 << index;

    let primary = Plane::<_>::new(
        dev,
        possible_crtcs,
        &[XRGB888],
        None,
        plane::Type::Primary,
        None,
        (),
    )?;

    let _crtc = Crtc::<_>::new(dev, primary, Option::<&Plane<KmsProbe>>::None, None, ())?;
    let connector = Connector::<_>::new(dev, connector::Type::Virtual, ())?;
    let encoder = Encoder::<_>::new(dev, encoder::Type::Virtual, possible_crtcs, 0, None, ())?;

    connector.attach_encoder(encoder)
}

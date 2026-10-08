// SPDX-License-Identifier: GPL-2.0
//! RVKMS's DRM plane implementation
use super::RvkmsDriver;
use core::marker::PhantomPinned;
use kernel::{
    drm::{
        Device,
        Normal,
        kms::{
            KmsProbe,
            plane::{
                self,
                DriverPlaneState,
                RawPlaneState
            },
        },
    },
    prelude::*,
};

#[pin_data]
pub(crate) struct RvkmsPlane {
    #[pin]
    _p: PhantomPinned,
}

pub(crate) type Plane<C = Normal> = plane::Plane<RvkmsPlane, C>;
//pub(crate) type PlaneState = plane::PlaneState<RvkmsPlaneState>;

#[vtable]
impl plane::DriverPlane for RvkmsPlane {
    type State = RvkmsPlaneState;
    type Driver = RvkmsDriver;
    type Args = ();

    fn new(
        _device: &Device<Self::Driver, KmsProbe>,
        _args: Self::Args
    ) -> impl PinInit<Self, Error> {
        try_pin_init!(Self { _p: PhantomPinned })
    }

    fn atomic_check(check: plane::PlaneAtomicCheck<'_, RvkmsPlane>) -> Result {
        let (state, mut new_state) = check.take_state_new_state();

        if new_state.framebuffer().is_none() {
            return Ok(());
        }

        if let Some(crtc) = new_state.crtc() {
            let crtc_state = state.add_crtc_state(crtc)?;
            new_state.atomic_helper_check(&crtc_state, true, true)
        } else {
            // TODO: We should be printing a warning here if we have no CRTC but do have an fb
            Ok(())
        }
    }

    fn atomic_update(_commit: plane::PlaneAtomicCommit<'_, RvkmsPlane>) {
        // TODO, no-op for now
    }
}

#[derive(Clone, Default)]
pub(crate) struct RvkmsPlaneState;

impl DriverPlaneState for RvkmsPlaneState {
    type Plane = RvkmsPlane;
    type Kind = plane::Plain;
}

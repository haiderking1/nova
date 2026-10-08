// SPDX-License-Identifier: GPL-2.0

//! Display support (kernel mode setting).
//!
//! nova-drm can't set modes yet. For now it drives the display the system firmware set up: one
//! output (primary plane, CRTC, encoder and connector) offering only the firmware's mode, which
//! the display engine keeps scanning out after nova-core boots the GSP.

use core::marker::{
    PhantomData,
    PhantomPinned, //
};

use kernel::{
    drm::{
        fourcc::XRGB888,
        kms::{
            connector::{
                self,
                ConnectorGuard, //
            },
            crtc::{
                self,
                DriverCrtcState, //
            },
            encoder,
            modes::{
                DisplayMode,
                ModeStatus, //
            },
            plane::{
                self,
                DriverPlaneState,
                RawPlaneState, //
            },
            KmsDriver,
            KmsProbe,
            ModeConfigGuard,
            ModeConfigInfo,
            ModeObject, //
        },
        Device,
        DeviceContext,
        Early,
        Normal, //
    },
    prelude::*,
};

use crate::driver::NovaDriver;

/// Largest framebuffer userspace may create, like C drivers using shadow planes.
const MAX_FB_SIZE: u32 = 4096;

pub(crate) type Plane<C = Normal> = plane::Plane<NovaPlane, C>;
pub(crate) type Crtc<C = Normal> = crtc::Crtc<NovaCrtc, C>;
pub(crate) type Encoder<C = Normal> = encoder::Encoder<NovaEncoder, C>;
pub(crate) type Connector<C = Normal> = connector::Connector<NovaConnector, C>;

#[vtable]
impl KmsDriver for NovaDriver {
    type Connector = NovaConnector;
    type Plane = NovaPlane;
    type Crtc = NovaCrtc;
    type Encoder = NovaEncoder;

    fn mode_config_info(drm: &Device<Self, Early>) -> Result<ModeConfigInfo> {
        let (width, height) = drm
            .boot_display
            .as_ref()
            .map_or((1, 1), |mode| (mode.hdisplay().into(), mode.vdisplay().into()));

        Ok(ModeConfigInfo {
            min_resolution: (width, height),
            max_resolution: (width.max(MAX_FB_SIZE), height.max(MAX_FB_SIZE)),
            max_cursor: (0, 0),
            preferred_depth: 24,
            preferred_fourcc: Some(XRGB888),
        })
    }

    fn probe(drm: &Device<Self, KmsProbe>) -> Result {
        // Without a display set up by the firmware there is nothing to drive yet.
        if drm.boot_display.is_none() {
            return Ok(());
        }

        let possible_crtcs = 1 << 0;
        let primary = Plane::new(
            drm,
            possible_crtcs,
            &[XRGB888],
            None,
            plane::Type::Primary,
            None,
            (),
        )?;
        let _crtc = Crtc::new(drm, primary, Option::<&Plane<KmsProbe>>::None, None, ())?;
        let encoder = Encoder::new(drm, encoder::Type::None, possible_crtcs, 0, None, ())?;
        // The firmware doesn't say what the display is connected through.
        let connector = Connector::new(drm, connector::Type::Unknown, ())?;

        connector.attach_encoder(encoder)
    }
}

/// The primary plane of the boot display.
#[pin_data]
pub(crate) struct NovaPlane {
    #[pin]
    _p: PhantomPinned,
}

#[vtable]
impl plane::DriverPlane for NovaPlane {
    type State = NovaPlaneState;
    type Driver = NovaDriver;
    type Args = ();

    fn new(_drm: &Device<NovaDriver, KmsProbe>, _args: Self::Args) -> impl PinInit<Self, Error> {
        try_pin_init!(Self { _p: PhantomPinned })
    }

    fn atomic_check(check: plane::PlaneAtomicCheck<'_, Self>) -> Result {
        let (state, mut new_state) = check.take_state_new_state();

        if new_state.framebuffer().is_none() {
            return Ok(());
        }
        let Some(crtc) = new_state.crtc() else {
            return Ok(());
        };

        // The plane covers the whole mode: no positioning, scaling or updates while disabled.
        let crtc_state = state.add_crtc_state(crtc)?;
        new_state.atomic_helper_check(&crtc_state, false, false)
    }

    fn atomic_update(_commit: plane::PlaneAtomicCommit<'_, Self>) {
        // Copying the framebuffer to the scanout buffer comes with shadow plane support.
    }
}

#[derive(Clone, Default)]
pub(crate) struct NovaPlaneState;

impl DriverPlaneState for NovaPlaneState {
    type Plane = NovaPlane;
}

/// The CRTC scanning out the boot display.
#[pin_data]
pub(crate) struct NovaCrtc {
    #[pin]
    _p: PhantomPinned,
}

#[vtable]
impl crtc::DriverCrtc for NovaCrtc {
    type Args = ();
    type Driver = NovaDriver;
    type State = NovaCrtcState;
    type VblankImpl = PhantomData<Self>;

    fn new(_drm: &Device<NovaDriver, KmsProbe>, _args: &Self::Args) -> impl PinInit<Self, Error> {
        try_pin_init!(Self { _p: PhantomPinned })
    }

    fn mode_valid(crtc: &Crtc, mode: &DisplayMode) -> ModeStatus {
        match crtc.drm_dev().boot_display.as_ref() {
            // nova-drm can't set modes yet: only the firmware's mode works.
            Some(boot_mode) => crtc.mode_valid_fixed(mode, boot_mode),
            None => ModeStatus::OK,
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct NovaCrtcState;

impl DriverCrtcState for NovaCrtcState {
    type Crtc = NovaCrtc;
}

/// The encoder of the boot display; the firmware doesn't tell which kind it is.
#[pin_data]
pub(crate) struct NovaEncoder {
    #[pin]
    _p: PhantomPinned,
}

#[vtable]
impl encoder::DriverEncoder for NovaEncoder {
    type Driver = NovaDriver;
    type Args = ();

    fn new(_drm: &Device<NovaDriver, KmsProbe>, _args: Self::Args) -> impl PinInit<Self, Error> {
        try_pin_init!(Self { _p: PhantomPinned })
    }
}

/// The connector of the boot display.
#[pin_data]
pub(crate) struct NovaConnector {
    #[pin]
    _p: PhantomPinned,
}

#[vtable]
impl connector::DriverConnector for NovaConnector {
    type State = NovaConnectorState;
    type Driver = NovaDriver;
    type Args = ();

    fn new<C: DeviceContext>(
        _drm: &Device<NovaDriver, C>,
        _args: Self::Args,
    ) -> impl PinInit<Self, Error> {
        try_pin_init!(Self { _p: PhantomPinned })
    }

    fn get_modes(connector: ConnectorGuard<'_, Self>, _guard: &ModeConfigGuard<'_, NovaDriver>) -> i32 {
        // Only created with a boot display, whose mode is the only one that works.
        connector
            .drm_dev()
            .boot_display
            .as_ref()
            .map_or(0, |mode| connector.add_fixed_mode(mode))
    }
}

#[derive(Clone, Default)]
pub(crate) struct NovaConnectorState;

impl connector::DriverConnectorState for NovaConnectorState {
    type Connector = NovaConnector;
}

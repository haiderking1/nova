// SPDX-License-Identifier: GPL-2.0

//! Display support (kernel mode setting).
//!
//! nova-drm can't set modes yet. For now it drives the display the system firmware set up: one
//! output (primary plane, CRTC, encoder and connector) offering only the firmware's mode, which
//! the display engine keeps scanning out after nova-core boots the GSP. Frames are copied by the
//! CPU from shadow-plane framebuffers into the firmware's framebuffer through nova-core.

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
                PlaneState,
                PlaneStateMutator,
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
    screen_info::{
        Channel,
        PixelFormat, //
    },
};

use crate::driver::{
    BootDisplay,
    NovaDevice,
    NovaDriver, //
};

/// Largest framebuffer userspace may create, like C drivers using shadow planes.
const MAX_FB_SIZE: u32 = 4096;

/// Bytes per pixel of the framebuffers the plane accepts (XRGB8888) and of the boot displays
/// nova-drm drives.
const CPP: usize = 4;

/// Pixel layout of XRGB8888, as `screen_info` reports it.
const XRGB8888_LAYOUT: PixelFormat = PixelFormat {
    bits_per_pixel: 32,
    alpha: Channel {
        offset: 0,
        length: 0,
    },
    red: Channel {
        offset: 16,
        length: 8,
    },
    green: Channel {
        offset: 8,
        length: 8,
    },
    blue: Channel {
        offset: 0,
        length: 8,
    },
};

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

    // Planes are copied to the display by the CPU, which needs to see every change.
    const FB_DIRTY: bool = true;

    fn mode_config_info(drm: &Device<Self, Early>) -> Result<ModeConfigInfo> {
        let (width, height) = drm.boot_display.as_ref().map_or((1, 1), |display| {
            (display.mode.hdisplay().into(), display.mode.vdisplay().into())
        });

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
        let Some(display) = drm.boot_display.as_ref() else {
            return Ok(());
        };
        if usize::from(display.format.bits_per_pixel) != CPP * 8 {
            pr_warn!(
                "nova-drm: boot display has {} bits per pixel, only 32 are supported\n",
                display.format.bits_per_pixel
            );
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

    fn atomic_update(commit: plane::PlaneAtomicCommit<'_, Self>) {
        let drm = commit.plane().drm_dev();
        let (old_state, new_state) = commit.take_old_new_state();

        if let Err(err) = copy_to_boot_display(drm, old_state, &new_state) {
            pr_err!("nova-drm: failed to update the display: {:?}\n", err);
        }
    }
}

/// Copies the parts of the framebuffer that changed since `old_state` to the boot display.
fn copy_to_boot_display(
    drm: &NovaDevice,
    old_state: &PlaneState<NovaPlaneState>,
    new_state: &PlaneStateMutator<'_, PlaneState<NovaPlaneState>>,
) -> Result {
    let Some(display) = drm.boot_display.as_ref() else {
        return Ok(());
    };
    // Nothing to do if the device is going away.
    let Some(registered) = drm.registration_guard() else {
        return Ok(());
    };

    let (src, dst) = (new_state.src(), new_state.dst());
    let stride = usize::try_from(display.stride)?;
    let mut row_buf = KVec::new();
    let mut convert_buf = KVec::new();

    registered.registration_data_with(|reg| {
        reg.api.with(|api| {
            for damage in new_state.damage(old_state) {
                // Damage is in framebuffer coordinates; the plane shows `src` at `dst`.
                let x = usize::try_from(dst.x1 + (damage.x1 - src.x1))?;

                new_state.read_rows(0, &damage, &mut row_buf, |fb_y, row| {
                    let y = usize::try_from(dst.y1 + (i32::try_from(fb_y)? - src.y1))?;
                    let pixels = convert(&display.format, row, &mut convert_buf)?;

                    api.write_boot_display(y * stride + x * CPP, pixels)
                })?;
            }

            Ok(())
        })
    })
}

/// Converts a row of XRGB8888 pixels to the pixel layout of `display`, which has 32 bits per
/// pixel, using `buf` if needed.
fn convert<'a>(format: &PixelFormat, row: &'a [u8], buf: &'a mut KVec<u8>) -> Result<&'a [u8]> {
    if *format == XRGB8888_LAYOUT {
        return Ok(row);
    }

    let channel = |c: &Channel, value: u32| match c.length {
        0 => 0,
        l @ 1..=8 => (value >> (8 - l)) << c.offset,
        l => (value << (l - 8)) << c.offset,
    };

    buf.clear();
    for pixel in row.chunks_exact(CPP) {
        let xrgb = u32::from_le_bytes([pixel[0], pixel[1], pixel[2], pixel[3]]);
        let encoded = channel(&format.red, (xrgb >> 16) & 0xff)
            | channel(&format.green, (xrgb >> 8) & 0xff)
            | channel(&format.blue, xrgb & 0xff);
        buf.extend_from_slice(&encoded.to_le_bytes(), GFP_KERNEL)?;
    }

    Ok(buf)
}

/// State of the boot display's plane, whose framebuffer the CPU copies to the display.
#[derive(Clone, Default)]
pub(crate) struct NovaPlaneState;

impl DriverPlaneState for NovaPlaneState {
    type Plane = NovaPlane;
    type Kind = plane::Shadow;
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
            Some(display) => crtc.mode_valid_fixed(mode, &display.mode),
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
        let display: Option<&BootDisplay> = connector.drm_dev().boot_display.as_ref();

        display.map_or(0, |display| connector.add_fixed_mode(&display.mode))
    }
}

#[derive(Clone, Default)]
pub(crate) struct NovaConnectorState;

impl connector::DriverConnectorState for NovaConnectorState {
    type Connector = NovaConnector;
}

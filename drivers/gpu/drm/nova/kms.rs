// SPDX-License-Identifier: GPL-2.0

//! Display support (kernel mode setting).
//!
//! nova-drm can't set modes yet. For now it drives the display the system firmware set up: one
//! output (primary plane, CRTC, encoder and connector) offering only the firmware's mode, which
//! the display engine keeps scanning out after nova-core boots the GSP. Frames are copied by the
//! CPU from shadow-plane framebuffers into the firmware's framebuffer through nova-core.
//!
//! The connector reports what GSP-RM knows about the display: the type of connector it is shown
//! through, and the EDID of the monitor (its name, physical size and modes, of which only those
//! of the firmware's resolution are kept).

use core::marker::{
    PhantomData,
    PhantomPinned, //
};

use kernel::{
    auxiliary,
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

use nova_core::api::ConnectorType;

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
        // Only the damaged parts of a frame are copied to the display.
        primary.enable_fb_damage_clips();
        let _crtc = Crtc::new(drm, primary, Option::<&Plane<KmsProbe>>::None, None, ())?;
        let (connector_type, encoder_type) = output_types(display.connector);
        let encoder = Encoder::new(drm, encoder_type, possible_crtcs, 0, None, ())?;
        let connector = Connector::new(drm, connector_type, ())?;

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

/// The encoder of the boot display.
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
        let drm = connector.drm_dev();
        let dev: &auxiliary::Device = drm.as_ref();
        // Only created with a boot display, whose mode is the only one that works.
        let Some(display): Option<&BootDisplay> = drm.boot_display.as_ref() else {
            return 0;
        };

        let edid = match read_edid_data(drm) {
            Ok(data) => {
                let edid = connector.read_edid(|block, buf| copy_edid_block(&data, block, buf));
                if edid.is_none() {
                    dev_warn!(
                        dev,
                        "the EDID of the boot display is invalid ({} bytes, starting {:02x?})\n",
                        data.len(),
                        data.get(..16).unwrap_or(&data)
                    );
                }
                edid
            }
            Err(err) => {
                dev_warn!(dev, "can't read the EDID of the boot display: {:?}\n", err);
                None
            }
        };

        // Without an EDID, this resets the connector's, so that the EDID of a monitor that may
        // have been unplugged isn't kept.
        match connector.update_edid(edid.as_ref()) {
            Ok(()) if edid.is_some() => {
                // The CRTC rejects the modes that differ from the boot display's.
                let count = connector.add_edid_modes();
                if count > 0 {
                    return count;
                }
                dev_warn!(dev, "the EDID of the boot display has no modes\n");
            }
            Ok(()) => {}
            Err(err) => dev_warn!(dev, "can't use the EDID of the boot display: {:?}\n", err),
        }

        connector.add_fixed_mode(&display.mode)
    }
}

/// Reads the raw EDID of the monitor the boot display is shown on, through nova-core.
fn read_edid_data(drm: &NovaDevice) -> Result<KVec<u8>> {
    // The connector is probed for modes once the device is registered.
    let registered = drm.registration_guard().ok_or(ENODEV)?;

    registered.registration_data_with(|reg| reg.api.with(|api| api.boot_display_edid()))
}

/// Copies EDID block number `block` out of the raw EDID `data` into `buf`.
///
/// GSP-RM may return more data than the EDID announces, which DRM leaves out by only reading the
/// blocks the EDID announces.
fn copy_edid_block(data: &[u8], block: usize, buf: &mut [u8]) -> Result {
    let start = block.checked_mul(buf.len()).ok_or(EINVAL)?;
    let src = data
        .get(start..)
        .and_then(|rest| rest.get(..buf.len()))
        .ok_or(EIO)?;

    buf.copy_from_slice(src);
    Ok(())
}

/// Returns the DRM connector and encoder types of an output shown through a connector of type
/// `connector`.
fn output_types(connector: ConnectorType) -> (connector::Type, encoder::Type) {
    match connector {
        ConnectorType::Vga => (connector::Type::Vga, encoder::Type::Dac),
        ConnectorType::DviI => (connector::Type::DviI, encoder::Type::Tmds),
        ConnectorType::DviD => (connector::Type::DviD, encoder::Type::Tmds),
        ConnectorType::DisplayPort => (connector::Type::DisplayPort, encoder::Type::Tmds),
        ConnectorType::EmbeddedDisplayPort => (connector::Type::Edp, encoder::Type::Tmds),
        ConnectorType::HdmiA | ConnectorType::HdmiC => (connector::Type::HdmiA, encoder::Type::Tmds),
        ConnectorType::UsbC => (connector::Type::Usb, encoder::Type::Tmds),
        ConnectorType::Dsi => (connector::Type::Dsi, encoder::Type::Dsi),
        ConnectorType::Unknown => (connector::Type::Unknown, encoder::Type::None),
    }
}

#[derive(Clone, Default)]
pub(crate) struct NovaConnectorState;

impl connector::DriverConnectorState for NovaConnectorState {
    type Connector = NovaConnector;
}

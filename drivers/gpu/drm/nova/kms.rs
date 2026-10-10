// SPDX-License-Identifier: GPL-2.0

//! Display support (kernel mode setting).
//!
//! nova-drm drives the display the system firmware set up: one output (primary plane, CRTC,
//! encoder and connector), on the head and output the firmware lit. The GPU's copy engine copies
//! frames from the framebuffers, in VRAM or system memory, into the back buffer of nova-core's own
//! scanout, which is then flipped, so frames show whole. Through that scanout, nova-core sets the
//! modes of the monitor's EDID that the display's link carries. Without it, frames are copied into
//! the firmware's framebuffer, which shows on screen as it is drawn, and only the firmware's mode
//! works.
//!
//! With nova-core's scanout, the CRTC has vblanks: nova-core passes on the vblank interrupts of the
//! head showing the scanout. A flip returns once the frame flipped is on screen, so a commit's
//! event is sent once its planes are updated, and a commit waits for that rather than for the
//! next vblank.
//!
//! The connector reports what GSP-RM knows about the display: the type of connector it is shown
//! through, and the EDID of the monitor (its name, physical size and modes).

use core::marker::PhantomPinned;

use kernel::{
    auxiliary,
    drm::{
        fourcc::XRGB888,
        kms::{
            atomic::{
                AtomicCommitTail,
                CommittedAtomicState,
                ModesetsReadyToken,
                PlaneUpdatesReadyToken, //
            },
            connector::{
                self,
                ConnectorGuard, //
            },
            crtc::{
                self,
                DriverCrtcState,
                RawCrtcState, //
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
                RawPlaneState,
                Rect, //
            },
            vblank::{
                RawVblankCrtcState,
                VblankGuard,
                VblankSupport,
                VblankTimestamp, //
            },
            KmsDriver,
            KmsProbe,
            KmsRef,
            ModeConfigGuard,
            ModeConfigInfo,
            ModeObject, //
        },
        Device,
        DeviceContext,
        Early,
        Normal, //
    },
    interrupt::LocalInterruptDisabled,
    prelude::*,
    screen_info::{
        Channel,
        PixelFormat, //
    },
    sync::Arc,
};

use nova_core::api::{
    ConnectorType,
    DisplayCopy,
    DisplaySource,
    ModeTimings,
    NovaCoreApi,
    PresentMode,
    VblankHandler, //
};

use crate::driver::{
    BootDisplay,
    NovaDevice,
    NovaDriver, //
};

/// Largest framebuffer userspace may create, the size of the largest scanouts.
const MAX_FB_SIZE: u32 = 4096;

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
            (
                display.mode.hdisplay().into(),
                display.mode.vdisplay().into(),
            )
        });

        Ok(ModeConfigInfo {
            min_resolution: (1, 1),
            max_resolution: (width.max(MAX_FB_SIZE), height.max(MAX_FB_SIZE)),
            max_cursor: (0, 0),
            preferred_depth: 24,
            preferred_fourcc: Some(XRGB888),
            // Flips of nova-core's scanout may tear, if userspace asks.
            async_page_flip: drm
                .boot_display
                .as_ref()
                .is_some_and(|display| display.scanout.is_some()),
        })
    }

    fn probe(drm: &Device<Self, KmsProbe>) -> Result {
        // Without a display set up by the firmware there is nothing to drive yet.
        let Some(display) = drm.boot_display.as_ref() else {
            return Ok(());
        };
        // Frames are copied as they are: without nova-core's scanout, into the framebuffer of the
        // boot display, whose pixels must then be XRGB8888 too.
        if display.scanout.is_none() && display.format != XRGB8888_LAYOUT {
            pr_warn!("nova-drm: boot display's pixels aren't XRGB8888, the only supported format\n");
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

    fn atomic_commit_tail<'a>(
        mut state: AtomicCommitTail<'a, Self>,
        modeset_token: ModesetsReadyToken<'_>,
        plane_update_token: PlaneUpdatesReadyToken<'_>,
    ) -> CommittedAtomicState<'a, Self> {
        // The CRTC's mode is set first: nova-core's scanout takes its size, which planes are drawn
        // into.
        let modeset_token = state.commit_modeset_disables(modeset_token);
        let modeset_token = state.commit_modeset_enables(modeset_token);
        let plane_update_token = state.commit_planes(plane_update_token, Default::default());
        state.fake_vblank();

        let state = state.commit_hw_done(modeset_token, plane_update_token);
        // The commit's event is sent once its frame is on screen, which is all to wait for: not
        // the vblank after, as the default commit tail does.
        state.wait_for_flip_done();

        state
    }
}

/// Runs `f` with nova-core's API, unless the device is going away. Doesn't sleep.
pub(crate) fn with_api<R>(
    drm: &NovaDevice,
    f: impl FnOnce(&NovaCoreApi<'_>) -> Result<R>,
) -> Result<R> {
    let registered = drm.registration_guard().ok_or(ENODEV)?;

    registered.registration_data_with(|reg| reg.api.with(|api| f(&api)))
}

/// Returns the timings of `mode`, which must be progressive.
fn mode_timings(mode: &DisplayMode) -> Result<ModeTimings> {
    if !mode.is_progressive() {
        return Err(EINVAL);
    }

    Ok(ModeTimings {
        clock_khz: u32::try_from(mode.clock())?,
        hsync_negative: mode.negative_hsync(),
        vsync_negative: mode.negative_vsync(),
        hdisplay: mode.hdisplay(),
        hsync_start: mode.hsync_start(),
        hsync_end: mode.hsync_end(),
        htotal: mode.htotal(),
        vdisplay: mode.vdisplay(),
        vsync_start: mode.vsync_start(),
        vsync_end: mode.vsync_end(),
        vtotal: mode.vtotal(),
    })
}

/// Returns the head showing nova-core's scanout, which the CRTC's vblanks are those of.
fn scanout_head(drm: &NovaDevice) -> Result<u32> {
    drm.boot_display
        .as_ref()
        .and_then(|display| display.scanout)
        .map(|scanout| scanout.head)
        .ok_or(ENODEV)
}

/// Passes on the vblanks of the head showing the CRTC to the CRTC.
struct NovaVblankHandler {
    crtc: KmsRef<Crtc>,
    head: u32,
}

impl VblankHandler for NovaVblankHandler {
    fn vblank(&self, head: u32) {
        if head == self.head {
            self.crtc.handle_vblank();
        }
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
        new_state.atomic_helper_check(&crtc_state, false, false)?;

        // After a modeset, nova-core's scanout holds nothing of the frame before, e.g. new
        // framebuffers for a new mode: the whole frame is copied, whatever the damage.
        new_state.atomic_helper_check_damage(state);

        Ok(())
    }

    fn atomic_update(commit: plane::PlaneAtomicCommit<'_, Self>) {
        let drm = commit.plane().drm_dev();
        let (state, old_state, mut new_state) = commit.take_all();

        let scanout = drm
            .boot_display
            .as_ref()
            .is_some_and(|display| display.scanout.is_some());
        // Vblank interrupts stay on until the flip is done, so that DRM counts the vblank the
        // frame shows from, which the commit's event then carries.
        let crtc = new_state.crtc().map(Crtc::from_opaque);
        let _vblank = crtc.and_then(|crtc| crtc.vblank_get().ok());
        // An asynchronous flip, which userspace asked for, takes effect right away, tearing.
        let present = match crtc.and_then(|crtc| state.get_new_crtc_state(crtc)) {
            Some(crtc_state) if crtc_state.async_flip() => PresentMode::Immediate,
            _ => PresentMode::Vsync,
        };
        let result = if scanout {
            draw_to_scanout(drm, old_state, &mut new_state, present)
        } else {
            copy_to_boot_display(drm, old_state, &new_state)
        };
        if let Err(err) = result {
            pr_err!("nova-drm: failed to update the display: {:?}\n", err);
        }
    }
}

/// Most rectangles copied for a frame: more are copied as the smallest one containing them.
const MAX_COPIES: usize = 64;

/// Has `copy` copy the `rects` of the framebuffer of `state`, areas in framebuffer coordinates, to
/// where the plane shows them, from the memory of the framebuffer's GEM object, as mapped for the
/// copy engine with `api`.
fn copy_framebuffer(
    api: &NovaCoreApi<'_>,
    state: &PlaneStateMutator<'_, PlaneState<NovaPlaneState>>,
    rects: &[Rect],
    copy: impl FnOnce(&DisplaySource, u64, u32, &[DisplayCopy]) -> Result,
) -> Result {
    let Some(fb) = state.framebuffer() else {
        return Ok(());
    };
    let object = fb.gem_object(0).ok_or(EINVAL)?;
    let pitch = fb.pitch(0).ok_or(EINVAL)?;
    let offset = fb.offset(0).ok_or(EINVAL)?;

    let whole;
    let rects = if rects.len() > MAX_COPIES {
        whole = rects.iter().skip(1).fold(rects[0], |all, rect| union(&all, rect));
        core::slice::from_ref(&whole)
    } else {
        rects
    };

    // The plane shows `src` of the framebuffer at `dst`.
    let (src, dst) = (state.src(), state.dst());
    let mut copies = KVec::with_capacity(rects.len(), GFP_KERNEL)?;
    for rect in rects {
        copies.push(
            DisplayCopy {
                src_x: u32::try_from(rect.x1)?,
                src_y: u32::try_from(rect.y1)?,
                dst_x: u32::try_from(dst.x1 + (rect.x1 - src.x1))?,
                dst_y: u32::try_from(dst.y1 + (rect.y1 - src.y1))?,
                width: u32::try_from(rect.x2 - rect.x1)?,
                height: u32::try_from(rect.y2 - rect.y1)?,
            },
            GFP_KERNEL,
        )?;
    }
    if copies.is_empty() {
        return Ok(());
    }

    object.with_display_source(api, |source| copy(source, u64::from(offset), pitch, &copies))
}

/// Copies the frame of `new_state` into the back buffer of nova-core's scanout, and flips it as
/// `present` says.
///
/// The back buffer holds the frame before the one shown: the parts of the framebuffer that changed
/// since either are copied.
fn draw_to_scanout(
    drm: &NovaDevice,
    old_state: &PlaneState<NovaPlaneState>,
    new_state: &mut PlaneStateMutator<'_, PlaneState<NovaPlaneState>>,
    present: PresentMode,
) -> Result {
    // Without a framebuffer, the last frame stays on screen.
    if new_state.framebuffer().is_none() {
        return Ok(());
    }
    // Nothing to do if the device is going away.
    let Some(registered) = drm.registration_guard() else {
        return Ok(());
    };

    let mut rects = KVec::new();
    let mut damage = None;
    for rect in new_state.damage(old_state) {
        damage = Some(damage.map_or(rect, |damage| union(&damage, &rect)));
        rects.push(rect, GFP_KERNEL)?;
    }
    // What changed in the frame before, which the back buffer misses unless copied again.
    let previous = old_state
        .damage
        .and_then(|rect| intersection(&rect, &new_state.src()));
    if let Some(previous) = previous {
        if !rects
            .iter()
            .any(|rect| intersection(&previous, rect) == Some(previous))
        {
            rects.push(previous, GFP_KERNEL)?;
        }
    }

    registered.registration_data_with(|reg| {
        reg.api.with(|api| {
            copy_framebuffer(&api, new_state, &rects, |source, offset, pitch, copies| {
                api.copy_to_scanout(source, offset, pitch, copies)
            })?;

            api.flip_scanout(present)
        })
    })?;

    new_state.damage = damage;
    Ok(())
}

/// Returns the smallest rectangle containing `a` and `b`.
fn union(a: &Rect, b: &Rect) -> Rect {
    Rect {
        x1: a.x1.min(b.x1),
        y1: a.y1.min(b.y1),
        x2: a.x2.max(b.x2),
        y2: a.y2.max(b.y2),
    }
}

/// Returns the part of `a` within `b`, if any.
fn intersection(a: &Rect, b: &Rect) -> Option<Rect> {
    let rect = Rect {
        x1: a.x1.max(b.x1),
        y1: a.y1.max(b.y1),
        x2: a.x2.min(b.x2),
        y2: a.y2.min(b.y2),
    };

    (rect.x1 < rect.x2 && rect.y1 < rect.y2).then_some(rect)
}

/// Copies the parts of the framebuffer that changed since `old_state` to the boot display.
fn copy_to_boot_display(
    drm: &NovaDevice,
    old_state: &PlaneState<NovaPlaneState>,
    new_state: &PlaneStateMutator<'_, PlaneState<NovaPlaneState>>,
) -> Result {
    // Nothing to do if the device is going away.
    let Some(registered) = drm.registration_guard() else {
        return Ok(());
    };

    let mut rects = KVec::new();
    for rect in new_state.damage(old_state) {
        rects.push(rect, GFP_KERNEL)?;
    }

    registered.registration_data_with(|reg| {
        reg.api.with(|api| {
            copy_framebuffer(&api, new_state, &rects, |source, offset, pitch, copies| {
                api.copy_to_boot_display(source, offset, pitch, copies)
            })
        })
    })
}

/// State of the boot display's plane, whose framebuffer the GPU copies to the display.
#[derive(Clone, Default)]
pub(crate) struct NovaPlaneState {
    /// Smallest rectangle containing what changed in the framebuffer in the frame of this state,
    /// in framebuffer coordinates, if anything did and it was drawn into nova-core's scanout.
    damage: Option<Rect>,
}

impl DriverPlaneState for NovaPlaneState {
    type Plane = NovaPlane;
    type Kind = plane::Plain;
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
    type VblankImpl = Self;

    fn new(_drm: &Device<NovaDriver, KmsProbe>, _args: &Self::Args) -> impl PinInit<Self, Error> {
        try_pin_init!(Self { _p: PhantomPinned })
    }

    fn mode_valid(crtc: &Crtc, mode: &DisplayMode) -> ModeStatus {
        let drm = crtc.drm_dev();
        let Some(display) = drm.boot_display.as_ref() else {
            return ModeStatus::OK;
        };
        // Without nova-core's scanout, only the firmware's mode works.
        if display.scanout.is_none() {
            return crtc.mode_valid_fixed(mode, &display.mode);
        }

        let Ok(timings) = mode_timings(mode) else {
            return ModeStatus::NO_INTERLACE;
        };
        match with_api(drm, |api| api.check_mode(&timings)) {
            Ok(()) => ModeStatus::OK,
            Err(ERANGE) => ModeStatus::CLOCK_HIGH,
            Err(_) => ModeStatus::BAD,
        }
    }

    fn atomic_flush(commit: crtc::CrtcAtomicCommit<'_, Self>) {
        let mut new_state = commit.take_new_state();

        // The planes were flipped, and the flip returned once the frame was on screen: from the
        // vblank just handled.
        if let Some(event) = new_state.get_pending_vblank_event() {
            event.send();
        }
    }

    fn atomic_enable(commit: crtc::CrtcAtomicCommit<'_, Self>) {
        let crtc = commit.crtc();
        let drm = crtc.drm_dev();
        let new_state = commit.take_new_state();

        // The resolution stays the firmware's: only the timings, e.g. the refresh rate, change.
        let mode = new_state.adjusted_mode();
        let set = scanout_head(drm).and_then(|head| {
            let timings = mode_timings(mode)?;
            with_api(drm, |api| api.set_mode(head, &timings))
        });
        pr_info!(
            "enabling {}x{}@{}: mode changed {:?}\n",
            mode.hdisplay(),
            mode.vdisplay(),
            mode.vrefresh(),
            set
        );

        // Without vblank interrupts, DRM completes commits without vblanks.
        let registered = scanout_head(drm).and_then(|head| {
            let handler: Arc<dyn VblankHandler> = Arc::new(
                NovaVblankHandler {
                    crtc: KmsRef::from(crtc),
                    head,
                },
                GFP_KERNEL,
            )?;
            with_api(drm, |api| {
                api.set_vblank_handler(Some(handler));
                Ok(())
            })
        });
        if registered.is_ok() {
            crtc.vblank_on();
        }
    }

    fn atomic_disable(commit: crtc::CrtcAtomicCommit<'_, Self>) {
        let crtc = commit.crtc();

        crtc.vblank_off();
        // The handler holds a reference to the CRTC, so it must not outlive its use.
        let _ = with_api(crtc.drm_dev(), |api| {
            api.set_vblank_handler(None);
            Ok(())
        });
    }
}

impl VblankSupport for NovaCrtc {
    type Crtc = Self;

    fn enable_vblank(
        crtc: &Crtc,
        _vblank: &VblankGuard<'_, Self::Crtc>,
        _irq: &LocalInterruptDisabled,
    ) -> Result {
        let drm = crtc.drm_dev();
        let head = scanout_head(drm)?;

        with_api(drm, |api| api.set_vblank_interrupt(head, true))
    }

    fn disable_vblank(
        crtc: &Crtc,
        _vblank: &VblankGuard<'_, Self::Crtc>,
        _irq: &LocalInterruptDisabled,
    ) {
        let drm = crtc.drm_dev();
        let _ = scanout_head(drm)
            .and_then(|head| with_api(drm, |api| api.set_vblank_interrupt(head, false)));
    }

    // DRM timestamps vblanks when they are handled.
    fn get_vblank_timestamp(_crtc: &Crtc, _in_vblank_irq: bool) -> Option<VblankTimestamp> {
        None
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

    fn get_modes(
        connector: ConnectorGuard<'_, Self>,
        _guard: &ModeConfigGuard<'_, NovaDriver>,
    ) -> i32 {
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
        ConnectorType::HdmiA | ConnectorType::HdmiC => {
            (connector::Type::HdmiA, encoder::Type::Tmds)
        }
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

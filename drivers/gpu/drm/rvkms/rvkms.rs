// SPDX-License-Identifier: GPL-2.0
//! A port of the VKMS driver to rust.
pub(crate) mod connector;
pub(crate) mod crtc;
pub(crate) mod encoder;
pub(crate) mod file;
pub(crate) mod gem;
pub(crate) mod output;
pub(crate) mod plane;

use core::marker::*;

use kernel::{
    c_str,
    device,
    drm::{
        self,
        driver,
        Device,
        Early,
        kms::{
            atomic::*,
            KmsDriver,
            ModeConfigInfo,
            KmsProbe,
        },
    },
    faux,
    prelude::*,
    str::CStr,
};

/// Convienence type alias for the DRM device type for this driver
pub(crate) type RvkmsDevice = drm::device::Device<RvkmsDriver>;

/// The name of the driver
const NAME: &CStr = c_str!("rvkms");

/// Driver metadata
const INFO: driver::DriverInfo = driver::DriverInfo {
    major: 0,
    minor: 0,
    patchlevel: 0,
    name: NAME,
    desc: c_str!("Rust VKMS PoC"),
};

/// The minimum supported resolution
const MIN_RES: (u32, u32) = (10, 10);

/// The maximum supported resolution
const MAX_RES: (u32, u32) = (8192, 8192);

/// The "preferred" resolution
const DEFAULT_RES: (u32, u32) = (1024, 768);

/// DRM Driver implementation for `RvkmsDriver`
#[vtable]
impl driver::Driver for RvkmsDriver {
    type Data = ();
    type RegistrationData<'a> = ();
    type Object = gem::Object;
    type ParentDevice<Ctx: device::DeviceContext> = faux::Device<Ctx>;
    type File = file::File;
    type Kms = Self;

    const INFO: driver::DriverInfo = INFO;

    kernel::declare_drm_ioctls! {}
}

#[vtable]
impl KmsDriver for RvkmsDriver {
    type Crtc = crtc::RvkmsCrtc;
    type Connector = connector::DriverConnector;
    type Plane = plane::RvkmsPlane;
    type Encoder = encoder::DriverEncoder;

    fn mode_config_info(_dev: &drm::Device<Self, Early>) -> Result<ModeConfigInfo> {
        Ok(MODE_CONFIG_INFO)
    }

    fn probe(drm: &Device<Self, KmsProbe>) -> Result
    where
        Self: Sized
    {
        output::create_output(drm, 0)
    }

    fn atomic_commit_tail<'a>(
        mut state: AtomicCommitTail<'a, Self>,
        modeset_token: ModesetsReadyToken<'_>,
        plane_update_token: PlaneUpdatesReadyToken<'_>,
    ) -> CommittedAtomicState<'a, Self>
    where
        Self: Sized,
    {
        let modeset_token = state.commit_modeset_disables(modeset_token);

        let plane_update_token = state.commit_planes(plane_update_token, Default::default());

        let modeset_token = state.commit_modeset_enables(modeset_token);

        state.fake_vblank();

        let state = state.commit_hw_done(modeset_token, plane_update_token);

        state.wait_for_flip_done();

        state
    }
}

pub(crate) struct RvkmsDriver;

const MODE_CONFIG_INFO: ModeConfigInfo = ModeConfigInfo {
    min_resolution: MIN_RES,
    max_resolution: MAX_RES,
    max_cursor: (512, 512),
    preferred_depth: 24,
    preferred_fourcc: None,
};

pub(crate) struct RvkmsModule<'bound> {
    _dev: faux::Registration,
    _drm: driver::Registration<'bound, RvkmsDriver>,
}

impl kernel::Module for RvkmsModule<'static> {
    fn init(_module: &'static ThisModule) -> Result<Self> {
        pr_info!("RVKMS Loaded\n");

        let dev = faux::Registration::new(NAME, None)?;
        dev_info!(dev.as_ref(), "Setting up DRM\n");

        let drm = drm::UnregisteredDevice::<RvkmsDriver>::new(dev.as_ref(), Ok(()))?;

        // FIXME: Another hack we need to do to workaround lifetimes again


        // SAFETY: `reg` is stored in `RvkmsModule` and dropped when the driver is unbound, it is
        // never forgotten.
        let reg = unsafe { drm::Registration::new(dev.as_ref().as_ref(), drm, (), 0)? };

        // FIXME: This is pretty gnarly, but we need to provide a lifetime and we have literally
        // nothing to get it with without the pin-init self-referential stuff. Once we get the
        // current WIP on that from Gary we can figure out how to fix this properly.
        // SAFETY: The registration literally can't go away until we drop it, so transmuting it to a
        // static lifetime is fine.
        let reg = unsafe {
            core::mem::transmute::<
                drm::Registration<'_, RvkmsDriver>,
                drm::Registration<'static, RvkmsDriver>
            >(reg)
        };

        let this = Self {
            _dev: dev,
            _drm: reg,
        };

        // FIXME: This isn't great, but we don't have the self-referential pin init stuff yet which
        // means we need to get a little hacky to make things work
        // SAFETY: No resources referenced by this structure go away until its contents are dropped.
        let this: RvkmsModule<'static> = unsafe {
            core::mem::transmute(this)
        };

        Ok(this)
        //let drm = driver::Registration::<RvkmsDriver>::new(dev.as_ref(), ())?;

        //Ok(Self {
            //_dev: dev,
            //_drm: drm,
        //})
    }
}

module! {
    type: RvkmsModule<'static>,
    name: "rvkms",
    authors: ["Lyude Paul"],
    description: "Rust VKMS Proof of Concept driver",
    license: "GPL v2",
}

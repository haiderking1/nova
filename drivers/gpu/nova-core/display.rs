// SPDX-License-Identifier: GPL-2.0

//! The display engine, as exposed by GSP-RM.
//!
//! GSP-RM owns the display hardware. Nova queries and controls it through RM control commands sent
//! to an `NV04_DISPLAY_COMMON` object, which Nova allocates under the device of its RM client.
//!
//! To drive the display engine itself, Nova takes it over from GSP-RM: see [`DisplayEngine`].

mod channel;
pub(crate) mod dp;
mod engine;
pub(crate) mod hal;
mod instmem;
pub(crate) mod mode;
mod regs;
mod scanout;
#[cfg(CONFIG_NOVA_CORE_SELFTESTS)]
pub(crate) mod scanout_test;
mod vblank;

pub(crate) use engine::{
    DisplayEngine,
    InternalHandles,
    Surface, //
};
pub use engine::PresentMode;
pub use mode::ModeTimings;
pub(crate) use scanout::Scanout;
pub use scanout::{
    DisplayCopy,
    DisplaySource,
    ScanoutMode, //
};
pub use vblank::VblankHandler;
pub(crate) use vblank::VblankHandlerSlot;

use kernel::{
    device,
    prelude::*,
    time::{
        delay::fsleep,
        Delta, //
    }, //
};

use crate::gsp::{
    cmdq::Cmdq,
    commands::{
        Client,
        Device,
        DisplayCommon,
        Handle, //
    },
    rm::{
        commands::{
            AllocDisplayCommon,
            DfpGetInfo,
            DispGetActive,
            DispGetConnectorType,
            DispGetEdid,
            DispGetNumHeads,
            DpAux,
            DpCtrl,
            DpGetCaps,
            DpGetLinkConfig,
            RmAlloc,
            RmControl,
            RmControlCommand,
            RmFree, //
        },
        device::RmDevice,
    },
    ConnectorType,
    DpAuxParams,
    DpCtrlParams,
    DpGpuCaps,
    DpLinkConfig,
    DpTraining, //
};

/// Handle of the display common object, after its class. The caller of an allocation picks the
/// handle; this is the value nouveau uses for the same object.
const DISPLAY_COMMON_HANDLE: u32 = 0x0073_0000;

/// A display, i.e. a display output path of the display engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DisplayId(u32);

impl DisplayId {
    /// Returns the RM display ID: a single bit set.
    pub(crate) fn into_raw(self) -> u32 {
        self.0
    }
}

/// The display engine, through the RM object allocated to query and control it.
///
/// The object must be released with [`Display::free`] while the GSP is running.
pub(crate) struct Display {
    client: Handle<Client>,
    device: Handle<Device>,
    common: Handle<DisplayCommon>,
}

impl Display {
    /// Allocates the RM object needed to query and control the display engine, under the device
    /// of `rm`.
    pub(crate) fn new(dev: &device::Device, cmdq: &Cmdq<'_>, rm: &RmDevice) -> Result<Self> {
        let common = RmAlloc::new(
            rm.client(),
            rm.device(),
            Handle::<DisplayCommon>::new(DISPLAY_COMMON_HANDLE),
            AllocDisplayCommon,
        )
        .send(dev, cmdq)?;

        Ok(Self {
            client: rm.client(),
            device: rm.device(),
            common,
        })
    }

    /// Returns the client the RM objects of the display engine are allocated under.
    pub(crate) fn client(&self) -> Handle<Client> {
        self.client
    }

    /// Returns the device the display engine's objects are allocated under.
    pub(crate) fn device(&self) -> Handle<Device> {
        self.device
    }

    /// Frees the RM object.
    pub(crate) fn free(self, dev: &device::Device, cmdq: &Cmdq<'_>) -> Result {
        Ok(RmFree::new(self.client, self.common).send(dev, cmdq)?)
    }

    /// Returns the first display that is active on a head, such as the one the system firmware
    /// lit up, or `None` if no display is active.
    pub(crate) fn active_display(
        &self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
    ) -> Result<Option<DisplayId>> {
        let num_heads =
            RmControl::new(self.client, self.common, DispGetNumHeads::new()).send(dev, cmdq)?;

        for head in 0..num_heads {
            let display = RmControl::new(self.client, self.common, DispGetActive::new(head))
                .send(dev, cmdq)?;
            dev_dbg!(
                dev,
                "display head {}: active display {:#x}\n",
                head,
                display
            );

            if display != 0 {
                return Ok(Some(DisplayId(display)));
            }
        }

        Ok(None)
    }

    /// Returns the type of connector `display` is wired to.
    pub(crate) fn connector_type(
        &self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        display: DisplayId,
    ) -> Result<ConnectorType> {
        Ok(RmControl::new(
            self.client,
            self.common,
            DispGetConnectorType::new(display.0),
        )
        .send(dev, cmdq)?)
    }

    /// Returns the configuration of the link of `display`, which must be a DisplayPort display.
    pub(crate) fn dp_link_config(
        &self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        display: DisplayId,
    ) -> Result<DpLinkConfig> {
        Ok(
            RmControl::new(self.client, self.common, DpGetLinkConfig::new(display.0))
                .send(dev, cmdq)?,
        )
    }

    /// Reads the EDID of `display` from the connected monitor, or returns the copy GSP-RM cached
    /// if reading it fails.
    pub(crate) fn edid(
        &self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        display: DisplayId,
    ) -> Result<KVec<u8>> {
        let read = |cached| -> Result<KVec<u8>> {
            RmControl::new(
                self.client,
                self.common,
                DispGetEdid::new(display.0, cached)?,
            )
            .send(dev, cmdq)
            .inspect(|edid| {
                dev_dbg!(
                    dev,
                    "EDID of display {:#x} (cached: {}): {} bytes\n",
                    display.0,
                    cached,
                    edid.len()
                );
            })
            .inspect_err(|e| {
                dev_warn!(
                    dev,
                    "reading the EDID of display {:#x} (cached: {}) failed: {:?}\n",
                    display.0,
                    cached,
                    e
                );
            })
            .map_err(Error::from)
        };

        read(false).or_else(|_| read(true))
    }
}

/// How many times an AUX transfer or a link training is sent while GSP-RM asks to retry it later,
/// as NVIDIA's driver does.
const DP_TRIES: usize = 3;

impl Display {
    /// Sends `cmd` to the display common object, waiting and sending it again as long as RM
    /// fails it but asks for a retry after the time `retry_ms` returns, up to [`DP_TRIES`] times.
    /// Returns the reply of the last try, with whether RM failed it.
    fn send_retrying<T: RmControlCommand<Target = DisplayCommon>>(
        &self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        cmd: impl Fn() -> Result<T>,
        retry_ms: impl Fn(&T::Reply) -> Option<u32>,
    ) -> Result<(Result, T::Reply)> {
        let mut tries = 0;
        loop {
            let (status, reply) =
                RmControl::new(self.client, self.common, cmd()?).send_with_status(dev, cmdq)?;
            tries += 1;
            match (status, retry_ms(&reply)) {
                (Err(_), Some(ms)) if tries < DP_TRIES => {
                    dev_dbg!(dev, "display: GSP-RM asks to retry in {} ms\n", ms);
                    fsleep(Delta::from_millis(ms.into()));
                }
                (status, _) => return Ok((status.map_err(Error::from), reply)),
            }
        }
    }

    /// Carries out the AUX transfer `request` at `addr` on the DisplayPort link of `display`,
    /// writing `data` for write requests or reading `len` bytes for read requests, and returns
    /// RM's report of it. `request` is coded as the DisplayPort specification does.
    #[expect(clippy::too_many_arguments)]
    pub(crate) fn dp_aux(
        &self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        display: DisplayId,
        request: u8,
        addr: u32,
        data: &[u8],
        len: usize,
    ) -> Result<DpAuxParams> {
        let (status, reply) = self.send_retrying(
            dev,
            cmdq,
            || {
                Ok(DpAux::new(DpAuxParams::new(
                    display.0, request, addr, data, len,
                )?))
            },
            DpAuxParams::retry_ms,
        )?;
        status.map(|()| reply)
    }

    /// Has RM train the DisplayPort link of `display` as `training` says, spreading the link's
    /// spectrum or not as RM's defaults are if `downspread`.
    ///
    /// Fails with `EIO` if the training fails.
    pub(crate) fn dp_train(
        &self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        display: DisplayId,
        training: &DpTraining,
        downspread: bool,
    ) -> Result {
        let (status, reply) = self.send_retrying(
            dev,
            cmdq,
            || {
                Ok(DpCtrl::new(DpCtrlParams::train(
                    display.0, training, downspread,
                )))
            },
            DpCtrlParams::retry_ms,
        )?;
        if status.is_err() || reply.err() != 0 {
            dev_err!(
                dev,
                "display: training {:?} failed: {:?}, errors {:#010x}\n",
                training,
                status,
                reply.err()
            );
            return Err(EIO);
        }

        Ok(())
    }

    /// Returns what the DisplayPort outputs of the GPU can do.
    pub(crate) fn dp_gpu_caps(&self, dev: &device::Device, cmdq: &Cmdq<'_>) -> Result<DpGpuCaps> {
        Ok(RmControl::new(self.client, self.common, DpGetCaps::new()).send(dev, cmdq)?)
    }

    /// Returns the highest link bandwidth code the board allows the DisplayPort link of
    /// `display`, if it limits it.
    pub(crate) fn dp_board_max_link_bw(
        &self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        display: DisplayId,
    ) -> Result<Option<u32>> {
        Ok(RmControl::new(self.client, self.common, DfpGetInfo::new(display.0)).send(dev, cmdq)?)
    }
}

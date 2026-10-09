// SPDX-License-Identifier: GPL-2.0

//! The display engine, as exposed by GSP-RM.
//!
//! GSP-RM owns the display hardware. Nova queries and controls it through RM control commands sent
//! to an `NV04_DISPLAY_COMMON` object, which Nova allocates under the device of its RM client.
//!
//! To drive the display engine itself, Nova takes it over from GSP-RM: see [`DisplayEngine`].

mod channel;
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
pub use mode::ModeTimings;
pub(crate) use scanout::Scanout;
pub use scanout::ScanoutMode;
pub use vblank::VblankHandler;
pub(crate) use vblank::VblankHandlerSlot;

use kernel::{
    device,
    prelude::*, //
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
            DispGetActive,
            DispGetConnectorType,
            DispGetEdid,
            DispGetNumHeads,
            DpGetLinkConfig,
            RmAlloc,
            RmControl,
            RmFree, //
        },
        device::RmDevice,
    },
    ConnectorType, DpLinkConfig,
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

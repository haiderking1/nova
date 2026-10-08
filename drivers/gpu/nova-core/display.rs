// SPDX-License-Identifier: GPL-2.0

//! The display engine, as exposed by GSP-RM.
//!
//! GSP-RM owns the display hardware. Nova queries and controls it through RM control commands sent
//! to an `NV04_DISPLAY_COMMON` object, which lives in an RM object tree that Nova allocates for
//! itself: client -> device -> subdevice, and the display common object under the device.
//!
//! To drive the display engine itself, Nova takes it over from GSP-RM: see [`DisplayEngine`].

mod channel;
mod engine;
mod instmem;
mod lut;
mod regs;
#[cfg(CONFIG_NOVA_CORE_SELFTESTS)]
pub(crate) mod scanout_test;

pub(crate) use engine::{
    DisplayEngine,
    InternalHandles,
    Surface, //
};

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
        Handle,
        Subdevice, //
    },
    rm::commands::{
        AllocDevice,
        AllocDisplayCommon,
        AllocRoot,
        AllocSubdevice,
        DispGetActive,
        DispGetConnectorType,
        DispGetEdid,
        DispGetNumHeads,
        RmAlloc,
        RmControl,
        RmFree, //
    },
    ConnectorType,
};

// Handles of the RM objects Nova allocates for the display engine. The caller of an allocation picks
// the handle; these are the values nouveau uses for the same objects.

/// Handle of the client, the first in the range GSP-RM uses for clients.
const CLIENT_HANDLE: u32 = 0xc1d0_0000;
/// Handle of the device.
const DEVICE_HANDLE: u32 = 0xde1d_0000;
/// Handle of the subdevice.
const SUBDEVICE_HANDLE: u32 = 0x5d1d_0000;
/// Handle of the display common object, after its class.
const DISPLAY_COMMON_HANDLE: u32 = 0x0073_0000;

/// A display, i.e. a display output path of the display engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DisplayId(u32);

/// The display engine, through the RM objects allocated to query and control it.
///
/// The objects must be released with [`Display::free`] while the GSP is running.
pub(crate) struct Display {
    client: Handle<Client>,
    device: Handle<Device>,
    common: Handle<DisplayCommon>,
}

impl Display {
    /// Allocates the RM objects needed to query and control the display engine.
    pub(crate) fn new(dev: &device::Device, cmdq: &Cmdq<'_>) -> Result<Self> {
        let client = Handle::<Client>::new(CLIENT_HANDLE);

        // A client is its own parent.
        RmAlloc::new(client, client, client, AllocRoot::new(client)).send(dev, cmdq)?;

        let alloc_children = || -> Result<(Handle<Device>, Handle<DisplayCommon>)> {
            let device = RmAlloc::new(
                client,
                client,
                Handle::<Device>::new(DEVICE_HANDLE),
                AllocDevice::new(client),
            )
            .send(dev, cmdq)?;

            RmAlloc::new(
                client,
                device,
                Handle::<Subdevice>::new(SUBDEVICE_HANDLE),
                AllocSubdevice::new(),
            )
            .send(dev, cmdq)?;

            let common = RmAlloc::new(
                client,
                device,
                Handle::<DisplayCommon>::new(DISPLAY_COMMON_HANDLE),
                AllocDisplayCommon,
            )
            .send(dev, cmdq)?;

            Ok((device, common))
        };

        match alloc_children() {
            Ok((device, common)) => Ok(Self {
                client,
                device,
                common,
            }),
            Err(err) => {
                // Freeing the client frees whatever was allocated under it.
                let _ = RmFree::new(client, client).send(dev, cmdq);
                Err(err)
            }
        }
    }

    /// Returns the client the RM objects of the display engine are allocated under.
    pub(crate) fn client(&self) -> Handle<Client> {
        self.client
    }

    /// Returns the device the display engine's objects are allocated under.
    pub(crate) fn device(&self) -> Handle<Device> {
        self.device
    }

    /// Frees the RM objects. GSP-RM frees all the objects of the client along with it.
    pub(crate) fn free(self, dev: &device::Device, cmdq: &Cmdq<'_>) -> Result {
        Ok(RmFree::new(self.client, self.client).send(dev, cmdq)?)
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

// SPDX-License-Identifier: GPL-2.0

//! Nova's own RM client.
//!
//! RM objects live in a tree rooted at a client. Nova allocates its client, with a device and a
//! subdevice under it, and allocates its other RM objects (display objects, address spaces,
//! channels) under those.

use kernel::{
    device,
    prelude::*, //
};

use crate::gsp::{
    cmdq::Cmdq,
    commands::{
        Client,
        Device,
        Handle,
        Subdevice, //
    },
    rm::commands::{
        AllocDevice,
        AllocRoot,
        AllocSubdevice,
        RmAlloc,
        RmFree, //
    },
};

// The caller of an allocation picks the handle; these are the values nouveau uses for the same
// objects.

/// Handle of the client, the first in the range GSP-RM uses for clients.
const CLIENT_HANDLE: u32 = 0xc1d0_0000;
/// Handle of the device.
const DEVICE_HANDLE: u32 = 0xde1d_0000;
/// Handle of the subdevice.
const SUBDEVICE_HANDLE: u32 = 0x5d1d_0000;

/// Nova's RM client, and the device and subdevice of the GPU under it.
///
/// It must be released with [`RmDevice::free`] while the GSP is running.
pub(crate) struct RmDevice {
    client: Handle<Client>,
    device: Handle<Device>,
    subdevice: Handle<Subdevice>,
}

impl RmDevice {
    /// Allocates the client, device and subdevice.
    pub(crate) fn new(dev: &device::Device, cmdq: &Cmdq<'_>) -> Result<Self> {
        let client = Handle::<Client>::new(CLIENT_HANDLE);

        // A client is its own parent.
        RmAlloc::new(client, client, client, AllocRoot::new(client)).send(dev, cmdq)?;

        let alloc_children = || -> Result<(Handle<Device>, Handle<Subdevice>)> {
            let device = RmAlloc::new(
                client,
                client,
                Handle::<Device>::new(DEVICE_HANDLE),
                AllocDevice::new(client),
            )
            .send(dev, cmdq)?;

            let subdevice = RmAlloc::new(
                client,
                device,
                Handle::<Subdevice>::new(SUBDEVICE_HANDLE),
                AllocSubdevice::new(),
            )
            .send(dev, cmdq)?;

            Ok((device, subdevice))
        };

        match alloc_children() {
            Ok((device, subdevice)) => Ok(Self {
                client,
                device,
                subdevice,
            }),
            Err(err) => {
                // Freeing the client frees whatever was allocated under it.
                let _ = RmFree::new(client, client).send(dev, cmdq);
                Err(err)
            }
        }
    }

    /// Returns the client.
    pub(crate) fn client(&self) -> Handle<Client> {
        self.client
    }

    /// Returns the device.
    pub(crate) fn device(&self) -> Handle<Device> {
        self.device
    }

    /// Returns the subdevice.
    pub(crate) fn subdevice(&self) -> Handle<Subdevice> {
        self.subdevice
    }

    /// Frees the client, and with it every RM object allocated under it.
    pub(crate) fn free(self, dev: &device::Device, cmdq: &Cmdq<'_>) -> Result {
        Ok(RmFree::new(self.client, self.client).send(dev, cmdq)?)
    }
}

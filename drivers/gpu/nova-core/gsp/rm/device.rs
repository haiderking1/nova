// SPDX-License-Identifier: GPL-2.0

//! RM clients of Nova.
//!
//! RM objects live in a tree rooted at a client. Nova allocates its own client, with a device and
//! a subdevice under it, and allocates its other RM objects (display objects, address spaces,
//! channels) under those.
//!
//! Every address space of Nova's clients has an RM client of its own too, under whose device
//! are the address space and the channels running in it, like nouveau gives each of its address
//! spaces (`r535_mmu_vaspace_new()`): they and what GSP-RM does for them through the device and
//! subdevice, such as setting their page directories and binding their contexts, stay apart
//! from Nova's own.

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

/// Handle of Nova's own client, the first in the range GSP-RM uses for clients
/// (`RS_CLIENT_HANDLE_BASE` in RM's `resserv.h`). Other clients follow it.
const CLIENT_HANDLE: u32 = 0xc1d0_0000;
/// Number of handles in the range GSP-RM uses for clients (`RS_CLIENT_HANDLE_MAX`).
const CLIENT_HANDLES: u32 = 0x10_0000;
/// Index of Nova's own client, see [`RmDevice::new`].
pub(crate) const NOVA_CLIENT: u32 = 0;
/// Handle of the device.
const DEVICE_HANDLE: u32 = 0xde1d_0000;
/// Handle of the subdevice.
const SUBDEVICE_HANDLE: u32 = 0x5d1d_0000;

/// An RM client of Nova, and the device and subdevice of the GPU under it.
///
/// It must be released with [`RmDevice::free`] while the GSP is running.
pub(crate) struct RmDevice {
    client: Handle<Client>,
    device: Handle<Device>,
    subdevice: Handle<Subdevice>,
}

impl RmDevice {
    /// Allocates the client of index `index`, and the device and subdevice under it:
    /// [`NOVA_CLIENT`] is Nova's own, the others those of the address spaces of its clients.
    ///
    /// Fails with `EINVAL` if `index` is beyond GSP-RM's range of client handles.
    pub(crate) fn new(dev: &device::Device, cmdq: &Cmdq<'_>, index: u32) -> Result<Self> {
        if index >= CLIENT_HANDLES {
            return Err(EINVAL);
        }
        let client = Handle::<Client>::new(CLIENT_HANDLE + index);

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

    /// Frees the subdevice, the device, then the client, which every other RM object allocated
    /// under it must have been freed from before: kernel RM and nouveau free a client once empty,
    /// rather than having GSP-RM free what is left under it.
    pub(crate) fn free(self, dev: &device::Device, cmdq: &Cmdq<'_>) -> Result {
        let subdevice = RmFree::new(self.client, self.subdevice).send(dev, cmdq);
        let device = RmFree::new(self.client, self.device).send(dev, cmdq);
        let client = RmFree::new(self.client, self.client).send(dev, cmdq);

        Ok(subdevice.and(device).and(client)?)
    }
}

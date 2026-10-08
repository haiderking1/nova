// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

use core::{
    array,
    convert::Infallible,
    ffi::FromBytesUntilNulError,
    marker::PhantomData,
    ops::Range,
    str::Utf8Error, //
};

use kernel::{
    device,
    pci,
    prelude::*,
    transmute::{
        AsBytes,
        FromBytes, //
    }, //
};

use crate::{
    gpu::Chipset,
    gsp::{
        cmdq::{
            Cmdq,
            CommandToGsp,
            MessageFromGsp,
            NoReply, //
        },
        fw::{
            self,
            MsgFunction, //
        },
    },
    sbuffer::SBufferIter,
    vgpu::VgpuState, //
};

/// Marker type for a GSP-RM client handle.
///
/// A client handle identifies a client which provides a namespace for RM objects. Lookup of objects
/// happens within the client namespace.
pub(crate) struct Client;

/// Marker type for a GSP-RM device handle.
///
/// A device handle identifies a logical GPU device instance under a client. In multi-GPU
/// configurations it can represent a grouped or logical device.
pub(crate) struct Device;

/// Marker type for a GSP-RM subdevice handle.
///
/// A subdevice handle identifies a child of a device that selects one specific GPU within that
/// device.
pub(crate) struct Subdevice;

/// Marker type for the GSP-RM handle of the display engine's common object.
///
/// This object, a child of a device, is the target of the display engine's RM control commands
/// that are not specific to a display channel.
pub(crate) struct DisplayCommon;

/// Marker type for the GSP-RM handle of a display object.
///
/// This object, a child of a device, owns the display engine's channels.
pub(crate) struct DisplayRoot;

/// Marker type for the GSP-RM handle of a display core channel.
///
/// The core channel, a child of the display object, carries the methods that configure heads and
/// outputs.
pub(crate) struct DisplayCoreChannel;

/// Marker type for the GSP-RM handle of a display window channel.
///
/// A window channel, a child of the display object, carries the methods of one window: the
/// surface it shows, and how.
pub(crate) struct DisplayWindowChannel;

/// Marker type for the GSP-RM handle of a GPU virtual address space.
///
/// An address space, a child of a device, holds the GPU virtual addresses channels access memory
/// through.
pub(crate) struct VaSpace;

/// Marker type for the GSP-RM handle of a GPFIFO channel.
///
/// A channel, a child of a device, fetches the command buffers listed in its GPFIFO and runs them
/// on the engine it's bound to.
pub(crate) struct Channel;

/// Marker type for the GSP-RM handle of a copy engine object.
///
/// A copy engine object, a child of a channel, makes the copy engine's methods available on the
/// channel.
pub(crate) struct CopyEngine;

/// Marker type for the GSP-RM handle of an object of a class of the graphics engine.
///
/// Such an object, a child of a channel on the graphics engine, makes the class's methods available
/// on the channel.
pub(crate) struct GraphicsObject;

/// A typed GSP-RM object handle.
///
/// These handles form a tree structure with different types of RM objects, with client at the root.
/// An RM object is anything identified by a valid handle. For example, the tree may look like
/// Client -> Device -> Subdevice, where there may be multiple devices under each client, and
/// multiple subdevices under each device.
#[derive(Debug)]
pub(crate) struct Handle<T>(u32, PhantomData<T>);

impl<T> Clone for Handle<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Handle<T> {}

impl<T> Handle<T> {
    /// Creates a new handle from a raw value.
    pub(crate) fn new(raw: u32) -> Self {
        Self(raw, PhantomData)
    }

    /// Returns the raw handle value.
    pub(crate) fn as_raw(self) -> u32 {
        self.0
    }
}

/// The `GspSetSystemInfo` command.
pub(crate) struct SetSystemInfo<'a> {
    pdev: &'a pci::Device<device::Bound>,
    chipset: Chipset,
}

impl<'a> SetSystemInfo<'a> {
    /// Creates a new `GspSetSystemInfo` command using the parameters of `pdev`.
    pub(crate) fn new(pdev: &'a pci::Device<device::Bound>, chipset: Chipset) -> Self {
        Self { pdev, chipset }
    }
}

impl<'a> CommandToGsp for SetSystemInfo<'a> {
    const FUNCTION: MsgFunction = MsgFunction::GspSetSystemInfo;
    type Command = fw::commands::GspSetSystemInfo;
    type Reply = NoReply;
    type InitError = Error;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        Self::Command::init(self.pdev, self.chipset)
    }
}

struct RegistryEntry {
    key: &'static str,
    value: u32,
}

/// The `SetRegistry` command.
pub(crate) struct SetRegistry {
    entries: KVec<RegistryEntry>,
}

impl SetRegistry {
    /// Creates a new `SetRegistry` command, using a set of hardcoded entries.
    pub(crate) fn new(vgpu_state: VgpuState) -> Result<Self> {
        let mut entries = KVec::new();

        // RMSecBusResetEnable - enables PCI secondary bus reset
        entries.push(
            RegistryEntry {
                key: "RMSecBusResetEnable",
                value: 1,
            },
            GFP_KERNEL,
        )?;

        // RMForcePcieConfigSave - forces GSP-RM to preserve PCI configuration registers on
        // any PCI reset.
        entries.push(
            RegistryEntry {
                key: "RMForcePcieConfigSave",
                value: 1,
            },
            GFP_KERNEL,
        )?;

        // RMDevidCheckIgnore - allows GSP-RM to boot even if the PCI dev ID is not found
        // in the internal product name database.
        entries.push(
            RegistryEntry {
                key: "RMDevidCheckIgnore",
                value: 1,
            },
            GFP_KERNEL,
        )?;

        if matches!(vgpu_state, VgpuState::Enabled { .. }) {
            // RMSetSriovMode - required when vGPU is enabled.
            entries.push(
                RegistryEntry {
                    key: "RMSetSriovMode",
                    value: 1,
                },
                GFP_KERNEL,
            )?;
        }

        Ok(Self { entries })
    }
}

impl CommandToGsp for SetRegistry {
    const FUNCTION: MsgFunction = MsgFunction::SetRegistry;
    type Command = fw::commands::PackedRegistryTable;
    type Reply = NoReply;
    type InitError = Infallible;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        Self::Command::init(self.entries.len() as u32, self.size() as u32)
    }

    fn variable_payload_len(&self) -> usize {
        let mut key_size = 0;
        for entry in self.entries.iter() {
            key_size += entry.key.len() + 1; // +1 for NULL terminator
        }
        self.entries.len() * size_of::<fw::commands::PackedRegistryEntry>() + key_size
    }

    fn init_variable_payload(
        &self,
        dst: &mut SBufferIter<core::array::IntoIter<&mut [u8], 2>>,
    ) -> Result {
        let string_data_start_offset = size_of::<Self::Command>()
            + self.entries.len() * size_of::<fw::commands::PackedRegistryEntry>();

        // Array for string data.
        let mut string_data = KVec::new();

        for entry in self.entries.iter() {
            dst.write_all(
                fw::commands::PackedRegistryEntry::new(
                    (string_data_start_offset + string_data.len()) as u32,
                    entry.value,
                )
                .as_bytes(),
            )?;

            let key_bytes = entry.key.as_bytes();
            string_data.extend_from_slice(key_bytes, GFP_KERNEL)?;
            string_data.push(0, GFP_KERNEL)?;
        }

        dst.write_all(string_data.as_slice())
    }
}

/// Message type for GSP initialization done notification.
struct GspInitDone;

// SAFETY: `GspInitDone` is a zero-sized type with no bytes, therefore it
// trivially has no uninitialized bytes.
unsafe impl FromBytes for GspInitDone {}

impl MessageFromGsp for GspInitDone {
    const FUNCTION: MsgFunction = MsgFunction::GspInitDone;
    type InitError = Infallible;
    type Message = ();

    fn read(
        _msg: &Self::Message,
        _sbuffer: &mut SBufferIter<array::IntoIter<&[u8], 2>>,
    ) -> Result<Self, Self::InitError> {
        Ok(GspInitDone)
    }
}

/// Waits for GSP initialization to complete.
pub(crate) fn wait_gsp_init_done(cmdq: &Cmdq<'_>) -> Result {
    cmdq.await_msg::<GspInitDone>()
        .map(|_| ())
        .map_err(Into::into)
}

/// The `GetGspStaticInfo` command.
pub(crate) struct GetGspStaticInfo;

impl CommandToGsp for GetGspStaticInfo {
    const FUNCTION: MsgFunction = MsgFunction::GetGspStaticInfo;
    type Command = fw::commands::GspStaticConfigInfo;
    type Reply = GetGspStaticInfoReply;
    type InitError = Infallible;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        Self::Command::init_zeroed()
    }
}

/// The reply from the GSP to the [`GetGspStaticInfo`] command.
pub struct GetGspStaticInfoReply {
    gpu_name: [u8; 64],
    gpu_short_name: [u8; 64],
    /// The 16-byte SHA-1 based GPU identifier (GID) reported by GSP-RM.
    pub gpu_gid: [u8; 16],
    /// BAR1 Page Directory Entry base address.
    pub(crate) bar1_pde_base: u64,
    /// Usable FB (VRAM) regions for driver memory allocation.
    pub(crate) usable_fb_regions: KVec<Range<u64>>,
    /// Exclusive end of the FB physical address space.
    pub(crate) total_fb_end: u64,
    client: Handle<Client>,
    subdevice: Handle<Subdevice>,
}

impl MessageFromGsp for GetGspStaticInfoReply {
    const FUNCTION: MsgFunction = MsgFunction::GetGspStaticInfo;
    type Message = fw::commands::GspStaticConfigInfo;
    type InitError = Error;

    fn read(
        msg: &Self::Message,
        _sbuffer: &mut SBufferIter<array::IntoIter<&[u8], 2>>,
    ) -> Result<Self, Self::InitError> {
        let mut usable_fb_regions = KVec::new();
        for region in msg.usable_fb_regions() {
            usable_fb_regions.push(region, GFP_KERNEL)?;
        }
        let total_fb_end = msg.total_fb_end().ok_or(EINVAL)?;

        Ok(GetGspStaticInfoReply {
            gpu_name: msg.gpu_name_str(),
            gpu_short_name: msg.gpu_short_name_str(),
            gpu_gid: msg.gpu_gid(),
            bar1_pde_base: msg.bar1_pde_base(),
            usable_fb_regions,
            total_fb_end,
            client: msg.client(),
            subdevice: msg.subdevice(),
        })
    }
}

/// Error type for [`GetGspStaticInfoReply::gpu_name`].
#[derive(Debug)]
pub enum GpuNameError {
    /// The GPU name string does not contain a null terminator.
    NoNullTerminator(FromBytesUntilNulError),

    /// The GPU name string contains invalid UTF-8.
    InvalidUtf8(Utf8Error),
}

impl GetGspStaticInfoReply {
    /// Returns the name of the GPU as a string.
    ///
    /// Returns an error if the string given by the GSP does not contain a null terminator or
    /// contains invalid UTF-8.
    pub fn gpu_name(&self) -> Result<&str, GpuNameError> {
        CStr::from_bytes_until_nul(&self.gpu_name)
            .map_err(GpuNameError::NoNullTerminator)?
            .to_str()
            .map_err(GpuNameError::InvalidUtf8)
    }

    /// Returns the short name of the GPU as a string.
    ///
    /// Returns an error if the string given by the GSP does not contain a null terminator or
    /// contains invalid UTF-8.
    pub fn gpu_short_name(&self) -> core::result::Result<&str, GpuNameError> {
        CStr::from_bytes_until_nul(&self.gpu_short_name)
            .map_err(GpuNameError::NoNullTerminator)?
            .to_str()
            .map_err(GpuNameError::InvalidUtf8)
    }

    /// Returns the total usable VRAM size in bytes, i.e. the summed lengths of all usable FB
    /// regions.
    pub fn vram_size(&self) -> u64 {
        self.usable_fb_regions.iter().fold(0, |size, region| {
            size.saturating_add(region.end - region.start)
        })
    }

    /// Returns the client handle allocated by GSP-RM.
    pub(crate) fn client(&self) -> Handle<Client> {
        self.client
    }

    /// Returns the subdevice handle allocated by GSP-RM.
    pub(crate) fn subdevice(&self) -> Handle<Subdevice> {
        self.subdevice
    }
}

pub(crate) use fw::commands::PowerStateLevel;

/// The `UnloadingGuestDriver` command, used to shut down the GSP.
///
/// Only used within the `gsp` module.
pub(super) struct UnloadingGuestDriver {
    level: PowerStateLevel,
}

impl UnloadingGuestDriver {
    /// Creates a new `UnloadingGuestDriver` command for the given [`PowerStateLevel`].
    pub(super) fn new(level: PowerStateLevel) -> Self {
        Self { level }
    }
}

impl CommandToGsp for UnloadingGuestDriver {
    const FUNCTION: MsgFunction = MsgFunction::UnloadingGuestDriver;
    type Command = fw::commands::UnloadingGuestDriver;
    type Reply = UnloadingGuestDriverReply;
    type InitError = Infallible;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        fw::commands::UnloadingGuestDriver::new(self.level)
    }
}

/// The reply from the GSP to the [`UnloadingGuestDriver`] command.
pub(super) struct UnloadingGuestDriverReply;

impl MessageFromGsp for UnloadingGuestDriverReply {
    const FUNCTION: MsgFunction = MsgFunction::UnloadingGuestDriver;
    type InitError = Infallible;
    type Message = ();

    fn read(
        _msg: &Self::Message,
        _sbuffer: &mut SBufferIter<array::IntoIter<&[u8], 2>>,
    ) -> Result<Self, Self::InitError> {
        Ok(UnloadingGuestDriverReply)
    }
}

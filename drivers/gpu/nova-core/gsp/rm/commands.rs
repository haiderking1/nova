// SPDX-License-Identifier: GPL-2.0

use core::{
    array,
    convert::Infallible,
    mem::size_of, //
};

use kernel::{
    device,
    prelude::*,
    transmute::{
        AsBytes,
        FromBytes, //
    }, //
};

use crate::{
    gsp::{
        cmdq::{
            Cmdq,
            CommandToGsp,
            GspRpcError,
            MessageFromGsp, //
        },
        commands::{
            Client,
            Device,
            DisplayCommon,
            Handle,
            Subdevice, //
        },
        fw::{
            rm::*,
            GspMsgRmStatus,
            MsgFunction, //
        },
    },
    sbuffer::SBufferIter, //
};

/// Wraps a [`RmControl`] command to provide the infrastructure for sending it on the command queue.
struct RawRmControl<'a, T>(&'a RmControl<T>)
where
    T: RmControlCommand;

impl<'a, T: RmControlCommand> CommandToGsp for RawRmControl<'a, T> {
    const FUNCTION: MsgFunction = MsgFunction::GspRmControl;
    type Command = GspRmControl;
    type Reply = RmControlReply;
    type InitError = Error;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        let params_size: u32 = T::LEN.try_into()?;
        Ok(GspRmControl::new(
            self.0.client,
            self.0.object,
            T::FUNCTION,
            params_size,
        ))
    }

    fn variable_payload_len(&self) -> usize {
        T::LEN
    }

    fn init_variable_payload(
        &self,
        dst: &mut SBufferIter<array::IntoIter<&mut [u8], 2>>,
    ) -> Result {
        self.0.cmd.write_payload(dst)
    }
}

/// Command for sending an RM control message to the GSP.
///
/// RM control messages are used to query or control RM objects (see [`Handle`] for more info on RM
/// objects). It takes a client handle and an RM object handle identifying the target of the
/// message, within the given client.
pub(crate) struct RmControl<T>
where
    T: RmControlCommand,
{
    /// The client handle under which `object` is allocated.
    client: Handle<Client>,
    /// The RM object handle to query or control.
    object: Handle<T::Target>,
    /// The specific RM control command to send.
    cmd: T,
}

impl<T: RmControlCommand> RmControl<T> {
    /// Creates a new RM control command.
    pub(crate) fn new(client: Handle<Client>, object: Handle<T::Target>, cmd: T) -> Self {
        Self {
            client,
            object,
            cmd,
        }
    }

    /// Sends an RM control command, checks the reply status, and returns the reply.
    pub(crate) fn send(
        self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
    ) -> Result<T::Reply, GspRpcError> {
        let reply = cmdq.send_command(RawRmControl(&self))?;

        reply
            .status
            .log_if_warning(dev, T::FUNCTION)
            .map_err(GspRpcError::Rm)?;

        self.cmd
            .parse_reply(&reply.params)
            .map_err(GspRpcError::Transport)
    }
}

/// Response from an RM control message.
struct RmControlReply {
    status: GspMsgRmStatus,
    params: KVVec<u8>,
}

impl MessageFromGsp for RmControlReply {
    const FUNCTION: MsgFunction = MsgFunction::GspRmControl;
    type Message = GspRmControl;
    type InitError = Error;

    fn read(
        msg: &Self::Message,
        sbuffer: &mut SBufferIter<array::IntoIter<&[u8], 2>>,
    ) -> Result<Self, Self::InitError> {
        Ok(RmControlReply {
            status: msg.status()?,
            params: sbuffer.read_to_vec(GFP_KERNEL)?,
        })
    }
}

/// Trait for RM control commands that can be sent using [`RmControl`].
pub(crate) trait RmControlCommand {
    /// The specific RM control message kind to send.
    const FUNCTION: RmControlMsgFunction;

    /// Length of the command-specific payload to send with the RM control message.
    const LEN: usize;

    /// The type of the reply after parsing the raw bytes from the RM control reply message.
    type Reply;

    /// The type of the RM object that this command targets.
    type Target;

    /// Writes the payload of the command into `dst`.
    fn write_payload(&self, dst: &mut SBufferIter<array::IntoIter<&mut [u8], 2>>) -> Result;

    /// Parses the reply bytes from an RM control reply message into the command-specific
    /// reply type.
    fn parse_reply(&self, params: &[u8]) -> Result<Self::Reply>;
}

/// Wraps a [`RmAlloc`] command to provide the infrastructure for sending it on the command queue.
struct RawRmAlloc<'a, T>(&'a RmAlloc<T>)
where
    T: RmAllocCommand;

impl<'a, T: RmAllocCommand> CommandToGsp for RawRmAlloc<'a, T> {
    const FUNCTION: MsgFunction = MsgFunction::GspRmAlloc;
    type Command = GspRmAlloc;
    type Reply = RmAllocReply;
    type InitError = Error;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        let params_size: u32 = size_of::<T::Params>().try_into()?;
        Ok(GspRmAlloc::new(
            self.0.client,
            self.0.parent,
            self.0.object,
            T::CLASS,
            params_size,
        ))
    }

    fn variable_payload_len(&self) -> usize {
        size_of::<T::Params>()
    }

    fn init_variable_payload(
        &self,
        dst: &mut SBufferIter<array::IntoIter<&mut [u8], 2>>,
    ) -> Result {
        dst.write_all(self.0.cmd.params().as_bytes())
    }
}

/// Command for allocating an RM object on the GSP.
///
/// The object is allocated under `parent` within `client`, and named by `object`, a handle chosen
/// by the caller that must be unique within the client.
pub(crate) struct RmAlloc<T>
where
    T: RmAllocCommand,
{
    /// The client under which the object is allocated.
    client: Handle<Client>,
    /// The parent of the new object.
    parent: Handle<T::Parent>,
    /// The handle of the new object.
    object: Handle<T::Object>,
    /// The class-specific allocation command.
    cmd: T,
}

impl<T: RmAllocCommand> RmAlloc<T> {
    /// Creates a new RM alloc command.
    pub(crate) fn new(
        client: Handle<Client>,
        parent: Handle<T::Parent>,
        object: Handle<T::Object>,
        cmd: T,
    ) -> Self {
        Self {
            client,
            parent,
            object,
            cmd,
        }
    }

    /// Sends the RM alloc command, checks the reply status, and returns the handle of the new
    /// object.
    pub(crate) fn send(
        self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
    ) -> Result<Handle<T::Object>, GspRpcError> {
        let reply = cmdq.send_command(RawRmAlloc(&self))?;

        reply
            .status
            .log_if_warning(dev, T::CLASS)
            .map_err(GspRpcError::Rm)?;

        Ok(self.object)
    }
}

/// Response from an RM alloc message.
struct RmAllocReply {
    status: GspMsgRmStatus,
}

impl MessageFromGsp for RmAllocReply {
    const FUNCTION: MsgFunction = MsgFunction::GspRmAlloc;
    type Message = GspRmAlloc;
    type InitError = Error;

    fn read(
        msg: &Self::Message,
        sbuffer: &mut SBufferIter<array::IntoIter<&[u8], 2>>,
    ) -> Result<Self, Self::InitError> {
        // GSP-RM echoes the allocation parameters back, possibly updated. None of the classes
        // allocated so far return anything of use in them.
        sbuffer.read_to_vec(GFP_KERNEL)?;

        Ok(RmAllocReply {
            status: msg.status()?,
        })
    }
}

/// Trait for RM objects that can be allocated using [`RmAlloc`].
pub(crate) trait RmAllocCommand {
    /// The class of the object to allocate.
    const CLASS: RmObjectClass;

    /// The type of the RM object under which the object is allocated.
    type Parent;

    /// The type of the allocated RM object.
    type Object;

    /// The class-specific allocation parameters.
    type Params: AsBytes;

    /// Returns the allocation parameters.
    fn params(&self) -> &Self::Params;
}

/// Allocation of a client, the root of an RM object tree.
///
/// A client is its own parent: allocate it with the same handle as client, parent and object.
pub(crate) struct AllocRoot(RootAllocParams);

impl AllocRoot {
    /// Creates the command to allocate the client `client`.
    pub(crate) fn new(client: Handle<Client>) -> Self {
        Self(RootAllocParams::new(client))
    }
}

impl RmAllocCommand for AllocRoot {
    const CLASS: RmObjectClass = RmObjectClass::Root;
    type Parent = Client;
    type Object = Client;
    type Params = RootAllocParams;

    fn params(&self) -> &Self::Params {
        &self.0
    }
}

/// Allocation of a device under a client.
pub(crate) struct AllocDevice(DeviceAllocParams);

impl AllocDevice {
    /// Creates the command to allocate a device under `client`.
    pub(crate) fn new(client: Handle<Client>) -> Self {
        Self(DeviceAllocParams::new(client))
    }
}

impl RmAllocCommand for AllocDevice {
    const CLASS: RmObjectClass = RmObjectClass::Device;
    type Parent = Client;
    type Object = Device;
    type Params = DeviceAllocParams;

    fn params(&self) -> &Self::Params {
        &self.0
    }
}

/// Allocation of a subdevice under a device.
pub(crate) struct AllocSubdevice(SubdeviceAllocParams);

impl AllocSubdevice {
    /// Creates the command to allocate subdevice 0.
    pub(crate) fn new() -> Self {
        Self(SubdeviceAllocParams::new())
    }
}

impl RmAllocCommand for AllocSubdevice {
    const CLASS: RmObjectClass = RmObjectClass::Subdevice;
    type Parent = Device;
    type Object = Subdevice;
    type Params = SubdeviceAllocParams;

    fn params(&self) -> &Self::Params {
        &self.0
    }
}

/// Allocation of the display engine's common object under a device.
pub(crate) struct AllocDisplayCommon;

impl RmAllocCommand for AllocDisplayCommon {
    const CLASS: RmObjectClass = RmObjectClass::DisplayCommon;
    type Parent = Device;
    type Object = DisplayCommon;
    // The class takes no parameters.
    type Params = [u8; 0];

    fn params(&self) -> &Self::Params {
        &[]
    }
}

/// Command for freeing an RM object, and all of its children, on the GSP.
pub(crate) struct RmFree<T> {
    /// The client under which `object` is allocated.
    client: Handle<Client>,
    /// The object to free.
    object: Handle<T>,
}

impl<T> RmFree<T> {
    /// Creates a new RM free command.
    pub(crate) fn new(client: Handle<Client>, object: Handle<T>) -> Self {
        Self { client, object }
    }

    /// Sends the RM free command and checks the reply status.
    pub(crate) fn send(self, dev: &device::Device, cmdq: &Cmdq<'_>) -> Result<(), GspRpcError> {
        let reply = cmdq.send_command(self)?;

        reply
            .status
            .log_if_warning(dev, MsgFunction::Free)
            .map_err(GspRpcError::Rm)
    }
}

impl<T> CommandToGsp for RmFree<T> {
    const FUNCTION: MsgFunction = MsgFunction::Free;
    type Command = GspRmFree;
    type Reply = RmFreeReply;
    type InitError = Infallible;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        GspRmFree::new(self.client, self.object)
    }
}

/// Response from an RM free message.
pub(crate) struct RmFreeReply {
    status: GspMsgRmStatus,
}

impl MessageFromGsp for RmFreeReply {
    const FUNCTION: MsgFunction = MsgFunction::Free;
    type Message = GspRmFree;
    type InitError = Error;

    fn read(
        msg: &Self::Message,
        _sbuffer: &mut SBufferIter<array::IntoIter<&[u8], 2>>,
    ) -> Result<Self, Self::InitError> {
        Ok(RmFreeReply {
            status: msg.status()?,
        })
    }
}

/// Implements [`RmControlCommand`] for a display control whose parameters are both sent and
/// returned as `$params`.
macro_rules! disp_control {
    ($cmd:ty, $function:ident, $params:ty, $reply:ty, |$p:ident| $parse:expr) => {
        impl RmControlCommand for $cmd {
            const FUNCTION: RmControlMsgFunction = RmControlMsgFunction::$function;
            const LEN: usize = size_of::<$params>();
            type Reply = $reply;
            type Target = DisplayCommon;

            fn write_payload(
                &self,
                dst: &mut SBufferIter<array::IntoIter<&mut [u8], 2>>,
            ) -> Result {
                dst.write_all(self.0.as_bytes())
            }

            fn parse_reply(&self, params: &[u8]) -> Result<Self::Reply> {
                let $p = <$params>::from_bytes(params).ok_or(EINVAL)?;
                Ok($parse)
            }
        }
    };
}

/// RM control command for querying the number of heads of the display engine.
pub(crate) struct DispGetNumHeads(DispGetNumHeadsParams);

impl DispGetNumHeads {
    /// Creates the command.
    pub(crate) fn new() -> Self {
        Self(DispGetNumHeadsParams::new())
    }
}

disp_control!(
    DispGetNumHeads,
    DispSystemGetNumHeads,
    DispGetNumHeadsParams,
    u32,
    |p| p.num_heads()
);

/// RM control command for querying the display active on a head.
///
/// The reply is the display ID (a mask with a single bit set) of the active display, or 0 if no
/// display is active on the head. A display may have been made active by the system firmware.
pub(crate) struct DispGetActive(DispGetActiveParams);

impl DispGetActive {
    /// Creates the command for `head`.
    pub(crate) fn new(head: u32) -> Self {
        Self(DispGetActiveParams::new(head))
    }
}

disp_control!(
    DispGetActive,
    DispSystemGetActive,
    DispGetActiveParams,
    u32,
    |p| p.display_id()
);

/// RM control command for querying the type of connector a display is wired to.
pub(crate) struct DispGetConnectorType(DispGetConnectorDataParams);

impl DispGetConnectorType {
    /// Creates the command for `display_id`.
    pub(crate) fn new(display_id: u32) -> Self {
        Self(DispGetConnectorDataParams::new(display_id))
    }
}

disp_control!(
    DispGetConnectorType,
    DispSpecificGetConnectorData,
    DispGetConnectorDataParams,
    ConnectorType,
    |p| p.connector_type()
);

/// RM control command for reading the EDID of a display.
pub(crate) struct DispGetEdid(KBox<DispGetEdidParams>);

impl DispGetEdid {
    /// Creates the command for `display_id`.
    pub(crate) fn new(display_id: u32) -> Result<Self> {
        Ok(Self(KBox::new(DispGetEdidParams::new(display_id), GFP_KERNEL)?))
    }
}

disp_control!(
    DispGetEdid,
    DispSpecificGetEdidV2,
    DispGetEdidParams,
    KVec<u8>,
    |p| {
        let mut edid = KVec::new();
        edid.extend_from_slice(p.edid().ok_or(EIO)?, GFP_KERNEL)?;
        edid
    }
);

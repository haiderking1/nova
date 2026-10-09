// SPDX-License-Identifier: GPL-2.0

use core::{
    array,
    convert::Infallible,
    marker::PhantomData,
    mem::size_of,
    ops::Range, //
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
            Channel,
            ChannelGroup,
            Client,
            ContextShare,
            CopyEngine,
            Device,
            DisplayCommon,
            DisplayCoreChannel,
            DisplayRoot,
            DisplayWindowChannel,
            GraphicsObject,
            Handle,
            Subdevice,
            VaSpace, //
        },
        fw::{
            rm::*,
            GspMsgRmStatus,
            MsgFunction, //
        },
    },
    mm::VramAddress,
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

/// Implements [`RmControlCommand`] for a control sent to an object of type `$target`, whose
/// parameters are both sent and returned as `$params`.
macro_rules! rm_control {
    ($cmd:ty, $target:ty, $function:ident, $params:ty, $reply:ty, |$p:ident| $parse:expr) => {
        impl RmControlCommand for $cmd {
            const FUNCTION: RmControlMsgFunction = RmControlMsgFunction::$function;
            const LEN: usize = size_of::<$params>();
            type Reply = $reply;
            type Target = $target;

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

rm_control!(
    DispGetNumHeads,
    DisplayCommon,
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

rm_control!(
    DispGetActive,
    DisplayCommon,
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

rm_control!(
    DispGetConnectorType,
    DisplayCommon,
    DispSpecificGetConnectorData,
    DispGetConnectorDataParams,
    ConnectorType,
    |p| p.connector_type()
);

/// RM control command for reading the EDID of a display.
pub(crate) struct DispGetEdid(KBox<DispGetEdidParams>);

impl DispGetEdid {
    /// Creates the command for `display_id`, reading the EDID from the display or, if `cached` is
    /// true, returning the copy GSP-RM cached.
    pub(crate) fn new(display_id: u32, cached: bool) -> Result<Self> {
        Ok(Self(KBox::new(
            DispGetEdidParams::new(display_id, cached),
            GFP_KERNEL,
        )?))
    }
}

rm_control!(
    DispGetEdid,
    DisplayCommon,
    DispSpecificGetEdidV2,
    DispGetEdidParams,
    KVec<u8>,
    |p| {
        let mut edid = KVec::new();
        edid.extend_from_slice(p.edid().ok_or(EIO)?, GFP_KERNEL)?;
        edid
    }
);

/// RM control command for querying the static configuration of the display engine.
///
/// The reply is the mask of the windows the display engine has. Sent to GSP-RM's internal
/// subdevice.
pub(crate) struct DispGetStaticInfo(DispGetStaticInfoParams);

impl DispGetStaticInfo {
    /// Creates the command.
    pub(crate) fn new() -> Self {
        Self(DispGetStaticInfoParams::new())
    }
}

rm_control!(
    DispGetStaticInfo,
    Subdevice,
    InternalDisplayGetStaticInfo,
    DispGetStaticInfoParams,
    u32,
    |p| p.window_present_mask()
);

/// RM control command for setting the instance memory of the display engine, which holds its
/// hash table and the context DMA objects of its channels.
///
/// Sent to GSP-RM's internal subdevice.
pub(crate) struct DispWriteInstMem(DispWriteInstMemParams);

impl DispWriteInstMem {
    /// Creates the command for the instance memory at `range`.
    pub(crate) fn new(range: Range<VramAddress>) -> Self {
        Self(DispWriteInstMemParams::new(
            range.start.into_raw(),
            range.end - range.start,
        ))
    }
}

rm_control!(
    DispWriteInstMem,
    Subdevice,
    InternalDisplayWriteInstMem,
    DispWriteInstMemParams,
    (),
    |_p| ()
);

/// RM control command for setting the push buffer of a display channel, which must precede the
/// allocation of the channel.
///
/// Sent to GSP-RM's internal subdevice.
pub(crate) struct DispChannelPushbuffer(DispChannelPushbufferParams);

impl DispChannelPushbuffer {
    /// Creates the command for the core channel of a GA10x display engine, whose push buffer is
    /// at DMA address `dma_addr`.
    pub(crate) fn core_ga102(dma_addr: u64) -> Self {
        // There is a single core channel.
        Self(DispChannelPushbufferParams::new(
            RmObjectClass::CoreChannelGa102,
            0,
            dma_addr,
        ))
    }

    /// Creates the command for the channel of window `window` of a GA10x display engine, whose
    /// push buffer is at DMA address `dma_addr`.
    pub(crate) fn window_ga102(window: u32, dma_addr: u64) -> Self {
        Self(DispChannelPushbufferParams::new(
            RmObjectClass::WindowChannelGa102,
            window,
            dma_addr,
        ))
    }
}

rm_control!(
    DispChannelPushbuffer,
    Subdevice,
    InternalDisplayChannelPushbuffer,
    DispChannelPushbufferParams,
    (),
    |_p| ()
);

/// Allocation of the display object of a GA10x display engine under a device.
pub(crate) struct AllocDisplayGa102;

impl RmAllocCommand for AllocDisplayGa102 {
    const CLASS: RmObjectClass = RmObjectClass::DisplayGa102;
    type Parent = Device;
    type Object = DisplayRoot;
    // The class takes no parameters.
    type Params = [u8; 0];

    fn params(&self) -> &Self::Params {
        &[]
    }
}

/// Allocation of the core channel of a GA10x display engine under its display object.
///
/// The push buffer of the channel must have been set with [`DispChannelPushbuffer`] first.
pub(crate) struct AllocCoreChannelGa102(DmaChannelAllocParams);

impl AllocCoreChannelGa102 {
    /// Creates the command, for a push buffer whose commands start at byte offset `put`.
    pub(crate) fn new(put: u32) -> Self {
        // There is a single core channel.
        Self(DmaChannelAllocParams::new(0, put))
    }
}

impl RmAllocCommand for AllocCoreChannelGa102 {
    const CLASS: RmObjectClass = RmObjectClass::CoreChannelGa102;
    type Parent = DisplayRoot;
    type Object = DisplayCoreChannel;
    type Params = DmaChannelAllocParams;

    fn params(&self) -> &Self::Params {
        &self.0
    }
}

/// Allocation of the channel of a window of a GA10x display engine under its display object.
///
/// The push buffer of the channel must have been set with [`DispChannelPushbuffer`] first.
pub(crate) struct AllocWindowChannelGa102(DmaChannelAllocParams);

impl AllocWindowChannelGa102 {
    /// Creates the command for the channel of window `window`, for a push buffer whose commands
    /// start at byte offset `put`.
    pub(crate) fn new(window: u32, put: u32) -> Self {
        Self(DmaChannelAllocParams::new(window, put))
    }
}

impl RmAllocCommand for AllocWindowChannelGa102 {
    const CLASS: RmObjectClass = RmObjectClass::WindowChannelGa102;
    type Parent = DisplayRoot;
    type Object = DisplayWindowChannel;
    type Params = DmaChannelAllocParams;

    fn params(&self) -> &Self::Params {
        &self.0
    }
}

/// Allocation of a GPU virtual address space under a device, whose page tables the client owns:
/// see [`DmaSetPageDirectory`].
pub(crate) struct AllocVaSpace(VaSpaceAllocParams);

impl AllocVaSpace {
    /// Creates the command for an address space whose page tables the client owns.
    pub(crate) fn externally_owned() -> Self {
        Self(VaSpaceAllocParams::externally_owned())
    }

    /// Creates the command for an address space split between the client and GSP-RM: see
    /// [`VaspaceCopyServerReservedPdes`].
    pub(crate) fn split() -> Self {
        Self(VaSpaceAllocParams::split())
    }
}

impl RmAllocCommand for AllocVaSpace {
    const CLASS: RmObjectClass = RmObjectClass::VaSpace;
    type Parent = Device;
    type Object = VaSpace;
    type Params = VaSpaceAllocParams;

    fn params(&self) -> &Self::Params {
        &self.0
    }
}

/// Allocation of an object of a GA10x copy engine on a channel bound to it.
pub(crate) struct AllocCopyEngineAmpereB(CopyEngineAllocParams);

impl AllocCopyEngineAmpereB {
    /// Creates the command for copy engine `index`.
    pub(crate) fn new(index: u32) -> Self {
        Self(CopyEngineAllocParams::new(index))
    }
}

impl RmAllocCommand for AllocCopyEngineAmpereB {
    const CLASS: RmObjectClass = RmObjectClass::CopyEngineAmpereB;
    type Parent = Channel;
    type Object = CopyEngine;
    type Params = CopyEngineAllocParams;

    fn params(&self) -> &Self::Params {
        &self.0
    }
}

/// RM control command making a page directory in VRAM the root of the page tables of an address
/// space whose page tables the client owns, sent to the address space's device.
pub(crate) struct DmaSetPageDirectory(SetPageDirectoryParams);

impl DmaSetPageDirectory {
    /// Creates the command for the page directory of `num_entries` entries at `pdb`, of the
    /// address space `va_space`.
    pub(crate) fn new(va_space: Handle<VaSpace>, pdb: VramAddress, num_entries: u32) -> Self {
        Self(SetPageDirectoryParams::new(
            va_space.as_raw(),
            pdb.into_raw(),
            num_entries,
        ))
    }
}

rm_control!(
    DmaSetPageDirectory,
    Device,
    DmaSetPageDirectory,
    SetPageDirectoryParams,
    (),
    |_p| ()
);

/// RM control command unsetting the page directory of an address space, sent to the address
/// space's device.
pub(crate) struct DmaUnsetPageDirectory(UnsetPageDirectoryParams);

impl DmaUnsetPageDirectory {
    /// Creates the command for the address space `va_space`.
    pub(crate) fn new(va_space: Handle<VaSpace>) -> Self {
        Self(UnsetPageDirectoryParams::new(va_space.as_raw()))
    }
}

rm_control!(
    DmaUnsetPageDirectory,
    Device,
    DmaUnsetPageDirectory,
    UnsetPageDirectoryParams,
    (),
    |_p| ()
);

/// RM control command binding a channel to the engine it runs on.
pub(crate) struct GpfifoBind(GpfifoBindParams);

impl GpfifoBind {
    /// Creates the command for the engine of type `engine_type`, an `NV2080_ENGINE_TYPE`.
    pub(crate) fn new(engine_type: u32) -> Self {
        Self(GpfifoBindParams::new(engine_type))
    }
}

rm_control!(
    GpfifoBind,
    Channel,
    GpfifoBind,
    GpfifoBindParams,
    (),
    |_p| ()
);

/// RM control command enabling the scheduling of a channel.
pub(crate) struct GpfifoSchedule(GpfifoScheduleParams);

impl GpfifoSchedule {
    /// Creates the command.
    pub(crate) fn enable() -> Self {
        Self(GpfifoScheduleParams::enable())
    }
}

rm_control!(
    GpfifoSchedule,
    Channel,
    GpfifoSchedule,
    GpfifoScheduleParams,
    (),
    |_p| ()
);

/// RM control command querying the size of the method buffer every channel needs, which the GPU
/// saves pending methods into on faults.
pub(crate) struct CeGetFaultMethodBufferSize(CeGetFaultMethodBufferSizeParams);

impl CeGetFaultMethodBufferSize {
    /// Creates the command.
    pub(crate) fn new() -> Self {
        Self(CeGetFaultMethodBufferSizeParams::new())
    }
}

rm_control!(
    CeGetFaultMethodBufferSize,
    Subdevice,
    CeGetFaultMethodBufferSize,
    CeGetFaultMethodBufferSizeParams,
    u32,
    |p| p.size()
);

/// Allocation of a GA10x GPFIFO channel under `P`: a [`Device`], when RM puts the channel in a
/// channel group of its own, or a [`ChannelGroup`].
pub(crate) struct AllocChannelGpfifoAmpere<P>(ChannelGpfifoAllocParams, PhantomData<P>);

impl AllocChannelGpfifoAmpere<Device> {
    /// Creates the command for a channel alone in the address space `va_space`, see
    /// [`ChannelGpfifoAllocParams::new`].
    pub(crate) fn alone(
        va_space: Handle<VaSpace>,
        engine_type: u32,
        chid: u32,
        gpfifo: u64,
        entries: u32,
        memory: &ChannelMemory,
        privileged: bool,
    ) -> Self {
        Self(
            ChannelGpfifoAllocParams::new(
                ChannelSpace::VaSpace(va_space.as_raw()),
                engine_type,
                chid,
                gpfifo,
                entries,
                memory,
                privileged,
            ),
            PhantomData,
        )
    }
}

impl AllocChannelGpfifoAmpere<ChannelGroup> {
    /// Creates the command for a channel of a group, in the group's context share
    /// `context_share`, see [`ChannelGpfifoAllocParams::new`]. The method buffer of `memory` must
    /// be the group's one of the channel's runqueue.
    pub(crate) fn in_group(
        context_share: Handle<ContextShare>,
        engine_type: u32,
        chid: u32,
        gpfifo: u64,
        entries: u32,
        memory: &ChannelMemory,
        privileged: bool,
    ) -> Self {
        Self(
            ChannelGpfifoAllocParams::new(
                ChannelSpace::ContextShare(context_share.as_raw()),
                engine_type,
                chid,
                gpfifo,
                entries,
                memory,
                privileged,
            ),
            PhantomData,
        )
    }
}

impl<P> RmAllocCommand for AllocChannelGpfifoAmpere<P> {
    const CLASS: RmObjectClass = RmObjectClass::ChannelGpfifoAmpere;
    type Parent = P;
    type Object = Channel;
    type Params = ChannelGpfifoAllocParams;

    fn params(&self) -> &Self::Params {
        &self.0
    }
}

/// Allocation of a channel group (TSG) under a device.
pub(crate) struct AllocChannelGroupKepler(ChannelGroupAllocParams);

impl AllocChannelGroupKepler {
    /// Creates the command for a group on engine `engine_type`, an `NV2080_ENGINE_TYPE`, whose
    /// channels run in the address space `va_space`.
    pub(crate) fn new(va_space: Handle<VaSpace>, engine_type: u32) -> Self {
        Self(ChannelGroupAllocParams::new(va_space.as_raw(), engine_type))
    }
}

impl RmAllocCommand for AllocChannelGroupKepler {
    const CLASS: RmObjectClass = RmObjectClass::ChannelGroupKepler;
    type Parent = Device;
    type Object = ChannelGroup;
    type Params = ChannelGroupAllocParams;

    fn params(&self) -> &Self::Params {
        &self.0
    }
}

/// Allocation of a context share under a channel group.
pub(crate) struct AllocContextShareFermi(ContextShareAllocParams);

impl AllocContextShareFermi {
    /// Creates the command for the group's synchronous subcontext, in the address space
    /// `va_space`: see [`ContextShareAllocParams::sync`].
    pub(crate) fn sync(va_space: Handle<VaSpace>) -> Self {
        Self(ContextShareAllocParams::sync(va_space.as_raw()))
    }
}

impl RmAllocCommand for AllocContextShareFermi {
    const CLASS: RmObjectClass = RmObjectClass::ContextShareFermi;
    type Parent = ChannelGroup;
    type Object = ContextShare;
    type Params = ContextShareAllocParams;

    fn params(&self) -> &Self::Params {
        &self.0
    }
}

/// RM control command giving GSP-RM the method buffers of a channel group, which kernel RM sends
/// right after allocating the group (`kchangrpapiConstruct`).
pub(crate) struct TsgPromoteFaultMethodBuffers(PromoteFaultMethodBuffersParams);

impl TsgPromoteFaultMethodBuffers {
    /// Creates the command, see [`PromoteFaultMethodBuffersParams::new`].
    pub(crate) fn new(buffers: [u64; RUNQUEUES], size: u64) -> Self {
        Self(PromoteFaultMethodBuffersParams::new(buffers, size))
    }
}

rm_control!(
    TsgPromoteFaultMethodBuffers,
    ChannelGroup,
    TsgInternalPromoteFaultMethodBuffers,
    PromoteFaultMethodBuffersParams,
    (),
    |_p| ()
);

/// RM control command querying a page of the device info table: the engines of the GPU and the
/// runlists they are on.
///
/// The reply is the engines listed, and whether more entries follow.
pub(crate) struct FifoGetDeviceInfoTable(DeviceInfoTableParams);

impl FifoGetDeviceInfoTable {
    /// Number of entries of a page.
    pub(crate) const PAGE_ENTRIES: u32 = DeviceInfoTableParams::MAX_ENTRIES;

    /// Creates the command for the page starting at entry `base`.
    pub(crate) fn new(base: u32) -> Self {
        Self(DeviceInfoTableParams::new(base))
    }
}

rm_control!(
    FifoGetDeviceInfoTable,
    Subdevice,
    FifoGetDeviceInfoTable,
    DeviceInfoTableParams,
    (KVec<EngineInfo>, bool),
    |p| {
        let mut engines = KVec::new();
        for engine in p.engines() {
            engines.push(engine, GFP_KERNEL)?;
        }
        (engines, p.more())
    }
);

/// Allocation of a GA10x 3D object on a channel on the graphics engine.
pub(crate) struct AllocThreeDAmpereB;

impl RmAllocCommand for AllocThreeDAmpereB {
    const CLASS: RmObjectClass = RmObjectClass::ThreeDAmpereB;
    type Parent = Channel;
    type Object = GraphicsObject;
    // The class takes no parameters.
    type Params = [u8; 0];

    fn params(&self) -> &Self::Params {
        &[]
    }
}

/// Allocation of a 2D object on a channel on the graphics engine.
pub(crate) struct AllocTwoDFermiA;

impl RmAllocCommand for AllocTwoDFermiA {
    const CLASS: RmObjectClass = RmObjectClass::TwoDFermiA;
    type Parent = Channel;
    type Object = GraphicsObject;
    // The class takes no parameters.
    type Params = [u8; 0];

    fn params(&self) -> &Self::Params {
        &[]
    }
}

/// Allocation of an inline-to-memory object on a channel on the graphics engine.
pub(crate) struct AllocInlineToMemoryKeplerB;

impl RmAllocCommand for AllocInlineToMemoryKeplerB {
    const CLASS: RmObjectClass = RmObjectClass::InlineToMemoryKeplerB;
    type Parent = Channel;
    type Object = GraphicsObject;
    // The class takes no parameters.
    type Params = [u8; 0];

    fn params(&self) -> &Self::Params {
        &[]
    }
}

/// Allocation of a GA10x compute object on a channel on the graphics engine.
pub(crate) struct AllocComputeAmpereB;

impl RmAllocCommand for AllocComputeAmpereB {
    const CLASS: RmObjectClass = RmObjectClass::ComputeAmpereB;
    type Parent = Channel;
    type Object = GraphicsObject;
    // The class takes no parameters.
    type Params = [u8; 0];

    fn params(&self) -> &Self::Params {
        &[]
    }
}

/// RM control command querying the sizes of the context buffers of the graphics engines, sent to
/// GSP-RM's internal subdevice.
///
/// The reply is the size and alignment of the buffers of [`GR_CONTEXT_BUFFERS`], in order.
pub(crate) struct GrGetContextBuffersInfo(GrContextBuffersInfoParams);

impl GrGetContextBuffersInfo {
    /// Creates the command.
    pub(crate) fn new() -> Self {
        Self(GrContextBuffersInfoParams::new())
    }
}

rm_control!(
    GrGetContextBuffersInfo,
    Subdevice,
    InternalStaticKgrGetContextBuffersInfo,
    GrContextBuffersInfoParams,
    [ContextBufferInfo; GR_CONTEXT_BUFFERS.len()],
    |p| {
        let mut infos = [ContextBufferInfo {
            size: 0,
            alignment: 0,
        }; GR_CONTEXT_BUFFERS.len()];
        for (info, buffer) in infos.iter_mut().zip(GR_CONTEXT_BUFFERS.iter()) {
            *info = p.info(buffer).ok_or(EINVAL)?;
        }
        infos
    }
);

/// RM control command giving the graphics context buffers of a channel to GSP-RM, sent to the
/// subdevice of the channel's client.
pub(crate) struct GpuPromoteCtx(PromoteCtxParams);

impl GpuPromoteCtx {
    /// Creates the command for `channel` of `client`.
    pub(crate) fn graphics(client: Handle<Client>, channel: Handle<Channel>) -> Self {
        Self(PromoteCtxParams::graphics(
            client.as_raw(),
            channel.as_raw(),
        ))
    }

    /// Adds `entry`.
    pub(crate) fn push(&mut self, entry: &PromoteEntry) -> Result {
        self.0.push(entry)
    }
}

rm_control!(
    GpuPromoteCtx,
    Subdevice,
    GpuPromoteCtx,
    PromoteCtxParams,
    (),
    |_p| ()
);

/// RM control command giving GSP-RM the page directories above its part of a split address
/// space, [`RM_SERVER_VA`], sent to the address space. GSP-RM then uses the client's page tables,
/// and fills the entry of the last level covering its part.
pub(crate) struct VaspaceCopyServerReservedPdes(CopyServerReservedPdesParams);

impl VaspaceCopyServerReservedPdes {
    /// Creates the command, see [`CopyServerReservedPdesParams::new`].
    pub(crate) fn new(levels: &[ServerReservedLevel], entry_size: u64) -> Result<Self> {
        Ok(Self(CopyServerReservedPdesParams::new(levels, entry_size)?))
    }
}

rm_control!(
    VaspaceCopyServerReservedPdes,
    VaSpace,
    VaspaceCopyServerReservedPdes,
    CopyServerReservedPdesParams,
    (),
    |_p| ()
);

/// RM control command querying the interrupt vectors of the units whose interrupts the CPU
/// services, rather than GSP-RM.
///
/// Sent to GSP-RM's internal subdevice. The reply is the vector of the display engine's
/// interrupts, if the GPU has a display engine.
pub(crate) struct IntrGetKernelTable(IntrKernelTableParams);

impl IntrGetKernelTable {
    /// Creates the command.
    pub(crate) fn new() -> Self {
        Self(IntrKernelTableParams::new())
    }
}

rm_control!(
    IntrGetKernelTable,
    Subdevice,
    InternalIntrGetKernelTable,
    IntrKernelTableParams,
    Option<u32>,
    |p| p.display_vector()
);

/// RM control command querying the mask of the enabled GPCs, sent to GSP-RM's internal subdevice.
pub(crate) struct GpuGetGpcMask(GpcInfoParams);

impl GpuGetGpcMask {
    /// Creates the command.
    pub(crate) fn new() -> Self {
        Self(GpcInfoParams::new())
    }
}

rm_control!(
    GpuGetGpcMask,
    Subdevice,
    GpuGetFermiGpcInfo,
    GpcInfoParams,
    u32,
    |p| p.gpc_mask()
);

/// RM control command querying the mask of the enabled TPCs of a GPC, sent to GSP-RM's internal
/// subdevice.
pub(crate) struct GpuGetTpcMask(TpcInfoParams);

impl GpuGetTpcMask {
    /// Creates the command for the GPC `gpc`.
    pub(crate) fn new(gpc: u32) -> Self {
        Self(TpcInfoParams::new(gpc))
    }
}

rm_control!(
    GpuGetTpcMask,
    Subdevice,
    GpuGetFermiTpcInfo,
    TpcInfoParams,
    u32,
    |p| p.tpc_mask()
);

/// RM control command querying the static information of the graphics engines, sent to GSP-RM's
/// internal subdevice.
pub(crate) struct GrGetInfo(GrInfoParams);

impl GrGetInfo {
    /// Creates the command.
    pub(crate) fn new() -> Self {
        Self(GrInfoParams::new())
    }
}

rm_control!(
    GrGetInfo,
    Subdevice,
    InternalStaticKgrGetInfo,
    GrInfoParams,
    GrStaticInfo,
    |p| p.info().ok_or(EINVAL)?
);

/// RM control command for querying the configuration of the link of a DisplayPort display.
pub(crate) struct DpGetLinkConfig(DpGetLinkConfigParams);

impl DpGetLinkConfig {
    /// Creates the command for `display_id`.
    pub(crate) fn new(display_id: u32) -> Self {
        Self(DpGetLinkConfigParams::new(display_id))
    }
}

rm_control!(
    DpGetLinkConfig,
    DisplayCommon,
    DispDpGetLinkConfig,
    DpGetLinkConfigParams,
    DpLinkConfig,
    |p| p.config()
);

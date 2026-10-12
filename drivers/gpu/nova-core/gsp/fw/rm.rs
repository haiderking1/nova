// SPDX-License-Identifier: GPL-2.0

use core::ops::RangeInclusive;

use kernel::{
    prelude::*,
    transmute::{
        AsBytes,
        FromBytes, //
    }, //
};

use crate::{
    gsp::commands::{
        Client,
        Handle, //
    },
    num, //
};

use super::{
    bindings,
    GspMsgRmStatus, //
};

/// Command code for RM control RPCs sent using [`MsgFunction::GspRmControl`].
#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(u32)]
pub(crate) enum RmControlMsgFunction {
    /// Get the CE fault method buffer size.
    CeGetFaultMethodBufferSize = bindings::NV2080_CTRL_CMD_CE_GET_FAULT_METHOD_BUFFER_SIZE,
    /// Get the number of heads of the display engine.
    DispSystemGetNumHeads = bindings::NV0073_CTRL_CMD_SYSTEM_GET_NUM_HEADS,
    /// Get the display that is active on a head.
    DispSystemGetActive = bindings::NV0073_CTRL_CMD_SYSTEM_GET_ACTIVE,
    /// Get the connector data of a display.
    DispSpecificGetConnectorData = bindings::NV0073_CTRL_CMD_SPECIFIC_GET_CONNECTOR_DATA,
    /// Get the link configuration of a DisplayPort display.
    DispDpGetLinkConfig = bindings::NV0073_CTRL_CMD_DP_GET_LINK_CONFIG,
    /// Carry out a transfer on the AUX channel of a DisplayPort display.
    DispDpAuxchCtrl = bindings::NV0073_CTRL_CMD_DP_AUXCH_CTRL,
    /// Train the link of a DisplayPort display.
    DispDpCtrl = bindings::NV0073_CTRL_CMD_DP_CTRL,
    /// Get what the DisplayPort outputs of the GPU can do.
    DispDpGetCaps = bindings::NV0073_CTRL_CMD_DP_GET_CAPS,
    /// Get the configuration of a digital display (DFP) of the board.
    DispDfpGetInfo = bindings::NV0073_CTRL_CMD_DFP_GET_INFO,
    /// Have the display engine fetch less ahead of scanout.
    DispSetMempoolWarForBlitTearing = bindings::NV5070_CTRL_CMD_SET_MEMPOOL_WAR_FOR_BLIT_TEARING,
    /// Get the EDID of a display.
    DispSpecificGetEdidV2 = bindings::NV0073_CTRL_CMD_SPECIFIC_GET_EDID_V2,
    /// Get the static configuration of the display engine.
    InternalDisplayGetStaticInfo = bindings::NV2080_CTRL_CMD_INTERNAL_DISPLAY_GET_STATIC_INFO,
    /// Set the instance memory of the display engine.
    InternalDisplayWriteInstMem = bindings::NV2080_CTRL_CMD_INTERNAL_DISPLAY_WRITE_INST_MEM,
    /// Set the push buffer of a display channel, before allocating the channel.
    InternalDisplayChannelPushbuffer =
        bindings::NV2080_CTRL_CMD_INTERNAL_DISPLAY_CHANNEL_PUSHBUFFER,
    /// Set the page directory of a GPU virtual address space whose page tables the client owns.
    DmaSetPageDirectory = bindings::NV0080_CTRL_CMD_DMA_SET_PAGE_DIRECTORY,
    /// Unset the page directory of a GPU virtual address space.
    DmaUnsetPageDirectory = bindings::NV0080_CTRL_CMD_DMA_UNSET_PAGE_DIRECTORY,
    /// Bind a channel to the engine it runs on.
    GpfifoBind = bindings::NVA06F_CTRL_CMD_BIND,
    /// Enable or disable the scheduling of a channel.
    GpfifoSchedule = bindings::NVA06F_CTRL_CMD_GPFIFO_SCHEDULE,
    /// Give the method buffers of a channel group to GSP-RM.
    TsgInternalPromoteFaultMethodBuffers =
        bindings::NVA06C_CTRL_CMD_INTERNAL_PROMOTE_FAULT_METHOD_BUFFERS,
    /// Get the engines of the GPU and the runlists they are on.
    FifoGetDeviceInfoTable = bindings::NV2080_CTRL_CMD_FIFO_GET_DEVICE_INFO_TABLE,
    /// Get the sizes of the context buffers of the graphics engines.
    InternalStaticKgrGetContextBuffersInfo =
        bindings::NV2080_CTRL_CMD_INTERNAL_STATIC_KGR_GET_CONTEXT_BUFFERS_INFO,
    /// Give the context buffers of a channel to GSP-RM.
    GpuPromoteCtx = bindings::NV2080_CTRL_CMD_GPU_PROMOTE_CTX,
    /// Give GSP-RM the page directories above its part of a split address space.
    VaspaceCopyServerReservedPdes = bindings::NV90F1_CTRL_CMD_VASPACE_COPY_SERVER_RESERVED_PDES,
    /// Get the interrupt vectors of the units whose interrupts the CPU services.
    InternalIntrGetKernelTable = bindings::NV2080_CTRL_CMD_INTERNAL_INTR_GET_KERNEL_TABLE,
    /// Get the mask of the enabled GPCs.
    GpuGetFermiGpcInfo = bindings::NV2080_CTRL_CMD_GPU_GET_FERMI_GPC_INFO,
    /// Get the mask of the enabled TPCs of a GPC.
    GpuGetFermiTpcInfo = bindings::NV2080_CTRL_CMD_GPU_GET_FERMI_TPC_INFO,
    /// Get the static information of the graphics engines.
    InternalStaticKgrGetInfo = bindings::NV2080_CTRL_CMD_INTERNAL_STATIC_KGR_GET_INFO,
    /// Get the falcon engines GSP-RM constructed, and the sizes of their channels' contexts.
    GpuGetConstructedFalconInfo = bindings::NV2080_CTRL_CMD_GPU_GET_CONSTRUCTED_FALCON_INFO,
}

// TODO[FPRI]: This is a temporary solution to be replaced with the corresponding derive macros
// once they land.
impl TryFrom<u32> for RmControlMsgFunction {
    type Error = kernel::error::Error;

    fn try_from(value: u32) -> Result<Self> {
        match value {
            bindings::NV2080_CTRL_CMD_CE_GET_FAULT_METHOD_BUFFER_SIZE => {
                Ok(Self::CeGetFaultMethodBufferSize)
            }
            bindings::NV0073_CTRL_CMD_SYSTEM_GET_NUM_HEADS => Ok(Self::DispSystemGetNumHeads),
            bindings::NV0073_CTRL_CMD_SYSTEM_GET_ACTIVE => Ok(Self::DispSystemGetActive),
            bindings::NV0073_CTRL_CMD_SPECIFIC_GET_CONNECTOR_DATA => {
                Ok(Self::DispSpecificGetConnectorData)
            }
            bindings::NV0073_CTRL_CMD_SPECIFIC_GET_EDID_V2 => Ok(Self::DispSpecificGetEdidV2),
            bindings::NV0073_CTRL_CMD_DP_GET_LINK_CONFIG => Ok(Self::DispDpGetLinkConfig),
            bindings::NV0073_CTRL_CMD_DP_AUXCH_CTRL => Ok(Self::DispDpAuxchCtrl),
            bindings::NV0073_CTRL_CMD_DP_CTRL => Ok(Self::DispDpCtrl),
            bindings::NV0073_CTRL_CMD_DP_GET_CAPS => Ok(Self::DispDpGetCaps),
            bindings::NV0073_CTRL_CMD_DFP_GET_INFO => Ok(Self::DispDfpGetInfo),
            bindings::NV5070_CTRL_CMD_SET_MEMPOOL_WAR_FOR_BLIT_TEARING => {
                Ok(Self::DispSetMempoolWarForBlitTearing)
            }
            bindings::NV2080_CTRL_CMD_INTERNAL_DISPLAY_GET_STATIC_INFO => {
                Ok(Self::InternalDisplayGetStaticInfo)
            }
            bindings::NV2080_CTRL_CMD_INTERNAL_DISPLAY_WRITE_INST_MEM => {
                Ok(Self::InternalDisplayWriteInstMem)
            }
            bindings::NV2080_CTRL_CMD_INTERNAL_DISPLAY_CHANNEL_PUSHBUFFER => {
                Ok(Self::InternalDisplayChannelPushbuffer)
            }
            bindings::NV0080_CTRL_CMD_DMA_SET_PAGE_DIRECTORY => Ok(Self::DmaSetPageDirectory),
            bindings::NV0080_CTRL_CMD_DMA_UNSET_PAGE_DIRECTORY => Ok(Self::DmaUnsetPageDirectory),
            bindings::NVA06F_CTRL_CMD_BIND => Ok(Self::GpfifoBind),
            bindings::NVA06F_CTRL_CMD_GPFIFO_SCHEDULE => Ok(Self::GpfifoSchedule),
            bindings::NVA06C_CTRL_CMD_INTERNAL_PROMOTE_FAULT_METHOD_BUFFERS => {
                Ok(Self::TsgInternalPromoteFaultMethodBuffers)
            }
            bindings::NV2080_CTRL_CMD_FIFO_GET_DEVICE_INFO_TABLE => {
                Ok(Self::FifoGetDeviceInfoTable)
            }
            bindings::NV2080_CTRL_CMD_INTERNAL_STATIC_KGR_GET_CONTEXT_BUFFERS_INFO => {
                Ok(Self::InternalStaticKgrGetContextBuffersInfo)
            }
            bindings::NV2080_CTRL_CMD_GPU_PROMOTE_CTX => Ok(Self::GpuPromoteCtx),
            bindings::NV90F1_CTRL_CMD_VASPACE_COPY_SERVER_RESERVED_PDES => {
                Ok(Self::VaspaceCopyServerReservedPdes)
            }
            bindings::NV2080_CTRL_CMD_INTERNAL_INTR_GET_KERNEL_TABLE => {
                Ok(Self::InternalIntrGetKernelTable)
            }
            bindings::NV2080_CTRL_CMD_GPU_GET_FERMI_GPC_INFO => Ok(Self::GpuGetFermiGpcInfo),
            bindings::NV2080_CTRL_CMD_GPU_GET_FERMI_TPC_INFO => Ok(Self::GpuGetFermiTpcInfo),
            bindings::NV2080_CTRL_CMD_INTERNAL_STATIC_KGR_GET_INFO => {
                Ok(Self::InternalStaticKgrGetInfo)
            }
            bindings::NV2080_CTRL_CMD_GPU_GET_CONSTRUCTED_FALCON_INFO => {
                Ok(Self::GpuGetConstructedFalconInfo)
            }
            _ => Err(EINVAL),
        }
    }
}

impl From<RmControlMsgFunction> for u32 {
    fn from(value: RmControlMsgFunction) -> Self {
        // CAST: `RmControlMsgFunction` is `repr(u32)` and can thus be cast losslessly.
        value as u32
    }
}

/// RM control message element structure.
#[repr(transparent)]
pub(crate) struct GspRmControl {
    inner: bindings::rpc_gsp_rm_control_v03_00,
}

impl GspRmControl {
    /// Creates a new RM control command.
    pub(crate) fn new<T>(
        client: Handle<Client>,
        object: Handle<T>,
        cmd: RmControlMsgFunction,
        params_size: u32,
    ) -> Self {
        Self {
            inner: bindings::rpc_gsp_rm_control_v03_00 {
                hClient: client.as_raw(),
                hObject: object.as_raw(),
                cmd: u32::from(cmd),
                status: 0,
                paramsSize: params_size,
                flags: 0,
                params: Default::default(),
            },
        }
    }

    /// Returns the status from the RM control response.
    ///
    /// This is distinct from the RPC transport status in the RPC message header.
    pub(crate) fn status(&self) -> Result<GspMsgRmStatus> {
        self.inner.status.try_into()
    }
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for GspRmControl {}

// SAFETY: This struct contains no padding.
unsafe impl AsBytes for GspRmControl {}

/// Class of an RM object, given when allocating it with [`MsgFunction::GspRmAlloc`].
///
/// [`MsgFunction::GspRmAlloc`]: super::MsgFunction::GspRmAlloc
#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(u32)]
pub(crate) enum RmObjectClass {
    /// A client, the root of an RM object tree.
    Root = bindings::NV01_ROOT,
    /// A device, child of a client.
    Device = bindings::NV01_DEVICE_0,
    /// A subdevice, child of a device.
    Subdevice = bindings::NV20_SUBDEVICE_0,
    /// The display engine's common object, child of a device.
    DisplayCommon = bindings::NV04_DISPLAY_COMMON,
    /// The display object of GA10x display engines, child of a device and parent of the display
    /// channels.
    DisplayGa102 = bindings::NVC670_DISPLAY,
    /// The core channel of GA10x display engines, child of the display object.
    CoreChannelGa102 = bindings::NVC67D_CORE_CHANNEL_DMA,
    /// A window channel of GA10x display engines, child of the display object.
    WindowChannelGa102 = bindings::NVC67E_WINDOW_CHANNEL_DMA,
    /// A GPU virtual address space, child of a device.
    VaSpace = bindings::FERMI_VASPACE_A,
    /// A channel group (TSG), child of a device: channels that share a context of their engine,
    /// and are scheduled together.
    ChannelGroupKepler = bindings::KEPLER_CHANNEL_GROUP_A,
    /// A context share, child of a channel group: a subcontext of the group's context, with an
    /// address space of its own.
    ContextShareFermi = bindings::FERMI_CONTEXT_SHARE_A,
    /// A GPFIFO channel of GA10x GPUs, child of a device or of a channel group.
    ChannelGpfifoAmpere = bindings::AMPERE_CHANNEL_GPFIFO_A,
    /// A copy engine object of GA10x GPUs, child of a channel.
    CopyEngineAmpereB = bindings::AMPERE_DMA_COPY_B,
    /// A video decoder object of GA10x GPUs, child of a channel on a video decoder engine
    /// (NVDEC).
    VideoDecoderAmpere = bindings::NVC7B0_VIDEO_DECODER,
    /// A 3D object of GA10x GPUs (but GA100), child of a channel on the graphics engine.
    ThreeDAmpereB = bindings::AMPERE_B,
    /// A compute object of GA10x GPUs (but GA100), child of a channel on the graphics engine.
    ComputeAmpereB = bindings::AMPERE_COMPUTE_B,
    /// A 2D object, child of a channel on the graphics engine.
    TwoDFermiA = bindings::FERMI_TWOD_A,
    /// An inline-to-memory object, which writes data from its push buffer to memory, child of a
    /// channel on the graphics engine.
    InlineToMemoryKeplerB = bindings::KEPLER_INLINE_TO_MEMORY_B,
}

impl From<RmObjectClass> for u32 {
    fn from(value: RmObjectClass) -> Self {
        // CAST: `RmObjectClass` is `repr(u32)` and can thus be cast losslessly.
        value as u32
    }
}

/// RM alloc message element structure.
#[repr(transparent)]
pub(crate) struct GspRmAlloc {
    inner: bindings::rpc_gsp_rm_alloc_v03_00,
}

impl GspRmAlloc {
    /// Creates a new RM alloc command, allocating `object` of class `class` under `parent`.
    pub(crate) fn new<P, O>(
        client: Handle<Client>,
        parent: Handle<P>,
        object: Handle<O>,
        class: RmObjectClass,
        params_size: u32,
    ) -> Self {
        Self {
            inner: bindings::rpc_gsp_rm_alloc_v03_00 {
                hClient: client.as_raw(),
                hParent: parent.as_raw(),
                hObject: object.as_raw(),
                hClass: u32::from(class),
                status: 0,
                paramsSize: params_size,
                flags: 0,
                reserved: [0; 4],
                params: Default::default(),
            },
        }
    }

    /// Returns the status from the RM alloc response.
    ///
    /// This is distinct from the RPC transport status in the RPC message header.
    pub(crate) fn status(&self) -> Result<GspMsgRmStatus> {
        self.inner.status.try_into()
    }
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for GspRmAlloc {}

// SAFETY: This struct contains no padding.
unsafe impl AsBytes for GspRmAlloc {}

/// RM free message element structure.
#[repr(transparent)]
pub(crate) struct GspRmFree {
    inner: bindings::rpc_free_v03_00,
}

impl GspRmFree {
    /// Creates a new RM free command, freeing `object` and all of its children.
    pub(crate) fn new<T>(client: Handle<Client>, object: Handle<T>) -> Self {
        Self {
            inner: bindings::rpc_free_v03_00 {
                params: bindings::NVOS00_PARAMETERS_v03_00 {
                    hRoot: client.as_raw(),
                    hObjectParent: 0,
                    hObjectOld: object.as_raw(),
                    status: 0,
                },
            },
        }
    }

    /// Returns the status from the RM free response.
    pub(crate) fn status(&self) -> Result<GspMsgRmStatus> {
        self.inner.params.status.try_into()
    }
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for GspRmFree {}

// SAFETY: This struct contains no padding.
unsafe impl AsBytes for GspRmFree {}

/// Wrapper for [`bindings::NV0000_ALLOC_PARAMETERS`], the parameters to allocate a client.
#[repr(transparent)]
pub(crate) struct RootAllocParams(bindings::NV0000_ALLOC_PARAMETERS);

impl RootAllocParams {
    /// Creates the parameters to allocate the client `client`, owned by the kernel rather than by
    /// a process.
    pub(crate) fn new(client: Handle<Client>) -> Self {
        Self(bindings::NV0000_ALLOC_PARAMETERS {
            hClient: client.as_raw(),
            processID: u32::MAX,
            ..Default::default()
        })
    }
}

// SAFETY: Padding is explicit and will not contain uninitialized data, and the only non-integer
// field, `pOsPidInfo`, is always null.
unsafe impl AsBytes for RootAllocParams {}

/// Wrapper for [`bindings::NV0080_ALLOC_PARAMETERS`], the parameters to allocate a device.
#[repr(transparent)]
pub(crate) struct DeviceAllocParams(bindings::NV0080_ALLOC_PARAMETERS);

impl DeviceAllocParams {
    /// Creates the parameters to allocate device 0, sharing the resources of `client`.
    pub(crate) fn new(client: Handle<Client>) -> Self {
        Self(bindings::NV0080_ALLOC_PARAMETERS {
            hClientShare: client.as_raw(),
            ..Default::default()
        })
    }
}

// SAFETY: Padding is explicit and will not contain uninitialized data.
unsafe impl AsBytes for DeviceAllocParams {}

/// Wrapper for [`bindings::NV2080_ALLOC_PARAMETERS`], the parameters to allocate a subdevice.
#[repr(transparent)]
pub(crate) struct SubdeviceAllocParams(bindings::NV2080_ALLOC_PARAMETERS);

impl SubdeviceAllocParams {
    /// Creates the parameters to allocate subdevice 0.
    pub(crate) fn new() -> Self {
        Self(bindings::NV2080_ALLOC_PARAMETERS { subDeviceId: 0 })
    }
}

// SAFETY: This struct contains no padding.
unsafe impl AsBytes for SubdeviceAllocParams {}

/// Wrapper for [`bindings::NV0073_CTRL_SYSTEM_GET_NUM_HEADS_PARAMS`].
#[repr(transparent)]
pub(crate) struct DispGetNumHeadsParams(bindings::NV0073_CTRL_SYSTEM_GET_NUM_HEADS_PARAMS);

impl DispGetNumHeadsParams {
    /// Creates the parameters to query the number of heads of subdevice 0.
    pub(crate) fn new() -> Self {
        Self(Default::default())
    }

    /// Returns the number of heads.
    pub(crate) fn num_heads(&self) -> u32 {
        self.0.numHeads
    }
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for DispGetNumHeadsParams {}

// SAFETY: This struct contains no padding.
unsafe impl AsBytes for DispGetNumHeadsParams {}

/// Wrapper for [`bindings::NV0073_CTRL_SYSTEM_GET_ACTIVE_PARAMS`].
#[repr(transparent)]
pub(crate) struct DispGetActiveParams(bindings::NV0073_CTRL_SYSTEM_GET_ACTIVE_PARAMS);

impl DispGetActiveParams {
    /// Creates the parameters to query the display active on `head` of subdevice 0, whoever made
    /// it active.
    pub(crate) fn new(head: u32) -> Self {
        Self(bindings::NV0073_CTRL_SYSTEM_GET_ACTIVE_PARAMS {
            head,
            ..Default::default()
        })
    }

    /// Returns the display ID (a mask with a single bit set) of the active display, or 0 if no
    /// display is active on the head.
    pub(crate) fn display_id(&self) -> u32 {
        self.0.displayId
    }
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for DispGetActiveParams {}

// SAFETY: This struct contains no padding.
unsafe impl AsBytes for DispGetActiveParams {}

/// Type of a display connector, as reported by [`DispGetConnectorDataParams`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ConnectorType {
    /// VGA.
    Vga,
    /// DVI-I.
    DviI,
    /// DVI-D.
    DviD,
    /// DisplayPort, including mini DisplayPort.
    DisplayPort,
    /// Embedded DisplayPort.
    EmbeddedDisplayPort,
    /// HDMI type A.
    HdmiA,
    /// HDMI type C (mini HDMI).
    HdmiC,
    /// USB type C.
    UsbC,
    /// DSI.
    Dsi,
    /// Any other connector, or one the firmware has no data for.
    Unknown,
}

impl From<u32> for ConnectorType {
    fn from(value: u32) -> Self {
        match value {
            bindings::NV0073_CTRL_SPECIFIC_CONNECTOR_DATA_TYPE_VGA_15_PIN => Self::Vga,
            bindings::NV0073_CTRL_SPECIFIC_CONNECTOR_DATA_TYPE_DVI_I
            | bindings::NV0073_CTRL_SPECIFIC_CONNECTOR_DATA_TYPE_LFH_DVI_I_1
            | bindings::NV0073_CTRL_SPECIFIC_CONNECTOR_DATA_TYPE_LFH_DVI_I_2 => Self::DviI,
            bindings::NV0073_CTRL_SPECIFIC_CONNECTOR_DATA_TYPE_DVI_D => Self::DviD,
            bindings::NV0073_CTRL_SPECIFIC_CONNECTOR_DATA_TYPE_DP_EXT
            | bindings::NV0073_CTRL_SPECIFIC_CONNECTOR_DATA_TYPE_DP_MINI_EXT
            | bindings::NV0073_CTRL_SPECIFIC_CONNECTOR_DATA_TYPE_LFH_DP_1
            | bindings::NV0073_CTRL_SPECIFIC_CONNECTOR_DATA_TYPE_LFH_DP_2 => Self::DisplayPort,
            bindings::NV0073_CTRL_SPECIFIC_CONNECTOR_DATA_TYPE_DP_INT => Self::EmbeddedDisplayPort,
            bindings::NV0073_CTRL_SPECIFIC_CONNECTOR_DATA_TYPE_HDMI_A => Self::HdmiA,
            bindings::NV0073_CTRL_SPECIFIC_CONNECTOR_DATA_TYPE_HDMI_C_MINI => Self::HdmiC,
            bindings::NV0073_CTRL_SPECIFIC_CONNECTOR_DATA_TYPE_USB_C
            | bindings::NV0073_CTRL_SPECIFIC_CONNECTOR_DATA_TYPE_USB_C_UHBR => Self::UsbC,
            bindings::NV0073_CTRL_SPECIFIC_CONNECTOR_DATA_TYPE_DSI => Self::Dsi,
            _ => Self::Unknown,
        }
    }
}

/// Wrapper for [`bindings::NV0073_CTRL_SPECIFIC_GET_CONNECTOR_DATA_PARAMS`].
#[repr(transparent)]
pub(crate) struct DispGetConnectorDataParams(
    bindings::NV0073_CTRL_SPECIFIC_GET_CONNECTOR_DATA_PARAMS,
);

impl DispGetConnectorDataParams {
    /// Creates the parameters to query the connector data of `display_id` on subdevice 0.
    pub(crate) fn new(display_id: u32) -> Self {
        Self(bindings::NV0073_CTRL_SPECIFIC_GET_CONNECTOR_DATA_PARAMS {
            displayId: display_id,
            ..Default::default()
        })
    }

    /// Returns the type of the first connector of the display.
    ///
    /// A display is usually wired to a single connector; a DVI-I port, for instance, shows up as
    /// two displays (analog and digital) sharing one connector.
    pub(crate) fn connector_type(&self) -> ConnectorType {
        match self.0.data.get(..num::u32_as_usize(self.0.count)) {
            Some([first, ..]) => ConnectorType::from(first.type_),
            _ => ConnectorType::Unknown,
        }
    }
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for DispGetConnectorDataParams {}

// SAFETY: This struct contains no padding.
unsafe impl AsBytes for DispGetConnectorDataParams {}

/// Configuration of a DisplayPort link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DpLinkConfig {
    /// Number of lanes.
    pub lanes: u32,
    /// Rate of each lane, in units of 270 Mb/s (`NV0073_CTRL_CMD_DP_GET_LINK_CONFIG_LINK_BW_*`,
    /// like the DPCD `LINK_BW_SET` register): 0x0a for HBR, 0x14 for HBR2, 0x1e for HBR3.
    pub link_bw: u32,
}

/// Wrapper for [`bindings::NV0073_CTRL_DP_GET_LINK_CONFIG_PARAMS`].
#[repr(transparent)]
pub(crate) struct DpGetLinkConfigParams(bindings::NV0073_CTRL_DP_GET_LINK_CONFIG_PARAMS);

impl DpGetLinkConfigParams {
    /// Creates the parameters to query the link configuration of `display_id` on subdevice 0.
    pub(crate) fn new(display_id: u32) -> Self {
        Self(bindings::NV0073_CTRL_DP_GET_LINK_CONFIG_PARAMS {
            displayId: display_id,
            ..Default::default()
        })
    }

    /// Returns the link configuration.
    pub(crate) fn config(&self) -> DpLinkConfig {
        DpLinkConfig {
            // `NV0073_CTRL_CMD_DP_GET_LINK_CONFIG_LANE_COUNT` is bits 3:0. `_LINK_BW` is said to
            // be bits 3:0 too, but its values (`_LINK_BW_8_10GBPS` is 0x1e) need more: the whole
            // value is taken.
            lanes: self.0.laneCount & 0xf,
            link_bw: self.0.linkBW,
        }
    }
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for DpGetLinkConfigParams {}

// SAFETY: This struct contains no padding.
unsafe impl AsBytes for DpGetLinkConfigParams {}

/// Returns the link bandwidth code, as in [`DpLinkConfig::link_bw`], of a highest link rate as
/// RM reports it in `NV0073_CTRL_CMD_DP_GET_CAPS_MAX_LINK_RATE` and
/// `NV0073_CTRL_DFP_FLAGS_DP_LINK_BW`, whose values are the same, or `None` for no rate.
fn link_bw_of_max_rate(rate: u32) -> Option<u32> {
    match rate {
        bindings::NV0073_CTRL_CMD_DP_GET_CAPS_MAX_LINK_RATE_1_62 => Some(0x06),
        bindings::NV0073_CTRL_CMD_DP_GET_CAPS_MAX_LINK_RATE_2_70 => Some(0x0a),
        bindings::NV0073_CTRL_CMD_DP_GET_CAPS_MAX_LINK_RATE_5_40 => Some(0x14),
        bindings::NV0073_CTRL_CMD_DP_GET_CAPS_MAX_LINK_RATE_8_10 => Some(0x1e),
        _ => None,
    }
}

/// Wrapper for [`bindings::NV0073_CTRL_DFP_GET_INFO_PARAMS`].
#[repr(transparent)]
pub(crate) struct DfpGetInfoParams(bindings::NV0073_CTRL_DFP_GET_INFO_PARAMS);

impl DfpGetInfoParams {
    /// Creates the parameters to query the configuration of the digital display `display_id` on
    /// subdevice 0.
    pub(crate) fn new(display_id: u32) -> Self {
        Self(bindings::NV0073_CTRL_DFP_GET_INFO_PARAMS {
            displayId: display_id,
            ..Default::default()
        })
    }

    /// Returns the highest link bandwidth code the board allows the display's DisplayPort link,
    /// or `None` if it doesn't say.
    pub(crate) fn max_link_bw(&self) -> Option<u32> {
        // `NV0073_CTRL_DFP_FLAGS_DP_LINK_BW` is bits 19:17.
        link_bw_of_max_rate(self.0.flags >> 17 & 0x7)
    }
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for DfpGetInfoParams {}

// SAFETY: This struct contains no padding.
unsafe impl AsBytes for DfpGetInfoParams {}

/// What the DisplayPort outputs of the GPU can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DpGpuCaps {
    /// Highest link bandwidth code, as in [`DpLinkConfig::link_bw`].
    pub(crate) max_link_bw: u32,
    /// Whether the watermarks of the outputs have the higher limits of newer GPUs.
    pub(crate) increased_watermark_limits: bool,
    /// Whether the outputs may spread the spectrum of their links.
    pub(crate) downspread: bool,
}

/// Wrapper for [`bindings::NV0073_CTRL_CMD_DP_GET_CAPS_PARAMS`].
#[repr(transparent)]
pub(crate) struct DpGetCapsParams(bindings::NV0073_CTRL_CMD_DP_GET_CAPS_PARAMS);

impl DpGetCapsParams {
    /// Creates the parameters to query what the DisplayPort outputs of subdevice 0 can do.
    pub(crate) fn new() -> Self {
        Self(bindings::NV0073_CTRL_CMD_DP_GET_CAPS_PARAMS {
            // Any output, as nouveau asks.
            sorIndex: !0,
            ..Default::default()
        })
    }

    /// Returns what the outputs can do, or `None` if RM reports no link rate.
    pub(crate) fn caps(&self) -> Option<DpGpuCaps> {
        Some(DpGpuCaps {
            // `NV0073_CTRL_CMD_DP_GET_CAPS_MAX_LINK_RATE` is bits 2:0.
            max_link_bw: link_bw_of_max_rate(self.0.maxLinkRate & 0x7)?,
            increased_watermark_limits: self.0.bHasIncreasedWatermarkLimits != 0,
            downspread: self.0.bSupportDPDownSpread != 0,
        })
    }
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for DpGetCapsParams {}

// SAFETY: The padding of this struct is explicit, as fields.
unsafe impl AsBytes for DpGetCapsParams {}

/// Highest number of bytes of an AUX transfer.
pub(crate) const DP_AUX_MAX_LEN: usize = bindings::NV0073_CTRL_DP_AUXCH_MAX_DATA_SIZE as usize;

/// Outcome of an AUX transfer, as RM reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DpAuxReply {
    /// The sink, or for I2C requests the I2C device, acknowledged the request.
    Ack,
    /// The sink refused the request.
    Nack,
    /// The sink wasn't ready: the request has to be retried.
    Defer,
    /// The I2C device refused the request.
    I2cNack,
    /// The I2C device wasn't ready: the request has to be retried.
    I2cDefer,
    /// The sink didn't reply.
    Timeout,
    /// The request was invalid.
    Invalid,
}

/// Wrapper for [`bindings::NV0073_CTRL_DP_AUXCH_CTRL_PARAMS`].
#[derive(Clone, Copy)]
#[repr(transparent)]
pub(crate) struct DpAuxParams(bindings::NV0073_CTRL_DP_AUXCH_CTRL_PARAMS);

impl DpAuxParams {
    /// Creates the parameters of an AUX transfer of `display_id` on subdevice 0: the request
    /// `request` at `addr`, writing `data` for write requests, or reading `len` bytes for read
    /// requests. A transfer of no bytes is an address-only transfer.
    ///
    /// `request` is coded as the DisplayPort specification does, which RM's
    /// `NV0073_CTRL_DP_AUXCH_CMD` follows: native (bit 3) or I2C, I2C middle-of-transaction (bit
    /// 2), and write, read or I2C write status update (bits 1:0).
    ///
    /// Fails with `EINVAL` if the transfer is longer than [`DP_AUX_MAX_LEN`].
    pub(crate) fn new(
        display_id: u32,
        request: u8,
        addr: u32,
        data: &[u8],
        len: usize,
    ) -> Result<Self> {
        if len > DP_AUX_MAX_LEN || data.len() > len {
            return Err(EINVAL);
        }

        let mut params = bindings::NV0073_CTRL_DP_AUXCH_CTRL_PARAMS {
            displayId: display_id,
            bAddrOnly: u8::from(len == 0),
            cmd: u32::from(request & 0xf),
            addr,
            // The length is sent less one, but for address-only transfers.
            size: u32::try_from(len.saturating_sub(1))?,
            ..Default::default()
        };
        if len == 0 {
            // Address-only transfers are sent as writes, without middle-of-transaction for I2C, as
            // nouveau does (`NV0073_CTRL_DP_AUXCH_CMD_REQ_TYPE_WRITE`, `_I2C_MOT_FALSE`).
            params.cmd &= !0b111;
        }
        params.data[..data.len()].copy_from_slice(data);

        Ok(Self(params))
    }

    /// Returns the outcome of the transfer.
    pub(crate) fn reply(&self) -> DpAuxReply {
        match self.0.replyType {
            bindings::NV0073_CTRL_DP_AUXCH_REPLYTYPE_ACK => DpAuxReply::Ack,
            bindings::NV0073_CTRL_DP_AUXCH_REPLYTYPE_NACK => DpAuxReply::Nack,
            bindings::NV0073_CTRL_DP_AUXCH_REPLYTYPE_DEFER => DpAuxReply::Defer,
            bindings::NV0073_CTRL_DP_AUXCH_REPLYTYPE_I2CNACK => DpAuxReply::I2cNack,
            bindings::NV0073_CTRL_DP_AUXCH_REPLYTYPE_I2CDEFER => DpAuxReply::I2cDefer,
            bindings::NV0073_CTRL_DP_AUXCH_REPLYTYPE_TIMEOUT => DpAuxReply::Timeout,
            _ => DpAuxReply::Invalid,
        }
    }

    /// Returns the bytes transferred, read or written: maybe fewer than requested.
    pub(crate) fn data(&self) -> &[u8] {
        let len = num::u32_as_usize(self.0.size).min(DP_AUX_MAX_LEN);

        &self.0.data[..len]
    }

    /// Returns how long to wait before retrying a transfer that failed, if RM asks for it.
    pub(crate) fn retry_ms(&self) -> Option<u32> {
        Some(self.0.retryTimeMs).filter(|&ms| ms != 0)
    }
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for DpAuxParams {}

// SAFETY: The padding of this struct is explicit, as fields.
unsafe impl AsBytes for DpAuxParams {}

/// A training of a DisplayPort link, by RM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DpTraining {
    /// The configuration to train the link to.
    pub link: DpLinkConfig,
    /// What is trained: 0 for the sink, `n` for LTTPR `n`, in non-transparent mode, as numbered
    /// in the DPCD from 1 for the one closest to the sink (`NV0073_CTRL_DP_DATA_TARGET`). A link
    /// is trained from the LTTPR closest to the source to the sink.
    pub target: u32,
    /// Whether the link uses enhanced framing, which the sink must support.
    pub enhanced_framing: bool,
    /// Whether the sink may ask for its drive settings to be adjusted after the training.
    pub post_lt_adj_req: bool,
    /// Whether the link has LTTPRs in non-transparent mode, which are trained one by one before
    /// the sink.
    pub lttprs: bool,
}

/// Wrapper for [`bindings::NV0073_CTRL_DP_CTRL_PARAMS`].
#[derive(Clone, Copy)]
#[repr(transparent)]
pub(crate) struct DpCtrlParams(bindings::NV0073_CTRL_DP_CTRL_PARAMS);

impl DpCtrlParams {
    /// Creates the parameters to have RM train the link of `display_id` on subdevice 0 as
    /// `training` says, spreading its spectrum or not as RM's defaults are if `downspread`, or
    /// without spreading it.
    pub(crate) fn train(display_id: u32, training: &DpTraining, downspread: bool) -> Self {
        // Bits of `NV0073_CTRL_DP_CMD`.
        const SET_LANE_COUNT: u32 = 0;
        const SET_LINK_BW: u32 = 1;
        const DISABLE_DOWNSPREAD: u32 = 2;
        const SET_ENHANCED_FRAMING: u32 = 7;
        const USE_DOWNSPREAD_SETTING: u32 = 8;
        const POST_LT_ADJ_REQ_GRANTED: u32 = 10;
        const TRAIN_PHY_REPEATER: u32 = 13;
        // Fields of `NV0073_CTRL_DP_DATA`.
        const DATA_LANE_COUNT_SHIFT: u32 = 0;
        const DATA_LINK_BW_SHIFT: u32 = 8;
        const DATA_ENHANCED_FRAMING: u32 = 18;
        const DATA_TARGET_SHIFT: u32 = 19;

        let flag = |value: bool, bit: u32, set: u32| if value { set << bit } else { 0 };

        let cmd = bindings::NV0073_CTRL_DP_CMD_SET_LANE_COUNT_TRUE << SET_LANE_COUNT
            | bindings::NV0073_CTRL_DP_CMD_SET_LINK_BW_TRUE << SET_LINK_BW
            | flag(
                training.enhanced_framing,
                SET_ENHANCED_FRAMING,
                bindings::NV0073_CTRL_DP_CMD_SET_ENHANCED_FRAMING_TRUE,
            )
            | flag(
                training.post_lt_adj_req,
                POST_LT_ADJ_REQ_GRANTED,
                bindings::NV0073_CTRL_DP_CMD_POST_LT_ADJ_REQ_GRANTED_YES,
            )
            // Each training of a link with non-transparent LTTPRs, the sink's too, says so, like
            // NVIDIA's driver does.
            | flag(
                training.lttprs,
                TRAIN_PHY_REPEATER,
                bindings::NV0073_CTRL_DP_CMD_TRAIN_PHY_REPEATER_YES,
            )
            // Outputs that can't spread the spectrum are told not to, like NVIDIA's driver does.
            | flag(
                !downspread,
                USE_DOWNSPREAD_SETTING,
                bindings::NV0073_CTRL_DP_CMD_USE_DOWNSPREAD_SETTING_FORCE,
            )
            | flag(
                !downspread,
                DISABLE_DOWNSPREAD,
                bindings::NV0073_CTRL_DP_CMD_DISABLE_DOWNSPREAD_TRUE,
            );
        let data = (training.link.lanes & 0x1f) << DATA_LANE_COUNT_SHIFT
            | (training.link.link_bw & 0xff) << DATA_LINK_BW_SHIFT
            | flag(
                training.enhanced_framing,
                DATA_ENHANCED_FRAMING,
                bindings::NV0073_CTRL_DP_DATA_SET_ENHANCED_FRAMING_YES,
            )
            | (training.target & 0xf) << DATA_TARGET_SHIFT;

        Self(bindings::NV0073_CTRL_DP_CTRL_PARAMS {
            displayId: display_id,
            cmd,
            data,
            ..Default::default()
        })
    }

    /// Returns the errors of the training, `NV0073_CTRL_DP_ERR`: 0 if it succeeded.
    pub(crate) fn err(&self) -> u32 {
        self.0.err
    }

    /// Returns how long to wait before retrying a training that failed, if RM asks for it.
    pub(crate) fn retry_ms(&self) -> Option<u32> {
        Some(self.0.retryTimeMs).filter(|&ms| ms != 0)
    }
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for DpCtrlParams {}

// SAFETY: This struct contains no padding.
unsafe impl AsBytes for DpCtrlParams {}

/// Wrapper for [`bindings::NV0073_CTRL_SPECIFIC_GET_EDID_V2_PARAMS`].
#[repr(transparent)]
pub(crate) struct DispGetEdidParams(bindings::NV0073_CTRL_SPECIFIC_GET_EDID_V2_PARAMS);

impl DispGetEdidParams {
    /// Creates the parameters to read the EDID of `display_id` on subdevice 0.
    ///
    /// GSP-RM reads the EDID from the display over DDC, or returns the copy it cached if `cached`
    /// is true. Either way it applies its usual EDID fix-ups.
    pub(crate) fn new(display_id: u32, cached: bool) -> Self {
        let mut params = bindings::NV0073_CTRL_SPECIFIC_GET_EDID_V2_PARAMS {
            displayId: display_id,
            flags: if cached {
                bindings::NV0073_CTRL_SPECIFIC_GET_EDID_FLAGS_COPY_CACHE_YES
            } else {
                bindings::NV0073_CTRL_SPECIFIC_GET_EDID_FLAGS_COPY_CACHE_NO
            },
            ..Default::default()
        };
        // CAST: the buffer is 2048 bytes long.
        params.bufferSize = params.edidBuffer.len() as u32;

        Self(params)
    }

    /// Returns the EDID, or `None` if GSP-RM reported a size larger than the buffer.
    pub(crate) fn edid(&self) -> Option<&[u8]> {
        self.0
            .edidBuffer
            .get(..num::u32_as_usize(self.0.bufferSize))
    }
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for DispGetEdidParams {}

// SAFETY: This struct contains no padding.
unsafe impl AsBytes for DispGetEdidParams {}

// The following values come from internal RM headers, which aren't part of NVIDIA's SDK headers
// the bindings are generated from.

/// System memory address space (`ADDR_SYSMEM` in RM's `mem_desc.h`).
const ADDR_SYSMEM: u32 = 1;
/// VRAM address space (`ADDR_FBMEM` in RM's `mem_desc.h`).
const ADDR_FBMEM: u32 = 2;
/// Push buffer in cache-coherent system memory (`PHYS_PCI_COHERENT` of `PBTARGETAPERTURE` in RM's
/// `kern_disp.h`).
const PB_TARGET_APERTURE_PHYS_PCI_COHERENT: u32 = 3;

/// Wrapper for [`bindings::NV2080_CTRL_INTERNAL_DISPLAY_GET_STATIC_INFO_PARAMS`].
#[repr(transparent)]
pub(crate) struct DispGetStaticInfoParams(
    bindings::NV2080_CTRL_INTERNAL_DISPLAY_GET_STATIC_INFO_PARAMS,
);

impl DispGetStaticInfoParams {
    /// Creates the parameters to query the static configuration of the display engine.
    pub(crate) fn new() -> Self {
        Self(Default::default())
    }

    /// Returns the mask of the windows the display engine has.
    pub(crate) fn window_present_mask(&self) -> u32 {
        self.0.windowPresentMask
    }
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for DispGetStaticInfoParams {}

// SAFETY: Padding is explicit and will not contain uninitialized data.
unsafe impl AsBytes for DispGetStaticInfoParams {}

/// Wrapper for [`bindings::NV2080_CTRL_INTERNAL_DISPLAY_WRITE_INST_MEM_PARAMS`].
#[repr(transparent)]
pub(crate) struct DispWriteInstMemParams(
    bindings::NV2080_CTRL_INTERNAL_DISPLAY_WRITE_INST_MEM_PARAMS,
);

impl DispWriteInstMemParams {
    /// Creates the parameters to make the `size` bytes of VRAM at `addr` the instance memory of
    /// the display engine, which the CPU writes write-combined.
    pub(crate) fn new(addr: u64, size: u64) -> Self {
        Self(
            bindings::NV2080_CTRL_INTERNAL_DISPLAY_WRITE_INST_MEM_PARAMS {
                instMemPhysAddr: addr,
                instMemSize: size,
                instMemAddrSpace: ADDR_FBMEM,
                instMemCpuCacheAttr: bindings::NV_MEMORY_WRITECOMBINED,
            },
        )
    }
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for DispWriteInstMemParams {}

// SAFETY: This struct contains no padding.
unsafe impl AsBytes for DispWriteInstMemParams {}

/// Wrapper for [`bindings::NV2080_CTRL_INTERNAL_DISPLAY_CHANNEL_PUSHBUFFER_PARAMS`].
#[repr(transparent)]
pub(crate) struct DispChannelPushbufferParams(
    bindings::NV2080_CTRL_INTERNAL_DISPLAY_CHANNEL_PUSHBUFFER_PARAMS,
);

impl DispChannelPushbufferParams {
    /// Creates the parameters to give instance `instance` of display channel class `class` a
    /// 4 KiB push buffer at DMA address `dma_addr`, in cache-coherent system memory.
    pub(crate) fn new(class: RmObjectClass, instance: u32, dma_addr: u64) -> Self {
        Self(
            bindings::NV2080_CTRL_INTERNAL_DISPLAY_CHANNEL_PUSHBUFFER_PARAMS {
                addressSpace: ADDR_SYSMEM,
                physicalAddr: dma_addr,
                limit: DISP_PUSHBUFFER_SIZE - 1,
                cacheSnoop: 1,
                hclass: u32::from(class),
                channelInstance: instance,
                valid: 1,
                pbTargetAperture: PB_TARGET_APERTURE_PHYS_PCI_COHERENT,
                channelPBSize: bindings::ChannelPBSize_PB_SIZE_4KB,
                // The channel addresses subdevice 0 in its push buffer.
                subDeviceId: 1 << 0,
                ..Default::default()
            },
        )
    }
}

/// Size of the push buffer of a display channel, in bytes.
pub(crate) const DISP_PUSHBUFFER_SIZE: u64 = 4096;

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for DispChannelPushbufferParams {}

// SAFETY: Padding is explicit and will not contain uninitialized data.
unsafe impl AsBytes for DispChannelPushbufferParams {}

/// Wrapper for [`bindings::NV50VAIO_CHANNELDMA_ALLOCATION_PARAMETERS`], the parameters to allocate
/// a display channel driven through a push buffer.
#[repr(transparent)]
pub(crate) struct DmaChannelAllocParams(bindings::NV50VAIO_CHANNELDMA_ALLOCATION_PARAMETERS);

impl DmaChannelAllocParams {
    /// Creates the parameters to allocate channel instance `instance`, whose push buffer was set
    /// with [`DispChannelPushbufferParams`] and starts at byte offset `put`.
    pub(crate) fn new(instance: u32, put: u32) -> Self {
        Self(bindings::NV50VAIO_CHANNELDMA_ALLOCATION_PARAMETERS {
            channelInstance: instance,
            offset: put,
            channelPBSize: bindings::ChannelPBSize_PB_SIZE_4KB,
            subDeviceId: 1 << 0,
            ..Default::default()
        })
    }
}

// SAFETY: Padding is explicit and will not contain uninitialized data, and the only non-integer
// field, `pControl`, is always null.
unsafe impl AsBytes for DmaChannelAllocParams {}

/// `NV_VASPACE_ALLOCATION_FLAGS_IS_EXTERNALLY_OWNED` in `nvos.h`, `BIT(3)`, which bindgen can't
/// translate: the client owns the page tables of the address space.
const VASPACE_FLAGS_IS_EXTERNALLY_OWNED: u32 = 1 << 3;

/// Wrapper for [`bindings::NV_VASPACE_ALLOCATION_PARAMETERS`].
#[repr(transparent)]
pub(crate) struct VaSpaceAllocParams(bindings::NV_VASPACE_ALLOCATION_PARAMETERS);

impl VaSpaceAllocParams {
    /// Creates the parameters to allocate a new GPU virtual address space whose page tables the
    /// client owns, and sets with [`SetPageDirectoryParams`].
    pub(crate) fn externally_owned() -> Self {
        Self(bindings::NV_VASPACE_ALLOCATION_PARAMETERS {
            index: bindings::NV_VASPACE_ALLOCATION_INDEX_GPU_NEW,
            flags: VASPACE_FLAGS_IS_EXTERNALLY_OWNED,
            ..Default::default()
        })
    }

    /// Creates the parameters to allocate a new GPU virtual address space split between the client
    /// and GSP-RM: GSP-RM maps its own buffers in [`RM_SERVER_VA`], whose page directories the
    /// client gives it with [`CopyServerReservedPdesParams`].
    pub(crate) fn split() -> Self {
        Self(bindings::NV_VASPACE_ALLOCATION_PARAMETERS {
            index: bindings::NV_VASPACE_ALLOCATION_INDEX_GPU_NEW,
            ..Default::default()
        })
    }
}

/// The part of a split address space GSP-RM maps its own buffers in: where it expects it
/// (`SPLIT_VAS_SERVER_RM_MANAGED_VA_START` and `_SIZE` in RM's `gpu_vaspace.h`).
pub(crate) const RM_SERVER_VA: core::ops::Range<u64> = 0x1_0000_0000..0x1_2000_0000;

// SAFETY: Padding is explicit and will not contain uninitialized data.
unsafe impl AsBytes for VaSpaceAllocParams {}

/// Wrapper for [`bindings::NV0080_CTRL_DMA_SET_PAGE_DIRECTORY_PARAMS`].
#[repr(transparent)]
pub(crate) struct SetPageDirectoryParams(bindings::NV0080_CTRL_DMA_SET_PAGE_DIRECTORY_PARAMS);

impl SetPageDirectoryParams {
    /// Creates the parameters to make the page directory of `num_entries` entries at `pdb` in
    /// VRAM the root of the page tables of the address space `va_space`.
    pub(crate) fn new(va_space: u32, pdb: u64, num_entries: u32) -> Self {
        Self(bindings::NV0080_CTRL_DMA_SET_PAGE_DIRECTORY_PARAMS {
            physAddress: pdb,
            numEntries: num_entries,
            flags: bindings::NV0080_CTRL_DMA_SET_PAGE_DIRECTORY_FLAGS_APERTURE_VIDMEM,
            hVASpace: va_space,
            ..Default::default()
        })
    }
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for SetPageDirectoryParams {}

// SAFETY: Padding is explicit and will not contain uninitialized data.
unsafe impl AsBytes for SetPageDirectoryParams {}

/// Wrapper for [`bindings::NV0080_CTRL_DMA_UNSET_PAGE_DIRECTORY_PARAMS`].
#[repr(transparent)]
pub(crate) struct UnsetPageDirectoryParams(bindings::NV0080_CTRL_DMA_UNSET_PAGE_DIRECTORY_PARAMS);

impl UnsetPageDirectoryParams {
    /// Creates the parameters to unset the page directory of the address space `va_space`.
    pub(crate) fn new(va_space: u32) -> Self {
        Self(bindings::NV0080_CTRL_DMA_UNSET_PAGE_DIRECTORY_PARAMS {
            hVASpace: va_space,
            ..Default::default()
        })
    }
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for UnsetPageDirectoryParams {}

// SAFETY: Padding is explicit and will not contain uninitialized data.
unsafe impl AsBytes for UnsetPageDirectoryParams {}

/// Wrapper for [`bindings::NVC0B5_ALLOCATION_PARAMETERS`].
#[repr(transparent)]
pub(crate) struct CopyEngineAllocParams(bindings::NVC0B5_ALLOCATION_PARAMETERS);

impl CopyEngineAllocParams {
    /// Creates the parameters to allocate an object of copy engine `index`.
    pub(crate) fn new(index: u32) -> Self {
        Self(bindings::NVC0B5_ALLOCATION_PARAMETERS {
            // The engine is given as an `NV2080_ENGINE_TYPE`.
            version: bindings::NVC0B5_ALLOCATION_PARAMETERS_VERSION_1,
            engineType: bindings::NV2080_ENGINE_TYPE_COPY0 + index,
        })
    }
}

// SAFETY: This struct only contains integer types, without padding.
unsafe impl AsBytes for CopyEngineAllocParams {}

/// Wrapper for [`bindings::NV_BSP_ALLOCATION_PARAMETERS`].
#[repr(transparent)]
pub(crate) struct VideoDecoderAllocParams(bindings::NV_BSP_ALLOCATION_PARAMETERS);

impl VideoDecoderAllocParams {
    /// Creates the parameters to allocate an object of video decoder engine `index`.
    pub(crate) fn new(index: u32) -> Self {
        Self(bindings::NV_BSP_ALLOCATION_PARAMETERS {
            size: num::usize_into_u32::<{ size_of::<bindings::NV_BSP_ALLOCATION_PARAMETERS>() }>(),
            prohibitMultipleInstances: 0,
            engineInstance: index,
        })
    }
}

// SAFETY: This struct only contains integer types, without padding.
unsafe impl AsBytes for VideoDecoderAllocParams {}

/// Parameters of `NV2080_CTRL_CMD_GPU_GET_CONSTRUCTED_FALCON_INFO`: the engines run by a falcon
/// that GSP-RM constructed, such as the video decoders, with the size of the context their
/// channels need.
#[repr(transparent)]
pub(crate) struct ConstructedFalconInfoParams(
    bindings::NV2080_CTRL_GPU_GET_CONSTRUCTED_FALCON_INFO_PARAMS,
);

impl ConstructedFalconInfoParams {
    /// Creates the parameters, all filled in by GSP-RM.
    pub(crate) fn new() -> Self {
        Self(Default::default())
    }

    /// Returns the size of the context of the channels on each engine.
    pub(crate) fn context_sizes(&self) -> Result<KVec<FalconContextSize>> {
        let count = usize::try_from(self.0.numConstructedFalcons).unwrap_or(0);

        let mut sizes = KVec::new();
        for falcon in self.0.constructedFalconsTable.iter().take(count) {
            sizes.push(
                FalconContextSize {
                    descriptor: falcon.engDesc,
                    size: falcon.ctxBufferSize,
                },
                GFP_KERNEL,
            )?;
        }

        Ok(sizes)
    }
}

/// The size of the context of the channels on an engine run by a falcon.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FalconContextSize {
    /// The engine, by its [`EngineInfo::descriptor`].
    pub(crate) descriptor: u32,
    /// Size in bytes.
    pub(crate) size: u32,
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for ConstructedFalconInfoParams {}

// SAFETY: This struct only contains integer types, without padding.
unsafe impl AsBytes for ConstructedFalconInfoParams {}

/// Wrapper for [`bindings::NVA06F_CTRL_BIND_PARAMS`].
#[repr(transparent)]
pub(crate) struct GpfifoBindParams(bindings::NVA06F_CTRL_BIND_PARAMS);

impl GpfifoBindParams {
    /// Creates the parameters to bind a channel to the engine of type `engine_type`, an
    /// `NV2080_ENGINE_TYPE`.
    pub(crate) fn new(engine_type: u32) -> Self {
        Self(bindings::NVA06F_CTRL_BIND_PARAMS {
            engineType: engine_type,
        })
    }
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for GpfifoBindParams {}

// SAFETY: This struct only contains integer types, without padding.
unsafe impl AsBytes for GpfifoBindParams {}

/// Wrapper for [`bindings::NVA06F_CTRL_GPFIFO_SCHEDULE_PARAMS`].
#[repr(transparent)]
pub(crate) struct GpfifoScheduleParams(bindings::NVA06F_CTRL_GPFIFO_SCHEDULE_PARAMS);

impl GpfifoScheduleParams {
    /// Creates the parameters to enable the scheduling of a channel.
    pub(crate) fn enable() -> Self {
        Self(bindings::NVA06F_CTRL_GPFIFO_SCHEDULE_PARAMS {
            bEnable: 1,
            ..Default::default()
        })
    }
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for GpfifoScheduleParams {}

// SAFETY: This struct only contains integer types, without padding.
unsafe impl AsBytes for GpfifoScheduleParams {}

/// Wrapper for [`bindings::NV2080_CTRL_CE_GET_FAULT_METHOD_BUFFER_SIZE_PARAMS`].
#[repr(transparent)]
pub(crate) struct CeGetFaultMethodBufferSizeParams(
    bindings::NV2080_CTRL_CE_GET_FAULT_METHOD_BUFFER_SIZE_PARAMS,
);

impl CeGetFaultMethodBufferSizeParams {
    /// Creates the parameters to query the size of the method buffer of channels.
    pub(crate) fn new() -> Self {
        Self(Default::default())
    }

    /// Returns the size in bytes of the buffer a channel saves pending methods into on faults.
    pub(crate) fn size(&self) -> u32 {
        self.0.size
    }
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for CeGetFaultMethodBufferSizeParams {}

// SAFETY: This struct only contains integer types, without padding.
unsafe impl AsBytes for CeGetFaultMethodBufferSizeParams {}

/// An engine of the GPU, as listed in the device info table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EngineInfo {
    /// The engine, as an `NV2080_ENGINE_TYPE`.
    pub(crate) engine_type: u32,
    /// The runlist the engine is on, which schedules the channels running on it.
    pub(crate) runlist: u32,
    /// The engine descriptor GSP-RM names the engine with, such as in
    /// [`ConstructedFalconInfoParams`].
    pub(crate) descriptor: u32,
}

impl EngineInfo {
    /// Returns whether the engine is the graphics engine.
    pub(crate) fn is_graphics(&self) -> bool {
        self.engine_type == bindings::NV2080_ENGINE_TYPE_GRAPHICS
    }

    /// Returns the index of the engine if it's a video decoder engine (NVDEC).
    pub(crate) fn video_decoder_index(&self) -> Option<u32> {
        (bindings::NV2080_ENGINE_TYPE_NVDEC0..=bindings::NV2080_ENGINE_TYPE_NVDEC7)
            .contains(&self.engine_type)
            .then(|| self.engine_type - bindings::NV2080_ENGINE_TYPE_NVDEC0)
    }

    /// Returns the index of the engine if it's one of the first 10 copy engines, whose engine
    /// types follow each other.
    pub(crate) fn copy_engine_index(&self) -> Option<u32> {
        (bindings::NV2080_ENGINE_TYPE_COPY0..=bindings::NV2080_ENGINE_TYPE_COPY9)
            .contains(&self.engine_type)
            .then(|| self.engine_type - bindings::NV2080_ENGINE_TYPE_COPY0)
    }
}

/// Wrapper for [`bindings::NV2080_CTRL_FIFO_GET_DEVICE_INFO_TABLE_PARAMS`].
#[repr(transparent)]
pub(crate) struct DeviceInfoTableParams(bindings::NV2080_CTRL_FIFO_GET_DEVICE_INFO_TABLE_PARAMS);

impl DeviceInfoTableParams {
    /// Number of entries of the table returned at once.
    pub(crate) const MAX_ENTRIES: u32 =
        bindings::NV2080_CTRL_FIFO_GET_DEVICE_INFO_TABLE_MAX_ENTRIES;

    /// Creates the parameters to query the entries of the table from `base`.
    pub(crate) fn new(base: u32) -> Self {
        Self(bindings::NV2080_CTRL_FIFO_GET_DEVICE_INFO_TABLE_PARAMS {
            baseIndex: base,
            ..Default::default()
        })
    }

    /// Returns the engine of type `rm_engine_type`, an `RM_ENGINE_TYPE` as the table names
    /// engines, as an `NV2080_ENGINE_TYPE`, as channels and their contexts name them. `None` for
    /// the engines nova-core doesn't run channels on.
    ///
    /// The two numberings only agree up to the copy engines (`RM_ENGINE_TYPE_NVDEC0` is 0x1d,
    /// `NV2080_ENGINE_TYPE_NVDEC0` 0x13), like nouveau translates them
    /// (`r535_fifo_xlat_rm_engine_type()`).
    fn nv2080_engine_type(rm_engine_type: u32) -> Option<u32> {
        const GRAPHICS: u32 = bindings::RM_ENGINE_TYPE_RM_ENGINE_TYPE_GR0;
        const COPY: RangeInclusive<u32> = bindings::RM_ENGINE_TYPE_RM_ENGINE_TYPE_COPY0
            ..=bindings::RM_ENGINE_TYPE_RM_ENGINE_TYPE_COPY9;
        const VIDEO_DECODER: RangeInclusive<u32> = bindings::RM_ENGINE_TYPE_RM_ENGINE_TYPE_NVDEC0
            ..=bindings::RM_ENGINE_TYPE_RM_ENGINE_TYPE_NVDEC7;

        match rm_engine_type {
            GRAPHICS => Some(bindings::NV2080_ENGINE_TYPE_GRAPHICS),
            t if COPY.contains(&t) => Some(bindings::NV2080_ENGINE_TYPE_COPY0 + t - COPY.start()),
            t if VIDEO_DECODER.contains(&t) => {
                Some(bindings::NV2080_ENGINE_TYPE_NVDEC0 + t - VIDEO_DECODER.start())
            }
            _ => None,
        }
    }

    /// Returns the engines of the entries returned that nova-core runs channels on.
    pub(crate) fn engines(&self) -> impl Iterator<Item = EngineInfo> + '_ {
        let count = usize::try_from(self.0.numEntries).unwrap_or(0);

        self.0.entries.iter().take(count).filter_map(|entry| {
            let data = |info: bindings::ENGINE_INFO_TYPE| {
                entry.engineData.get(usize::try_from(info).ok()?).copied()
            };

            Some(EngineInfo {
                engine_type: Self::nv2080_engine_type(data(
                    bindings::ENGINE_INFO_TYPE_ENGINE_INFO_TYPE_RM_ENGINE_TYPE,
                )?)?,
                runlist: data(bindings::ENGINE_INFO_TYPE_ENGINE_INFO_TYPE_RUNLIST)?,
                descriptor: data(bindings::ENGINE_INFO_TYPE_ENGINE_INFO_TYPE_ENG_DESC)?,
            })
        })
    }

    /// Returns whether more entries follow those returned.
    pub(crate) fn more(&self) -> bool {
        self.0.bMore != 0
    }
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for DeviceInfoTableParams {}

// SAFETY: Padding is explicit and will not contain uninitialized data.
unsafe impl AsBytes for DeviceInfoTableParams {}

/// `NV_INTR_VECTOR_INVALID`: the vector of a unit whose interrupts GSP-RM doesn't leave to the CPU.
const NV_INTR_VECTOR_INVALID: u32 = u32::MAX;

/// The interrupt vectors that GSP-RM leaves to the CPU of the units nova-core services.
#[derive(Debug)]
pub(crate) struct KernelIntrVectors {
    /// The display engine's stall vector, on which the CPU services its interrupts, such as
    /// those of head timing.
    pub(crate) display: Option<u32>,
    /// The notification (non-stall) vectors of the graphics, copy and video decoder engines,
    /// without duplicates, which channels on them raise with `NON_STALL_INTERRUPT`.
    pub(crate) notifications: KVec<u32>,
}

/// Parameters of `NV2080_CTRL_CMD_INTERNAL_INTR_GET_KERNEL_TABLE`: the interrupt vectors of the
/// units whose interrupts the CPU services, rather than GSP-RM.
#[repr(transparent)]
pub(crate) struct IntrKernelTableParams(
    bindings::NV2080_CTRL_INTERNAL_INTR_GET_KERNEL_TABLE_PARAMS,
);

impl IntrKernelTableParams {
    /// Creates the parameters, all filled in by GSP-RM.
    pub(crate) fn new() -> Self {
        Self(Default::default())
    }

    /// Returns the vectors that nova-core services, as GSP-RM reports them.
    pub(crate) fn vectors(&self) -> Result<KernelIntrVectors> {
        let len = usize::try_from(self.0.tableLen).unwrap_or(0);
        let entries = || self.0.table.iter().take(len);

        let mut notifications = KVec::new();
        for entry in entries().filter(|entry| {
            let engine = u32::from(entry.engineIdx);
            (bindings::MC_ENGINE_IDX_GR0..=bindings::MC_ENGINE_IDX_GR7).contains(&engine)
                || (bindings::MC_ENGINE_IDX_CE0..=bindings::MC_ENGINE_IDX_CE16).contains(&engine)
                || (bindings::MC_ENGINE_IDX_NVDEC0..=bindings::MC_ENGINE_IDX_NVDEC7)
                    .contains(&engine)
        }) {
            let vector = entry.vectorNonStall;
            if vector != NV_INTR_VECTOR_INVALID && !notifications.contains(&vector) {
                notifications.push(vector, GFP_KERNEL)?;
            }
        }

        Ok(KernelIntrVectors {
            display: entries()
                .find(|entry| u32::from(entry.engineIdx) == bindings::MC_ENGINE_IDX_DISP)
                .map(|entry| entry.vectorStall)
                .filter(|&vector| vector != NV_INTR_VECTOR_INVALID),
            notifications,
        })
    }
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for IntrKernelTableParams {}

// SAFETY: Padding is explicit and will not contain uninitialized data.
unsafe impl AsBytes for IntrKernelTableParams {}

/// Parameters of `NV2080_CTRL_CMD_GPU_GET_FERMI_GPC_INFO`: the mask of the enabled GPCs.
#[repr(transparent)]
pub(crate) struct GpcInfoParams(bindings::NV2080_CTRL_GPU_GET_FERMI_GPC_INFO_PARAMS);

impl GpcInfoParams {
    /// Creates the parameters, all filled in by GSP-RM.
    pub(crate) fn new() -> Self {
        Self(Default::default())
    }

    /// Returns the mask of the enabled GPCs, one bit per GPC ID.
    pub(crate) fn gpc_mask(&self) -> u32 {
        self.0.gpcMask
    }
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for GpcInfoParams {}

// SAFETY: This struct only contains integer types, without padding.
unsafe impl AsBytes for GpcInfoParams {}

/// Parameters of `NV2080_CTRL_CMD_GPU_GET_FERMI_TPC_INFO`: the mask of the enabled TPCs of a GPC.
#[repr(transparent)]
pub(crate) struct TpcInfoParams(bindings::NV2080_CTRL_GPU_GET_FERMI_TPC_INFO_PARAMS);

impl TpcInfoParams {
    /// Creates the parameters querying the TPCs of the GPC `gpc`.
    pub(crate) fn new(gpc: u32) -> Self {
        Self(bindings::NV2080_CTRL_GPU_GET_FERMI_TPC_INFO_PARAMS {
            gpcId: gpc,
            ..Default::default()
        })
    }

    /// Returns the mask of the enabled TPCs of the GPC, one bit per TPC ID: 0 if the GPC isn't
    /// enabled.
    pub(crate) fn tpc_mask(&self) -> u32 {
        self.0.tpcMask
    }
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for TpcInfoParams {}

// SAFETY: This struct only contains integer types, without padding.
unsafe impl AsBytes for TpcInfoParams {}

/// What the graphics engine's static information says about its SMs.
#[derive(Clone, Copy, Debug)]
pub(crate) struct GrStaticInfo {
    /// Version of the shader instruction set, as major and minor numbers.
    pub(crate) sm_version: (u8, u8),
    /// Number of warps an SM runs at most at once.
    pub(crate) max_warps_per_sm: u32,
    /// Number of SMs of a TPC.
    pub(crate) sms_per_tpc: u32,
}

/// Parameters of `NV2080_CTRL_CMD_INTERNAL_STATIC_KGR_GET_INFO`: the static information of the
/// graphics engines, which kernel RM loads once (`kgraphicsLoadStaticInfo`).
#[repr(transparent)]
pub(crate) struct GrInfoParams(bindings::NV2080_CTRL_INTERNAL_STATIC_KGR_GET_INFO_PARAMS);

impl GrInfoParams {
    /// Creates the parameters, all filled in by GSP-RM.
    pub(crate) fn new() -> Self {
        Self(Default::default())
    }

    /// Returns the information `index` (`NV2080_CTRL_GR_INFO_INDEX_*`) of the first graphics
    /// engine. The list is indexed by it, as kernel RM reads it.
    fn value(&self, index: u32) -> Option<u32> {
        self.0
            .engineInfo
            .first()?
            .infoList
            .get(usize::try_from(index).ok()?)
            .map(|info| info.data)
    }

    /// Returns what the information says about the SMs, if it has all of it.
    pub(crate) fn info(&self) -> Option<GrStaticInfo> {
        // `NV2080_CTRL_GR_INFO_SM_VERSION_*`: the major number in bits 15:8, the minor in 7:0.
        let version = self.value(bindings::NV2080_CTRL_GR_INFO_INDEX_SM_VERSION)?;

        Some(GrStaticInfo {
            sm_version: (
                u8::try_from(version >> 8).ok()?,
                u8::try_from(version & 0xff).ok()?,
            ),
            max_warps_per_sm: self.value(bindings::NV2080_CTRL_GR_INFO_INDEX_MAX_WARPS_PER_SM)?,
            sms_per_tpc: self.value(bindings::NV2080_CTRL_GR_INFO_INDEX_LITTER_NUM_SM_PER_TPC)?,
        })
    }
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for GrInfoParams {}

// SAFETY: This struct only contains integer types, without padding.
unsafe impl AsBytes for GrInfoParams {}

/// Memory of a GPFIFO channel, which the client allocates for GSP-RM to set up.
pub(crate) struct ChannelMemory {
    /// VRAM address of the instance block, 4 KiB aligned, whose first bytes are its RAMFC.
    pub(crate) instance: u64,
    /// VRAM address of the USERD, through which the channel is told about new work.
    pub(crate) userd: u64,
    /// DMA address of the method buffer in coherent system memory.
    pub(crate) method_buffer: u64,
    /// Size of the method buffer, from `NV2080_CTRL_CMD_CE_GET_FAULT_METHOD_BUFFER_SIZE`.
    pub(crate) method_buffer_size: u64,
}

/// Size of the instance block of a channel (`NV_RAMIN_ALLOC_SIZE` in RM).
pub(crate) const CHANNEL_INSTANCE_SIZE: u64 = 0x1000;
/// Size of the RAMFC of a channel, at the start of its instance block.
const CHANNEL_RAMFC_SIZE: u64 = 0x200;
/// Size of the USERD of a channel (`Nvc56fControl`).
pub(crate) const CHANNEL_USERD_SIZE: u64 = size_of::<bindings::Nvc56fControl>() as u64;
/// Number of USERDs in a USERD page, which RM indexes channel IDs by.
const CHANNEL_USERDS_PER_PAGE: u32 = 8;
/// Number of channel IDs the allocation of a channel can give: `NVOS04_FLAGS_CHANNEL_USERD_INDEX_PAGE_VALUE`
/// is 9 bits wide, `NVOS04_FLAGS_CHANNEL_USERD_INDEX_VALUE` 3 bits.
pub(crate) const CHANNEL_IDS: u32 = (1 << 9) * CHANNEL_USERDS_PER_PAGE;

// `NVOS04_FLAGS_*` fields in `alloc_channel.h`, whose positions bindgen can't translate.

/// `NVOS04_FLAGS_PRIVILEGED_CHANNEL_TRUE`: the channel may run privileged methods.
const NVOS04_FLAGS_PRIVILEGED_CHANNEL: u32 = 1 << 5;
/// `NVOS04_FLAGS_CHANNEL_USERD_INDEX_VALUE`: index of the channel's USERD in its page.
const NVOS04_FLAGS_CHANNEL_USERD_INDEX_VALUE_SHIFT: u32 = 8;
/// `NVOS04_FLAGS_CHANNEL_USERD_INDEX_PAGE_VALUE`: page of the channel's USERD.
const NVOS04_FLAGS_CHANNEL_USERD_INDEX_PAGE_VALUE_SHIFT: u32 = 12;
/// `NVOS04_FLAGS_CHANNEL_USERD_INDEX_PAGE_FIXED_TRUE`: the USERD page, and thus the channel ID,
/// is the one given.
const NVOS04_FLAGS_CHANNEL_USERD_INDEX_PAGE_FIXED: u32 = 1 << 21;

// `NV_KERNELCHANNEL_ALLOC_INTERNALFLAGS_*` in RM's `kernel_channel.h`, not part of the SDK.

/// `NV_KERNELCHANNEL_ALLOC_INTERNALFLAGS_PRIVILEGE_USER`.
const INTERNALFLAGS_PRIVILEGE_USER: u32 = 0;
/// `NV_KERNELCHANNEL_ALLOC_INTERNALFLAGS_PRIVILEGE_ADMIN`.
const INTERNALFLAGS_PRIVILEGE_ADMIN: u32 = 1;
/// `NV_KERNELCHANNEL_ALLOC_INTERNALFLAGS_ERROR_NOTIFIER_TYPE_NONE`: no error notifier.
const INTERNALFLAGS_ERROR_NOTIFIER_TYPE_NONE: u32 = 1 << 2;
/// `NV_KERNELCHANNEL_ALLOC_INTERNALFLAGS_ECC_ERROR_NOTIFIER_TYPE_NONE`: no ECC error notifier.
const INTERNALFLAGS_ECC_ERROR_NOTIFIER_TYPE_NONE: u32 = 1 << 4;

/// Where a channel runs, by RM handle.
#[derive(Clone, Copy)]
pub(crate) enum ChannelSpace {
    /// In an address space, for a channel allocated alone.
    VaSpace(u32),
    /// In a context share of the channel's group.
    ContextShare(u32),
}

/// Wrapper for [`bindings::NV_CHANNEL_GROUP_ALLOCATION_PARAMETERS`].
#[repr(transparent)]
pub(crate) struct ChannelGroupAllocParams(bindings::NV_CHANNEL_GROUP_ALLOCATION_PARAMETERS);

impl ChannelGroupAllocParams {
    /// Creates the parameters to allocate a channel group on engine `engine_type` (an
    /// `NV2080_ENGINE_TYPE`), whose channels run in the address space `va_space`.
    pub(crate) fn new(va_space: u32, engine_type: u32) -> Self {
        Self(bindings::NV_CHANNEL_GROUP_ALLOCATION_PARAMETERS {
            hVASpace: va_space,
            engineType: engine_type,
            ..Default::default()
        })
    }
}

// SAFETY: Padding is explicit and will not contain uninitialized data.
unsafe impl AsBytes for ChannelGroupAllocParams {}

/// Wrapper for [`bindings::NV_CTXSHARE_ALLOCATION_PARAMETERS`].
#[repr(transparent)]
pub(crate) struct ContextShareAllocParams(bindings::NV_CTXSHARE_ALLOCATION_PARAMETERS);

impl ContextShareAllocParams {
    /// Creates the parameters to allocate the synchronous subcontext of a channel group, the one
    /// graphics work runs in (VEID 0), in the address space `va_space`.
    pub(crate) fn sync(va_space: u32) -> Self {
        Self(bindings::NV_CTXSHARE_ALLOCATION_PARAMETERS {
            hVASpace: va_space,
            flags: bindings::NV_CTXSHARE_ALLOCATION_FLAGS_SUBCONTEXT_SYNC,
            subctxId: 0,
        })
    }
}

// SAFETY: This struct only contains integer types, without padding.
unsafe impl AsBytes for ContextShareAllocParams {}

/// Number of runqueues of a runlist, each of which has a method buffer in a channel group
/// (`kfifoGetNumRunqueues_GV100` in RM, the size of the arrays of
/// [`PromoteFaultMethodBuffersParams`]).
pub(crate) const RUNQUEUES: usize =
    num::u32_as_usize(bindings::NVA06C_CTRL_INTERNAL_PROMOTE_FAULT_METHOD_BUFFERS_MAX_RUNQUEUES);

/// Wrapper for [`bindings::NVA06C_CTRL_INTERNAL_PROMOTE_FAULT_METHOD_BUFFERS_PARAMS`].
#[repr(transparent)]
pub(crate) struct PromoteFaultMethodBuffersParams(
    bindings::NVA06C_CTRL_INTERNAL_PROMOTE_FAULT_METHOD_BUFFERS_PARAMS,
);

impl PromoteFaultMethodBuffersParams {
    /// Creates the parameters to give GSP-RM the method buffers of a channel group, one per
    /// runqueue, of `size` bytes at the DMA addresses `buffers` in coherent system memory.
    pub(crate) fn new(buffers: [u64; RUNQUEUES], size: u64) -> Self {
        Self(
            bindings::NVA06C_CTRL_INTERNAL_PROMOTE_FAULT_METHOD_BUFFERS_PARAMS {
                methodBufferMemdesc: buffers.map(|base| {
                    bindings::NV2080_CTRL_INTERNAL_MEMDESC_INFO {
                        base,
                        size,
                        alignment: 1,
                        addressSpace: ADDR_SYSMEM,
                        cpuCacheAttrib: bindings::NV_MEMORY_CACHED,
                    }
                }),
                numValidEntries:
                    bindings::NVA06C_CTRL_INTERNAL_PROMOTE_FAULT_METHOD_BUFFERS_MAX_RUNQUEUES,
                ..Default::default()
            },
        )
    }
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for PromoteFaultMethodBuffersParams {}

// SAFETY: Padding is explicit and will not contain uninitialized data.
unsafe impl AsBytes for PromoteFaultMethodBuffersParams {}

/// Wrapper for [`bindings::NV_CHANNEL_ALLOC_PARAMS`].
#[repr(transparent)]
pub(crate) struct ChannelGpfifoAllocParams(bindings::NV_CHANNEL_ALLOC_PARAMS);

impl ChannelGpfifoAllocParams {
    /// Creates the parameters to allocate the channel `chid` on engine `engine_type` (an
    /// `NV2080_ENGINE_TYPE`), in `space`, whose GPFIFO of `entries` entries is at GPU virtual
    /// address `gpfifo`.
    ///
    /// Kernel channels are `privileged`, those of clients aren't, like nouveau's: their push
    /// buffers can't use the methods reserved to privileged channels.
    pub(crate) fn new(
        space: ChannelSpace,
        engine_type: u32,
        chid: u32,
        gpfifo: u64,
        entries: u32,
        memory: &ChannelMemory,
        privileged: bool,
    ) -> Self {
        // A channel of a group runs in the address space of its context share instead of naming
        // one (`kchannelConstruct` in RM).
        let (va_space, context_share) = match space {
            ChannelSpace::VaSpace(va_space) => (va_space, 0),
            ChannelSpace::ContextShare(context_share) => (0, context_share),
        };
        let vram = |base, size| bindings::NV_MEMORY_DESC_PARAMS {
            base,
            size,
            addressSpace: ADDR_FBMEM,
            cacheAttrib: bindings::NV_MEMORY_UNCACHED,
        };

        Self(bindings::NV_CHANNEL_ALLOC_PARAMS {
            gpFifoOffset: gpfifo,
            gpFifoEntries: entries,
            // The channel ID is set through the index of the channel's USERD.
            flags: if privileged {
                NVOS04_FLAGS_PRIVILEGED_CHANNEL
            } else {
                0
            } | (chid % CHANNEL_USERDS_PER_PAGE)
                << NVOS04_FLAGS_CHANNEL_USERD_INDEX_VALUE_SHIFT
                | (chid / CHANNEL_USERDS_PER_PAGE)
                    << NVOS04_FLAGS_CHANNEL_USERD_INDEX_PAGE_VALUE_SHIFT
                | NVOS04_FLAGS_CHANNEL_USERD_INDEX_PAGE_FIXED,
            hVASpace: va_space,
            hContextShare: context_share,
            engineType: engine_type,
            instanceMem: vram(memory.instance, CHANNEL_INSTANCE_SIZE),
            userdMem: vram(memory.userd, CHANNEL_USERD_SIZE),
            ramfcMem: vram(memory.instance, CHANNEL_RAMFC_SIZE),
            mthdbufMem: bindings::NV_MEMORY_DESC_PARAMS {
                base: memory.method_buffer,
                size: memory.method_buffer_size,
                addressSpace: ADDR_SYSMEM,
                cacheAttrib: bindings::NV_MEMORY_CACHED,
            },
            internalFlags: if privileged {
                INTERNALFLAGS_PRIVILEGE_ADMIN
            } else {
                INTERNALFLAGS_PRIVILEGE_USER
            } | INTERNALFLAGS_ERROR_NOTIFIER_TYPE_NONE
                | INTERNALFLAGS_ECC_ERROR_NOTIFIER_TYPE_NONE,
            ..Default::default()
        })
    }
}

// SAFETY: Padding is explicit and will not contain uninitialized data.
unsafe impl AsBytes for ChannelGpfifoAllocParams {}

/// A kind of context buffer of the graphics engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GrContextBuffer {
    /// The buffer in [`GrContextBuffersInfoParams`]
    /// (`NV0080_CTRL_FIFO_GET_ENGINE_CONTEXT_PROPERTIES_ENGINE_ID_*`).
    pub(crate) engine_id: u32,
    /// The buffer in [`PromoteCtxParams`] (`NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_*`).
    pub(crate) promote_id: u16,
    /// Whether all the channels share the buffer, or each has its own.
    pub(crate) global: bool,
    /// Whether GSP-RM initializes the buffer when it's given.
    pub(crate) init: bool,
    /// Whether the graphics engine only reads the buffer.
    pub(crate) read_only: bool,
    /// Whether the buffer is in coherent system memory rather than VRAM.
    pub(crate) sysmem: bool,
}

impl GrContextBuffer {
    /// Returns whether the buffer is the main context buffer, which ends with per-subcontext
    /// headers.
    pub(crate) fn is_main(&self) -> bool {
        u32::from(self.promote_id) == bindings::NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_MAIN
    }

    /// Returns whether the buffer is the attribute circular buffer, whose GPU virtual address must
    /// be aligned to its size.
    pub(crate) fn is_attribute_cb(&self) -> bool {
        u32::from(self.promote_id) == bindings::NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_ATTRIBUTE_CB
    }

    /// Returns whether the buffer is the privileged register access map, which the golden context
    /// takes unmapped, and twice: also as the unrestricted map.
    pub(crate) fn is_priv_access_map(&self) -> bool {
        u32::from(self.promote_id)
            == bindings::NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_PRIV_ACCESS_MAP
    }
}

/// [`GrContextBuffer::promote_id`] of the unrestricted privileged register access map, which only
/// the golden context takes: a copy of the privileged register access map.
pub(crate) const GR_UNRESTRICTED_PRIV_ACCESS_MAP: u16 =
    bindings::NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_UNRESTRICTED_PRIV_ACCESS_MAP as u16;

/// The context buffers of the graphics engine that channels need, like nouveau's.
pub(crate) const GR_CONTEXT_BUFFERS: [GrContextBuffer; 8] = {
    const fn buffer(
        engine_id: u32,
        promote_id: u32,
        global: bool,
        init: bool,
        read_only: bool,
        sysmem: bool,
    ) -> GrContextBuffer {
        GrContextBuffer {
            engine_id,
            // CAST: promote IDs are below 16.
            promote_id: promote_id as u16,
            global,
            init,
            read_only,
            sysmem,
        }
    }

    [
        buffer(
            bindings::NV0080_CTRL_FIFO_GET_ENGINE_CONTEXT_PROPERTIES_ENGINE_ID_GRAPHICS,
            bindings::NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_MAIN,
            false,
            true,
            false,
            false,
        ),
        buffer(
            bindings::NV0080_CTRL_FIFO_GET_ENGINE_CONTEXT_PROPERTIES_ENGINE_ID_GRAPHICS_PATCH,
            bindings::NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_PATCH,
            false,
            true,
            false,
            false,
        ),
        buffer(
            bindings::NV0080_CTRL_FIFO_GET_ENGINE_CONTEXT_PROPERTIES_ENGINE_ID_GRAPHICS_BUNDLE_CB,
            bindings::NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_BUFFER_BUNDLE_CB,
            true,
            false,
            false,
            false,
        ),
        buffer(
            bindings::NV0080_CTRL_FIFO_GET_ENGINE_CONTEXT_PROPERTIES_ENGINE_ID_GRAPHICS_PAGEPOOL,
            bindings::NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_PAGEPOOL,
            true,
            false,
            false,
            false,
        ),
        buffer(
            bindings::NV0080_CTRL_FIFO_GET_ENGINE_CONTEXT_PROPERTIES_ENGINE_ID_GRAPHICS_ATTRIBUTE_CB,
            bindings::NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_ATTRIBUTE_CB,
            true,
            false,
            false,
            false,
        ),
        buffer(
            bindings::NV0080_CTRL_FIFO_GET_ENGINE_CONTEXT_PROPERTIES_ENGINE_ID_GRAPHICS_RTV_CB_GLOBAL,
            bindings::NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_RTV_CB_GLOBAL,
            true,
            false,
            false,
            false,
        ),
        buffer(
            bindings::NV0080_CTRL_FIFO_GET_ENGINE_CONTEXT_PROPERTIES_ENGINE_ID_GRAPHICS_FECS_EVENT,
            bindings::NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_FECS_EVENT,
            true,
            true,
            false,
            // In VRAM, like nouveau gives it (`r535_gr_promote_ctx()`), rather than in cached
            // system memory, kernel RM's default for it (`kgraphicsConstructEngine()`).
            false,
        ),
        buffer(
            bindings::NV0080_CTRL_FIFO_GET_ENGINE_CONTEXT_PROPERTIES_ENGINE_ID_GRAPHICS_PRIV_ACCESS_MAP,
            bindings::NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_PRIV_ACCESS_MAP,
            true,
            true,
            true,
            false,
        ),
    ]
};

/// Size and alignment of a context buffer of the graphics engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ContextBufferInfo {
    /// Size in bytes.
    pub(crate) size: u32,
    /// Alignment in bytes.
    pub(crate) alignment: u32,
}

/// Wrapper for [`bindings::NV2080_CTRL_INTERNAL_STATIC_GR_GET_CONTEXT_BUFFERS_INFO_PARAMS`].
#[repr(transparent)]
pub(crate) struct GrContextBuffersInfoParams(
    bindings::NV2080_CTRL_INTERNAL_STATIC_GR_GET_CONTEXT_BUFFERS_INFO_PARAMS,
);

impl GrContextBuffersInfoParams {
    /// Creates the parameters to query the sizes of the context buffers of the graphics engines.
    pub(crate) fn new() -> Self {
        Self(Default::default())
    }

    /// Returns the size and alignment of the context buffer `buffer` of the first graphics engine.
    pub(crate) fn info(&self, buffer: &GrContextBuffer) -> Option<ContextBufferInfo> {
        let info = self
            .0
            .engineContextBuffersInfo
            .first()?
            .engine
            .get(usize::try_from(buffer.engine_id).ok()?)?;

        Some(ContextBufferInfo {
            size: info.size,
            alignment: info.alignment,
        })
    }
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for GrContextBuffersInfoParams {}

// SAFETY: This struct only contains integer types, without padding.
unsafe impl AsBytes for GrContextBuffersInfoParams {}

/// A context buffer given to GSP-RM with [`PromoteCtxParams`].
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PromoteEntry {
    /// The buffer (`NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_*`).
    pub(crate) promote_id: u16,
    /// GPU virtual address of the buffer in the channel's address space, if mapped there.
    pub(crate) gpu_va: Option<u64>,
    /// Where the buffer is and its size, if GSP-RM is to initialize it.
    pub(crate) init: Option<PromoteInit>,
}

/// A context buffer GSP-RM initializes when given it with [`PromoteCtxParams`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct PromoteInit {
    /// VRAM address of the buffer, or its DMA address if `sysmem`.
    pub(crate) addr: u64,
    /// Size in bytes.
    pub(crate) size: u64,
    /// Whether the buffer is in coherent system memory.
    pub(crate) sysmem: bool,
}

/// `physAttr` of initialized buffers in VRAM, not cached by the GPU
/// (`NV2080_CTRL_GPU_INITIALIZE_CTX_*`, aperture in bits 1:0, `GPU_CACHEABLE` in bit 2).
const PROMOTE_PHYS_ATTR_VIDMEM_UNCACHED: u32 =
    bindings::NV2080_CTRL_GPU_INITIALIZE_CTX_APERTURE_VIDMEM
        | bindings::NV2080_CTRL_GPU_INITIALIZE_CTX_GPU_CACHEABLE_NO << 2;
/// `physAttr` of initialized buffers in coherent system memory, not cached by the GPU, as kernel
/// RM gives them (`kgrctxPrepareInitializeCtxBuffer()`).
const PROMOTE_PHYS_ATTR_COH_SYS_UNCACHED: u32 =
    bindings::NV2080_CTRL_GPU_INITIALIZE_CTX_APERTURE_COH_SYS
        | bindings::NV2080_CTRL_GPU_INITIALIZE_CTX_GPU_CACHEABLE_NO << 2;

/// Wrapper for [`bindings::NV2080_CTRL_GPU_PROMOTE_CTX_PARAMS`].
#[repr(transparent)]
pub(crate) struct PromoteCtxParams(bindings::NV2080_CTRL_GPU_PROMOTE_CTX_PARAMS);

impl PromoteCtxParams {
    /// Creates the parameters to give the graphics context buffers of the channel `channel` of
    /// `client` to GSP-RM.
    pub(crate) fn graphics(client: u32, channel: u32) -> Self {
        Self(bindings::NV2080_CTRL_GPU_PROMOTE_CTX_PARAMS {
            engineType: bindings::NV2080_ENGINE_TYPE_GRAPHICS,
            hChanClient: client,
            hObject: channel,
            ..Default::default()
        })
    }

    /// Creates the parameters to give GSP-RM the context of channel `chid`, `channel` of
    /// `client`, on an engine run by a falcon such as a video decoder: the `size` bytes mapped at
    /// `gpu_va` in the channel's address space. `engine_type` is the engine's `NV2080_ENGINE_TYPE`.
    pub(crate) fn falcon(
        engine_type: u32,
        client: u32,
        channel: u32,
        chid: u32,
        gpu_va: u64,
        size: u64,
    ) -> Self {
        // As nouveau gives them (`r535_flcn_bind()`): the buffer as a whole, without entries.
        Self(bindings::NV2080_CTRL_GPU_PROMOTE_CTX_PARAMS {
            engineType: engine_type,
            hClient: client,
            ChID: chid,
            hChanClient: client,
            hObject: channel,
            virtAddress: gpu_va,
            size,
            ..Default::default()
        })
    }

    /// Adds `entry`.
    pub(crate) fn push(&mut self, entry: &PromoteEntry) -> Result {
        let slot = self
            .0
            .promoteEntry
            .get_mut(usize::try_from(self.0.entryCount)?)
            .ok_or(ENOSPC)?;

        *slot = bindings::NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ENTRY {
            bufferId: entry.promote_id,
            gpuVirtAddr: entry.gpu_va.unwrap_or(0),
            bNonmapped: u8::from(entry.gpu_va.is_none()),
            ..Default::default()
        };
        if let Some(init) = entry.init {
            slot.bInitialize = 1;
            slot.gpuPhysAddr = init.addr;
            slot.size = init.size;
            slot.physAttr = if init.sysmem {
                PROMOTE_PHYS_ATTR_COH_SYS_UNCACHED
            } else {
                PROMOTE_PHYS_ATTR_VIDMEM_UNCACHED
            };
        }
        self.0.entryCount += 1;

        Ok(())
    }
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for PromoteCtxParams {}

// SAFETY: Padding is explicit and will not contain uninitialized data.
unsafe impl AsBytes for PromoteCtxParams {}

/// A page directory level given to GSP-RM with [`CopyServerReservedPdesParams`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct ServerReservedLevel {
    /// VRAM address of the page directory.
    pub(crate) addr: u64,
    /// Size of the page directory in bytes.
    pub(crate) size: u64,
    /// Position of the lowest virtual address bit indexing the level.
    pub(crate) va_shift: u8,
}

/// Wrapper for [`bindings::NV90F1_CTRL_VASPACE_COPY_SERVER_RESERVED_PDES_PARAMS`].
#[repr(transparent)]
pub(crate) struct CopyServerReservedPdesParams(
    bindings::NV90F1_CTRL_VASPACE_COPY_SERVER_RESERVED_PDES_PARAMS,
);

impl CopyServerReservedPdesParams {
    /// Creates the parameters giving GSP-RM `levels`, the page directories in VRAM above
    /// [`RM_SERVER_VA`] from the root one down, whose last level's entries cover `entry_size`
    /// bytes.
    pub(crate) fn new(levels: &[ServerReservedLevel], entry_size: u64) -> Result<Self> {
        let mut params = bindings::NV90F1_CTRL_VASPACE_COPY_SERVER_RESERVED_PDES_PARAMS {
            pageSize: entry_size,
            virtAddrLo: RM_SERVER_VA.start,
            virtAddrHi: RM_SERVER_VA.end - 1,
            numLevelsToCopy: u32::try_from(levels.len())?,
            ..Default::default()
        };
        if levels.len() > params.levels.len() {
            return Err(EINVAL);
        }
        for (dst, level) in params.levels.iter_mut().zip(levels) {
            dst.physAddress = level.addr;
            dst.size = level.size;
            dst.aperture = bindings::GMMU_APERTURE_GMMU_APERTURE_VIDEO;
            dst.pageShift = level.va_shift;
        }

        Ok(Self(params))
    }
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for CopyServerReservedPdesParams {}

// SAFETY: Padding is explicit and will not contain uninitialized data.
unsafe impl AsBytes for CopyServerReservedPdesParams {}

/// Wrapper for [`bindings::NV5070_CTRL_SET_MEMPOOL_WAR_FOR_BLIT_TEARING_PARAMS`].
#[repr(transparent)]
pub(crate) struct DispMempoolWarParams(
    bindings::NV5070_CTRL_SET_MEMPOOL_WAR_FOR_BLIT_TEARING_PARAMS,
);

impl DispMempoolWarParams {
    /// Creates the parameters to have the display engine of subdevice 0 fetch less ahead of
    /// scanout if `engage`, or as much as it is configured to otherwise.
    pub(crate) fn new(engage: bool) -> Self {
        Self(
            bindings::NV5070_CTRL_SET_MEMPOOL_WAR_FOR_BLIT_TEARING_PARAMS {
                bEngageWAR: u8::from(engage),
                ..Default::default()
            },
        )
    }
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for DispMempoolWarParams {}

// SAFETY: The padding of this struct is explicit, as fields.
unsafe impl AsBytes for DispMempoolWarParams {}

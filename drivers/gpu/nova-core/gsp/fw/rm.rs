// SPDX-License-Identifier: GPL-2.0

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
    /// Get the engines of the GPU and the runlists they are on.
    FifoGetDeviceInfoTable = bindings::NV2080_CTRL_CMD_FIFO_GET_DEVICE_INFO_TABLE,
    /// Get the sizes of the context buffers of the graphics engines.
    InternalStaticKgrGetContextBuffersInfo =
        bindings::NV2080_CTRL_CMD_INTERNAL_STATIC_KGR_GET_CONTEXT_BUFFERS_INFO,
    /// Give the context buffers of a channel to GSP-RM.
    GpuPromoteCtx = bindings::NV2080_CTRL_CMD_GPU_PROMOTE_CTX,
    /// Give GSP-RM the page directories above its part of a split address space.
    VaspaceCopyServerReservedPdes = bindings::NV90F1_CTRL_CMD_VASPACE_COPY_SERVER_RESERVED_PDES,
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
    /// A GPFIFO channel of GA10x GPUs, child of a device.
    ChannelGpfifoAmpere = bindings::AMPERE_CHANNEL_GPFIFO_A,
    /// A copy engine object of GA10x GPUs, child of a channel.
    CopyEngineAmpereB = bindings::AMPERE_DMA_COPY_B,
    /// A 3D object of GA10x GPUs (but GA100), child of a channel on the graphics engine.
    ThreeDAmpereB = bindings::AMPERE_B,
    /// A compute object of GA10x GPUs (but GA100), child of a channel on the graphics engine.
    ComputeAmpereB = bindings::AMPERE_COMPUTE_B,
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
}

impl EngineInfo {
    /// Returns whether the engine is the graphics engine.
    pub(crate) fn is_graphics(&self) -> bool {
        self.engine_type == bindings::NV2080_ENGINE_TYPE_GRAPHICS
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

    /// Returns the engines of the entries returned.
    pub(crate) fn engines(&self) -> impl Iterator<Item = EngineInfo> + '_ {
        let count = usize::try_from(self.0.numEntries).unwrap_or(0);

        self.0.entries.iter().take(count).filter_map(|entry| {
            let data = |info: bindings::ENGINE_INFO_TYPE| {
                entry.engineData.get(usize::try_from(info).ok()?).copied()
            };

            Some(EngineInfo {
                engine_type: data(bindings::ENGINE_INFO_TYPE_ENGINE_INFO_TYPE_RM_ENGINE_TYPE)?,
                runlist: data(bindings::ENGINE_INFO_TYPE_ENGINE_INFO_TYPE_RUNLIST)?,
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

/// `NV_KERNELCHANNEL_ALLOC_INTERNALFLAGS_PRIVILEGE_ADMIN`.
const INTERNALFLAGS_PRIVILEGE_ADMIN: u32 = 1;
/// `NV_KERNELCHANNEL_ALLOC_INTERNALFLAGS_ERROR_NOTIFIER_TYPE_NONE`: no error notifier.
const INTERNALFLAGS_ERROR_NOTIFIER_TYPE_NONE: u32 = 1 << 2;
/// `NV_KERNELCHANNEL_ALLOC_INTERNALFLAGS_ECC_ERROR_NOTIFIER_TYPE_NONE`: no ECC error notifier.
const INTERNALFLAGS_ECC_ERROR_NOTIFIER_TYPE_NONE: u32 = 1 << 4;

/// Wrapper for [`bindings::NV_CHANNEL_ALLOC_PARAMS`].
#[repr(transparent)]
pub(crate) struct ChannelGpfifoAllocParams(bindings::NV_CHANNEL_ALLOC_PARAMS);

impl ChannelGpfifoAllocParams {
    /// Creates the parameters to allocate the privileged channel `chid` on engine `engine_type`
    /// (an `NV2080_ENGINE_TYPE`), in the address space `va_space`, whose GPFIFO of `entries`
    /// entries is at GPU virtual address `gpfifo`.
    pub(crate) fn kernel(
        va_space: u32,
        engine_type: u32,
        chid: u32,
        gpfifo: u64,
        entries: u32,
        memory: &ChannelMemory,
    ) -> Self {
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
            flags: NVOS04_FLAGS_PRIVILEGED_CHANNEL
                | (chid % CHANNEL_USERDS_PER_PAGE) << NVOS04_FLAGS_CHANNEL_USERD_INDEX_VALUE_SHIFT
                | (chid / CHANNEL_USERDS_PER_PAGE)
                    << NVOS04_FLAGS_CHANNEL_USERD_INDEX_PAGE_VALUE_SHIFT
                | NVOS04_FLAGS_CHANNEL_USERD_INDEX_PAGE_FIXED,
            hVASpace: va_space,
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
            internalFlags: INTERNALFLAGS_PRIVILEGE_ADMIN
                | INTERNALFLAGS_ERROR_NOTIFIER_TYPE_NONE
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
    ) -> GrContextBuffer {
        GrContextBuffer {
            engine_id,
            // CAST: promote IDs are below 16.
            promote_id: promote_id as u16,
            global,
            init,
            read_only,
        }
    }

    [
        buffer(
            bindings::NV0080_CTRL_FIFO_GET_ENGINE_CONTEXT_PROPERTIES_ENGINE_ID_GRAPHICS,
            bindings::NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_MAIN,
            false,
            true,
            false,
        ),
        buffer(
            bindings::NV0080_CTRL_FIFO_GET_ENGINE_CONTEXT_PROPERTIES_ENGINE_ID_GRAPHICS_PATCH,
            bindings::NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_PATCH,
            false,
            true,
            false,
        ),
        buffer(
            bindings::NV0080_CTRL_FIFO_GET_ENGINE_CONTEXT_PROPERTIES_ENGINE_ID_GRAPHICS_BUNDLE_CB,
            bindings::NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_BUFFER_BUNDLE_CB,
            true,
            false,
            false,
        ),
        buffer(
            bindings::NV0080_CTRL_FIFO_GET_ENGINE_CONTEXT_PROPERTIES_ENGINE_ID_GRAPHICS_PAGEPOOL,
            bindings::NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_PAGEPOOL,
            true,
            false,
            false,
        ),
        buffer(
            bindings::NV0080_CTRL_FIFO_GET_ENGINE_CONTEXT_PROPERTIES_ENGINE_ID_GRAPHICS_ATTRIBUTE_CB,
            bindings::NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_ATTRIBUTE_CB,
            true,
            false,
            false,
        ),
        buffer(
            bindings::NV0080_CTRL_FIFO_GET_ENGINE_CONTEXT_PROPERTIES_ENGINE_ID_GRAPHICS_RTV_CB_GLOBAL,
            bindings::NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_RTV_CB_GLOBAL,
            true,
            false,
            false,
        ),
        buffer(
            bindings::NV0080_CTRL_FIFO_GET_ENGINE_CONTEXT_PROPERTIES_ENGINE_ID_GRAPHICS_FECS_EVENT,
            bindings::NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_FECS_EVENT,
            true,
            true,
            false,
        ),
        buffer(
            bindings::NV0080_CTRL_FIFO_GET_ENGINE_CONTEXT_PROPERTIES_ENGINE_ID_GRAPHICS_PRIV_ACCESS_MAP,
            bindings::NV2080_CTRL_GPU_PROMOTE_CTX_BUFFER_ID_PRIV_ACCESS_MAP,
            true,
            true,
            true,
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
    /// VRAM address and size of the buffer, if GSP-RM is to initialize it.
    pub(crate) init: Option<(u64, u64)>,
}

/// `physAttr` of initialized buffers: in VRAM, not cached by the GPU (`NV2080_CTRL_GPU_INITIALIZE_CTX_*`,
/// aperture in bits 1:0, `GPU_CACHEABLE` in bit 2).
const PROMOTE_PHYS_ATTR_VIDMEM_UNCACHED: u32 =
    bindings::NV2080_CTRL_GPU_INITIALIZE_CTX_APERTURE_VIDMEM
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
        if let Some((addr, size)) = entry.init {
            slot.bInitialize = 1;
            slot.gpuPhysAddr = addr;
            slot.size = size;
            slot.physAttr = PROMOTE_PHYS_ATTR_VIDMEM_UNCACHED;
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

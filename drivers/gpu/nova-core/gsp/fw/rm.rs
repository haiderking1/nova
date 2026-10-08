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
        self.0.edidBuffer.get(..num::u32_as_usize(self.0.bufferSize))
    }
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for DispGetEdidParams {}

// SAFETY: This struct contains no padding.
unsafe impl AsBytes for DispGetEdidParams {}

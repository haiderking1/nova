// SPDX-License-Identifier: GPL-2.0 OR MIT

//! DRM DisplayPort helpers.
//!
//! A DisplayPort sink is talked to over the AUX channel of its link: the source reads and writes
//! the sink's DPCD registers, which describe what the sink and the repeaters on the way (LTTPRs)
//! can do and control the link. A driver provides the transfers on the AUX channel of its hardware
//! with an [`AuxTransfer`] implementation, and [`Aux`] gives it DRM's DPCD helpers on top.
//!
//! C header: [`include/drm/display/drm_dp_helper.h`](srctree/include/drm/display/drm_dp_helper.h)
#![cfg(CONFIG_RUST_DRM_DISPLAY_DP_HELPER)]

use crate::{
    bindings,
    device,
    error::to_result,
    prelude::*,
    sync::aref::ARef,
    types::Opaque, //
};
use core::marker::PhantomPinned;

/// DPCD registers and their values, from [`include/drm/display/drm_dp.h`].
///
/// [`include/drm/display/drm_dp.h`]: srctree/include/drm/display/drm_dp.h
pub mod dpcd {
    use crate::bindings;

    /// Power state of the sink (`DP_SET_POWER`).
    pub const SET_POWER: u32 = bindings::DP_SET_POWER;
    /// [`SET_POWER`] value waking the sink up (`DP_SET_POWER_D0`).
    // CAST: the value fits in the 8-bit register.
    pub const SET_POWER_D0: u8 = bindings::DP_SET_POWER_D0 as u8;
    /// Revision of the LTTPR registers, which LTTPRs answer for, 0x14 or more
    /// (`DP_LT_TUNABLE_PHY_REPEATER_FIELD_DATA_STRUCTURE_REV`).
    pub const LTTPR_FIELD_DATA_STRUCTURE_REV: u32 =
        bindings::DP_LT_TUNABLE_PHY_REPEATER_FIELD_DATA_STRUCTURE_REV;
}

/// Requests of AUX transfers, as defined by the DisplayPort specification (`DP_AUX_*`).
pub mod request {
    use crate::bindings;

    // CAST: the request codes are 4-bit values.

    /// Write to an I2C device over AUX.
    pub const I2C_WRITE: u8 = bindings::DP_AUX_I2C_WRITE as u8;
    /// Read from an I2C device over AUX.
    pub const I2C_READ: u8 = bindings::DP_AUX_I2C_READ as u8;
    /// Request the status of an I2C write over AUX.
    pub const I2C_WRITE_STATUS_UPDATE: u8 = bindings::DP_AUX_I2C_WRITE_STATUS_UPDATE as u8;
    /// Flag of I2C requests keeping the I2C transaction going ("middle of transaction").
    pub const I2C_MOT: u8 = bindings::DP_AUX_I2C_MOT as u8;
    /// Write to the DPCD.
    pub const NATIVE_WRITE: u8 = bindings::DP_AUX_NATIVE_WRITE as u8;
    /// Read from the DPCD.
    pub const NATIVE_READ: u8 = bindings::DP_AUX_NATIVE_READ as u8;
}

/// Replies to AUX transfers, as defined by the DisplayPort specification (`DP_AUX_*_REPLY_*`).
///
/// A reply holds the native reply in bits 1:0 and, for I2C requests, the I2C reply in bits 3:2.
pub mod reply {
    use crate::bindings;

    // CAST: the reply codes are 4-bit values.

    /// The sink did what was requested.
    pub const NATIVE_ACK: u8 = bindings::DP_AUX_NATIVE_REPLY_ACK as u8;
    /// The sink refused the request.
    pub const NATIVE_NACK: u8 = bindings::DP_AUX_NATIVE_REPLY_NACK as u8;
    /// The sink isn't ready for the request: it has to be retried.
    pub const NATIVE_DEFER: u8 = bindings::DP_AUX_NATIVE_REPLY_DEFER as u8;
    /// The I2C device did what was requested.
    pub const I2C_ACK: u8 = bindings::DP_AUX_I2C_REPLY_ACK as u8;
    /// The I2C device refused the request.
    pub const I2C_NACK: u8 = bindings::DP_AUX_I2C_REPLY_NACK as u8;
    /// The I2C device isn't ready for the request: it has to be retried.
    pub const I2C_DEFER: u8 = bindings::DP_AUX_I2C_REPLY_DEFER as u8;
}

/// Returns the link rate of the link bandwidth code `code` of the DPCD, such as
/// `DP_LINK_BW_2_7`, in units of 10 kb/s per lane (`drm_dp_bw_code_to_link_rate()`).
pub fn bw_code_to_link_rate(code: u8) -> u32 {
    // SAFETY: The function has no requirements.
    let rate = unsafe { bindings::drm_dp_bw_code_to_link_rate(code) };
    // A code's rate isn't negative.
    rate.try_into().unwrap_or(0)
}

/// Returns the link bandwidth code of the DPCD for the link rate `rate`, in units of 10 kb/s per
/// lane (`drm_dp_link_rate_to_bw_code()`).
pub fn link_rate_to_bw_code(rate: u32) -> u8 {
    // SAFETY: The function has no requirements.
    unsafe { bindings::drm_dp_link_rate_to_bw_code(rate.try_into().unwrap_or(i32::MAX)) }
}

/// Size of the receiver capabilities at the start of the DPCD.
const RECEIVER_CAP_SIZE: usize = bindings::DP_RECEIVER_CAP_SIZE as usize;
/// Size of the common capabilities of the LTTPRs.
const LTTPR_COMMON_CAP_SIZE: usize = bindings::DP_LTTPR_COMMON_CAP_SIZE as usize;

/// The receiver capabilities of a sink: the start of its DPCD, as read by [`Aux::read_dpcd_caps`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DpcdCaps([u8; RECEIVER_CAP_SIZE]);

impl DpcdCaps {
    /// Returns the raw capabilities, indexed by DPCD address.
    pub fn as_bytes(&self) -> &[u8; RECEIVER_CAP_SIZE] {
        &self.0
    }

    /// Returns the DPCD revision, such as 0x12 for DisplayPort 1.2.
    pub fn revision(&self) -> u8 {
        self.0[bindings::DP_DPCD_REV as usize]
    }

    /// Returns the highest link rate of the sink, in units of 10 kb/s per lane
    /// (`drm_dp_max_link_rate()`).
    pub fn max_link_rate(&self) -> u32 {
        bw_code_to_link_rate(self.0[bindings::DP_MAX_LINK_RATE as usize])
    }

    /// Returns the highest number of lanes of the sink (`drm_dp_max_lane_count()`).
    pub fn max_lane_count(&self) -> u8 {
        // CAST: the mask fits in 8 bits.
        self.0[bindings::DP_MAX_LANE_COUNT as usize] & bindings::DP_MAX_LANE_COUNT_MASK as u8
    }

    /// Returns whether the sink does enhanced framing (`drm_dp_enhanced_frame_cap()`).
    pub fn enhanced_frame_cap(&self) -> bool {
        self.revision() >= 0x11 && self.lane_count_flag(bindings::DP_ENHANCED_FRAME_CAP)
    }

    /// Returns whether the sink may ask for its drive settings to be adjusted after link training
    /// (`drm_dp_post_lt_adj_req_supported()`).
    pub fn post_lt_adj_req_supported(&self) -> bool {
        self.revision() >= 0x13 && self.lane_count_flag(bindings::DP_POST_LT_ADJ_REQ_SUPPORTED)
    }

    /// Returns whether the sink supports training pattern 3 (`drm_dp_tps3_supported()`).
    pub fn tps3_supported(&self) -> bool {
        self.revision() >= 0x12 && self.lane_count_flag(bindings::DP_TPS3_SUPPORTED)
    }

    /// Returns whether the sink supports training pattern 4 (`drm_dp_tps4_supported()`).
    pub fn tps4_supported(&self) -> bool {
        self.revision() >= 0x14
            // CAST: the flag fits in 8 bits.
            && self.0[bindings::DP_MAX_DOWNSPREAD as usize] & bindings::DP_TPS4_SUPPORTED as u8 != 0
    }

    /// Returns whether `flag` of the `DP_MAX_LANE_COUNT` register is set.
    fn lane_count_flag(&self, flag: u32) -> bool {
        // CAST: the flags fit in 8 bits.
        self.0[bindings::DP_MAX_LANE_COUNT as usize] & flag as u8 != 0
    }
}

/// The common capabilities of the LTTPRs of a link, as read by
/// [`Aux::read_lttpr_common_caps`].
///
/// LTTPRs (Link-Training Tunable PHY Repeaters) are retimers between the source and the sink, such
/// as in some cables and docks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LttprCaps([u8; LTTPR_COMMON_CAP_SIZE]);

impl LttprCaps {
    /// Returns the number of LTTPRs, 0 if there are none (`drm_dp_lttpr_count()`).
    ///
    /// Fails with `ERANGE` if the LTTPRs report an invalid count.
    pub fn count(&self) -> Result<u32> {
        // SAFETY: `self.0` is an array of the size the function reads.
        let count = unsafe { bindings::drm_dp_lttpr_count(self.0.as_ptr()) };
        to_result(count.min(0))?;
        // `count` isn't negative.
        Ok(count.try_into().unwrap_or(0))
    }

    /// Returns the highest link rate all LTTPRs support, in units of 10 kb/s per lane, or 0 if
    /// there are no LTTPRs (`drm_dp_lttpr_max_link_rate()`).
    pub fn max_link_rate(&self) -> u32 {
        // SAFETY: `self.0` is an array of the size the function reads.
        let rate = unsafe { bindings::drm_dp_lttpr_max_link_rate(self.0.as_ptr()) };
        rate.try_into().unwrap_or(0)
    }

    /// Returns the highest number of lanes all LTTPRs support, or 0 if there are no LTTPRs
    /// (`drm_dp_lttpr_max_lane_count()`).
    pub fn max_lane_count(&self) -> u8 {
        // SAFETY: `self.0` is an array of the size the function reads.
        let lanes = unsafe { bindings::drm_dp_lttpr_max_lane_count(self.0.as_ptr()) };
        lanes.try_into().unwrap_or(0)
    }
}

/// An AUX transfer, as DRM's helpers request it from [`AuxTransfer::transfer`].
pub struct AuxMsg<'a>(&'a mut bindings::drm_dp_aux_msg);

impl AuxMsg<'_> {
    /// Returns the address of the transfer: a DPCD address for native requests, the address of
    /// the I2C device for I2C requests.
    pub fn address(&self) -> u32 {
        self.0.address
    }

    /// Returns the request, one of [`request`] and for I2C requests maybe with
    /// [`request::I2C_MOT`].
    pub fn request(&self) -> u8 {
        self.0.request
    }

    /// Returns the bytes to write, or the room for those read, depending on the request. Empty
    /// for address-only transfers.
    pub fn buffer(&self) -> &[u8] {
        if self.0.size == 0 {
            return &[];
        }

        // SAFETY: DRM's helpers request transfers of `size` bytes from or into `buffer`, which
        // they keep valid for the transfer.
        unsafe { core::slice::from_raw_parts(self.0.buffer.cast::<u8>(), self.0.size) }
    }

    /// Returns the room for the bytes read. Empty for address-only transfers.
    pub fn buffer_mut(&mut self) -> &mut [u8] {
        if self.0.size == 0 {
            return &mut [];
        }

        // SAFETY: DRM's helpers request transfers of `size` bytes from or into `buffer`, which
        // they keep valid for the transfer and don't access during it.
        unsafe { core::slice::from_raw_parts_mut(self.0.buffer.cast::<u8>(), self.0.size) }
    }

    /// Sets the reply to the transfer, a combination of [`reply`] values.
    pub fn set_reply(&mut self, reply: u8) {
        self.0.reply = reply;
    }
}

/// The transfers of an AUX channel, which a driver implements for its hardware.
pub trait AuxTransfer: Send + Sync {
    /// Carries out the transfer `msg`: sends its request, sets its reply with
    /// [`AuxMsg::set_reply`], and returns the number of bytes transferred, which may be fewer than
    /// requested.
    ///
    /// Errors make DRM's helpers retry the transfer, as replies deferring it do, except for
    /// `ETIMEDOUT`, which they return after a few retries without waiting.
    fn transfer(&self, msg: &mut AuxMsg<'_>) -> Result<usize>;
}

/// An AUX channel, with DRM's helpers on top of the transfers `T` provides.
///
/// The channel is initialized but not registered: it has neither a userspace interface nor an I2C
/// adapter for others to use.
///
/// # Invariants
///
/// `aux` is initialized by `drm_dp_aux_init()`, with [`Self::transfer_callback`] as its transfer
/// function, and its device is `dev`.
#[pin_data]
pub struct Aux<T: AuxTransfer> {
    #[pin]
    aux: Opaque<bindings::drm_dp_aux>,
    dev: ARef<device::Device>,
    data: T,
    #[pin]
    _pin: PhantomPinned,
}

// SAFETY: DRM's helpers serialize the transfers of the channel with its `hw_mutex`, so its state
// can be used from any thread, and `T` is `Send` and `Sync`.
unsafe impl<T: AuxTransfer> Send for Aux<T> {}

// SAFETY: See `Send`: the helpers lock the channel for what they change in it.
unsafe impl<T: AuxTransfer> Sync for Aux<T> {}

impl<T: AuxTransfer> Aux<T> {
    /// Creates the AUX channel named `name` of the device `dev`, carrying out its transfers with
    /// `data`.
    pub fn new(dev: &device::Device, name: &'static CStr, data: T) -> impl PinInit<Self> {
        let dev: ARef<device::Device> = dev.into();
        let raw_dev = dev.as_raw();

        pin_init!(Self {
            aux <- Opaque::ffi_init(move |aux: *mut bindings::drm_dp_aux| {
                // SAFETY: `aux` is valid for writes, and all-zeroes is a valid `drm_dp_aux` before
                // initialization.
                unsafe { aux.write_bytes(0, 1) };
                // SAFETY: `aux` is valid for writes, and initialized above.
                unsafe {
                    (*aux).name = name.as_char_ptr();
                    (*aux).dev = raw_dev;
                    (*aux).transfer = Some(Self::transfer_callback);
                }
                // SAFETY: `aux` is a zeroed `drm_dp_aux` with its name, device and transfer
                // function set, at the address it keeps, as `Self` is pinned.
                unsafe { bindings::drm_dp_aux_init(aux) };
            }),
            dev,
            data,
            _pin: PhantomPinned,
        })
    }

    /// Returns the transfers of the channel.
    pub fn data(&self) -> &T {
        &self.data
    }

    /// Returns the device the channel belongs to.
    pub fn device(&self) -> &device::Device {
        &self.dev
    }

    fn as_raw(&self) -> *mut bindings::drm_dp_aux {
        self.aux.get()
    }

    /// Reads `buf.len()` bytes of the DPCD from `offset` (`drm_dp_dpcd_read_data()`).
    ///
    /// Fails with `EIO` if the sink refuses the read or retrying it doesn't help, and with
    /// `EPROTO` if fewer bytes were read.
    pub fn dpcd_read(&self, offset: u32, buf: &mut [u8]) -> Result {
        // SAFETY: `self.as_raw()` is an initialized `drm_dp_aux` by the type invariants, and `buf`
        // is valid for writes of its length.
        to_result(unsafe {
            bindings::drm_dp_dpcd_read_data(
                self.as_raw(),
                offset,
                buf.as_mut_ptr().cast(),
                buf.len(),
            )
        })
    }

    /// Writes `buf` to the DPCD from `offset` (`drm_dp_dpcd_write_data()`).
    ///
    /// Fails with `EIO` if the sink refuses the write or retrying it doesn't help, and with
    /// `EPROTO` if fewer bytes were written.
    pub fn dpcd_write(&self, offset: u32, buf: &[u8]) -> Result {
        // SAFETY: `self.as_raw()` is an initialized `drm_dp_aux` by the type invariants, and `buf`
        // is valid for reads of its length; the function doesn't write to it.
        to_result(unsafe {
            bindings::drm_dp_dpcd_write_data(
                self.as_raw(),
                offset,
                buf.as_ptr().cast_mut().cast(),
                buf.len(),
            )
        })
    }

    /// Reads the receiver capabilities of the sink, from the extended ones if the sink has them
    /// (`drm_dp_read_dpcd_caps()`).
    pub fn read_dpcd_caps(&self) -> Result<DpcdCaps> {
        let mut caps = [0; RECEIVER_CAP_SIZE];

        // SAFETY: `self.as_raw()` is an initialized `drm_dp_aux` by the type invariants, and
        // `caps` is an array of the size the function writes.
        to_result(unsafe { bindings::drm_dp_read_dpcd_caps(self.as_raw(), caps.as_mut_ptr()) })?;

        Ok(DpcdCaps(caps))
    }

    /// Reads the common capabilities of the LTTPRs of the link, of the sink with the
    /// capabilities `dpcd` (`drm_dp_read_lttpr_common_caps()`).
    pub fn read_lttpr_common_caps(&self, dpcd: &DpcdCaps) -> Result<LttprCaps> {
        let mut caps = [0; LTTPR_COMMON_CAP_SIZE];

        // SAFETY: `self.as_raw()` is an initialized `drm_dp_aux` by the type invariants, and
        // `dpcd.0` and `caps` are arrays of the sizes the function reads and writes.
        to_result(unsafe {
            bindings::drm_dp_read_lttpr_common_caps(
                self.as_raw(),
                dpcd.0.as_ptr(),
                caps.as_mut_ptr(),
            )
        })?;

        Ok(LttprCaps(caps))
    }

    /// Puts the `count` LTTPRs of the link in non-transparent mode, in which each of them is
    /// trained on its own, or leaves them transparent if that fails (`drm_dp_lttpr_init()`).
    pub fn lttpr_init(&self, count: u32) -> Result {
        // SAFETY: `self.as_raw()` is an initialized `drm_dp_aux` by the type invariants.
        to_result(unsafe { bindings::drm_dp_lttpr_init(self.as_raw(), count.try_into()?) })
    }

    /// Carries out the transfer `msg` of the channel `aux` with the channel's [`AuxTransfer`].
    ///
    /// # Safety
    ///
    /// `aux` must be the `drm_dp_aux` of an [`Aux<T>`], and `msg` a valid transfer request.
    unsafe extern "C" fn transfer_callback(
        aux: *mut bindings::drm_dp_aux,
        msg: *mut bindings::drm_dp_aux_msg,
    ) -> isize {
        // SAFETY: By the safety requirements, `aux` is the `aux` field of an `Aux<T>`, which is
        // alive as long as its channel is used.
        let this = unsafe { &*crate::container_of!(Opaque::cast_from(aux), Self, aux) };
        // SAFETY: By the safety requirements, `msg` is a valid transfer request, which DRM's
        // helpers don't touch during the transfer.
        let mut msg = AuxMsg(unsafe { &mut *msg });

        match this.data.transfer(&mut msg) {
            // A transfer is at most 16 bytes long.
            Ok(len) => len.try_into().unwrap_or(isize::MAX),
            // CAST: errnos are small negative numbers.
            Err(err) => err.to_errno() as isize,
        }
    }
}

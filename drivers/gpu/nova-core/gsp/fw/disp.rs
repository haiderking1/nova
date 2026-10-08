// SPDX-License-Identifier: GPL-2.0

//! Methods of the GA10x display channel classes.
//!
//! Display channels are driven through push buffers of methods: a method is the byte offset of
//! a field of the channel's class, followed by its value.

use super::bindings;

/// Methods of the GA10x core channel (`NVC67D_CORE_CHANNEL_DMA`, `clc67d.h`).
pub(crate) mod core_c67d {
    use super::bindings;

    /// Sets the context DMA of the notifier written on the next update.
    pub(crate) const SET_CONTEXT_DMA_NOTIFIER: u32 = bindings::NVC67D_SET_CONTEXT_DMA_NOTIFIER;
}

/// Methods of the GA10x window channel (`NVC67E_WINDOW_CHANNEL_DMA`, `clc67e.h`).
pub(crate) mod window_c67e {
    use super::bindings;

    /// Makes the methods sent since the last update take effect.
    pub(crate) const UPDATE: u32 = bindings::NVC67E_UPDATE;
    /// Size of the surface: width in bits 15:0, height in bits 31:16.
    pub(crate) const SET_SIZE: u32 = bindings::NVC67E_SET_SIZE;
    /// Block height of block-linear surfaces, 0 for pitch-linear ones.
    pub(crate) const SET_STORAGE: u32 = bindings::NVC67E_SET_STORAGE;
    /// Parameters of the surface: its pixel format in bits 7:0.
    pub(crate) const SET_PARAMS: u32 = bindings::NVC67E_SET_PARAMS;
    /// The X8R8G8B8 pixel format for [`SET_PARAMS`].
    pub(crate) const FORMAT_X8R8G8B8: u32 = bindings::NVC67E_SET_PARAMS_FORMAT_X8R8G8B8;
    /// Size of the part of the surface that is shown, in the same layout as [`SET_SIZE`].
    pub(crate) const SET_SIZE_IN: u32 = bindings::NVC67E_SET_SIZE_IN;
    /// Size the window is shown at, in the same layout as [`SET_SIZE`].
    pub(crate) const SET_SIZE_OUT: u32 = bindings::NVC67E_SET_SIZE_OUT;
    /// Composition: color key selection in bits 1:0, depth of the window in bits 11:4, bypass in
    /// bit 16.
    pub(crate) const SET_COMPOSITION_CONTROL: u32 = bindings::NVC67E_SET_COMPOSITION_CONTROL;
    /// Bypass of composition for [`SET_COMPOSITION_CONTROL`]: the window is shown as it is,
    /// without blending, keying or input LUT.
    pub(crate) const COMPOSITION_BYPASS: u32 =
        bindings::NVC67E_SET_COMPOSITION_CONTROL_BYPASS_ENABLE << 16;
    /// Presentation: minimum interval in bits 3:0, begin mode (0: without tearing) in bits 6:4.
    pub(crate) const SET_PRESENT_CONTROL: u32 = bindings::NVC67E_SET_PRESENT_CONTROL;
    /// Channels and cursors this window's updates wait for.
    pub(crate) const SET_INTERLOCK_FLAGS: u32 = bindings::NVC67E_SET_INTERLOCK_FLAGS;
    /// Windows this window's updates wait for.
    pub(crate) const SET_WINDOW_INTERLOCK_FLAGS: u32 = bindings::NVC67E_SET_WINDOW_INTERLOCK_FLAGS;
    /// First of the 12 coefficients of the format conversion matrix, in S5.16 fixed point.
    pub(crate) const SET_FMT_COEFFICIENT_C00: u32 = bindings::NVC67E_SET_FMT_COEFFICIENT_C00;
    /// Context DMA of the input LUT, 0 for none.
    pub(crate) const SET_CONTEXT_DMA_ILUT: u32 = bindings::NVC67E_SET_CONTEXT_DMA_ILUT;

    // The following methods take the index of a surface plane: bindgen can't translate their
    // function-like macros, so they are defined here, after `clc67e.h`.

    /// Pitch of plane `plane` of the surface, in units of 64 bytes (`NVC67E_SET_PLANAR_STORAGE`).
    pub(crate) const fn set_planar_storage(plane: u32) -> u32 {
        0x230 + plane * 4
    }

    /// Context DMA of plane `plane` of the surface (`NVC67E_SET_CONTEXT_DMA_ISO`).
    pub(crate) const fn set_context_dma_iso(plane: u32) -> u32 {
        0x240 + plane * 4
    }

    /// Offset of plane `plane` of the surface in its context DMA, in units of 256 bytes
    /// (`NVC67E_SET_OFFSET`).
    pub(crate) const fn set_offset(plane: u32) -> u32 {
        0x260 + plane * 4
    }

    /// Point of plane `plane` of the surface shown at the window's origin: X in bits 15:0, Y in
    /// bits 31:16 (`NVC67E_SET_POINT_IN`).
    pub(crate) const fn set_point_in(plane: u32) -> u32 {
        0x290 + plane * 4
    }
}

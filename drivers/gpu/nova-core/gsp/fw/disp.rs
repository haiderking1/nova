// SPDX-License-Identifier: GPL-2.0

//! Methods of the GA10x display channel classes.
//!
//! Display channels are driven through push buffers of methods: a method is the byte offset of
//! a field of the channel's class, followed by its value.

use super::bindings;

/// Notifiers, which display channels write to report the progress of an update
/// (`NV_DISP_NOTIFIER` in `clc67d.h`).
pub(crate) mod notifier {
    use super::bindings;

    /// Size of a notifier in bytes. Its first word holds its status in bits 31:30.
    pub(crate) const SIZE: u32 = bindings::NV_DISP_NOTIFIER_SIZEOF;
    /// Status of an update that hasn't begun yet, as set before the update.
    pub(crate) const STATUS_NOT_BEGUN: u32 = bindings::NV_DISP_NOTIFIER__0_STATUS_NOT_BEGUN;
    /// Status of an update that began taking effect.
    pub(crate) const STATUS_BEGUN: u32 = bindings::NV_DISP_NOTIFIER__0_STATUS_BEGUN;
    /// Status of an update that finished.
    pub(crate) const STATUS_FINISHED: u32 = bindings::NV_DISP_NOTIFIER__0_STATUS_FINISHED;
}

/// Methods of the GA10x core channel (`NVC67D_CORE_CHANNEL_DMA`, `clc67d.h`).
pub(crate) mod core_c67d {
    use super::bindings;

    /// Sets the context DMA of the notifier written on the next update.
    pub(crate) const SET_CONTEXT_DMA_NOTIFIER: u32 = bindings::NVC67D_SET_CONTEXT_DMA_NOTIFIER;
    /// Cursor channels the next update waits for.
    pub(crate) const SET_INTERLOCK_FLAGS: u32 = bindings::NVC67D_SET_INTERLOCK_FLAGS;
    /// Window channels the next update waits for, one bit per window.
    pub(crate) const SET_WINDOW_INTERLOCK_FLAGS: u32 = bindings::NVC67D_SET_WINDOW_INTERLOCK_FLAGS;
    /// Makes the methods sent since the last update take effect.
    pub(crate) const UPDATE: u32 = bindings::NVC67D_UPDATE;
    /// [`UPDATE`] releasing the methods to the display engine.
    pub(crate) const UPDATE_RELEASE: u32 = bindings::NVC67D_UPDATE_RELEASE_ELV_TRUE;

    // The following methods take the index of a head: bindgen can't translate their
    // function-like macros, so they are defined here, after `clc67d.h`. Positions are X in bits
    // 14:0 and Y in bits 30:16.

    /// Output of head `head`: sync polarities in bits 2 (horizontal) and 3 (vertical), negative
    /// if set (`NVC67D_HEAD_SET_CONTROL_OUTPUT_RESOURCE`).
    pub(crate) const fn head_set_control_output_resource(head: u32) -> u32 {
        0x2004 + head * 0x400
    }

    /// Pixel clock of head `head`, in Hz in bits 30:0 (`NVC67D_HEAD_SET_PIXEL_CLOCK_FREQUENCY`).
    pub(crate) const fn head_set_pixel_clock_frequency(head: u32) -> u32 {
        0x200c + head * 0x400
    }

    /// Control of SOR `sor`: the heads it is driven by in bits 7:0, its protocol in bits 11:8
    /// (`NVC67D_SOR_SET_CONTROL`).
    pub(crate) const fn sor_set_control(sor: u32) -> u32 {
        0x300 + sor * 0x20
    }

    /// Number of SORs.
    pub(crate) const SORS: u32 = 4;

    /// RM display ID of the display head `head` drives (`NVC67D_HEAD_SET_DISPLAY_ID`, index 0),
    /// which GSP-RM programs the output of the head for.
    pub(crate) const fn head_set_display_id(head: u32) -> u32 {
        0x2020 + head * 0x400
    }

    /// Pixel clock configuration of head `head` (`NVC67D_HEAD_SET_PIXEL_CLOCK_CONFIGURATION`).
    pub(crate) const fn head_set_pixel_clock_configuration(head: u32) -> u32 {
        0x201c + head * 0x400
    }

    /// Highest pixel clock of head `head`, in Hz in bits 30:0
    /// (`NVC67D_HEAD_SET_PIXEL_CLOCK_FREQUENCY_MAX`).
    pub(crate) const fn head_set_pixel_clock_frequency_max(head: u32) -> u32 {
        0x2028 + head * 0x400
    }

    /// Total width and height of the raster of head `head` (`NVC67D_HEAD_SET_RASTER_SIZE`).
    pub(crate) const fn head_set_raster_size(head: u32) -> u32 {
        0x2064 + head * 0x400
    }

    /// End of the sync pulse of head `head` (`NVC67D_HEAD_SET_RASTER_SYNC_END`).
    pub(crate) const fn head_set_raster_sync_end(head: u32) -> u32 {
        0x2068 + head * 0x400
    }

    /// End of blanking of head `head` (`NVC67D_HEAD_SET_RASTER_BLANK_END`).
    pub(crate) const fn head_set_raster_blank_end(head: u32) -> u32 {
        0x206c + head * 0x400
    }

    /// Start of blanking of head `head` (`NVC67D_HEAD_SET_RASTER_BLANK_START`).
    pub(crate) const fn head_set_raster_blank_start(head: u32) -> u32 {
        0x2070 + head * 0x400
    }

    /// Start and end of the second vertical blanking of interlaced rasters of head `head`, in bits
    /// 14:0 and 30:16. The method follows [`head_set_raster_blank_start`]; `clc67d.h` doesn't name
    /// it, nouveau writes it raw.
    pub(crate) const fn head_set_raster_vert_blank2(head: u32) -> u32 {
        0x2074 + head * 0x400
    }
}

/// Methods of the GA10x window channel (`NVC67E_WINDOW_CHANNEL_DMA`, `clc67e.h`).
pub(crate) mod window_c67e {
    use super::bindings;

    /// Makes the methods sent since the last update take effect.
    pub(crate) const UPDATE: u32 = bindings::NVC67E_UPDATE;
    /// Sets the context DMA of the notifier written by the next update, 0 for none.
    pub(crate) const SET_CONTEXT_DMA_NOTIFIER: u32 = bindings::NVC67E_SET_CONTEXT_DMA_NOTIFIER;
    /// Notifier of the next update: mode in bit 0, offset in its context DMA in units of 16 bytes
    /// in bits 11:4.
    pub(crate) const SET_NOTIFIER_CONTROL: u32 = bindings::NVC67E_SET_NOTIFIER_CONTROL;
    /// Notifier mode: the notifier is written, without raising an interrupt.
    pub(crate) const NOTIFIER_MODE_WRITE: u32 = bindings::NVC67E_SET_NOTIFIER_CONTROL_MODE_WRITE;
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

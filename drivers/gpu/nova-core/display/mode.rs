// SPDX-License-Identifier: GPL-2.0

//! Display modes, and the raster timings heads scan them out with.

use kernel::prelude::*;

/// Timings of a progressive display mode, as DRM defines them: positions from the start of the
/// active area, in pixels horizontally and lines vertically.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModeTimings {
    /// Pixel clock in kHz.
    pub clock_khz: u32,
    /// Whether the horizontal sync pulse is negative.
    pub hsync_negative: bool,
    /// Whether the vertical sync pulse is negative.
    pub vsync_negative: bool,
    /// Width of the active area.
    pub hdisplay: u16,
    /// First pixel of the horizontal sync pulse.
    pub hsync_start: u16,
    /// Pixel after the horizontal sync pulse.
    pub hsync_end: u16,
    /// Length of a line.
    pub htotal: u16,
    /// Height of the active area.
    pub vdisplay: u16,
    /// First line of the vertical sync pulse.
    pub vsync_start: u16,
    /// Line after the vertical sync pulse.
    pub vsync_end: u16,
    /// Length of a frame, in lines.
    pub vtotal: u16,
}

/// The raster of a head: the same timings as positions from the start of the sync pulse, plus one,
/// as the display engine takes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HeadRaster {
    /// Total width and height: the length of a line in pixels, and of a frame in lines.
    pub(crate) total: (u16, u16),
    /// Last pixel and line of the sync pulse.
    pub(crate) sync_end: (u16, u16),
    /// Last pixel and line of blanking, before the active area.
    pub(crate) blank_end: (u16, u16),
    /// Last pixel and line of the active area, after which blanking starts.
    pub(crate) blank_start: (u16, u16),
    /// Pixel clock in Hz.
    pub(crate) pixel_clock: u32,
    /// Whether the horizontal and vertical sync pulses are negative.
    pub(crate) sync_negative: (bool, bool),
}

/// Largest value of a raster position (15-bit fields).
const MAX_POSITION: u16 = 0x7fff;

impl HeadRaster {
    /// Returns the raster of `mode`, computed like nouveau does (`nv50_head_atomic_check_mode()`).
    pub(crate) fn new(mode: &ModeTimings) -> Result<Self> {
        let ordered = |display: u16, sync_start: u16, sync_end: u16, total: u16| {
            0 < display && display <= sync_start && sync_start < sync_end && sync_end <= total
        };
        if !ordered(mode.hdisplay, mode.hsync_start, mode.hsync_end, mode.htotal)
            || !ordered(mode.vdisplay, mode.vsync_start, mode.vsync_end, mode.vtotal)
            || mode.htotal > MAX_POSITION
            || mode.vtotal > MAX_POSITION
        {
            return Err(EINVAL);
        }

        // Blanking ends with the total, which the raster starts one pixel into the sync pulse.
        let h_blank_end = mode.htotal - mode.hsync_start - 1;
        let v_blank_end = mode.vtotal - mode.vsync_start - 1;

        Ok(Self {
            total: (mode.htotal, mode.vtotal),
            sync_end: (
                mode.hsync_end - mode.hsync_start - 1,
                mode.vsync_end - mode.vsync_start - 1,
            ),
            blank_end: (h_blank_end, v_blank_end),
            blank_start: (h_blank_end + mode.hdisplay, v_blank_end + mode.vdisplay),
            pixel_clock: mode.clock_khz.checked_mul(1000).ok_or(EINVAL)?,
            sync_negative: (mode.hsync_negative, mode.vsync_negative),
        })
    }
}

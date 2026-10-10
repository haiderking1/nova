// SPDX-License-Identifier: GPL-2.0

//! What a DisplayPort link of the display engine can carry.

use crate::gsp::DpLinkConfig;

use super::mode::ModeTimings;

/// What an output sending a single stream over a DisplayPort link needs of it.
pub(crate) struct SstOutput {
    /// Bits per pixel the head sends.
    pub(crate) bpp: u32,
    /// Whether the link uses enhanced framing.
    pub(crate) enhanced_framing: bool,
    /// Whether the outputs have the higher watermark limits of newer GPUs.
    pub(crate) increased_watermark_limits: bool,
}

/// Fixed-point precision of the computations, as in NVIDIA's driver.
const PRECISION: u64 = 100_000;
/// Symbols of a transfer unit.
const TU_SIZE: u64 = 64;
/// Highest watermark of the outputs.
const MAX_WATERMARK: u64 = 39;

/// Returns whether `link` carries `mode` from `output` as a single stream, with 8b/10b channel
/// coding and without DSC.
///
/// This is `isModePossibleSST()` of NVIDIA's DisplayPort library (`dp_watermark.cpp`): beyond the
/// bandwidth, the watermark of the output's FIFO and the horizontal blanking, which carries the
/// framing symbols, must fit.
pub(crate) fn sst_mode_fits(link: &DpLinkConfig, output: &SstOutput, mode: &ModeTimings) -> bool {
    let lanes = u64::from(link.lanes);
    if !matches!(lanes, 1 | 2 | 4) || output.bpp == 0 {
        return false;
    }
    let bpp = u64::from(output.bpp);
    let pclk = u64::from(mode.clock_khz) * 1000;
    let hdisplay = u64::from(mode.hdisplay);
    let htotal = u64::from(mode.htotal);

    // Bytes each lane carries a second: 8 data bits for each 10 sent, at the lane's rate less the
    // 0.6% the spectrum's downspread may take.
    let rate = u64::from(link.link_bw) * 270_000_000;
    let min_rate = rate - 6 * rate / 1000;
    let lane_data_rate = min_rate * 8 / 10 / 8;
    if lane_data_rate == 0 {
        return false;
    }

    let capacity = 8 * lane_data_rate * lanes;
    if pclk * bpp >= capacity {
        return false;
    }

    // The watermark of the output's FIFO. NVIDIA's driver raises it to a minimum too, which
    // doesn't decide whether the mode fits.
    let adjust = if output.increased_watermark_limits {
        8
    } else {
        2
    };
    let ratio = pclk * bpp * PRECISION / capacity;
    let watermark_f = ratio * TU_SIZE * (PRECISION - ratio) / PRECISION;
    let watermark = adjust + (2 * (bpp * PRECISION / (8 * lanes)) + watermark_f) / PRECISION;
    let symbols_per_line = hdisplay * bpp / (8 * lanes);
    if watermark > MAX_WATERMARK || watermark > symbols_per_line {
        return false;
    }

    // The horizontal blanking must hold the blanking start and end symbols, doubled with enhanced
    // framing, VB-ID, Mvid and Maud four times, and the padding of the last pixels over the lanes.
    let framing = 3 * 8 * lanes;
    let mut blanking_bits = framing + if output.enhanced_framing { framing } else { 0 };
    blanking_bits += 3 * 8 * 4;
    if hdisplay % lanes != 0 {
        blanking_bits += (lanes - hdisplay % lanes) * bpp;
    }
    let blanking_link_clocks = blanking_bits * PRECISION / (8 * lanes);
    let min_hblank = blanking_link_clocks * pclk / lane_data_rate / PRECISION + 12;
    if min_hblank > htotal.saturating_sub(hdisplay) {
        return false;
    }

    // Too narrow active areas aren't supported.
    hdisplay > 60
}

// SPDX-License-Identifier: GPL-2.0

//! The DisplayPort link of the boot display: its AUX channel, what its sink and LTTPRs can do, and
//! training it for modes it doesn't carry as it is.

use kernel::{
    auxiliary,
    device::Bound,
    drm::dp::{
        self,
        Aux,
        AuxMsg,
        AuxTransfer,
        DpcdCaps, //
    },
    prelude::*,
};

use nova_core::api::{
    DpLinkConfig,
    DpTraining,
    ModeTimings,
    NovaCoreApi,
    NovaCoreApiHandle, //
};

/// Link rates of DisplayPort 1.4 with 8b/10b channel coding, slowest first, in units of 10 kb/s
/// per lane, as DRM's helpers have them: RBR, HBR, HBR2 and HBR3.
const LINK_RATES: [u32; 4] = [162_000, 270_000, 540_000, 810_000];
/// Numbers of lanes a link may use.
const LANE_COUNTS: [u32; 3] = [1, 2, 4];

/// The transfers on the AUX channel of the boot display, which nova-core carries out.
pub(crate) struct NovaAux<'a> {
    api: NovaCoreApiHandle<'a>,
}

impl AuxTransfer for NovaAux<'_> {
    fn transfer(&self, msg: &mut AuxMsg<'_>) -> Result<usize> {
        let (request, addr) = (msg.request(), msg.address());
        let (reply, len) = self
            .api
            .with(|api| api.boot_display_dp_aux(request, addr, msg.buffer_mut()))?;
        msg.set_reply(reply);

        Ok(len)
    }
}

/// The DisplayPort link of the boot display.
pub(crate) struct BootDp<'a> {
    aux: Pin<KBox<Aux<NovaAux<'a>>>>,
    /// What the sink can do.
    dpcd: DpcdCaps,
    /// Number of LTTPRs in non-transparent mode, each trained on its own.
    lttprs: u32,
    /// The configurations the source, the LTTPRs and the sink allow the link, slowest first.
    configs: KVec<DpLinkConfig>,
}

impl<'a> BootDp<'a> {
    /// Finds out what the link of the boot display, shown through nova-core on `adev`, may be
    /// trained to.
    pub(crate) fn probe(adev: &'a auxiliary::Device<Bound>) -> Result<Self> {
        let api = NovaCoreApi::of(adev)?;
        let source_max = api.with(|api| api.boot_display_dp_max_link())?;
        let aux = KBox::pin_init(
            Aux::new(adev.as_ref(), c"nova-drm DP AUX", NovaAux { api }),
            GFP_KERNEL,
        )?;

        // Like nouveau: LTTPRs, if the link has any, are put in non-transparent mode before
        // anything else is read from the sink, whose capabilities they may change.
        let mut lttpr_caps = None;
        let mut rev = [0];
        if aux
            .dpcd_read(dp::dpcd::LTTPR_FIELD_DATA_STRUCTURE_REV, &mut rev)
            .is_ok()
            && rev[0] >= 0x14
        {
            let dpcd = aux.read_dpcd_caps()?;
            let caps = aux.read_lttpr_common_caps(&dpcd)?;
            match caps.count() {
                Ok(0) => {}
                Ok(count) => match aux.lttpr_init(count) {
                    Ok(()) => lttpr_caps = Some((count, caps)),
                    Err(err) => pr_warn!("DP: {} LTTPRs stay transparent: {:?}\n", count, err),
                },
                Err(err) => pr_warn!("DP: invalid LTTPR count: {:?}\n", err),
            }
        }
        let dpcd = aux.read_dpcd_caps()?;

        let mut max_rate = dpcd
            .max_link_rate()
            .min(dp::bw_code_to_link_rate(source_max.link_bw.try_into()?));
        let mut max_lanes = u32::from(dpcd.max_lane_count()).min(source_max.lanes);
        if let Some((_, caps)) = lttpr_caps {
            if caps.max_link_rate() != 0 {
                max_rate = max_rate.min(caps.max_link_rate());
            }
            if caps.max_lane_count() != 0 {
                max_lanes = max_lanes.min(caps.max_lane_count().into());
            }
        }

        // Like NVIDIA's driver, the configurations are sorted by bandwidth, with fewer lanes first
        // for the same bandwidth (`dp_linkconfig.h`).
        let mut sorted = KVec::new();
        for lanes in LANE_COUNTS.into_iter().filter(|&lanes| lanes <= max_lanes) {
            for rate in LINK_RATES.into_iter().filter(|&rate| rate <= max_rate) {
                sorted.push((lanes * rate, lanes, rate), GFP_KERNEL)?;
            }
        }
        sorted.sort_unstable();
        let mut configs = KVec::with_capacity(sorted.len(), GFP_KERNEL)?;
        for (_, lanes, rate) in sorted {
            configs.push(
                DpLinkConfig {
                    lanes,
                    link_bw: dp::link_rate_to_bw_code(rate).into(),
                },
                GFP_KERNEL,
            )?;
        }
        if configs.is_empty() {
            return Err(ENODEV);
        }

        let lttprs = lttpr_caps.map_or(0, |(count, _)| count);
        pr_info!(
            "DP: sink DPCD {:#04x}, up to {} lanes at {} kb/s, {} LTTPRs\n",
            dpcd.revision(),
            max_lanes,
            u64::from(max_rate) * 10,
            lttprs
        );

        Ok(Self {
            aux,
            dpcd,
            lttprs,
            configs,
        })
    }

    /// Returns whether some configuration of the link carries `mode` from head `head`.
    pub(crate) fn carries(&self, api: &NovaCoreApi<'_>, head: u32, mode: &ModeTimings) -> bool {
        self.configs
            .iter()
            .any(|link| api.dp_mode_fits(head, link, self.dpcd.enhanced_frame_cap(), mode))
    }

    /// Readies the link for head `head` to show `mode`: like NVIDIA's driver, has the link run at
    /// the slowest configuration that carries the mode. If it doesn't yet, detaches the head and
    /// trains the link to it, or to the next faster ones if that fails. The next modeset of the
    /// head attaches it again.
    ///
    /// Fails with `EIO` if no configuration trains, leaving the head detached.
    pub(crate) fn ready_for(&self, api: &NovaCoreApi<'_>, head: u32, mode: &ModeTimings) -> Result {
        let enhanced_framing = self.dpcd.enhanced_frame_cap();
        let mut fitting = self
            .configs
            .iter()
            .filter(|link| api.dp_mode_fits(head, link, enhanced_framing, mode))
            .peekable();
        let current = api.boot_display_dp_link()?;
        if fitting.peek() == Some(&&current) {
            return Ok(());
        }

        // Like NVIDIA's driver, the link isn't trained while it carries a stream.
        api.detach_head(head)?;
        // Like nouveau, the sink is woken up first, in case it sleeps.
        if let Err(err) = self
            .aux
            .dpcd_write(dp::dpcd::SET_POWER, &[dp::dpcd::SET_POWER_D0])
        {
            pr_warn!("DP: can't wake the sink up: {:?}\n", err);
        }

        for link in fitting {
            match self.train(api, link) {
                Ok(()) => return Ok(()),
                Err(err) => pr_warn!("DP: training to {:?} failed: {:?}\n", link, err),
            }
        }

        pr_err!("DP: no link configuration carrying the mode trains\n");
        Err(EIO)
    }

    /// Trains the link to `link`: each LTTPR, from the one closest to the source, then the sink,
    /// as NVIDIA's driver and nouveau do.
    fn train(&self, api: &NovaCoreApi<'_>, link: &DpLinkConfig) -> Result {
        for target in (0..=self.lttprs).rev() {
            api.train_boot_display_dp_link(&DpTraining {
                link: *link,
                target,
                enhanced_framing: self.dpcd.enhanced_frame_cap(),
                // As nouveau grants it: to the sink, if it can't use training pattern 4.
                post_lt_adj_req: target == 0
                    && self.dpcd.post_lt_adj_req_supported()
                    && !self.dpcd.tps4_supported(),
                lttprs: self.lttprs != 0,
            })?;
        }

        Ok(())
    }
}

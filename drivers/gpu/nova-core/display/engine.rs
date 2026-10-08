// SPDX-License-Identifier: GPL-2.0

//! Taking over the display engine from GSP-RM.
//!
//! Nova plays the part of the CPU-side RM of NVIDIA's driver: it provides the display engine's
//! instance memory and the push buffers of its channels, has GSP-RM allocate the channels, and
//! then drives them directly.

use core::ops::Range;

use kernel::{
    device,
    dma::Coherent,
    io::{
        io_read,
        io_write,
        poll::read_poll_timeout,
        register::Array,
        Io, //
    },
    prelude::*,
    time::Delta, //
};

use crate::{
    driver::Bar0,
    gpu::Chipset,
    gsp::{
        cmdq::Cmdq,
        commands::{
            Client,
            DisplayCoreChannel,
            DisplayRoot,
            DisplayWindowChannel,
            Handle,
            Subdevice, //
        },
        disp::{
            core_c67d,
            notifier,
            window_c67e, //
        },
        rm::commands::{
            AllocCoreChannelGa102,
            AllocDisplayGa102,
            AllocWindowChannelGa102,
            DispChannelPushbuffer,
            DispGetStaticInfo,
            DispWriteInstMem,
            RmAlloc,
            RmControl,
            RmFree, //
        },
    },
    mm::{
        GpuMm,
        VramAddress, //
    },
    num, //
};

use super::{
    channel::{
        DmaChannel,
        PushBuffer, //
    },
    instmem::InstMem,
    regs,
    Display, //
};

// Handles of the RM objects of the display engine, picked like nouveau's.

/// Handle of the display object.
const DISPLAY_HANDLE: u32 = 0xc670_0000;
/// Handle of the core channel.
const CORE_CHANNEL_HANDLE: u32 = 0xc67d_0000;
/// Handle of the channel of window 0; those of the other windows follow.
const WINDOW_CHANNEL_HANDLE: u32 = 0xc67e_0000;

/// Handle of the context DMA giving the window channels access to all of VRAM, for surfaces.
const VRAM_CTXDMA: u32 = 0xfb00_0000;
/// Handle of the context DMA of the window's notifier, after nouveau's.
const NOTIFIER_CTXDMA: u32 = 0xf000_0000;

/// Number of 32-bit words of the window's notifier buffer: a context DMA covers whole 256-byte
/// blocks.
const NOTIFIER_WORDS: usize = 256 / size_of::<u32>();
static_assert!(num::u32_as_usize(notifier::SIZE) <= NOTIFIER_WORDS * size_of::<u32>());

/// How long an update may take to begin: a frame, even at the lowest refresh rates, with margin.
const UPDATE_TIMEOUT: Delta = Delta::from_millis(500);

/// The window Nova shows surfaces on: window 0, which belongs to head 0.
const WINDOW: usize = 0;

/// GSP-RM's own client and subdevice, the targets of its internal controls.
pub(crate) struct InternalHandles {
    pub(crate) client: Handle<Client>,
    pub(crate) subdevice: Handle<Subdevice>,
}

/// A pitch-linear XRGB8888 surface in VRAM, for a window to show.
#[derive(Clone, Debug)]
pub(crate) struct Surface {
    /// Address of the first pixel, which must be 256-byte aligned.
    pub(crate) addr: VramAddress,
    /// Width in pixels.
    pub(crate) width: u32,
    /// Height in pixels.
    pub(crate) height: u32,
    /// Distance between the starts of two lines, in bytes: a multiple of 64.
    pub(crate) pitch: u32,
}

impl Surface {
    /// Checks that a window can show the surface.
    pub(crate) fn check(&self) -> Result {
        let fits = |len: u32| len > 0 && len <= 0xffff;
        if self.addr.into_raw() % 256 != 0
            || self.pitch % 64 != 0
            || self.pitch / 4 < self.width
            || !fits(self.width)
            || !fits(self.height)
        {
            return Err(EINVAL);
        }

        Ok(())
    }
}

/// The display engine, taken over from GSP-RM.
pub(crate) struct DisplayEngine<'a> {
    /// Instance memory. GSP-RM uses it from when it's set on, so it must outlive the channels and
    /// stay allocated as long as the GSP runs.
    _instmem: InstMem,
    /// Mask of the windows of the display engine.
    windows: u32,
    /// The display object and its channels, if they could be allocated.
    channels: Option<Channels<'a>>,
}

/// The display object and its channels.
struct Channels<'a> {
    root: Handle<DisplayRoot>,
    _core: Handle<DisplayCoreChannel>,
    core_push: PushBuffer<'a>,
    _window: Handle<DisplayWindowChannel>,
    window_push: PushBuffer<'a>,
    /// Notifier of the window's updates, at the start of the buffer.
    notifier: Coherent<'a, [u32; NOTIFIER_WORDS]>,
}

impl<'a> DisplayEngine<'a> {
    /// Takes over the display engine, whose RM objects are allocated under `display`. `vram` is
    /// all of VRAM, which surfaces are taken from.
    ///
    /// Only GA10x display engines are supported so far.
    #[expect(clippy::too_many_arguments)]
    pub(crate) fn new(
        dev: &'a device::Device<device::Bound>,
        bar: Bar0<'_>,
        cmdq: &Cmdq<'_>,
        mm: &mut GpuMm<'_>,
        chipset: Chipset,
        display: &Display,
        internal: &InternalHandles,
        vram: Range<VramAddress>,
    ) -> Result<Self> {
        if !matches!(
            chipset,
            Chipset::GA102 | Chipset::GA103 | Chipset::GA104 | Chipset::GA106 | Chipset::GA107
        ) {
            return Err(ENOTSUPP);
        }

        let mut instmem = InstMem::new(mm)?;
        RmControl::new(
            internal.client,
            internal.subdevice,
            DispWriteInstMem::new(instmem.range()),
        )
        .send(dev, cmdq)?;

        let windows = RmControl::new(
            internal.client,
            internal.subdevice,
            DispGetStaticInfo::new(),
        )
        .send(dev, cmdq)?;
        dev_dbg!(dev, "display: windows {:#x}\n", windows);
        if windows & (1 << WINDOW) == 0 {
            return Err(ENODEV);
        }

        log_armed_state(dev, bar);

        // From here on GSP-RM may use the instance memory, which must thus not be freed before the
        // GSP is unloaded, even if the channels can't be set up.
        let channels = Channels::new(dev, bar, cmdq, mm, &mut instmem, display, internal, vram)
            .inspect_err(|e| dev_err!(dev, "display: can't set up the channels: {:?}\n", e))
            .ok();

        Ok(Self {
            _instmem: instmem,
            windows,
            channels,
        })
    }

    /// Frees the RM objects of the display engine. The instance memory is freed when `self` is
    /// dropped.
    pub(crate) fn free(&mut self, dev: &device::Device, cmdq: &Cmdq<'_>, display: &Display) {
        if let Some(channels) = self.channels.take() {
            // Freeing the display object frees its channels; their push buffers are freed
            // afterwards, when `channels` is dropped.
            let _ = RmFree::new(display.client(), channels.root)
                .send(dev, cmdq)
                .inspect_err(|e| dev_err!(dev, "display: can't free the channels: {:?}\n", e));
        }
    }

    /// Returns the mask of the windows of the display engine.
    pub(crate) fn windows(&self) -> u32 {
        self.windows
    }

    /// Shows `surface` on head 0, at the head's current mode, from the next frame on.
    ///
    /// Fails with `ENODEV` if the channels couldn't be set up.
    pub(crate) fn show(&mut self, bar: Bar0<'_>, surface: &Surface) -> Result {
        let channels = self.channels.as_mut().ok_or(ENODEV)?;
        channels.show(bar, surface)
    }
}

impl<'a> Channels<'a> {
    #[expect(clippy::too_many_arguments)]
    fn new(
        dev: &'a device::Device<device::Bound>,
        bar: Bar0<'_>,
        cmdq: &Cmdq<'_>,
        mm: &mut GpuMm<'_>,
        instmem: &mut InstMem,
        display: &Display,
        internal: &InternalHandles,
        vram: Range<VramAddress>,
    ) -> Result<Self> {
        let client = display.client();
        let root = RmAlloc::new(
            client,
            display.device(),
            Handle::new(DISPLAY_HANDLE),
            AllocDisplayGa102,
        )
        .send(dev, cmdq)?;

        let setup = || -> Result<Self> {
            let core_push = PushBuffer::new(dev, DmaChannel::Core)?;
            // The push buffer of a channel is set before the channel is allocated.
            RmControl::new(
                internal.client,
                internal.subdevice,
                DispChannelPushbuffer::core_ga102(core_push.dma_address()),
            )
            .send(dev, cmdq)?;
            let core = RmAlloc::new(
                client,
                root,
                Handle::new(CORE_CHANNEL_HANDLE),
                AllocCoreChannelGa102::new(0),
            )
            .send(dev, cmdq)?;

            // CAST: `WINDOW` is a window index, below 32.
            let window_index = WINDOW as u32;
            let window_push = PushBuffer::new(dev, DmaChannel::Window(WINDOW))?;
            RmControl::new(
                internal.client,
                internal.subdevice,
                DispChannelPushbuffer::window_ga102(window_index, window_push.dma_address()),
            )
            .send(dev, cmdq)?;
            let window = RmAlloc::new(
                client,
                root,
                Handle::new(WINDOW_CHANNEL_HANDLE | window_index),
                AllocWindowChannelGa102::new(window_index, 0),
            )
            .send(dev, cmdq)?;

            // Surfaces are taken from anywhere in VRAM.
            instmem.bind_vram(mm, client, DmaChannel::Window(WINDOW), VRAM_CTXDMA, vram)?;

            let notifier = Coherent::<[u32; NOTIFIER_WORDS]>::zeroed(dev, GFP_KERNEL)?;
            let start = notifier.dma_address();
            let end = start + u64::try_from(NOTIFIER_WORDS * size_of::<u32>())?;
            instmem.bind_coherent(
                mm,
                client,
                DmaChannel::Window(WINDOW),
                NOTIFIER_CTXDMA,
                start..end,
            )?;

            let mut channels = Self {
                root,
                _core: core,
                core_push,
                _window: window,
                window_push,
                notifier,
            };
            channels.check_core(dev, bar)?;
            Ok(channels)
        };

        setup().inspect_err(|_| {
            // Freeing the display object frees the channels allocated under it.
            let _ = RmFree::new(client, root).send(dev, cmdq);
        })
    }

    /// Checks that the core channel reads its push buffer, with a method that changes nothing on
    /// screen: it only takes effect on the next update of the core channel, which isn't sent.
    fn check_core(&mut self, dev: &device::Device, bar: Bar0<'_>) -> Result {
        dev_dbg!(
            dev,
            "display: core channel GET {:#x}\n",
            self.core_push.get(bar)
        );

        // No notifier for the next update.
        self.core_push
            .push(bar, core_c67d::SET_CONTEXT_DMA_NOTIFIER, &[0])?;
        self.core_push.kick(bar)?;
        self.core_push
            .wait_idle(bar)
            .inspect_err(|_| dev_err!(dev, "display: the core channel doesn't read methods\n"))?;

        dev_info!(dev, "display: core channel up\n");
        Ok(())
    }

    /// Points the window at `surface`, shown 1:1, and updates it, returning once the update began
    /// taking effect: from then on, the surface shown before isn't read anymore.
    ///
    /// The window bypasses composition, as NVIDIA's driver does to show a console: it is shown as
    /// it is, which needs neither an input LUT on the window nor an output LUT on the head. The
    /// head keeps the mode and output it has; the update is not interlocked with any other
    /// channel, so it only changes the window.
    fn show(&mut self, bar: Bar0<'_>, surface: &Surface) -> Result {
        use window_c67e::*;

        surface.check()?;
        let size = surface.width | surface.height << 16;
        let offset = u32::try_from(surface.addr.into_raw() >> 8)?;

        /// Format conversion matrix leaving colors as they are, in S5.16 fixed point.
        const IDENTITY: [u32; 12] = [
            0x10000, 0, 0, 0, //
            0, 0x10000, 0, 0, //
            0, 0, 0x10000, 0,
        ];

        let methods = [
            // The update reports its progress in the notifier.
            (SET_CONTEXT_DMA_NOTIFIER, NOTIFIER_CTXDMA),
            (SET_NOTIFIER_CONTROL, NOTIFIER_MODE_WRITE),
            // At most one new surface per frame, switching between frames.
            (SET_PRESENT_CONTROL, 1),
            (SET_SIZE, size),
            // Pitch-linear.
            (SET_STORAGE, 0),
            (SET_PARAMS, FORMAT_X8R8G8B8),
            (set_planar_storage(0), surface.pitch >> 6),
            (set_context_dma_iso(0), VRAM_CTXDMA),
            (set_offset(0), offset),
            (set_point_in(0), 0),
            (SET_SIZE_IN, size),
            (SET_SIZE_OUT, size),
            // Shown as it is, without the input LUT that composition would need.
            (SET_CONTEXT_DMA_ILUT, 0),
            (SET_COMPOSITION_CONTROL, COMPOSITION_BYPASS),
            // The update doesn't wait for any other channel.
            (SET_INTERLOCK_FLAGS, 0),
            (SET_WINDOW_INTERLOCK_FLAGS, 0),
        ];

        let push = &mut self.window_push;
        for (method, value) in methods {
            push.push(bar, method, &[value])?;
        }
        push.push(bar, SET_FMT_COEFFICIENT_C00, &IDENTITY)?;
        push.push(bar, UPDATE, &[1])?;

        // The status is in bits 31:30 of the notifier's first word, the rest of which the update
        // writes.
        io_write!(self.notifier, [build: 0], notifier::STATUS_NOT_BEGUN << 30);
        push.kick(bar)?;
        push.wait_idle(bar)?;

        read_poll_timeout(
            || Ok(io_read!(self.notifier, [build: 0]) >> 30),
            |&status| matches!(status, notifier::STATUS_BEGUN | notifier::STATUS_FINISHED),
            Delta::from_micros(50),
            UPDATE_TIMEOUT,
        )
        .map(|_| ())
    }
}

/// Core channel methods of head 0 whose armed state is logged, with their names in `clc67d.h`.
const LOGGED_HEAD_METHODS: [(&str, usize); 26] = [
    ("PROCAMP", 0x2000),
    ("CONTROL_OUTPUT_RESOURCE", 0x2004),
    ("CONTROL", 0x2008),
    ("PIXEL_CLOCK_FREQUENCY", 0x200c),
    ("PIXEL_CLOCK_CONFIGURATION", 0x201c),
    ("DISPLAY_ID(0)", 0x2020),
    ("PIXEL_CLOCK_FREQUENCY_MAX", 0x2028),
    ("HEAD_USAGE_BOUNDS", 0x2030),
    ("VIEWPORT_POINT_IN", 0x2048),
    ("VIEWPORT_SIZE_IN", 0x204c),
    ("VIEWPORT_SIZE_OUT", 0x2058),
    ("RASTER_SIZE", 0x2064),
    ("RASTER_SYNC_END", 0x2068),
    ("RASTER_BLANK_END", 0x206c),
    ("RASTER_BLANK_START", 0x2070),
    ("0x2074", 0x2074),
    ("CONTEXT_DMA_CURSOR(0)", 0x2088),
    ("OFFSET_CURSOR(0)", 0x2090),
    ("CONTROL_CURSOR", 0x209c),
    ("PRESENT_CONTROL", 0x218c),
    ("DISPLAY_RATE", 0x21a8),
    ("OLUT_CONTROL", 0x2280),
    ("OLUT_FP_NORM_SCALE", 0x2284),
    ("CONTEXT_DMA_OLUT", 0x2288),
    ("OFFSET_OLUT", 0x228c),
    ("DSC_CONTROL", 0x22d4),
];

/// Returns the armed value of the core channel method at `method`.
fn armed(bar: Bar0<'_>, method: usize) -> Option<u32> {
    let reg = regs::NV_UDISP_CORE_ARMED::try_at(method / size_of::<u32>())?;
    Some(bar.read(reg).into_raw())
}

/// Logs the parts of the armed state of the core channel that taking over head 0 depends on.
fn log_armed_state(dev: &device::Device, bar: Bar0<'_>) {
    let log = |name: &str, method: usize| {
        if let Some(value) = armed(bar, method) {
            dev_dbg!(
                dev,
                "display: armed {} ({:#06x}) = {:#010x}\n",
                name,
                method,
                value
            );
        }
    };

    log("SET_CONTEXT_DMA_NOTIFIER", 0x208);
    for sor in 0..4 {
        log("SOR_SET_CONTROL", 0x300 + sor * 0x20);
    }
    for window in 0..8 {
        log("WINDOW_SET_CONTROL", 0x1000 + window * 0x80);
    }
    // What the window Nova uses is allowed to do.
    let window = WINDOW * 0x80;
    log("WINDOW_SET_WINDOW_FORMAT_USAGE_BOUNDS", 0x1004 + window);
    log("WINDOW_SET_WINDOW_ROTATED_FORMAT_USAGE_BOUNDS", 0x1008 + window);
    log("WINDOW_SET_MAX_INPUT_SCALE_FACTOR", 0x100c + window);
    log("WINDOW_SET_WINDOW_USAGE_BOUNDS", 0x1010 + window);
    for (name, method) in LOGGED_HEAD_METHODS {
        log(name, method);
    }
}

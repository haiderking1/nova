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
        disp::notifier,
        rm::commands::{
            DispGetStaticInfo,
            DispWriteInstMem,
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
    hal::{
        self,
        DisplayHal,
        WindowImage, //
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
/// Number of notifiers in the buffer that updates take turns using, like nouveau: the display
/// engine may still write the notifier of an update after the next one was sent.
const NOTIFIER_SLOTS: usize = 2;
/// Number of 32-bit words of a notifier.
const NOTIFIER_SLOT_WORDS: usize = num::u32_as_usize(notifier::SIZE) / size_of::<u32>();
static_assert!(NOTIFIER_SLOTS * NOTIFIER_SLOT_WORDS <= NOTIFIER_WORDS);

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
    dev: &'a device::Device<device::Bound>,
    hal: &'static dyn DisplayHal,
    root: Handle<DisplayRoot>,
    _core: Handle<DisplayCoreChannel>,
    core_push: PushBuffer<'a>,
    _window: Handle<DisplayWindowChannel>,
    window_push: PushBuffer<'a>,
    /// Notifiers of the window's updates, at the start of the buffer.
    notifier: Coherent<'a, [u32; NOTIFIER_WORDS]>,
    /// Notifier slot of the next update.
    notifier_slot: usize,
}

impl<'a> DisplayEngine<'a> {
    /// Takes over the display engine, whose RM objects are allocated under `display`. `vram` is
    /// all of VRAM, which surfaces are taken from.
    ///
    /// Fails with `ENOTSUPP` if Nova doesn't support the display engine of `chipset`.
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
        let hal = hal::display_hal(chipset).ok_or(ENOTSUPP)?;

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

        hal.log_armed_state(dev, bar, WINDOW);

        // From here on GSP-RM may use the instance memory, which must thus not be freed before the
        // GSP is unloaded, even if the channels can't be set up.
        let channels = Channels::new(
            dev,
            hal,
            bar,
            cmdq,
            mm,
            &mut instmem,
            display,
            internal,
            vram,
        )
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
        hal: &'static dyn DisplayHal,
        bar: Bar0<'_>,
        cmdq: &Cmdq<'_>,
        mm: &mut GpuMm<'_>,
        instmem: &mut InstMem,
        display: &Display,
        internal: &InternalHandles,
        vram: Range<VramAddress>,
    ) -> Result<Self> {
        let client = display.client();
        let root = hal.alloc_display(dev, cmdq, client, display.device(), DISPLAY_HANDLE)?;

        let setup = || -> Result<Self> {
            let core_push = PushBuffer::new(dev, DmaChannel::Core)?;
            let core = hal.alloc_core(
                dev,
                cmdq,
                internal,
                client,
                root,
                CORE_CHANNEL_HANDLE,
                &core_push,
            )?;

            // CAST: `WINDOW` is a window index, below 32.
            let window_index = WINDOW as u32;
            let window_push = PushBuffer::new(dev, DmaChannel::Window(WINDOW))?;
            let window = hal.alloc_window(
                dev,
                cmdq,
                internal,
                client,
                root,
                window_index,
                WINDOW_CHANNEL_HANDLE | window_index,
                &window_push,
            )?;

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
                dev,
                hal,
                root,
                _core: core,
                core_push,
                _window: window,
                window_push,
                notifier,
                notifier_slot: 0,
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

        self.hal.push_core_nop(bar, &mut self.core_push)?;
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
    /// The window bypasses composition, as NVIDIA's driver does to show a console. The head keeps
    /// the mode and output it has.
    fn show(&mut self, bar: Bar0<'_>, surface: &Surface) -> Result {
        surface.check()?;

        let slot = self.notifier_slot;
        let status_word = slot * NOTIFIER_SLOT_WORDS;
        // CAST: the slots are within the first 256 bytes of the buffer.
        let notifier_offset = (status_word * size_of::<u32>()) as u32;

        // The update reports its progress in the notifier of the slot.
        let push = &mut self.window_push;
        self.hal.push_window_update(
            bar,
            push,
            &WindowImage {
                surface,
                ctxdma: VRAM_CTXDMA,
                notifier_ctxdma: NOTIFIER_CTXDMA,
                notifier_offset,
            },
        )?;

        // The status is in bits 31:30 of the notifier's first word, the rest of which the update
        // writes.
        io_write!(self.notifier, [try: status_word], notifier::STATUS_NOT_BEGUN << 30);
        push.kick(bar)?;
        self.notifier_slot = (slot + 1) % NOTIFIER_SLOTS;
        push.wait_idle(bar)?;

        let notifier = &self.notifier;
        let read_status = || -> Result<u32> { Ok(io_read!(notifier, [try: status_word])) };
        read_poll_timeout(
            || Ok(read_status()? >> 30),
            |&status| matches!(status, notifier::STATUS_BEGUN | notifier::STATUS_FINISHED),
            Delta::from_micros(50),
            UPDATE_TIMEOUT,
        )
        .map(|_| ())
        .inspect_err(|_| {
            dev_err!(
                self.dev,
                "display: window update didn't begin, notifier {:#010x?}\n",
                read_status().ok()
            )
        })
    }
}

/// Returns the armed value of the core channel method at `method`.
pub(super) fn armed(bar: Bar0<'_>, method: usize) -> Option<u32> {
    let reg = regs::NV_UDISP_CORE_ARMED::try_at(method / size_of::<u32>())?;
    Some(bar.read(reg).into_raw())
}

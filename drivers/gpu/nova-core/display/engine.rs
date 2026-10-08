// SPDX-License-Identifier: GPL-2.0

//! Taking over the display engine from GSP-RM.
//!
//! Nova plays the part of the CPU-side RM of NVIDIA's driver: it provides the display engine's
//! instance memory and the push buffers of its channels, has GSP-RM allocate the channels, and
//! then drives them directly.

use kernel::{
    device,
    prelude::*,
    ptr::Alignment,
    sizes::SZ_64K, //
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
            Handle,
            Subdevice, //
        },
        rm::commands::{
            AllocCoreChannelGa102,
            AllocDisplayGa102,
            DispChannelPushbuffer,
            DispGetStaticInfo,
            DispWriteInstMem,
            RmAlloc,
            RmControl,
            RmFree, //
        },
    },
    mm::{
        ContiguousVram,
        GpuMm, //
    },
};

use super::{
    channel::PushBuffer,
    Display, //
};

/// Size of the display engine's instance memory: a hash table of the context DMA objects of its
/// channels, followed by the objects (`NV_UDISP_HASH_*` and `NV_UDISP_OBJ_MEM_*` in `dev_disp.h`).
const INSTMEM_SIZE: usize = SZ_64K;

// Handles of the RM objects of the display engine, picked like nouveau's.

/// Handle of the display object.
const DISPLAY_HANDLE: u32 = 0xc670_0000;
/// Handle of the core channel.
const CORE_CHANNEL_HANDLE: u32 = 0xc67d_0000;

/// GSP-RM's own client and subdevice, the targets of its internal controls.
pub(crate) struct InternalHandles {
    pub(crate) client: Handle<Client>,
    pub(crate) subdevice: Handle<Subdevice>,
}

/// The display engine, taken over from GSP-RM.
pub(crate) struct DisplayEngine<'a> {
    /// Instance memory. GSP-RM uses it from when it's set on, so it must outlive the channels and
    /// stay allocated as long as the GSP runs.
    _instmem: ContiguousVram,
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
}

impl<'a> DisplayEngine<'a> {
    /// Takes over the display engine, whose RM objects are allocated under `display`.
    ///
    /// Only GA10x display engines are supported so far.
    pub(crate) fn new(
        dev: &'a device::Device<device::Bound>,
        bar: Bar0<'_>,
        cmdq: &Cmdq<'_>,
        mm: &mut GpuMm<'_>,
        chipset: Chipset,
        display: &Display,
        internal: &InternalHandles,
    ) -> Result<Self> {
        if !matches!(
            chipset,
            Chipset::GA102 | Chipset::GA103 | Chipset::GA104 | Chipset::GA106 | Chipset::GA107
        ) {
            return Err(ENOTSUPP);
        }

        let instmem = mm.alloc_contiguous(INSTMEM_SIZE, Alignment::new::<SZ_64K>())?;
        // The hash table must start out empty.
        mm.clear_vram(instmem.range())?;
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

        // From here on GSP-RM may use the instance memory, which must thus not be freed before the
        // GSP is unloaded, even if the channels can't be set up.
        let channels = Channels::new(dev, bar, cmdq, display, internal)
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
}

impl<'a> Channels<'a> {
    fn new(
        dev: &'a device::Device<device::Bound>,
        bar: Bar0<'_>,
        cmdq: &Cmdq<'_>,
        display: &Display,
        internal: &InternalHandles,
    ) -> Result<Self> {
        let client = display.client();
        let root = RmAlloc::new(
            client,
            display.device(),
            Handle::new(DISPLAY_HANDLE),
            AllocDisplayGa102,
        )
        .send(dev, cmdq)?;

        let setup_core = || -> Result<(Handle<DisplayCoreChannel>, PushBuffer<'a>)> {
            let push = PushBuffer::new(dev)?;
            // The push buffer of a channel is set before the channel is allocated.
            RmControl::new(
                internal.client,
                internal.subdevice,
                DispChannelPushbuffer::core_ga102(push.dma_address()),
            )
            .send(dev, cmdq)?;

            let core = RmAlloc::new(
                client,
                root,
                Handle::new(CORE_CHANNEL_HANDLE),
                AllocCoreChannelGa102::new(0),
            )
            .send(dev, cmdq)?;

            Ok((core, push))
        };

        match setup_core() {
            Ok((core, core_push)) => {
                let mut channels = Self {
                    root,
                    _core: core,
                    core_push,
                };
                if let Err(err) = channels.check_core(dev, bar) {
                    let _ = RmFree::new(client, root).send(dev, cmdq);
                    return Err(err);
                }
                Ok(channels)
            }
            Err(err) => {
                let _ = RmFree::new(client, root).send(dev, cmdq);
                Err(err)
            }
        }
    }

    /// Checks that the core channel reads its push buffer, with a method that changes nothing on
    /// screen: it only takes effect on the next update, which is never sent.
    fn check_core(&mut self, dev: &device::Device, bar: Bar0<'_>) -> Result {
        dev_dbg!(
            dev,
            "display: core channel GET {:#x}\n",
            self.core_push.get(bar)
        );

        // No notifier for the next update.
        self.core_push
            .push(bar, crate::gsp::DISP_CORE_SET_CONTEXT_DMA_NOTIFIER, &[0])?;
        self.core_push.kick(bar)?;
        self.core_push
            .wait_idle(bar)
            .inspect_err(|_| dev_err!(dev, "display: the core channel doesn't read methods\n"))?;

        dev_info!(dev, "display: core channel up\n");
        Ok(())
    }
}

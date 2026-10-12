// SPDX-License-Identifier: GPL-2.0

//! Channels of nova-core's clients, such as the userspace drivers nova-drm serves.
//!
//! A client's channel runs in the client's address space, where the kernel maps the channel's
//! control memory and context buffers, in the client's kernel range. It isn't privileged. Its
//! work is the client's push buffers, at the client's virtual addresses: the kernel lists them in
//! the channel's GPFIFO ring, then a push buffer of its own, which releases a semaphore in system
//! memory once they are done and raises the engine's notification interrupt, on which the
//! submission's fence signals (see [`super::fence`]).
//!
//! On the graphics engine, the channel has the 3D, compute, 2D and inline-to-memory objects the
//! client asks for, on a copy engine a copy object, on a video decoder engine a video decoder
//! object, which the client binds to subchannels itself. Channels on the graphics engine also
//! have an object of the copy engine sharing its runlist.

use core::ops::Range;

use kernel::{
    device,
    dma_buf::Fence,
    io::poll::read_poll_timeout,
    page::PAGE_SIZE,
    prelude::*,
    sync::{
        aref::ARef,
        Arc, //
    },
    time::Delta, //
};

use crate::{
    driver::Bar0,
    gpu::Chipset,
    gsp::{
        cmdq::Cmdq,
        rm::device::RmDevice,
        EngineInfo, //
    },
    mm::{
        pagetable::Access,
        vmm::MappedRange,
        MmAccess,
        VramAddress, //
    },
    num,
};

use super::{
    copy,
    fence::{
        ChannelFenceContext,
        Timeline,
        PROGRESS_TIMEOUT, //
    },
    gpfifo::{
        Channel,
        NewSpace,
        GPFIFO_ENTRIES,
        TAIL_SLOTS, //
    },
    gr::{
        ChannelContext,
        GrContext, //
    },
    hal::{
        GpEntry,
        Release, //
    },
    vaspace::AddressSpace,
    video::{
        self,
        DecoderContext, //
    },
};

/// Handles of the objects of clients' channels, after their classes, completed with the
/// channel's ID.
const THREE_D_HANDLE: u32 = 0xc797_0000;
const COMPUTE_HANDLE: u32 = 0xc7c0_0000;
const TWO_D_HANDLE: u32 = 0x902d_0000;
const INLINE_TO_MEMORY_HANDLE: u32 = 0xa140_0000;
const COPY_HANDLE: u32 = 0xc7b5_0000;
const VIDEO_DECODER_HANDLE: u32 = 0xc7b0_0000;

/// The objects of the classes of the graphics engine a channel on it has.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GraphicsObjects {
    /// A 3D object.
    pub three_d: bool,
    /// A compute object.
    pub compute: bool,
    /// A 2D object.
    pub two_d: bool,
    /// An inline-to-memory object.
    pub inline_to_memory: bool,
}

/// The engines a client's channel runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelEngine {
    /// The graphics engine, with objects of its classes.
    Graphics(GraphicsObjects),
    /// A copy engine, which doesn't share its runlist with the graphics engine.
    Copy,
    /// A video decoder engine (NVDEC).
    VideoDecoder,
}

/// Number of push buffers a client's channel runs at once: those of the GPFIFO ring but the
/// kernel's push buffer and the one entry a full ring keeps free.
pub const PUSH_MAX: u32 = GPFIFO_ENTRIES - 2;

/// A push buffer of a client.
#[derive(Clone, Copy, Debug)]
pub struct PushBuffer {
    /// GPU virtual address in the client's address space, 4-byte aligned.
    pub addr: u64,
    /// Number of 32-bit words.
    pub words: u32,
    /// Whether the channel fetches it once the work before is done, rather than ahead.
    pub wait: bool,
}

/// A channel of a client.
///
/// It must be released with [`ClientChannel::free`] while the GSP is running.
pub(crate) struct ClientChannel<'a> {
    channel: Channel<'a>,
    engine: ChannelEngine,
    /// The context buffers of channels on the graphics engine.
    context: Option<ChannelContext>,
    /// The context of channels on a video decoder engine.
    decoder: Option<DecoderContext>,
    /// The fences of the channel's submissions, and the semaphore they release.
    timeline: Arc<Timeline<'a>>,
    /// The mapping of the semaphore into the client's address space.
    fence_mapped: Option<MappedRange>,
    /// The value released by the last work submitted.
    sequence: u32,
    /// The submissions that may still run, oldest first: the value each releases, and the number
    /// of entries of the GPFIFO ring it takes.
    in_flight: KVec<(u32, u32)>,
}

impl<'a> ClientChannel<'a> {
    /// Allocates channel `chid` on `engine`, under the device of `rm`, in `space`, the client's
    /// address space, with its fences on `fence_context`. `gr` is the context of the graphics
    /// engine, which channels on it need, and `clear` clears VRAM for their context.
    #[expect(clippy::too_many_arguments)]
    pub(crate) fn new(
        dev: &'a device::Device<device::Bound>,
        cmdq: &Cmdq<'_>,
        mm: &mut impl MmAccess,
        clear: &mut impl FnMut(Range<VramAddress>) -> Result,
        chipset: Chipset,
        rm: &RmDevice,
        gr: Option<&GrContext<'_>>,
        space: &mut AddressSpace,
        chid: u32,
        engine: ChannelEngine,
        fence_context: &'a ChannelFenceContext,
    ) -> Result<Self> {
        let (engine_info, index) = match engine {
            ChannelEngine::Graphics(_) => (gr.ok_or(ENODEV)?.engine(), None),
            ChannelEngine::Copy => {
                let info = copy::find_engine(&super::engines(dev, cmdq, rm)?)?;
                (info, Some(info.copy_engine_index().ok_or(ENODEV)?))
            }
            ChannelEngine::VideoDecoder => {
                let info = video::find_engine(&super::engines(dev, cmdq, rm)?)?;
                (info, Some(info.video_decoder_index().ok_or(ENODEV)?))
            }
        };
        let timeline = Timeline::new(dev, fence_context, chid)?;

        let channel = Channel::new(
            dev,
            cmdq,
            mm,
            chipset,
            rm,
            engine_info,
            chid,
            true,
            NewSpace::Client(space),
        )?;
        let mut client = Self {
            channel,
            engine,
            context: None,
            decoder: None,
            timeline,
            fence_mapped: None,
            sequence: 0,
            in_flight: KVec::new(),
        };

        match client.set_up(dev, cmdq, mm, clear, rm, gr, space, engine_info, index) {
            Ok(()) => Ok(client),
            Err(err) => {
                client.free(dev, cmdq, mm, space);
                Err(err)
            }
        }
    }

    #[expect(clippy::too_many_arguments)]
    fn set_up(
        &mut self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        mm: &mut impl MmAccess,
        clear: &mut impl FnMut(Range<VramAddress>) -> Result,
        rm: &RmDevice,
        gr: Option<&GrContext<'_>>,
        space: &mut AddressSpace,
        engine_info: EngineInfo,
        index: Option<u32>,
    ) -> Result {
        let semaphore = self.timeline.semaphore();
        let start = semaphore.dma_address();
        let end = start + num::usize_as_u64(semaphore.size());
        self.fence_mapped = Some(mm.with(|mm| {
            space.map_sysmem_aligned(
                mm,
                start..end,
                num::usize_as_u64(PAGE_SIZE),
                Access::READ_WRITE,
            )
        })?);

        let chid = self.channel.chid();
        let hal = self.channel.hal();
        match (self.engine, gr, index) {
            (ChannelEngine::Graphics(objects), Some(gr), _) => {
                // The channel has a context on every engine of its runlist, as nouveau gives its
                // channels: the graphics engine's runlist also has copy engines, of which the
                // first gets an object, before the graphics context is given to GSP-RM.
                let runlist = gr.engine().runlist;
                let graphics_copy = super::engines(dev, cmdq, rm)?
                    .iter()
                    .filter(|engine| engine.runlist == runlist)
                    .find_map(EngineInfo::copy_engine_index);
                if let Some(index) = graphics_copy {
                    hal.alloc_copy(dev, cmdq, &mut self.channel, COPY_HANDLE | chid, index)?;
                }

                // Not privileged: the restricted register access map.
                self.context = Some(gr.set_up_channel(
                    dev,
                    cmdq,
                    mm,
                    clear,
                    rm,
                    &self.channel,
                    space,
                    false,
                )?);
                if objects.three_d {
                    hal.alloc_3d(dev, cmdq, &mut self.channel, THREE_D_HANDLE | chid)?;
                }
                if objects.compute {
                    hal.alloc_compute(dev, cmdq, &mut self.channel, COMPUTE_HANDLE | chid)?;
                }
                if objects.two_d {
                    hal.alloc_2d(dev, cmdq, &mut self.channel, TWO_D_HANDLE | chid)?;
                }
                if objects.inline_to_memory {
                    hal.alloc_inline_to_memory(
                        dev,
                        cmdq,
                        &mut self.channel,
                        INLINE_TO_MEMORY_HANDLE | chid,
                    )?;
                }
            }
            (ChannelEngine::Copy, _, Some(index)) => {
                hal.alloc_copy(dev, cmdq, &mut self.channel, COPY_HANDLE | chid, index)?;
            }
            (ChannelEngine::VideoDecoder, _, Some(index)) => {
                // The engine's context first, like nouveau binds it before allocating objects on
                // the channel.
                self.decoder = Some(DecoderContext::set_up(
                    dev,
                    cmdq,
                    mm,
                    clear,
                    rm,
                    &self.channel,
                    space,
                    engine_info,
                )?);
                hal.alloc_video_decoder(
                    dev,
                    cmdq,
                    &mut self.channel,
                    VIDEO_DECODER_HANDLE | chid,
                    index,
                )?;
            }
            _ => return Err(EINVAL),
        }

        Ok(())
    }

    /// Returns the number of entries of the GPFIFO ring that submitting `pushes` push buffers
    /// takes: theirs and the kernel's push buffer's.
    fn ring_entries(pushes: usize) -> Result<u32> {
        u32::try_from(pushes)?.checked_add(1).ok_or(EINVAL)
    }

    /// Forgets the submissions that are done.
    fn reclaim(&mut self) {
        let done = self.timeline.completed();
        // Sequence numbers wrap: one is reached when it is at most 2^31 before the value.
        self.in_flight
            .retain(|(sequence, _)| done.wrapping_sub(*sequence) >= 1 << 31);
    }

    /// Returns whether the ring and the kernel's push buffer have room for a submission of
    /// `entries` entries of the ring.
    fn has_room(&self, entries: u32) -> bool {
        let used: u32 = self.in_flight.iter().map(|(_, entries)| entries).sum();

        // The ring keeps one entry free, so that a full ring isn't taken for an empty one.
        used + entries < GPFIFO_ENTRIES && self.in_flight.len() < TAIL_SLOTS
    }

    /// Waits until the channel has room to submit `pushes` push buffers, as the work submitted
    /// before completes.
    ///
    /// Fails with `ETIMEDOUT` if the work submitted before makes no progress in time: the channel
    /// then runs no more work. Fails with `EINVAL` if no ring could take that many.
    pub(crate) fn wait_for_room(&mut self, pushes: usize) -> Result {
        let entries = Self::ring_entries(pushes)?;
        if entries >= GPFIFO_ENTRIES {
            return Err(EINVAL);
        }

        self.reclaim();
        if self.has_room(entries) {
            return Ok(());
        }
        let result = read_poll_timeout(
            || {
                self.reclaim();
                Ok(self.has_room(entries))
            },
            |&room| room,
            Delta::from_micros(50),
            PROGRESS_TIMEOUT,
        );
        if result.is_err() {
            self.fail(ETIMEDOUT);
            return Err(ETIMEDOUT);
        }

        Ok(())
    }

    /// Waits until the work submitted is done, for at most a while: the channel's work is then
    /// considered hung, and runs no more.
    pub(crate) fn wait_idle(&mut self) {
        if self.timeline.failed() {
            return;
        }
        let last = self.sequence;
        let result = read_poll_timeout(
            || Ok(self.timeline.completed()),
            |&done| done.wrapping_sub(last) < 1 << 31,
            Delta::from_micros(50),
            PROGRESS_TIMEOUT,
        );
        if result.is_err() {
            self.fail(ETIMEDOUT);
        }
    }

    /// Has the channel run the push buffers `pushes`, in order, after the work submitted before,
    /// and returns the fence that signals once they are done.
    ///
    /// The channel must have room, see [`ClientChannel::wait_for_room`]: fails with `EBUSY`
    /// otherwise. Fails with `ENODEV` once the channel runs no more work.
    pub(crate) fn submit(
        &mut self,
        bar: Bar0<'_>,
        mm: &mut impl MmAccess,
        pushes: &[PushBuffer],
    ) -> Result<ARef<Fence>> {
        let hal = self.channel.hal();
        let ring_entries = Self::ring_entries(pushes.len())?;
        if !self.has_room(ring_entries) {
            return Err(EBUSY);
        }

        let mut entries: KVec<GpEntry> = KVec::with_capacity(pushes.len(), GFP_KERNEL)?;
        for push in pushes {
            entries.push(hal.gp_entry(push.addr, push.words, push.wait)?, GFP_KERNEL)?;
        }

        // Once the client's work is done and its memory writes are visible, release the
        // semaphore, then tell the CPU.
        let sequence = self.sequence.wrapping_add(1);
        let mut tail = KVec::new();
        hal.push_release(
            &mut tail,
            Release {
                addr: self.fence_mapped.as_ref().ok_or(ENODEV)?.start().into_raw(),
                payload: sequence,
            },
        )?;
        hal.push_notify(&mut tail)?;
        self.in_flight.reserve(1, GFP_KERNEL)?;

        // Before the submission, so that its notification finds the fence.
        let fence = self.timeline.add(sequence)?;
        // The slot was last used by the submission `TAIL_SLOTS` before, done since there is room.
        let slot = num::u32_as_usize(sequence) % TAIL_SLOTS;
        if let Err(err) = self.channel.submit_queued(bar, mm, &entries, &tail, slot) {
            self.timeline.cancel(sequence, err);
            return Err(err);
        }
        self.sequence = sequence;
        self.in_flight
            .push_within_capacity((sequence, ring_entries))
            .map_err(|_| ENOMEM)?;

        Ok(fence)
    }

    /// Has the fences of the work submitted, and of the work submitted later, signal `err`: the
    /// channel runs no more work, such as once its work didn't finish in time.
    pub(crate) fn fail(&mut self, err: Error) {
        pr_err!(
            "client channel {}: runs no more work ({:?})\n",
            self.channel.chid(),
            err
        );
        self.timeline.fail(err);
    }

    /// Returns the fences of the channel's submissions.
    pub(crate) fn timeline(&self) -> &Arc<Timeline<'a>> {
        &self.timeline
    }

    /// Frees the channel in GSP-RM, with the objects allocated on it, and forgets its mappings:
    /// the client's address space is gone with the GPU.
    pub(crate) fn abandon(&mut self, dev: &device::Device, cmdq: &Cmdq<'_>) {
        self.channel.abandon(dev, cmdq);
        if let Some(context) = self.context.take() {
            context.forget();
        }
        if let Some(decoder) = self.decoder.take() {
            decoder.forget();
        }
        if let Some(mapped) = self.fence_mapped.take() {
            mapped.forget();
        }
        // The work done before counts, the rest never will be.
        self.timeline.signal_completed();
        self.timeline.fail(ECANCELED);
    }

    /// Frees the channel, with the objects allocated on it, and unmaps its memory from `space`,
    /// the client's address space it runs in.
    pub(crate) fn free(
        &mut self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        mm: &mut impl MmAccess,
        space: &mut AddressSpace,
    ) {
        // The memory stays mapped until GSP-RM freed the channel.
        self.channel.release(dev, cmdq);
        if let Some(mut context) = self.context.take() {
            mm.with(|mm| context.unmap(mm, space));
        }
        if let Some(decoder) = self.decoder.take() {
            mm.with(|mm| decoder.unmap(mm, space));
        }
        if let Some(mapped) = self.fence_mapped.take() {
            let _ = mm.with(|mm| space.unmap(mm, mapped));
        }
        self.channel.free(dev, cmdq, mm, Some(space));
        // The work done before counts, the rest never will be.
        self.timeline.signal_completed();
        self.timeline.fail(ECANCELED);
    }
}

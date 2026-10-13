// SPDX-License-Identifier: GPL-2.0

//! GPFIFO channels.
//!
//! A kernel channel lives in an address space of its own, a client's channel in the client's.
//! Its control memory, in system memory, holds the kernel's push buffer, the GPFIFO ring listing
//! the push buffers to run, and a semaphore the channel releases once the work submitted before is
//! done: the CPU writes and reads them without going through VRAM. The kernel's
//! work is run synchronously: one push buffer at a time, waited for through the semaphore.

use core::ops::Range;

use kernel::{
    device,
    dma::Coherent,
    io::{
        io_project,
        poll::read_poll_timeout,
        Io,
        IoBase, //
    },
    page,
    prelude::*,
    ptr::Alignment,
    sizes::{
        SZ_4K,
        SZ_64K, //
    },
    sync::barrier::{
        dma_mb,
        Write, //
    },
    time::Delta, //
};

use crate::{
    driver::Bar0,
    gpu::Chipset,
    gsp::{
        cmdq::Cmdq,
        commands::{
            Channel as RmChannel,
            ChannelGroup,
            ChannelObject,
            Client,
            ContextShare,
            Device,
            Handle,
            VaSpace, //
        },
        rm::{
            commands::{
                AllocChannelGroupKepler,
                AllocContextShareFermi,
                CeGetFaultMethodBufferSize,
                GpfifoBind,
                GpfifoSchedule,
                RmAlloc,
                RmAllocCommand,
                RmControl,
                RmFree,
                TsgPromoteFaultMethodBuffers, //
            },
            device::RmDevice,
        },
        ChannelMemory, EngineInfo, RUNQUEUES,
    },
    mm::{
        pagetable::Access,
        vmm::{
            MappedRange,
            Memory, //
        },
        ContiguousVram,
        GpuMm,
        MmAccess,
        Pages,
        Pfn,
        PfnRuns,
        VramAddress, //
    },
    num, //
};

use super::{
    hal::{
        self,
        ChannelHal,
        GpEntry,
        GpfifoAlloc,
        GpfifoParent,
        Release, //
    },
    vaspace::{
        AddressSpace,
        Mode,
        VaLayout, //
    },
};

/// Handle of the address space of a channel, after nouveau's, completed with the channel's ID.
const VA_SPACE_HANDLE: u32 = 0x90f1_0000;
/// Handle of a channel, after nouveau's, completed with its ID.
const CHANNEL_HANDLE: u32 = 0xf1f0_0000;

/// Handle of the channel group of a channel, completed with the channel's ID.
const GROUP_HANDLE: u32 = 0xa06c_0000;
/// Handle of the context share of a channel's group, completed with the channel's ID.
const CONTEXT_SHARE_HANDLE: u32 = 0x9067_0000;

// Layout of the control memory of a channel.

/// Size of the control memory.
const CONTROL_SIZE: usize = SZ_64K;
/// Size of the parts of the control memory allocated each: a page of the CPU, so that allocating
/// the control memory needs no contiguous memory, or the whole of it if pages are larger.
const CONTROL_CHUNK_SIZE: usize = if page::PAGE_SIZE < CONTROL_SIZE {
    page::PAGE_SIZE
} else {
    CONTROL_SIZE
};
static_assert!(CONTROL_SIZE % CONTROL_CHUNK_SIZE == 0 && CONTROL_CHUNK_SIZE % SZ_4K == 0);
/// Offset of the push buffer.
const PUSH_BUFFER: usize = 0;
/// Size of the push buffer.
const PUSH_BUFFER_SIZE: usize = 0x8000;
/// Offset of the GPFIFO ring.
const GPFIFO: usize = PUSH_BUFFER + PUSH_BUFFER_SIZE;
/// Number of entries of the GPFIFO ring: a power of two.
pub(super) const GPFIFO_ENTRIES: u32 = 1024;
/// Offset of the semaphore the channel releases.
const SEMAPHORE: usize = GPFIFO + num::u32_as_usize(GPFIFO_ENTRIES) * size_of::<GpEntry>();
static_assert!(SEMAPHORE + size_of::<u32>() <= CONTROL_SIZE);
/// Size of a slot of the push buffer, which [`Channel::submit_queued`] writes the head and the
/// tail of a submission to, while earlier ones may still run from other slots.
pub(super) const TAIL_SLOT_SIZE: usize = 0x200;
/// Number of slots of the push buffer.
pub(super) const TAIL_SLOTS: usize = PUSH_BUFFER_SIZE / TAIL_SLOT_SIZE;
/// Size of the head of a slot, which comes first in it, the tail taking the rest.
pub(super) const HEAD_SIZE: usize = 0x100;
static_assert!(HEAD_SIZE < TAIL_SLOT_SIZE);

/// Where a submission's own methods go in the kernel's push buffer: those run before the push
/// buffers submitted, and those after.
struct Methods<'a> {
    head: &'a [u32],
    head_offset: usize,
    tail: &'a [u32],
    tail_offset: usize,
    /// The room for the tail, in bytes.
    tail_room: usize,
}

/// How long work may take: much longer than any fill of a framebuffer.
const TIMEOUT: Delta = Delta::from_secs(1);

/// The address space a channel runs in.
enum Space {
    /// One of the channel's own.
    Own(AddressSpace),
    /// A client's, which the client keeps, and gives for the channel's mappings.
    Client(Handle<VaSpace>),
}

/// The address space a new channel runs in.
pub(crate) enum NewSpace<'a> {
    /// One of the channel's own, in this mode.
    Own(Mode),
    /// A client's.
    Client(&'a mut AddressSpace),
}

/// A GPFIFO channel.
///
/// A channel on the graphics engine is alone in a channel group of its own, with a context share,
/// like kernel RM allocates them for its clients' channels: GSP-RM's own groups for channels
/// allocated alone aren't fit for the graphics engine's.
///
/// It must be released with [`Channel::free`] while the GSP is running.
pub(crate) struct Channel<'a> {
    hal: &'static dyn ChannelHal,
    client: Handle<Client>,
    space: Space,
    /// The channel group, once allocated, for channels on the graphics engine.
    group: Option<Handle<ChannelGroup>>,
    /// The context share of the group, once allocated.
    context_share: Option<Handle<ContextShare>>,
    /// The channel, once allocated.
    handle: Option<Handle<RmChannel>>,
    /// The objects allocated on the channel, oldest first.
    objects: KVec<Handle<ChannelObject>>,
    /// Channel ID.
    chid: u32,
    /// The engine the channel runs on.
    engine: EngineInfo,
    instance: ContiguousVram,
    userd: ContiguousVram,
    /// The buffers the GPU saves pending methods into on faults: the channel group's, one per
    /// runqueue, or the channel's own. The channel runs on the first runqueue.
    method_buffers: KVec<Coherent<'a, [u8]>>,
    /// The control memory, in chunks of [`CONTROL_CHUNK_SIZE`], which the GPU accesses coherently
    /// with the CPU's caches.
    control: KVec<Coherent<'a, [u8]>>,
    /// GPU mapping of the control memory.
    control_mapped: Option<MappedRange>,
    /// Next GPFIFO entry to write.
    put: u32,
    /// Value of the semaphore released by the last work run.
    sequence: u32,
}

impl<'a> Channel<'a> {
    /// Allocates channel `chid` on `engine` under the device of `rm`, in the address space
    /// `space`. Fails with `ENOTSUPP` if Nova doesn't support the channels of `chipset`. Unless
    /// `scheduled`, the channel is only allocated: it is neither bound to its engine nor
    /// scheduled, so it can't run work.
    #[expect(clippy::too_many_arguments)]
    pub(crate) fn new(
        dev: &'a device::Device<device::Bound>,
        cmdq: &Cmdq<'_>,
        mm: &mut impl MmAccess,
        chipset: Chipset,
        rm: &RmDevice,
        engine: EngineInfo,
        chid: u32,
        scheduled: bool,
        space: NewSpace<'_>,
    ) -> Result<Self> {
        let hal = hal::channel_hal(chipset).ok_or(ENOTSUPP)?;
        let (client, device) = (rm.client(), rm.device());

        let grouped = engine.is_graphics();
        let method_buffer_size =
            RmControl::new(client, rm.subdevice(), CeGetFaultMethodBufferSize::new())
                .send(dev, cmdq)?;
        let mut method_buffers = KVec::new();
        for _ in 0..if grouped { RUNQUEUES } else { 1 } {
            method_buffers.push(
                Coherent::<u8>::zeroed_slice(
                    dev,
                    num::u32_as_usize(method_buffer_size),
                    GFP_KERNEL,
                )?,
                GFP_KERNEL,
            )?;
        }

        let alloc_cleared = |mm: &mut GpuMm<'_>, size: usize| -> Result<ContiguousVram> {
            let vram = mm.alloc_contiguous(size, Alignment::new::<SZ_4K>())?;
            mm.clear_vram(vram.range())?;
            Ok(vram)
        };
        let instance = mm.with(|mm| alloc_cleared(mm, SZ_4K))?;
        let userd = mm.with(|mm| alloc_cleared(mm, SZ_4K))?;
        let mut control = KVec::with_capacity(CONTROL_SIZE / CONTROL_CHUNK_SIZE, GFP_KERNEL)?;
        for _ in 0..CONTROL_SIZE / CONTROL_CHUNK_SIZE {
            control.push(
                Coherent::<u8>::zeroed_slice(dev, CONTROL_CHUNK_SIZE, GFP_KERNEL)?,
                GFP_KERNEL,
            )?;
        }

        // Kernel channels have address spaces of their own, and are privileged.
        let privileged = matches!(space, NewSpace::Own(_));
        let (own, mut client_space, mode) = match space {
            NewSpace::Own(mode) => (
                Some(AddressSpace::new(
                    dev,
                    cmdq,
                    mm,
                    chipset.mmu_version(),
                    client,
                    device,
                    VA_SPACE_HANDLE | chid,
                    mode,
                    VaLayout::Kernel,
                )?),
                None,
                mode,
            ),
            NewSpace::Client(space) => (None, Some(space), Mode::ClientOwned),
        };
        let space = match own {
            Some(own) => Space::Own(own),
            None => Space::Client(client_space.as_ref().ok_or(EINVAL)?.handle()),
        };

        let mut channel = Self {
            hal,
            client,
            space,
            group: None,
            context_share: None,
            handle: None,
            chid,
            engine,
            instance,
            userd,
            method_buffers,
            control,
            control_mapped: None,
            objects: KVec::new(),
            put: 0,
            sequence: 0,
        };

        // The memory manager is only taken for the memory: GSP-RM's work goes on without it.
        let mut set_up = |channel: &mut Self, client_space: Option<&mut AddressSpace>| -> Result {
            // The GPU's 4 KiB frames of the chunks, in order.
            let mut control = PfnRuns::with_capacity(channel.control.len())?;
            for chunk in &channel.control {
                control.push(
                    Pfn::new(chunk.dma_address() >> 12),
                    CONTROL_CHUNK_SIZE / SZ_4K,
                )?;
            }
            let space = channel.space_mut(client_space)?;
            let control_mapped = mm.with(|mm| {
                space.map_sysmem_pages_aligned(
                    mm,
                    control.pages(),
                    num::usize_as_u64(SZ_4K),
                    Access::READ_WRITE,
                )
            })?;
            let gpfifo = control_mapped.start().into_raw() + num::usize_as_u64(GPFIFO);

            // The control memory's mapping created the page directories GSP-RM takes.
            let shared = if mode == Mode::Split {
                mm.with(|mm| space.share_with_rm(dev, cmdq, mm))
            } else {
                Ok(())
            };
            let va_space = space.handle();
            channel.control_mapped = Some(control_mapped);
            shared?;

            let parent = if grouped {
                channel.alloc_group(dev, cmdq, device, va_space, method_buffer_size)?
            } else {
                GpfifoParent::Device { device, va_space }
            };

            let memory = ChannelMemory {
                instance: channel.instance.range().start.into_raw(),
                userd: channel.userd.range().start.into_raw(),
                method_buffer: channel.method_buffers[0].dma_address(),
                method_buffer_size: u64::from(method_buffer_size),
            };
            let handle = hal.alloc_gpfifo(
                dev,
                cmdq,
                client,
                CHANNEL_HANDLE | chid,
                &GpfifoAlloc {
                    parent,
                    engine_type: engine.engine_type,
                    chid,
                    gpfifo,
                    entries: GPFIFO_ENTRIES,
                    memory: &memory,
                    privileged,
                },
            )?;
            channel.handle = Some(handle);

            if scheduled {
                RmControl::new(client, handle, GpfifoBind::new(engine.engine_type))
                    .send(dev, cmdq)?;
                RmControl::new(client, handle, GpfifoSchedule::enable()).send(dev, cmdq)?;
            }

            Ok(())
        };

        match set_up(&mut channel, client_space.as_deref_mut()) {
            Ok(()) => Ok(channel),
            Err(err) => {
                channel.free(dev, cmdq, mm, client_space);
                Err(err)
            }
        }
    }

    /// Allocates the channel group of the channel under `device`, its channels running in the
    /// address space `va_space`, with the group's method buffers of `method_buffer_size` bytes
    /// and its context share, as kernel RM does (`kchangrpapiConstruct`), and returns where the
    /// channel is allocated.
    fn alloc_group(
        &mut self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        device: Handle<Device>,
        va_space: Handle<VaSpace>,
        method_buffer_size: u32,
    ) -> Result<GpfifoParent> {
        let group = RmAlloc::new(
            self.client,
            device,
            Handle::new(GROUP_HANDLE | self.chid),
            AllocChannelGroupKepler::new(va_space, self.engine.engine_type),
        )
        .send(dev, cmdq)?;
        self.group = Some(group);

        let mut buffers = [0; RUNQUEUES];
        for (buffer, method_buffer) in buffers.iter_mut().zip(&self.method_buffers) {
            *buffer = method_buffer.dma_address();
        }
        RmControl::new(
            self.client,
            group,
            TsgPromoteFaultMethodBuffers::new(buffers, u64::from(method_buffer_size)),
        )
        .send(dev, cmdq)?;

        let context_share = RmAlloc::new(
            self.client,
            group,
            Handle::new(CONTEXT_SHARE_HANDLE | self.chid),
            AllocContextShareFermi::sync(va_space),
        )
        .send(dev, cmdq)?;
        self.context_share = Some(context_share);

        Ok(GpfifoParent::Group {
            group,
            context_share,
        })
    }

    /// Returns the address space the channel runs in: its own, or `client_space`, which must then
    /// be the client's one it runs in.
    fn space_mut<'s>(
        &'s mut self,
        client_space: Option<&'s mut AddressSpace>,
    ) -> Result<&'s mut AddressSpace> {
        match (&mut self.space, client_space) {
            (Space::Own(space), _) => Ok(space),
            (Space::Client(handle), Some(space)) if space.handle().as_raw() == handle.as_raw() => {
                Ok(space)
            }
            (Space::Client(_), _) => Err(EINVAL),
        }
    }

    /// Returns the address space of the channel's own, in which the kernel maps its buffers.
    ///
    /// Fails with `EINVAL` for the channels of clients.
    pub(crate) fn own_space(&mut self) -> Result<&mut AddressSpace> {
        self.space_mut(None)
    }

    /// Returns the ID of the channel.
    pub(crate) fn chid(&self) -> u32 {
        self.chid
    }

    /// Returns the HAL of the channel's GPU.
    pub(crate) fn hal(&self) -> &'static dyn ChannelHal {
        self.hal
    }

    /// Returns the client the channel is allocated under.
    pub(crate) fn client(&self) -> Handle<Client> {
        self.client
    }

    /// Returns the RM handle of the channel.
    pub(crate) fn handle(&self) -> Result<Handle<RmChannel>> {
        self.handle.ok_or(ENODEV)
    }

    /// Allocates the object `handle` of an engine class on the channel. It is freed before the
    /// channel, by [`Channel::release`].
    pub(crate) fn alloc_object<A>(
        &mut self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        handle: u32,
        command: A,
    ) -> Result<Handle<A::Object>>
    where
        A: RmAllocCommand<Parent = RmChannel>,
    {
        // Room for the object first: once allocated, it must be tracked.
        self.objects.reserve(1, GFP_KERNEL)?;
        let object = RmAlloc::new(self.client, self.handle()?, Handle::new(handle), command)
            .send(dev, cmdq)
            .inspect_err(|e| {
                dev_err!(
                    dev,
                    "channel {}: can't allocate an object of class {:#x}: {:?}\n",
                    self.chid,
                    u32::from(A::CLASS),
                    e
                )
            })?;
        self.objects
            .push(Handle::new(object.as_raw()), GFP_KERNEL)?;

        Ok(object)
    }

    /// Maps the page-aligned VRAM `range` with `access` into the channel's address space,
    /// returning its GPU virtual address through [`MappedRange::start`]. The mapping must be given
    /// back to [`Channel::unmap`].
    pub(crate) fn map_vram(
        &mut self,
        mm: &mut GpuMm<'_>,
        range: Range<VramAddress>,
        access: Access,
    ) -> Result<MappedRange> {
        self.own_space()?.map_vram(mm, range, access)
    }

    /// Like [`Channel::map_vram`], for the pages `pages` of `memory`, in order.
    pub(crate) fn map_pages(
        &mut self,
        mm: &mut GpuMm<'_>,
        pages: Pages<'_>,
        memory: Memory,
        access: Access,
    ) -> Result<MappedRange> {
        self.own_space()?.map_pages(mm, pages, memory, access)
    }

    /// Like [`Channel::map_vram`], but at a virtual address aligned to `alignment`, a power of
    /// two. Aligned mappings aren't meant to be torn down before the channel is freed.
    pub(crate) fn map_vram_aligned(
        &mut self,
        mm: &mut GpuMm<'_>,
        range: Range<VramAddress>,
        alignment: u64,
        access: Access,
    ) -> Result<MappedRange> {
        self.own_space()?
            .map_vram_aligned(mm, range, alignment, access)
    }

    /// Tears down a mapping made by [`Channel::map_vram`] or [`Channel::map_vram_aligned`].
    pub(crate) fn unmap(&mut self, mm: &mut GpuMm<'_>, mapped: MappedRange) -> Result {
        self.own_space()?.unmap(mm, mapped)
    }

    /// Runs the methods of `push`, and returns once the work they do is done.
    pub(crate) fn run(&mut self, bar: Bar0<'_>, mm: &mut impl MmAccess, push: &[u32]) -> Result {
        let control_va = self.control_va()?;
        let sequence = self.sequence.wrapping_add(1);

        let mut words = KVec::with_capacity(push.len(), GFP_KERNEL)?;
        words.extend_from_slice(push, GFP_KERNEL)?;
        // Once the work is done and its memory writes are visible, release the semaphore.
        self.hal.push_release(
            &mut words,
            Release {
                addr: control_va + num::usize_as_u64(SEMAPHORE),
                payload: sequence,
            },
        )?;
        self.submit(bar, mm, &[], &words)?;

        self.sequence = sequence;
        self.wait_semaphore(mm, sequence)
    }

    /// Has the channel run the push buffers of `entries`, then the methods of `tail`, from the
    /// kernel's push buffer, and returns without waiting for them.
    ///
    /// The kernel's push buffer is overwritten: the work submitted before must be done.
    pub(crate) fn submit(
        &mut self,
        bar: Bar0<'_>,
        mm: &mut impl MmAccess,
        entries: &[GpEntry],
        tail: &[u32],
    ) -> Result {
        // The ring is empty: the work submitted before is done.
        if entries.len() + usize::from(!tail.is_empty()) >= num::u32_as_usize(GPFIFO_ENTRIES) {
            return Err(EINVAL);
        }

        self.submit_at(
            bar,
            mm,
            entries,
            Methods {
                head: &[],
                head_offset: PUSH_BUFFER,
                tail,
                tail_offset: PUSH_BUFFER,
                tail_room: PUSH_BUFFER_SIZE,
            },
        )
    }

    /// Has the channel run the push buffers of `entries`, then the methods of `tail`, from slot
    /// `slot` of the kernel's push buffer, after the work submitted before, and returns without
    /// waiting for them.
    ///
    /// The work submitted before may still run, but the caller must make sure that the ring has
    /// room for the entries and the tail's, and that the work last submitted from `slot` is done.
    pub(crate) fn submit_queued(
        &mut self,
        bar: Bar0<'_>,
        mm: &mut impl MmAccess,
        head: &[u32],
        entries: &[GpEntry],
        tail: &[u32],
        slot: usize,
    ) -> Result {
        if slot >= TAIL_SLOTS || size_of_val(head) > HEAD_SIZE {
            return Err(EINVAL);
        }

        let slot_offset = PUSH_BUFFER + slot * TAIL_SLOT_SIZE;
        self.submit_at(
            bar,
            mm,
            entries,
            Methods {
                head,
                head_offset: slot_offset,
                tail,
                tail_offset: slot_offset + HEAD_SIZE,
                tail_room: TAIL_SLOT_SIZE - HEAD_SIZE,
            },
        )
    }

    /// Has the channel run the methods of `methods.head`, the push buffers of `entries`, then the
    /// methods of `methods.tail`, writing the methods to the kernel's push buffer where `methods`
    /// says, and returns without waiting for them.
    fn submit_at(
        &mut self,
        bar: Bar0<'_>,
        mm: &mut impl MmAccess,
        entries: &[GpEntry],
        methods: Methods<'_>,
    ) -> Result {
        let control_va = self.control_va()?;
        let Methods {
            head,
            head_offset,
            tail,
            tail_offset,
            tail_room,
        } = methods;
        let count = usize::from(!head.is_empty()) + entries.len() + usize::from(!tail.is_empty());
        if count >= num::u32_as_usize(GPFIFO_ENTRIES) || size_of_val(tail) > tail_room {
            return Err(EINVAL);
        }
        let mut ring = KVec::with_capacity(count, GFP_KERNEL)?;
        if !head.is_empty() {
            ring.push(
                self.write_methods(control_va, head_offset, head)?,
                GFP_KERNEL,
            )?;
        }
        ring.extend_from_slice(entries, GFP_KERNEL)?;
        if !tail.is_empty() {
            ring.push(
                self.write_methods(control_va, tail_offset, tail)?,
                GFP_KERNEL,
            )?;
        }

        for entry in &ring {
            let entry_offset = GPFIFO + num::u32_as_usize(self.put) * size_of::<GpEntry>();
            self.write_control(entry_offset, entry.as_bytes())?;
            self.put = (self.put + 1) % GPFIFO_ENTRIES;
        }

        // The GPU must see the push buffer and the ring above before `GPPut` moves.
        dma_mb(Write);
        // The memory manager is only taken for this write: the copies of the display wait for it.
        let gp_put = self.userd.range().start + num::usize_as_u64(self.hal.userd_gp_put());
        mm.with(|mm| mm.write_vram(gp_put, &[self.put]))?;

        // PRAMIN and the doorbell are both in BAR0: the channel sees `GPPut` when told to look.
        self.hal.ring_doorbell(bar, &self.engine, self.chid)
    }

    /// Writes `methods` at `offset` of the control memory, whose GPU virtual address is
    /// `control_va`, and returns the GPFIFO entry running them.
    fn write_methods(&self, control_va: u64, offset: usize, methods: &[u32]) -> Result<GpEntry> {
        self.write_control(offset, methods.as_bytes())?;
        self.hal.gp_entry(
            control_va + num::usize_as_u64(offset),
            u32::try_from(methods.len())?,
            false,
        )
    }

    /// Writes `bytes` at `offset` of the control memory.
    fn write_control(&self, offset: usize, bytes: &[u8]) -> Result {
        let end = offset.checked_add(bytes.len()).ok_or(EINVAL)?;
        if end > CONTROL_SIZE {
            return Err(EINVAL);
        }

        // Chunk by chunk.
        let mut at = offset;
        while at < end {
            let within = at % CONTROL_CHUNK_SIZE;
            let len = (CONTROL_CHUNK_SIZE - within).min(end - at);
            let view = (&self.control[at / CONTROL_CHUNK_SIZE]).as_view();
            io_project!(view, [try: within..within + len])
                .copy_from_slice(&bytes[at - offset..at - offset + len]);
            at += len;
        }

        Ok(())
    }

    /// Returns the value of the semaphore.
    fn semaphore(&self) -> u32 {
        // SAFETY: `SEMAPHORE` is in the control memory, which the GPU writes coherently with the
        // CPU's caches, 4-byte aligned, so within a chunk.
        unsafe {
            self.control[SEMAPHORE / CONTROL_CHUNK_SIZE]
                .as_ptr()
                .cast::<u8>()
                .add(SEMAPHORE % CONTROL_CHUNK_SIZE)
                .cast::<u32>()
                .read_volatile()
        }
    }

    /// Returns the GPU virtual address of the control memory.
    fn control_va(&self) -> Result<u64> {
        Ok(self
            .control_mapped
            .as_ref()
            .ok_or(ENODEV)?
            .start()
            .into_raw())
    }

    /// Waits for the semaphore to be released with `value`.
    fn wait_semaphore(&self, mm: &mut impl MmAccess, value: u32) -> Result {
        let userd = self.userd.range().start;

        let result = read_poll_timeout(
            || Ok(self.semaphore()),
            |&released| released == value,
            Delta::from_micros(10),
            TIMEOUT,
        );
        if result.is_err() {
            let gp_get =
                mm.with(|mm| mm.read_vram(userd + num::usize_as_u64(self.hal.userd_gp_get())));
            pr_err!(
                "nova-core: channel {}: work not done: semaphore {}, GPGet {:?}, GPPut {}\n",
                self.chid,
                self.semaphore(),
                gp_get,
                self.put
            );
        }

        result.map(|_| ())
    }

    /// Frees the channel in GSP-RM, with the objects allocated on it and its group. The memory it
    /// used may be unmapped afterwards, before the rest of the channel is freed with
    /// [`Channel::free`].
    ///
    /// The objects allocated on the channel are freed first, newest first, then the channel, the
    /// context share and the group, like kernel RM frees them, rather than having GSP-RM free
    /// children along with their parents.
    pub(crate) fn release(&mut self, dev: &device::Device, cmdq: &Cmdq<'_>) {
        while let Some(object) = self.objects.pop() {
            let _ = RmFree::new(self.client, object)
                .send(dev, cmdq)
                .inspect_err(|e| {
                    dev_err!(
                        dev,
                        "channel {}: can't free object {:#x}: {:?}\n",
                        self.chid,
                        object.as_raw(),
                        e
                    )
                });
        }
        if let Some(handle) = self.handle.take() {
            let _ = RmFree::new(self.client, handle)
                .send(dev, cmdq)
                .inspect_err(|e| {
                    dev_err!(dev, "channel: can't free channel {}: {:?}\n", self.chid, e)
                });
        }
        if let Some(context_share) = self.context_share.take() {
            let _ = RmFree::new(self.client, context_share)
                .send(dev, cmdq)
                .inspect_err(|e| {
                    dev_err!(
                        dev,
                        "channel {}: can't free the context share: {:?}\n",
                        self.chid,
                        e
                    )
                });
        }
        if let Some(group) = self.group.take() {
            let _ = RmFree::new(self.client, group)
                .send(dev, cmdq)
                .inspect_err(|e| {
                    dev_err!(
                        dev,
                        "channel {}: can't free the group: {:?}\n",
                        self.chid,
                        e
                    )
                });
        }
    }

    /// Frees the channel of a client in GSP-RM, with the objects allocated on it, and forgets its
    /// mappings: the client's address space is gone with the GPU.
    pub(crate) fn abandon(&mut self, dev: &device::Device, cmdq: &Cmdq<'_>) {
        self.release(dev, cmdq);
        if let Some(mapped) = self.control_mapped.take() {
            mapped.forget();
        }
    }

    /// Frees the channel, with the objects allocated on it, and its address space of its own,
    /// which mustn't have mappings left but those of the channel, or unmaps its mappings from
    /// `client_space`, the client's one it runs in. Its memory is freed when it is dropped.
    pub(crate) fn free(
        &mut self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        mm: &mut impl MmAccess,
        client_space: Option<&mut AddressSpace>,
    ) {
        self.release(dev, cmdq);
        let mapped = self.control_mapped.take();
        let Ok(space) = self.space_mut(client_space) else {
            dev_err!(
                dev,
                "channel {}: freed without its address space\n",
                self.chid
            );
            if let Some(mapped) = mapped {
                mapped.forget();
            }
            return;
        };
        if let Some(mapped) = mapped {
            let _ = mm.with(|mm| space.unmap(mm, mapped));
        }
        if let Space::Own(space) = &self.space {
            let _ = space
                .free(dev, cmdq)
                .inspect_err(|e| dev_err!(dev, "channel: can't free the address space: {:?}\n", e));
        }
    }
}

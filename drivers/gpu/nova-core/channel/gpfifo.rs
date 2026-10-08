// SPDX-License-Identifier: GPL-2.0

//! GPFIFO channels.
//!
//! A channel lives in an address space of its own. Its control memory holds the push buffer, the
//! GPFIFO ring listing the parts of the push buffer to run, and a semaphore the channel releases
//! once the work submitted before is done. Work is run synchronously: one push buffer at a time,
//! waited for through the semaphore.

use core::ops::Range;

use kernel::{
    device,
    dma::Coherent,
    io::{
        poll::read_poll_timeout,
        Io, //
    },
    prelude::*,
    ptr::Alignment,
    sizes::{
        SZ_4K,
        SZ_64K, //
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
            Client,
            Handle, //
        },
        fifo::host_c56f,
        rm::{
            commands::{
                AllocChannelGpfifoAmpere,
                CeGetFaultMethodBufferSize,
                GpfifoBind,
                GpfifoSchedule,
                RmAlloc,
                RmAllocCommand,
                RmControl,
                RmFree, //
            },
            device::RmDevice,
        },
        ChannelMemory, EngineInfo,
    },
    mm::{
        pagetable::Access,
        vmm::MappedRange,
        ContiguousVram,
        GpuMm,
        VramAddress, //
    },
    num,
    regs, //
};

use super::vaspace::{
    AddressSpace,
    Mode, //
};

/// Handle of the address space of a channel, after nouveau's, completed with the channel's ID.
const VA_SPACE_HANDLE: u32 = 0x90f1_0000;
/// Handle of a channel, after nouveau's, completed with its ID.
const CHANNEL_HANDLE: u32 = 0xf1f0_0000;

// Layout of the control memory of a channel.

/// Size of the control memory.
const CONTROL_SIZE: usize = SZ_64K;
/// Offset of the push buffer.
const PUSH_BUFFER: usize = 0;
/// Size of the push buffer.
const PUSH_BUFFER_SIZE: usize = 0x8000;
/// Offset of the GPFIFO ring.
const GPFIFO: usize = PUSH_BUFFER + PUSH_BUFFER_SIZE;
/// Number of entries of the GPFIFO ring: a power of two.
const GPFIFO_ENTRIES: u32 = 1024;
/// Offset of the semaphore the channel releases.
const SEMAPHORE: usize = GPFIFO + num::u32_as_usize(GPFIFO_ENTRIES) * host_c56f::GP_ENTRY_SIZE;
static_assert!(SEMAPHORE + size_of::<u32>() <= CONTROL_SIZE);

/// How long work may take: much longer than any fill of a framebuffer.
const TIMEOUT: Delta = Delta::from_secs(1);

/// A kernel GPFIFO channel.
///
/// It must be released with [`Channel::free`] while the GSP is running.
pub(crate) struct Channel<'a> {
    client: Handle<Client>,
    address_space: AddressSpace,
    /// The channel, once allocated.
    handle: Option<Handle<RmChannel>>,
    /// Channel ID.
    chid: u32,
    /// The engine the channel runs on.
    engine: EngineInfo,
    instance: ContiguousVram,
    userd: ContiguousVram,
    method_buffer: Coherent<'a, [u8]>,
    control: ContiguousVram,
    /// GPU mapping of the control memory.
    control_mapped: Option<MappedRange>,
    /// Next GPFIFO entry to write.
    put: u32,
    /// Value of the semaphore released by the last work run.
    sequence: u32,
}

impl<'a> Channel<'a> {
    /// Allocates channel `chid` on `engine` under the device of `rm`, in an address space of its
    /// own in `mode`. Unless `scheduled`, the channel is only allocated: it is neither bound to its
    /// engine nor scheduled, so it can't run work.
    #[expect(clippy::too_many_arguments)]
    pub(crate) fn new(
        dev: &'a device::Device<device::Bound>,
        cmdq: &Cmdq<'_>,
        mm: &mut GpuMm<'_>,
        chipset: Chipset,
        rm: &RmDevice,
        engine: EngineInfo,
        chid: u32,
        scheduled: bool,
        mode: Mode,
    ) -> Result<Self> {
        let (client, device) = (rm.client(), rm.device());

        let method_buffer_size =
            RmControl::new(client, rm.subdevice(), CeGetFaultMethodBufferSize::new())
                .send(dev, cmdq)?;
        let method_buffer =
            Coherent::<u8>::zeroed_slice(dev, num::u32_as_usize(method_buffer_size), GFP_KERNEL)?;

        let alloc_cleared = |mm: &mut GpuMm<'_>, size: usize| -> Result<ContiguousVram> {
            let vram = mm.alloc_contiguous(size, Alignment::new::<SZ_4K>())?;
            mm.clear_vram(vram.range())?;
            Ok(vram)
        };
        let instance = alloc_cleared(mm, SZ_4K)?;
        let userd = alloc_cleared(mm, SZ_4K)?;
        let control = alloc_cleared(mm, CONTROL_SIZE)?;

        let address_space = AddressSpace::new(
            dev,
            cmdq,
            mm,
            chipset.mmu_version(),
            client,
            device,
            VA_SPACE_HANDLE | chid,
            mode,
        )?;

        let mut channel = Self {
            client,
            address_space,
            handle: None,
            chid,
            engine,
            instance,
            userd,
            method_buffer,
            control,
            control_mapped: None,
            put: 0,
            sequence: 0,
        };

        let set_up = |channel: &mut Self, mm: &mut GpuMm<'_>| -> Result {
            let control_mapped =
                channel
                    .address_space
                    .map_vram(mm, channel.control.range(), Access::READ_WRITE)?;
            let gpfifo = control_mapped.start().into_raw() + num::usize_as_u64(GPFIFO);
            channel.control_mapped = Some(control_mapped);

            // The control memory's mapping created the page directories GSP-RM takes.
            if mode == Mode::Split {
                channel.address_space.share_with_rm(dev, cmdq, mm)?;
            }

            let memory = ChannelMemory {
                instance: channel.instance.range().start.into_raw(),
                userd: channel.userd.range().start.into_raw(),
                method_buffer: channel.method_buffer.dma_address(),
                method_buffer_size: u64::from(method_buffer_size),
            };
            let handle = RmAlloc::new(
                client,
                device,
                Handle::new(CHANNEL_HANDLE | chid),
                AllocChannelGpfifoAmpere::kernel(
                    channel.address_space.handle(),
                    engine.engine_type,
                    chid,
                    gpfifo,
                    GPFIFO_ENTRIES,
                    &memory,
                ),
            )
            .send(dev, cmdq)?;
            channel.handle = Some(handle);

            if scheduled {
                RmControl::new(client, handle, GpfifoBind::new(engine.engine_type))
                    .send(dev, cmdq)?;
                RmControl::new(client, handle, GpfifoSchedule::enable()).send(dev, cmdq)?;
            }

            Ok(())
        };

        match set_up(&mut channel, mm) {
            Ok(()) => Ok(channel),
            Err(err) => {
                channel.free(dev, cmdq, mm);
                Err(err)
            }
        }
    }

    /// Returns the client the channel is allocated under.
    pub(crate) fn client(&self) -> Handle<Client> {
        self.client
    }

    /// Returns the RM handle of the channel.
    pub(crate) fn handle(&self) -> Result<Handle<RmChannel>> {
        self.handle.ok_or(ENODEV)
    }

    /// Allocates the object `handle` of an engine class on the channel. It is freed along with the
    /// channel.
    pub(crate) fn alloc_object<A>(
        &self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        handle: u32,
        command: A,
    ) -> Result<Handle<A::Object>>
    where
        A: RmAllocCommand<Parent = RmChannel>,
    {
        Ok(
            RmAlloc::new(self.client, self.handle()?, Handle::new(handle), command)
                .send(dev, cmdq)
                .inspect_err(|e| {
                    dev_err!(
                        dev,
                        "channel {}: can't allocate an object of class {:#x}: {:?}\n",
                        self.chid,
                        u32::from(A::CLASS),
                        e
                    )
                })?,
        )
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
        self.address_space.map_vram(mm, range, access)
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
        self.address_space
            .map_vram_aligned(mm, range, alignment, access)
    }

    /// Tears down a mapping made by [`Channel::map_vram`] or [`Channel::map_vram_aligned`].
    pub(crate) fn unmap(&mut self, mm: &mut GpuMm<'_>, mapped: MappedRange) -> Result {
        self.address_space.unmap(mm, mapped)
    }

    /// Runs the methods of `push`, and returns once the work they do is done.
    pub(crate) fn run(&mut self, bar: Bar0<'_>, mm: &mut GpuMm<'_>, push: &[u32]) -> Result {
        let control_va = self.control_va()?;
        let sequence = self.sequence.wrapping_add(1);
        let release =
            host_c56f::semaphore_release(control_va + num::usize_as_u64(SEMAPHORE), sequence)?;

        let words = push.len() + release.len();
        if words * size_of::<u32>() > PUSH_BUFFER_SIZE {
            return Err(EINVAL);
        }
        let control = self.control.range().start;
        let push_buffer = control + num::usize_as_u64(PUSH_BUFFER);
        mm.write_vram(push_buffer, push)?;
        mm.write_vram(
            push_buffer + num::usize_as_u64(push.len() * size_of::<u32>()),
            &release,
        )?;

        let entry = host_c56f::gp_entry(
            control_va + num::usize_as_u64(PUSH_BUFFER),
            u32::try_from(words)?,
        )?;
        let entry_offset = GPFIFO + num::u32_as_usize(self.put) * host_c56f::GP_ENTRY_SIZE;
        mm.write_vram(control + num::usize_as_u64(entry_offset), &entry)?;

        self.put = (self.put + 1) % GPFIFO_ENTRIES;
        mm.write_vram(
            self.userd.range().start + num::usize_as_u64(host_c56f::USERD_GP_PUT),
            &[self.put],
        )?;

        // PRAMIN and the doorbell are both in BAR0: the channel sees the writes above when told to
        // look.
        bar.write_reg(
            regs::NV_VIRTUAL_FUNCTION_DOORBELL::zeroed()
                .try_with_vector(self.chid)?
                .try_with_runlist_id(self.engine.runlist)?,
        );

        self.sequence = sequence;
        self.wait_semaphore(mm, sequence)
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
    fn wait_semaphore(&self, mm: &mut GpuMm<'_>, value: u32) -> Result {
        let semaphore = self.control.range().start + num::usize_as_u64(SEMAPHORE);
        let userd = self.userd.range().start;

        let result = read_poll_timeout(
            || mm.read_vram(semaphore),
            |&released| released == value,
            Delta::from_micros(10),
            TIMEOUT,
        );
        if result.is_err() {
            let gp_get = mm.read_vram(userd + num::usize_as_u64(host_c56f::USERD_GP_GET));
            pr_err!(
                "nova-core: channel {}: work not done: semaphore {:?}, GPGet {:?}, GPPut {}\n",
                self.chid,
                mm.read_vram(semaphore),
                gp_get,
                self.put
            );
        }

        result.map(|_| ())
    }

    /// Frees the channel in GSP-RM, with the objects allocated on it. The memory it used may be
    /// unmapped afterwards, before the rest of the channel is freed with [`Channel::free`].
    pub(crate) fn release(&mut self, dev: &device::Device, cmdq: &Cmdq<'_>) {
        if let Some(handle) = self.handle.take() {
            let _ = RmFree::new(self.client, handle)
                .send(dev, cmdq)
                .inspect_err(|e| {
                    dev_err!(dev, "channel: can't free channel {}: {:?}\n", self.chid, e)
                });
        }
    }

    /// Frees the channel, with the objects allocated on it, and its address space, which mustn't
    /// have mappings left but those of the channel. Its memory is freed when it is dropped.
    pub(crate) fn free(&mut self, dev: &device::Device, cmdq: &Cmdq<'_>, mm: &mut GpuMm<'_>) {
        self.release(dev, cmdq);
        if let Some(mapped) = self.control_mapped.take() {
            let _ = self.address_space.unmap(mm, mapped);
        }
        let _ = self
            .address_space
            .free(dev, cmdq)
            .inspect_err(|e| dev_err!(dev, "channel: can't free the address space: {:?}\n", e));
    }
}

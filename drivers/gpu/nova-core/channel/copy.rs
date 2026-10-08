// SPDX-License-Identifier: GPL-2.0

//! A kernel channel on a copy engine, which Nova uses to fill and copy GPU memory.
//!
//! The channel lives in an address space of its own. Its control memory holds the push buffer,
//! the GPFIFO ring listing the parts of the push buffer to run, and a semaphore the copy engine
//! releases once done. Work is submitted synchronously: one push buffer at a time, waited for
//! through the semaphore.

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
            Channel,
            Client,
            CopyEngine,
            Handle,
            Subdevice, //
        },
        fifo::{
            copy_c7b5,
            host_c56f, //
        },
        rm::{
            commands::{
                AllocChannelGpfifoAmpere,
                AllocCopyEngineAmpereB,
                CeGetFaultMethodBufferSize,
                FifoGetDeviceInfoTable,
                GpfifoBind,
                GpfifoSchedule,
                RmAlloc,
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
        VirtualAddress,
        VramAddress, //
    },
    num,
    regs, //
};

use super::vaspace::AddressSpace;

/// Handle of the address space of the channel, after nouveau's.
const VA_SPACE_HANDLE: u32 = 0x90f1_0000;
/// Handle of the channel, after nouveau's, completed with its channel ID.
const CHANNEL_HANDLE: u32 = 0xf1f0_0000;
/// Handle of the copy engine object of the channel.
const COPY_ENGINE_HANDLE: u32 = 0xc7b5_0000;

/// Channel ID of the channel. GSP-RM keeps channel ID 0 for itself.
const CHID: u32 = 1;

/// Subchannel the copy engine object is bound to, as recommended for copy engines.
const SUBCHANNEL: u32 = 4;

// Layout of the control memory of the channel.

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
/// Offset of the semaphore the copy engine releases.
const SEMAPHORE: usize = GPFIFO + num::u32_as_usize(GPFIFO_ENTRIES) * host_c56f::GP_ENTRY_SIZE;
static_assert!(SEMAPHORE + size_of::<u32>() <= CONTROL_SIZE);

/// How long work may take: much longer than any fill of a framebuffer.
const TIMEOUT: Delta = Delta::from_secs(1);

/// A kernel channel on a copy engine.
///
/// It must be released with [`CopyChannel::free`] while the GSP is running.
pub(crate) struct CopyChannel<'a> {
    client: Handle<Client>,
    address_space: AddressSpace,
    /// The channel, once allocated.
    channel: Option<Handle<Channel>>,
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
    /// Value of the semaphore released by the last submission.
    sequence: u32,
}

impl<'a> CopyChannel<'a> {
    /// Allocates a channel on a copy engine that doesn't share its runlist with the graphics
    /// engine, under the device of `rm`.
    pub(crate) fn new(
        dev: &'a device::Device<device::Bound>,
        cmdq: &Cmdq<'_>,
        mm: &mut GpuMm<'_>,
        chipset: Chipset,
        rm: &RmDevice,
    ) -> Result<Self> {
        let (client, device, subdevice) = (rm.client(), rm.device(), rm.subdevice());
        let engine = Self::find_engine(dev, cmdq, client, subdevice)?;
        let copy_engine_index = engine.copy_engine_index().ok_or(ENODEV)?;
        dev_dbg!(dev, "channel: copy engine {:?}\n", engine);

        let method_buffer_size =
            RmControl::new(client, subdevice, CeGetFaultMethodBufferSize::new()).send(dev, cmdq)?;
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
            VA_SPACE_HANDLE,
        )?;

        let mut channel = Self {
            client,
            address_space,
            channel: None,
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

            let memory = ChannelMemory {
                instance: channel.instance.range().start.into_raw(),
                userd: channel.userd.range().start.into_raw(),
                method_buffer: channel.method_buffer.dma_address(),
                method_buffer_size: u64::from(method_buffer_size),
            };
            let handle = RmAlloc::new(
                client,
                device,
                Handle::new(CHANNEL_HANDLE | CHID),
                AllocChannelGpfifoAmpere::kernel(
                    channel.address_space.handle(),
                    engine.engine_type,
                    CHID,
                    gpfifo,
                    GPFIFO_ENTRIES,
                    &memory,
                ),
            )
            .send(dev, cmdq)?;
            channel.channel = Some(handle);

            RmControl::new(client, handle, GpfifoBind::new(engine.engine_type)).send(dev, cmdq)?;
            RmControl::new(client, handle, GpfifoSchedule::enable()).send(dev, cmdq)?;

            // Freed along with the channel.
            let _copy_engine: Handle<CopyEngine> = RmAlloc::new(
                client,
                handle,
                Handle::new(COPY_ENGINE_HANDLE),
                AllocCopyEngineAmpereB::new(copy_engine_index),
            )
            .send(dev, cmdq)?;

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

    /// Returns a copy engine whose runlist has no graphics engine, so that its channels don't
    /// wait for graphics work.
    fn find_engine(
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        client: Handle<Client>,
        subdevice: Handle<Subdevice>,
    ) -> Result<EngineInfo> {
        let mut engines = KVec::new();
        let mut base = 0;
        loop {
            let (page, more) = RmControl::new(client, subdevice, FifoGetDeviceInfoTable::new(base))
                .send(dev, cmdq)?;
            engines.extend_from_slice(&page, GFP_KERNEL)?;
            if !more {
                break;
            }
            base += FifoGetDeviceInfoTable::PAGE_ENTRIES;
        }

        let shares_graphics = |runlist| {
            engines
                .iter()
                .any(|engine| engine.is_graphics() && engine.runlist == runlist)
        };

        engines
            .iter()
            .copied()
            .find(|engine| engine.copy_engine_index().is_some() && !shares_graphics(engine.runlist))
            .ok_or(ENODEV)
    }

    /// Maps the page-aligned VRAM `range` into the channel's address space, returning its GPU
    /// virtual address through [`MappedRange::start`]. The mapping must be given back to
    /// [`CopyChannel::unmap`].
    pub(crate) fn map_vram(
        &mut self,
        mm: &mut GpuMm<'_>,
        range: core::ops::Range<VramAddress>,
    ) -> Result<MappedRange> {
        self.address_space.map_vram(mm, range, Access::READ_WRITE)
    }

    /// Tears down a mapping made by [`CopyChannel::map_vram`].
    pub(crate) fn unmap(&mut self, mm: &mut GpuMm<'_>, mapped: MappedRange) -> Result {
        self.address_space.unmap(mm, mapped)
    }

    /// Fills `height` lines of `width` 32-bit words with `value`, the first at GPU virtual address
    /// `addr`, the next ones `pitch` bytes apart. Returns once the fill is done.
    pub(crate) fn fill(
        &mut self,
        bar: Bar0<'_>,
        mm: &mut GpuMm<'_>,
        addr: VirtualAddress,
        pitch: u32,
        width: u32,
        height: u32,
        value: u32,
    ) -> Result {
        let semaphore = self.control_va()? + num::usize_as_u64(SEMAPHORE);
        let addr = addr.into_raw();
        let sequence = self.sequence.wrapping_add(1);
        let methods = |method, count| host_c56f::methods(SUBCHANNEL, method, count);

        // CAST: the upper and lower halves of addresses are set separately.
        let push = [
            methods(host_c56f::SET_OBJECT, 1)?,
            copy_c7b5::CLASS,
            methods(copy_c7b5::SET_SEMAPHORE_A, 3)?,
            (semaphore >> 32) as u32,
            semaphore as u32,
            sequence,
            methods(copy_c7b5::SET_REMAP_CONST_A, 3)?,
            value,
            0,
            copy_c7b5::REMAP_FILL_32,
            methods(copy_c7b5::OFFSET_OUT_UPPER, 2)?,
            (addr >> 32) as u32,
            addr as u32,
            methods(copy_c7b5::PITCH_IN, 4)?,
            pitch,
            pitch,
            width,
            height,
            methods(copy_c7b5::LAUNCH_DMA, 1)?,
            copy_c7b5::LAUNCH_FILL_AND_RELEASE,
        ];

        self.submit(bar, mm, &push)?;
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

    /// Writes `push` to the push buffer, and has the channel run it.
    fn submit(&mut self, bar: Bar0<'_>, mm: &mut GpuMm<'_>, push: &[u32]) -> Result {
        let control = self.control.range().start;
        let words = u32::try_from(push.len())?;
        if push.len() * size_of::<u32>() > PUSH_BUFFER_SIZE {
            return Err(EINVAL);
        }

        mm.write_vram(control + num::usize_as_u64(PUSH_BUFFER), push)?;

        let entry =
            host_c56f::gp_entry(self.control_va()? + num::usize_as_u64(PUSH_BUFFER), words)?;
        let entry_offset = GPFIFO + num::u32_as_usize(self.put) * host_c56f::GP_ENTRY_SIZE;
        mm.write_vram(control + num::usize_as_u64(entry_offset), &entry)?;

        self.put = (self.put + 1) % GPFIFO_ENTRIES;
        let userd = self.userd.range().start;
        mm.write_vram(
            userd + num::usize_as_u64(host_c56f::USERD_GP_PUT),
            &[self.put],
        )?;

        // PRAMIN and the doorbell are both in BAR0: the channel sees the writes above when told
        // to look.
        bar.write_reg(
            regs::NV_VIRTUAL_FUNCTION_DOORBELL::zeroed()
                .try_with_vector(CHID)?
                .try_with_runlist_id(self.engine.runlist)?,
        );

        Ok(())
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
                "nova-core: copy channel: work not done: semaphore {:?}, GPGet {:?}, GPPut {}\n",
                mm.read_vram(semaphore),
                gp_get,
                self.put
            );
        }

        result.map(|_| ())
    }

    /// Frees the channel and its address space. Its memory is freed when it is dropped.
    pub(crate) fn free(&mut self, dev: &device::Device, cmdq: &Cmdq<'_>, mm: &mut GpuMm<'_>) {
        // Freeing the channel frees its copy engine object.
        if let Some(channel) = self.channel.take() {
            let _ = RmFree::new(self.client, channel)
                .send(dev, cmdq)
                .inspect_err(|e| dev_err!(dev, "channel: can't free the channel: {:?}\n", e));
        }
        if let Some(mapped) = self.control_mapped.take() {
            let _ = self.address_space.unmap(mm, mapped);
        }
        let _ = self
            .address_space
            .free(dev, cmdq)
            .inspect_err(|e| dev_err!(dev, "channel: can't free the address space: {:?}\n", e));
    }
}

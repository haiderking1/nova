// SPDX-License-Identifier: GPL-2.0

//! Channels of GA10x GPUs (but GA100): GPFIFO channels of class `AMPERE_CHANNEL_GPFIFO_A`, copy
//! engines of class `AMPERE_DMA_COPY_B`, and a graphics engine of classes `AMPERE_B` and
//! `AMPERE_COMPUTE_B`, running SM 8.6 shaders.

use kernel::{
    device,
    io::Io,
    prelude::*, //
};

use crate::{
    driver::Bar0,
    gsp::{
        cmdq::Cmdq,
        commands::{
            Channel as RmChannel,
            Client,
            Handle, //
        },
        fifo::{
            compute_c7c0::{
                self,
                Qmd, //
            },
            copy_c7b5,
            host_c56f, //
        },
        rm::commands::{
            AllocChannelGpfifoAmpere,
            AllocComputeAmpereB,
            AllocCopyEngineAmpereB,
            AllocInlineToMemoryKeplerB,
            AllocThreeDAmpereB,
            AllocTwoDFermiA,
            RmAlloc, //
        },
        EngineInfo, RmObjectClass,
    },
    regs,
};

use super::{
    super::gpfifo::Channel,
    ChannelHal,
    Classes,
    GpEntry,
    GpfifoAlloc,
    GpfifoParent,
    Grid,
    PitchCopy,
    Release, //
};

/// Subchannel the compute object is bound to, as in NVIDIA's drivers.
const COMPUTE_SUBCHANNEL: u32 = 1;
/// Subchannel the copy engine object is bound to, as recommended for copy engines.
const COPY_SUBCHANNEL: u32 = 4;

// GPFIFO entries are as the class defines them.
static_assert!(host_c56f::GP_ENTRY_SIZE == size_of::<GpEntry>());

struct Ga102;

pub(super) const GA102_HAL: &dyn ChannelHal = &Ga102;

impl ChannelHal for Ga102 {
    fn classes(&self) -> Classes {
        Classes {
            gpfifo: RmObjectClass::ChannelGpfifoAmpere.into(),
            copy: RmObjectClass::CopyEngineAmpereB.into(),
            two_d: RmObjectClass::TwoDFermiA.into(),
            three_d: RmObjectClass::ThreeDAmpereB.into(),
            inline_to_memory: RmObjectClass::InlineToMemoryKeplerB.into(),
            compute: RmObjectClass::ComputeAmpereB.into(),
        }
    }

    fn alloc_gpfifo(
        &self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        client: Handle<Client>,
        handle: u32,
        alloc: &GpfifoAlloc<'_>,
    ) -> Result<Handle<RmChannel>> {
        let handle = Handle::new(handle);
        let (engine_type, chid, gpfifo, entries, memory, privileged) = (
            alloc.engine_type,
            alloc.chid,
            alloc.gpfifo,
            alloc.entries,
            alloc.memory,
            alloc.privileged,
        );

        Ok(match alloc.parent {
            GpfifoParent::Device { device, va_space } => RmAlloc::new(
                client,
                device,
                handle,
                AllocChannelGpfifoAmpere::alone(
                    va_space,
                    engine_type,
                    chid,
                    gpfifo,
                    entries,
                    memory,
                    privileged,
                ),
            )
            .send(dev, cmdq)?,
            GpfifoParent::Group {
                group,
                context_share,
            } => RmAlloc::new(
                client,
                group,
                handle,
                AllocChannelGpfifoAmpere::in_group(
                    context_share,
                    engine_type,
                    chid,
                    gpfifo,
                    entries,
                    memory,
                    privileged,
                ),
            )
            .send(dev, cmdq)?,
        })
    }

    fn userd_gp_get(&self) -> usize {
        host_c56f::USERD_GP_GET
    }

    fn userd_gp_put(&self) -> usize {
        host_c56f::USERD_GP_PUT
    }

    fn gp_entry(&self, addr: u64, words: u32, wait: bool) -> Result<GpEntry> {
        host_c56f::gp_entry(addr, words, wait)
    }

    fn push_release(&self, push: &mut KVec<u32>, release: Release) -> Result {
        // The memory writes of the work must be visible before the release, which the release's
        // own wait for idle doesn't ensure.
        push.extend_from_slice(&host_c56f::wait_idle_and_flush()?, GFP_KERNEL)?;
        push.extend_from_slice(
            &host_c56f::semaphore_release(release.addr, release.payload)?,
            GFP_KERNEL,
        )?;

        Ok(())
    }

    fn push_notify(&self, push: &mut KVec<u32>) -> Result {
        // The release must be visible to the CPU once the interrupt is, like nouveau orders them
        // (`gv100_fence_emit32()`).
        push.extend_from_slice(&host_c56f::sys_membar()?, GFP_KERNEL)?;
        push.extend_from_slice(&host_c56f::non_stall_interrupt()?, GFP_KERNEL)?;

        Ok(())
    }

    fn ring_doorbell(&self, bar: Bar0<'_>, engine: &EngineInfo, chid: u32) -> Result {
        bar.write_reg(
            regs::NV_VIRTUAL_FUNCTION_DOORBELL::zeroed()
                .try_with_vector(chid)?
                .try_with_runlist_id(engine.runlist)?,
        );

        Ok(())
    }

    fn alloc_copy(
        &self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        channel: &mut Channel<'_>,
        handle: u32,
        index: u32,
    ) -> Result {
        channel
            .alloc_object(dev, cmdq, handle, AllocCopyEngineAmpereB::new(index))
            .map(|_| ())
    }

    fn push_fill(
        &self,
        push: &mut KVec<u32>,
        addr: u64,
        pitch: u32,
        width: u32,
        height: u32,
        value: u32,
    ) -> Result {
        let methods = |method, count| host_c56f::methods(COPY_SUBCHANNEL, method, count);

        // CAST: the upper and lower halves of addresses are set separately.
        push.extend_from_slice(
            &[
                methods(host_c56f::SET_OBJECT, 1)?,
                copy_c7b5::CLASS,
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
                copy_c7b5::LAUNCH_FILL,
            ],
            GFP_KERNEL,
        )?;

        Ok(())
    }

    fn push_copy(&self, push: &mut KVec<u32>, copy: &PitchCopy) -> Result {
        let methods = |method, count| host_c56f::methods(COPY_SUBCHANNEL, method, count);

        let dst = copy.dst.into_raw();

        // CAST: the upper and lower halves of addresses are set separately.
        push.extend_from_slice(
            &[
                methods(host_c56f::SET_OBJECT, 1)?,
                copy_c7b5::CLASS,
                methods(copy_c7b5::SET_DST_PHYS_MODE, 1)?,
                copy_c7b5::PHYS_MODE_VRAM,
                methods(copy_c7b5::OFFSET_IN_UPPER, 4)?,
                (copy.src >> 32) as u32,
                copy.src as u32,
                (dst >> 32) as u32,
                dst as u32,
                methods(copy_c7b5::PITCH_IN, 4)?,
                copy.src_pitch,
                copy.dst_pitch,
                copy.line_bytes,
                copy.lines,
                methods(copy_c7b5::LAUNCH_DMA, 1)?,
                copy_c7b5::LAUNCH_COPY | copy_c7b5::LAUNCH_DST_PHYSICAL,
            ],
            GFP_KERNEL,
        )?;

        Ok(())
    }

    fn alloc_3d(
        &self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        channel: &mut Channel<'_>,
        handle: u32,
    ) -> Result {
        channel
            .alloc_object(dev, cmdq, handle, AllocThreeDAmpereB)
            .map(|_| ())
    }

    fn alloc_2d(
        &self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        channel: &mut Channel<'_>,
        handle: u32,
    ) -> Result {
        channel
            .alloc_object(dev, cmdq, handle, AllocTwoDFermiA)
            .map(|_| ())
    }

    fn alloc_inline_to_memory(
        &self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        channel: &mut Channel<'_>,
        handle: u32,
    ) -> Result {
        channel
            .alloc_object(dev, cmdq, handle, AllocInlineToMemoryKeplerB)
            .map(|_| ())
    }

    fn alloc_compute(
        &self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        channel: &mut Channel<'_>,
        handle: u32,
    ) -> Result {
        channel
            .alloc_object(dev, cmdq, handle, AllocComputeAmpereB)
            .map(|_| ())
    }

    fn push_compute_init(&self, push: &mut KVec<u32>) -> Result {
        let methods = |method, count| host_c56f::methods(COMPUTE_SUBCHANNEL, method, count);

        // Windows of shared and local memory in the generic address space, as in Mesa.
        push.extend_from_slice(
            &[
                methods(host_c56f::SET_OBJECT, 1)?,
                compute_c7c0::CLASS,
                methods(compute_c7c0::SET_SHADER_SHARED_MEMORY_WINDOW_A, 2)?,
                0,
                0xfe00_0000,
                methods(compute_c7c0::SET_SHADER_LOCAL_MEMORY_WINDOW_A, 2)?,
                0,
                0xff00_0000,
                methods(compute_c7c0::INVALIDATE_SKED_CACHES, 1)?,
                0,
            ],
            GFP_KERNEL,
        )?;

        Ok(())
    }

    fn qmd(&self, program: u64, grid: &Grid, release: Option<Release>) -> Result<KVec<u32>> {
        let mut qmd = Qmd::new(program, grid.blocks, grid.threads, grid.registers);
        if let Some(release) = release {
            qmd = qmd.release(release.addr, release.payload)?;
        }

        let mut words = KVec::new();
        words.extend_from_slice(qmd.words(), GFP_KERNEL)?;
        Ok(words)
    }

    fn push_compute_launch(&self, push: &mut KVec<u32>, qmd: u64) -> Result {
        let methods = |method, count| host_c56f::methods(COMPUTE_SUBCHANNEL, method, count);
        if qmd % u64::try_from(compute_c7c0::QMD_SIZE)? != 0 || qmd >> 40 != 0 {
            return Err(EINVAL);
        }

        // CAST: the descriptor's address was checked to fit in 40 bits.
        push.extend_from_slice(
            &[
                methods(compute_c7c0::SEND_PCAS_A, 1)?,
                (qmd >> 8) as u32,
                methods(compute_c7c0::SEND_SIGNALING_PCAS2_B, 1)?,
                compute_c7c0::PCAS_ACTION_INVALIDATE_COPY_SCHEDULE,
            ],
            GFP_KERNEL,
        )?;

        Ok(())
    }

    fn shader_model(&self) -> u8 {
        86
    }
}

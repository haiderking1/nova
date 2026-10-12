// SPDX-License-Identifier: GPL-2.0

//! Channels on the video decoder engines (NVDEC).
//!
//! GSP-RM boots and runs the falcons of the video decoders, but a channel on one needs a context
//! of its own: a buffer the kernel allocates in VRAM, cleared, maps into the channel's address
//! space and gives to GSP-RM, as nouveau does (`r535_flcn_ctor()`). Its size is the one GSP-RM
//! reports for the engine (`NV2080_CTRL_CMD_GPU_GET_CONSTRUCTED_FALCON_INFO`).

use core::ops::Range;

use kernel::{
    device,
    prelude::*,
    ptr::Alignment,
    sizes::SZ_4K, //
};

use crate::{
    gsp::{
        cmdq::Cmdq,
        rm::{
            commands::{
                GpuGetConstructedFalconInfo,
                GpuPromoteCtx,
                RmControl, //
            },
            device::RmDevice,
        },
        EngineInfo,
    },
    mm::{
        pagetable::Access,
        vmm::MappedRange,
        ContiguousVram,
        GpuMm,
        MmAccess,
        VramAddress, //
    },
    num,
};

use super::{
    gpfifo::Channel,
    vaspace::AddressSpace, //
};

/// What userspace drivers need to know to build work for the video decoder engines.
#[derive(Clone, Copy, Debug)]
pub struct VideoDecoderInfo {
    /// Class of the video decoder objects.
    pub class: u32,
}

/// Returns the first video decoder engine of `engines`.
///
/// Fails with `ENODEV` if the GPU has none.
pub(crate) fn find_engine(engines: &[EngineInfo]) -> Result<EngineInfo> {
    engines
        .iter()
        .copied()
        .find(|engine| engine.video_decoder_index().is_some())
        .ok_or(ENODEV)
}

/// The context of a channel on a video decoder engine, mapped into the channel's address space.
pub(crate) struct DecoderContext {
    /// The buffer of the context.
    _vram: ContiguousVram,
    /// Its mapping into the channel's address space.
    mapped: MappedRange,
}

impl DecoderContext {
    /// Sets up the context of `channel`, which runs in `space` on the video decoder `engine`:
    /// allocates it, has `clear` clear it, maps it into `space`, and gives it to GSP-RM.
    #[expect(clippy::too_many_arguments)]
    pub(crate) fn set_up(
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        mm: &mut impl MmAccess,
        clear: &mut impl FnMut(Range<VramAddress>) -> Result,
        rm: &RmDevice,
        channel: &Channel<'_>,
        space: &mut AddressSpace,
        engine: EngineInfo,
    ) -> Result<Self> {
        let falcons = RmControl::new(
            rm.client(),
            rm.subdevice(),
            GpuGetConstructedFalconInfo::new(),
        )
        .send(dev, cmdq)?;
        let size = falcons
            .iter()
            .find(|falcon| falcon.descriptor == engine.descriptor)
            .map(|falcon| falcon.size)
            .filter(|&size| size != 0)
            .ok_or_else(|| {
                dev_err!(dev, "nvdec: GSP-RM reports no context for the engine\n");
                ENODEV
            })?;
        let size = num::u32_as_usize(size)
            .checked_next_multiple_of(SZ_4K)
            .ok_or(EINVAL)?;

        // The memory manager is only taken for the allocation and the mapping: not while the
        // context is cleared, nor while GSP-RM takes it.
        let vram = mm.with(|mm| mm.alloc_contiguous(size, Alignment::new::<SZ_4K>()))?;
        clear(vram.range())?;
        let mapped = mm.with(|mm| {
            space.map_vram_aligned(
                mm,
                vram.range(),
                num::usize_as_u64(SZ_4K),
                Access::READ_WRITE,
            )
        })?;
        let context = Self {
            _vram: vram,
            mapped,
        };

        let promoted = channel.handle().and_then(|handle| {
            let command = GpuPromoteCtx::falcon(
                engine.engine_type,
                channel.client(),
                handle,
                channel.chid(),
                context.mapped.start().into_raw(),
                num::usize_as_u64(size),
            );
            RmControl::new(rm.client(), rm.subdevice(), command)
                .send(dev, cmdq)
                .inspect_err(|e| dev_err!(dev, "nvdec: GSP-RM rejected the context: {:?}\n", e))
                .map_err(Error::from)
        });
        if let Err(err) = promoted {
            mm.with(|mm| context.unmap(mm, space));
            return Err(err);
        }

        Ok(context)
    }

    /// Unmaps the context from `space`, the address space of its channel, which GSP-RM must have
    /// freed.
    pub(crate) fn unmap(self, mm: &mut GpuMm<'_>, space: &mut AddressSpace) {
        let _ = space.unmap(mm, self.mapped);
    }

    /// Forgets the mapping of the context: its address space is gone with the GPU.
    pub(crate) fn forget(self) {
        self.mapped.forget();
    }
}

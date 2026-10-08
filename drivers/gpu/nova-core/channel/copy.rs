// SPDX-License-Identifier: GPL-2.0

//! A kernel channel on a copy engine, which Nova uses to fill and copy GPU memory.

use core::ops::Range;

use kernel::{
    device,
    prelude::*, //
};

use crate::{
    driver::Bar0,
    gpu::Chipset,
    gsp::{
        cmdq::Cmdq,
        fifo::{
            copy_c7b5,
            host_c56f, //
        },
        rm::{
            commands::AllocCopyEngineAmpereB,
            device::RmDevice, //
        },
        EngineInfo,
    },
    mm::{
        pagetable::Access,
        vmm::MappedRange,
        GpuMm,
        VirtualAddress,
        VramAddress, //
    },
};

use super::{
    gpfifo::Channel,
    vaspace::Mode, //
};

/// Handle of the copy engine object of the channel.
const COPY_ENGINE_HANDLE: u32 = 0xc7b5_0000;

/// Subchannel the copy engine object is bound to, as recommended for copy engines.
const SUBCHANNEL: u32 = 4;

/// A kernel channel on a copy engine.
///
/// It must be released with [`CopyChannel::free`] while the GSP is running.
pub(crate) struct CopyChannel<'a> {
    channel: Channel<'a>,
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
        let engine = find_engine(&super::engines(dev, cmdq, rm)?)?;
        let index = engine.copy_engine_index().ok_or(ENODEV)?;
        dev_dbg!(dev, "channel: copy engine {:?}\n", engine);

        let mut channel = Channel::new(
            dev,
            cmdq,
            mm,
            chipset,
            rm,
            engine,
            super::COPY_CHID,
            true,
            Mode::ClientOwned,
        )?;
        if let Err(err) = channel.alloc_object(
            dev,
            cmdq,
            COPY_ENGINE_HANDLE,
            AllocCopyEngineAmpereB::new(index),
        ) {
            channel.free(dev, cmdq, mm);
            return Err(err);
        }

        Ok(Self { channel })
    }

    /// Maps the page-aligned VRAM `range` into the channel's address space, returning its GPU
    /// virtual address through [`MappedRange::start`]. The mapping must be given back to
    /// [`CopyChannel::unmap`].
    pub(crate) fn map_vram(
        &mut self,
        mm: &mut GpuMm<'_>,
        range: Range<VramAddress>,
    ) -> Result<MappedRange> {
        self.channel.map_vram(mm, range, Access::READ_WRITE)
    }

    /// Tears down a mapping made by [`CopyChannel::map_vram`].
    pub(crate) fn unmap(&mut self, mm: &mut GpuMm<'_>, mapped: MappedRange) -> Result {
        self.channel.unmap(mm, mapped)
    }

    /// Fills `height` lines of `width` 32-bit words with `value`, the first at GPU virtual address
    /// `addr`, the next ones `pitch` bytes apart. Returns once the fill is done.
    #[expect(clippy::too_many_arguments)]
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
        let addr = addr.into_raw();
        let methods = |method, count| host_c56f::methods(SUBCHANNEL, method, count);

        // CAST: the upper and lower halves of addresses are set separately.
        let push = [
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
        ];

        self.channel.run(bar, mm, &push)
    }

    /// Frees the channel. Its memory is freed when it is dropped.
    pub(crate) fn free(&mut self, dev: &device::Device, cmdq: &Cmdq<'_>, mm: &mut GpuMm<'_>) {
        self.channel.free(dev, cmdq, mm);
    }
}

/// Returns a copy engine of `engines` whose runlist has no graphics engine, so that its channels
/// don't wait for graphics work.
fn find_engine(engines: &[EngineInfo]) -> Result<EngineInfo> {
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

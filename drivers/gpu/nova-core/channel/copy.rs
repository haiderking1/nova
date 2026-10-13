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
    gsp::{cmdq::Cmdq, rm::device::RmDevice, EngineInfo},
    mm::{
        pagetable::Access,
        vmm::{
            MappedRange,
            Memory, //
        },
        GpuMm,
        MmAccess,
        Pages,
        VirtualAddress,
        VramAddress, //
    },
};

use super::{
    gpfifo::{
        Channel,
        NewSpace, //
    },
    hal::{
        CopyDst,
        PitchCopy, //
    },
    vaspace::Mode, //
};

/// Handle of the copy engine object of the channel.
const COPY_ENGINE_HANDLE: u32 = 0xc7b5_0000;

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
            NewSpace::Own(Mode::ClientOwned),
        )?;
        if let Err(err) =
            channel
                .hal()
                .alloc_copy(dev, cmdq, &mut channel, COPY_ENGINE_HANDLE, index)
        {
            channel.free(dev, cmdq, mm, None);
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

    /// Like [`CopyChannel::map_vram`], for the pages `pages` of `memory`, in order.
    pub(crate) fn map_pages(
        &mut self,
        mm: &mut GpuMm<'_>,
        pages: Pages<'_>,
        memory: Memory,
    ) -> Result<MappedRange> {
        self.channel
            .map_pages(mm, pages, memory, Access::READ_WRITE)
    }

    /// Tears down a mapping made by [`CopyChannel::map_vram`] or [`CopyChannel::map_pages`].
    pub(crate) fn unmap(&mut self, mm: &mut GpuMm<'_>, mapped: MappedRange) -> Result {
        self.channel.unmap(mm, mapped)
    }

    /// Fills `height` lines of `width` 32-bit words with `value`, the first at GPU virtual address
    /// `addr`, the next ones `pitch` bytes apart. Returns once the fill is done.
    #[expect(clippy::too_many_arguments)]
    pub(crate) fn fill(
        &mut self,
        bar: Bar0<'_>,
        mm: &mut impl MmAccess,
        addr: VirtualAddress,
        pitch: u32,
        width: u32,
        height: u32,
        value: u32,
    ) -> Result {
        let mut push = KVec::new();
        self.channel.hal().push_fill(
            &mut push,
            CopyDst::Virtual(addr.into_raw()),
            pitch,
            width,
            height,
            value,
        )?;

        self.channel.run(bar, mm, &push)
    }

    /// Fills the VRAM of `range`, which must be 4 KiB aligned, with the 32-bit `value`, writing it
    /// by address, without a mapping. Returns once the fill is done.
    ///
    /// Unlike CPU writes through PRAMIN, the fill keeps the memory manager only to start it.
    pub(crate) fn fill_vram(
        &mut self,
        bar: Bar0<'_>,
        mm: &mut impl MmAccess,
        range: Range<VramAddress>,
        value: u32,
    ) -> Result {
        /// Bytes of a line of the fill.
        const LINE: u64 = 4096;

        let size = range.end - range.start;
        if range.start.into_raw() % LINE != 0 || size % LINE != 0 {
            return Err(EINVAL);
        }
        if size == 0 {
            return Ok(());
        }

        let mut push = KVec::new();
        // CAST: a line is 4 KiB, of 1024 32-bit words.
        self.channel.hal().push_fill(
            &mut push,
            CopyDst::Vram(range.start),
            LINE as u32,
            (LINE / 4) as u32,
            u32::try_from(size / LINE)?,
            value,
        )?;

        self.channel.run(bar, mm, &push)
    }

    /// Makes the `copies`, in order, between addresses of the channel's address space. Returns
    /// once they are done.
    pub(crate) fn copy(
        &mut self,
        bar: Bar0<'_>,
        mm: &mut impl MmAccess,
        copies: &[PitchCopy],
    ) -> Result {
        let mut push = KVec::new();
        for copy in copies {
            self.channel.hal().push_copy(&mut push, copy)?;
        }

        self.channel.run(bar, mm, &push)
    }

    /// Frees the channel. Its memory is freed when it is dropped.
    pub(crate) fn free(&mut self, dev: &device::Device, cmdq: &Cmdq<'_>, mm: &mut GpuMm<'_>) {
        self.channel.free(dev, cmdq, mm, None);
    }
}

/// Returns a copy engine of `engines` whose runlist has no graphics engine, so that its channels
/// don't wait for graphics work.
pub(crate) fn find_engine(engines: &[EngineInfo]) -> Result<EngineInfo> {
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

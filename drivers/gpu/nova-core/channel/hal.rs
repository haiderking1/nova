// SPDX-License-Identifier: GPL-2.0

//! What differs between the channels of GPU families: the classes of the channels and of the
//! engine objects on them, the methods of those classes, how the GPU is told about new work, the
//! format of compute work descriptors, and the shader instruction set.

use kernel::{
    device,
    prelude::*, //
};

use crate::{
    driver::Bar0,
    gpu::Chipset,
    gsp::{
        cmdq::Cmdq,
        commands::{
            Channel as RmChannel,
            ChannelGroup,
            Client,
            ContextShare,
            Device,
            Handle,
            VaSpace, //
        },
        ChannelMemory, EngineInfo,
    },
    mm::VramAddress,
};

use super::gpfifo::Channel;

mod ga102;

/// A GPFIFO entry: where a push buffer is and how long it is.
pub(crate) type GpEntry = [u32; 2];

/// Where a GPFIFO channel is allocated.
pub(crate) enum GpfifoParent {
    /// Under `device`, in the address space `va_space`. GSP-RM puts the channel in a channel group
    /// of its own.
    Device {
        device: Handle<Device>,
        va_space: Handle<VaSpace>,
    },
    /// In the channel group `group`, in its context share `context_share`.
    Group {
        group: Handle<ChannelGroup>,
        context_share: Handle<ContextShare>,
    },
}

/// What a GPFIFO channel is allocated with.
pub(crate) struct GpfifoAlloc<'a> {
    /// Where the channel is allocated, and the address space it runs in.
    pub(crate) parent: GpfifoParent,
    /// The `NV2080_ENGINE_TYPE` of the engine the channel runs on.
    pub(crate) engine_type: u32,
    /// The channel ID.
    pub(crate) chid: u32,
    /// GPU virtual address of the GPFIFO ring.
    pub(crate) gpfifo: u64,
    /// Number of entries of the GPFIFO ring, a power of two.
    pub(crate) entries: u32,
    /// The memory of the channel.
    pub(crate) memory: &'a ChannelMemory,
    /// Whether the channel may use the methods reserved to privileged channels: kernel channels
    /// may, those of clients mayn't.
    pub(crate) privileged: bool,
}

/// Size and shape of compute work.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Grid {
    /// Number of blocks of threads.
    pub(crate) blocks: [u32; 3],
    /// Number of threads of each block.
    pub(crate) threads: [u16; 3],
    /// Number of registers each thread uses.
    pub(crate) registers: u8,
}

/// A semaphore release: `payload` written to the 32-bit word at GPU virtual address `addr`, 4-byte
/// aligned, once the work before is done and its memory writes visible to the system.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Release {
    pub(crate) addr: u64,
    pub(crate) payload: u32,
}

/// A copy of `lines` lines of `line_bytes` bytes, the first from GPU virtual address `src` to VRAM
/// address `dst`, which the copy engine writes without a mapping, the next ones `src_pitch` and
/// `dst_pitch` bytes further.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PitchCopy {
    pub(crate) src: u64,
    pub(crate) src_pitch: u32,
    pub(crate) dst: VramAddress,
    pub(crate) dst_pitch: u32,
    pub(crate) line_bytes: u32,
    pub(crate) lines: u32,
}

/// Where the copy engine writes.
#[derive(Clone, Copy, Debug)]
pub(crate) enum CopyDst {
    /// A GPU virtual address of the channel's address space.
    Virtual(u64),
    /// A VRAM address, written without a mapping.
    Vram(VramAddress),
}

/// Classes of the channels and of the engine objects allocated on them.
#[derive(Clone, Copy, Debug)]
pub struct Classes {
    /// The GPFIFO channel.
    pub gpfifo: u32,
    /// The copy engine.
    pub copy: u32,
    /// The 2D engine, on channels on the graphics engine.
    pub two_d: u32,
    /// The 3D engine, on channels on the graphics engine.
    pub three_d: u32,
    /// The inline-to-memory engine, writing data from the push buffer to memory, on channels on
    /// the graphics engine.
    pub inline_to_memory: u32,
    /// The compute engine, on channels on the graphics engine.
    pub compute: u32,
}

pub(crate) trait ChannelHal: Sync {
    /// The classes of the channels and of the engine objects allocated on them.
    fn classes(&self) -> Classes;

    // The GPFIFO channel.

    /// Allocates the GPFIFO channel `handle` under the parent of `alloc`.
    fn alloc_gpfifo(
        &self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        client: Handle<Client>,
        handle: u32,
        alloc: &GpfifoAlloc<'_>,
    ) -> Result<Handle<RmChannel>>;

    /// Offset in the USERD of the GPFIFO entry the channel fetches next.
    fn userd_gp_get(&self) -> usize;

    /// Offset in the USERD of the GPFIFO entry the channel stops fetching at.
    fn userd_gp_put(&self) -> usize;

    /// Returns the GPFIFO entry of the push buffer of `words` 32-bit words at GPU virtual address
    /// `addr`, 4-byte aligned. If `wait`, the channel fetches it once the work before is done.
    fn gp_entry(&self, addr: u64, words: u32, wait: bool) -> Result<GpEntry>;

    /// Appends to `push` the methods doing `release` once the channel's engine is idle.
    fn push_release(&self, push: &mut KVec<u32>, release: Release) -> Result;

    /// Appends to `push` the methods raising the non-stall interrupt of the channel's engine, once
    /// the methods before are processed: after a release, to tell the CPU it happened.
    fn push_notify(&self, push: &mut KVec<u32>) -> Result;

    /// Tells the GPU that channel `chid`, on `engine`, has new work.
    fn ring_doorbell(&self, bar: Bar0<'_>, engine: &EngineInfo, chid: u32) -> Result;

    // The copy engine.

    /// Allocates the object `handle` of copy engine `index` on `channel`, which runs on it.
    fn alloc_copy(
        &self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        channel: &mut Channel<'_>,
        handle: u32,
        index: u32,
    ) -> Result;

    /// Appends to `push` the methods filling `height` lines of `width` 32-bit words with `value`,
    /// the first at `dst`, the next ones `pitch` bytes apart.
    fn push_fill(
        &self,
        push: &mut KVec<u32>,
        dst: CopyDst,
        pitch: u32,
        width: u32,
        height: u32,
        value: u32,
    ) -> Result;

    /// Appends to `push` the methods making `copy`.
    fn push_copy(&self, push: &mut KVec<u32>, copy: &PitchCopy) -> Result;

    // The graphics engine.

    /// Allocates the 3D object `handle` on `channel`, which runs on the graphics engine.
    fn alloc_3d(
        &self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        channel: &mut Channel<'_>,
        handle: u32,
    ) -> Result;

    /// Allocates the 2D object `handle` on `channel`, which runs on the graphics engine.
    fn alloc_2d(
        &self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        channel: &mut Channel<'_>,
        handle: u32,
    ) -> Result;

    /// Allocates the inline-to-memory object `handle` on `channel`, which runs on the graphics
    /// engine.
    fn alloc_inline_to_memory(
        &self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        channel: &mut Channel<'_>,
        handle: u32,
    ) -> Result;

    /// Allocates the compute object `handle` on `channel`, which runs on the graphics engine.
    fn alloc_compute(
        &self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        channel: &mut Channel<'_>,
        handle: u32,
    ) -> Result;

    /// Appends to `push` the methods binding the compute object to its subchannel and setting up
    /// the state compute work runs with.
    fn push_compute_init(&self, push: &mut KVec<u32>) -> Result;

    /// Returns the work descriptor of `grid`, running the program at GPU virtual address
    /// `program`, with no shared memory, and doing `release` once done. It must be at a GPU
    /// virtual address aligned to its size.
    fn qmd(&self, program: u64, grid: &Grid, release: Option<Release>) -> Result<KVec<u32>>;

    /// Appends to `push` the methods running the work described at GPU virtual address `qmd`.
    fn push_compute_launch(&self, push: &mut KVec<u32>, qmd: u64) -> Result;

    /// The version of the shader instruction set, as in `sm_86`.
    fn shader_model(&self) -> u8;
}

/// Returns the HAL of the channels of `chipset`, if Nova supports them.
pub(crate) fn channel_hal(chipset: Chipset) -> Option<&'static dyn ChannelHal> {
    match chipset {
        Chipset::GA102 | Chipset::GA103 | Chipset::GA104 | Chipset::GA106 | Chipset::GA107 => {
            Some(ga102::GA102_HAL)
        }
        _ => None,
    }
}

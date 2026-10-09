// SPDX-License-Identifier: GPL-2.0

//! The graphics engine (GR), which runs 3D and compute work.
//!
//! Channels on the graphics engine have context buffers, where the engine saves their state when
//! switching between channels. The client allocates them and gives them to GSP-RM, which
//! initializes them ("promotes" them). Some are global, shared by all channels; the others are
//! per channel. A channel's context starts as a copy of the golden context, which GSP-RM builds
//! once, the first time a 3D object is allocated on a channel whose context buffers it was given.
//!
//! This follows nouveau's handling of GSP-RM's graphics engine.

use kernel::{
    device,
    dma::Coherent,
    prelude::*,
    ptr::Alignment,
    sizes::{
        SZ_2M,
        SZ_4K,
        SZ_64K, //
    },
};

use crate::{
    display::InternalHandles,
    driver::Bar0,
    gpu::Chipset,
    gsp::{
        cmdq::Cmdq,
        rm::{
            commands::{
                GpuPromoteCtx,
                GrGetContextBuffersInfo,
                RmControl, //
            },
            device::RmDevice,
        },
        ContextBufferInfo, EngineInfo, GrContextBuffer, PromoteEntry, PromoteInit,
        GR_CONTEXT_BUFFERS, GR_UNRESTRICTED_PRIV_ACCESS_MAP,
    },
    mm::{
        pagetable::Access,
        vmm::MappedRange,
        ContiguousVram,
        GpuMm,
        VramAddress, //
    },
    num,
};

use super::{
    gpfifo::Channel,
    hal::{
        Grid,
        Release, //
    },
    vaspace::Mode, //
};

/// Handle of the 3D object allocated to build the golden context.
const THREE_D_HANDLE: u32 = 0xc797_0000;
/// Handle of the compute object of compute channels, completed with the channel ID.
const COMPUTE_HANDLE: u32 = 0xc7c0_0000;

/// Number of subcontexts whose headers end the main context buffer.
const SUBCONTEXT_HEADERS: u64 = 64;

/// Access of the graphics engine's context switch firmware to a context buffer.
fn buffer_access(buffer: &GrContextBuffer) -> Access {
    Access {
        writable: !buffer.read_only,
        privileged: true,
    }
}

/// How a context buffer is laid out in memory, after nouveau.
#[derive(Clone, Copy, Debug)]
struct Layout {
    /// Size in bytes.
    size: u64,
    /// Alignment of the buffer in VRAM: the size of the pages it would be mapped with.
    alignment: u64,
    /// Alignment of the buffer's GPU virtual address.
    va_alignment: u64,
}

impl Layout {
    fn new(buffer: &GrContextBuffer, info: &ContextBufferInfo) -> Result<Self> {
        let page = num::usize_as_u64(SZ_4K);
        let mut size = u64::from(info.size);
        if buffer.is_main() {
            size = size.next_multiple_of(page) + SUBCONTEXT_HEADERS * page;
        }
        size = size.next_multiple_of(page);

        let alignment = [SZ_2M, SZ_64K, SZ_4K]
            .into_iter()
            .map(num::usize_as_u64)
            .find(|&page| size >= page)
            .unwrap_or(page)
            .max(u64::from(info.alignment).next_power_of_two());
        let va_alignment = if buffer.is_attribute_cb() {
            size.next_power_of_two()
        } else {
            alignment
        };

        Ok(Self {
            size,
            alignment,
            va_alignment,
        })
    }

    /// Allocates a buffer of this layout, cleared if GSP-RM is to initialize it, as it expects.
    fn alloc(&self, mm: &mut GpuMm<'_>, cleared: bool) -> Result<ContiguousVram> {
        let size = usize::try_from(self.size)?;
        let alignment = Alignment::new_checked(usize::try_from(self.alignment)?).ok_or(EINVAL)?;
        let vram = mm.alloc_contiguous(size, alignment)?;
        if cleared {
            mm.clear_vram(vram.range())?;
        }

        Ok(vram)
    }
}

/// A global context buffer, in VRAM or coherent system memory as its [`GrContextBuffer`] says.
enum GlobalBuffer<'a> {
    Vram(ContiguousVram),
    System(Coherent<'a, [u8]>),
}

impl<'a> GlobalBuffer<'a> {
    /// Allocates `buffer` with `layout`, cleared if GSP-RM is to initialize it.
    fn alloc(
        dev: &'a device::Device<device::Bound>,
        mm: &mut GpuMm<'_>,
        buffer: &GrContextBuffer,
        layout: &Layout,
    ) -> Result<Self> {
        Ok(if buffer.sysmem {
            // Coherent allocations are zeroed and page-aligned.
            Self::System(Coherent::<u8>::zeroed_slice(
                dev,
                usize::try_from(layout.size)?,
                GFP_KERNEL,
            )?)
        } else {
            Self::Vram(layout.alloc(mm, buffer.init)?)
        })
    }

    /// Maps the buffer into `channel`'s address space with `access`, at a virtual address aligned
    /// to `alignment`.
    fn map(
        &self,
        channel: &mut Channel<'_>,
        mm: &mut GpuMm<'_>,
        alignment: u64,
        access: Access,
    ) -> Result<MappedRange> {
        match self {
            Self::Vram(vram) => channel.map_vram_aligned(mm, vram.range(), alignment, access),
            Self::System(memory) => {
                let start = memory.dma_address();
                let end = start + num::usize_as_u64(memory.size());
                channel.map_sysmem_aligned(mm, start..end, alignment, access)
            }
        }
    }

    /// Returns where the buffer is, for GSP-RM to initialize its first `size` bytes.
    fn init(&self, size: u64) -> PromoteInit {
        match self {
            Self::Vram(vram) => PromoteInit {
                addr: vram.range().start.into_raw(),
                size,
                sysmem: false,
            },
            Self::System(memory) => PromoteInit {
                addr: memory.dma_address(),
                size,
                sysmem: true,
            },
        }
    }
}

/// Where a context buffer of a channel's own is, for GSP-RM to initialize its first `size` bytes.
fn vram_init(vram: &ContiguousVram, size: u64) -> PromoteInit {
    PromoteInit {
        addr: vram.range().start.into_raw(),
        size,
        sysmem: false,
    }
}

/// Gives the context buffers of `channel` described by `entries` to GSP-RM.
fn promote(
    dev: &device::Device,
    cmdq: &Cmdq<'_>,
    rm: &RmDevice,
    channel: &Channel<'_>,
    entries: &[PromoteEntry],
) -> Result {
    let mut command = GpuPromoteCtx::graphics(channel.client(), channel.handle()?);
    for entry in entries {
        dev_dbg!(dev, "gr: promote {:?}\n", entry);
        command.push(entry)?;
    }

    Ok(RmControl::new(rm.client(), rm.subdevice(), command)
        .send(dev, cmdq)
        .inspect_err(|e| dev_err!(dev, "gr: GSP-RM rejected the context buffers: {:?}\n", e))?)
}

/// The context of the graphics engine: the layout of its context buffers, and the global ones.
///
/// GSP-RM has built the golden context, which channels' contexts start from.
pub(crate) struct GrContext<'a> {
    /// The graphics engine.
    engine: EngineInfo,
    /// Layouts of the context buffers of [`GR_CONTEXT_BUFFERS`], in order.
    layouts: KVec<Layout>,
    /// The global context buffers of [`GR_CONTEXT_BUFFERS`], in order, and `None` for the
    /// per-channel ones.
    globals: KVec<Option<GlobalBuffer<'a>>>,
    /// The unrestricted privileged register access map, only given to the golden context.
    unrestricted_priv_access_map: ContiguousVram,
}

impl<'a> GrContext<'a> {
    /// Allocates the global context buffers of the graphics engine `engine`, and has GSP-RM build
    /// the golden context, with a channel `golden_chid` of its own.
    ///
    /// Must be done before any other channel is allocated, like nouveau does.
    #[expect(clippy::too_many_arguments)]
    pub(crate) fn new(
        dev: &'a device::Device<device::Bound>,
        cmdq: &Cmdq<'_>,
        mm: &mut GpuMm<'_>,
        chipset: Chipset,
        rm: &RmDevice,
        internal: &InternalHandles,
        engine: EngineInfo,
        golden_chid: u32,
    ) -> Result<Self> {
        let infos = RmControl::new(
            internal.client,
            internal.subdevice,
            GrGetContextBuffersInfo::new(),
        )
        .send(dev, cmdq)?;

        let mut layouts = KVec::with_capacity(GR_CONTEXT_BUFFERS.len(), GFP_KERNEL)?;
        let mut globals = KVec::with_capacity(GR_CONTEXT_BUFFERS.len(), GFP_KERNEL)?;
        for (buffer, info) in GR_CONTEXT_BUFFERS.iter().zip(infos.iter()) {
            let layout = Layout::new(buffer, info)?;
            dev_dbg!(
                dev,
                "gr: context buffer {:?}: {:?} {:?}\n",
                buffer,
                info,
                layout
            );
            layouts.push(layout, GFP_KERNEL)?;
            globals.push(
                if buffer.global {
                    Some(GlobalBuffer::alloc(dev, mm, buffer, &layout)?)
                } else {
                    None
                },
                GFP_KERNEL,
            )?;
        }

        let priv_access_map = GR_CONTEXT_BUFFERS
            .iter()
            .position(GrContextBuffer::is_priv_access_map)
            .ok_or(EINVAL)?;
        let unrestricted = layouts[priv_access_map].alloc(mm, true)?;

        let context = Self {
            engine,
            layouts,
            globals,
            unrestricted_priv_access_map: unrestricted,
        };
        context.build_golden(dev, cmdq, mm, chipset, rm, golden_chid)?;

        Ok(context)
    }

    /// Has GSP-RM build the golden context, on a channel `chid` given all the context buffers.
    fn build_golden(
        &self,
        dev: &device::Device<device::Bound>,
        cmdq: &Cmdq<'_>,
        mm: &mut GpuMm<'_>,
        chipset: Chipset,
        rm: &RmDevice,
        chid: u32,
    ) -> Result {
        // The golden context is built when the 3D object is allocated: the channel runs no work.
        let mut channel = Channel::new(
            dev,
            cmdq,
            mm,
            chipset,
            rm,
            self.engine,
            chid,
            false,
            // GSP-RM maps buffers of its own to build the golden context.
            Mode::Split,
        )?;
        let mut own = KVec::new();
        let mut mappings = KVec::new();

        let mut build = |channel: &mut Channel<'_>, mm: &mut GpuMm<'_>| -> Result {
            let mut entries = KVec::new();
            for (i, buffer) in GR_CONTEXT_BUFFERS.iter().enumerate() {
                let layout = &self.layouts[i];
                let global = match &self.globals[i] {
                    Some(global) => global,
                    None => {
                        own.push(
                            GlobalBuffer::Vram(layout.alloc(mm, buffer.init)?),
                            GFP_KERNEL,
                        )?;
                        own.last().ok_or(EINVAL)?
                    }
                };

                let init = buffer.init.then(|| global.init(layout.size));
                // The golden context takes the privileged register access map unmapped.
                let gpu_va = if buffer.is_priv_access_map() {
                    None
                } else {
                    let mapped =
                        global.map(channel, mm, layout.va_alignment, buffer_access(buffer))?;
                    let va = mapped.start().into_raw();
                    mappings.push(mapped, GFP_KERNEL)?;
                    Some(va)
                };
                entries.push(
                    PromoteEntry {
                        promote_id: buffer.promote_id,
                        gpu_va,
                        init,
                    },
                    GFP_KERNEL,
                )?;

                if buffer.is_priv_access_map() {
                    let range = self.unrestricted_priv_access_map.range();
                    let mapped = channel.map_vram_aligned(
                        mm,
                        range.clone(),
                        layout.va_alignment,
                        buffer_access(buffer),
                    )?;
                    entries.push(
                        PromoteEntry {
                            promote_id: GR_UNRESTRICTED_PRIV_ACCESS_MAP,
                            gpu_va: Some(mapped.start().into_raw()),
                            init: Some(vram_init(&self.unrestricted_priv_access_map, layout.size)),
                        },
                        GFP_KERNEL,
                    )?;
                    mappings.push(mapped, GFP_KERNEL)?;
                }
            }

            promote(dev, cmdq, rm, channel, &entries)?;
            channel.hal().alloc_3d(dev, cmdq, channel, THREE_D_HANDLE)?;
            dev_info!(dev, "gr: golden context built\n");

            Ok(())
        };

        let result = build(&mut channel, mm);

        // GSP-RM keeps the golden context: the channel isn't needed anymore. Its context buffers
        // stay mapped until it is freed.
        channel.release(dev, cmdq);
        for mapped in mappings {
            let _ = channel.unmap(mm, mapped);
        }
        channel.free(dev, cmdq, mm);

        result
    }
}

/// A kernel channel on the graphics engine, with a compute object.
///
/// It must be released with [`ComputeChannel::free`] while the GSP is running.
pub(crate) struct ComputeChannel<'a> {
    channel: Channel<'a>,
    /// The per-channel context buffers.
    _buffers: KVec<ContiguousVram>,
    /// Mappings of the context buffers.
    mappings: KVec<MappedRange>,
}

impl<'a> ComputeChannel<'a> {
    /// Allocates channel `chid` on the graphics engine of `context`, under the device of `rm`,
    /// with a compute object.
    pub(crate) fn new(
        dev: &'a device::Device<device::Bound>,
        cmdq: &Cmdq<'_>,
        mm: &mut GpuMm<'_>,
        chipset: Chipset,
        rm: &RmDevice,
        context: &GrContext<'_>,
        chid: u32,
    ) -> Result<Self> {
        let channel = Channel::new(
            dev,
            cmdq,
            mm,
            chipset,
            rm,
            context.engine,
            chid,
            true,
            Mode::ClientOwned,
        )?;
        let mut compute = Self {
            channel,
            _buffers: KVec::new(),
            mappings: KVec::new(),
        };

        match compute.set_up(dev, cmdq, mm, rm, context, chid) {
            Ok(()) => Ok(compute),
            Err(err) => {
                compute.free(dev, cmdq, mm);
                Err(err)
            }
        }
    }

    fn set_up(
        &mut self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        mm: &mut GpuMm<'_>,
        rm: &RmDevice,
        context: &GrContext<'_>,
        chid: u32,
    ) -> Result {
        let mut entries = KVec::new();
        for (i, buffer) in GR_CONTEXT_BUFFERS.iter().enumerate() {
            let layout = &context.layouts[i];
            let access = buffer_access(buffer);
            let mut promote_id = buffer.promote_id;
            let (mapped, init) = match &context.globals[i] {
                // Like kernel RM gives kernel channels (`kgrctxGetRegisterAccessMapId()`): the
                // unrestricted privileged register access map, initialized with the golden
                // context, instead of the restricted one.
                Some(_) if buffer.is_priv_access_map() => {
                    promote_id = GR_UNRESTRICTED_PRIV_ACCESS_MAP;
                    let mapped = self.channel.map_vram_aligned(
                        mm,
                        context.unrestricted_priv_access_map.range(),
                        layout.va_alignment,
                        access,
                    )?;
                    (mapped, None)
                }
                // GSP-RM initialized the global buffers with the golden context.
                Some(global) => (
                    global.map(&mut self.channel, mm, layout.va_alignment, access)?,
                    None,
                ),
                None => {
                    let vram = layout.alloc(mm, buffer.init)?;
                    let mapped = self.channel.map_vram_aligned(
                        mm,
                        vram.range(),
                        layout.va_alignment,
                        access,
                    )?;
                    let init = buffer.init.then(|| vram_init(&vram, layout.size));
                    self._buffers.push(vram, GFP_KERNEL)?;
                    (mapped, init)
                }
            };

            entries.push(
                PromoteEntry {
                    promote_id,
                    gpu_va: Some(mapped.start().into_raw()),
                    init,
                },
                GFP_KERNEL,
            )?;
            self.mappings.push(mapped, GFP_KERNEL)?;
        }
        promote(dev, cmdq, rm, &self.channel, &entries)?;

        self.channel
            .hal()
            .alloc_compute(dev, cmdq, &self.channel, COMPUTE_HANDLE | chid)?;

        Ok(())
    }

    /// Binds the compute object to its subchannel, and sets up the compute state work runs with.
    pub(crate) fn init(&mut self, bar: Bar0<'_>, mm: &mut GpuMm<'_>) -> Result {
        let mut push = KVec::new();
        self.channel.hal().push_compute_init(&mut push)?;

        self.channel.run(bar, mm, &push)
    }

    /// Maps the page-aligned VRAM `range` into the channel's address space, returning its GPU
    /// virtual address through [`MappedRange::start`]. The mapping must be given back to
    /// [`ComputeChannel::unmap`].
    pub(crate) fn map_vram(
        &mut self,
        mm: &mut GpuMm<'_>,
        range: core::ops::Range<VramAddress>,
    ) -> Result<MappedRange> {
        self.channel.map_vram(mm, range, Access::READ_WRITE)
    }

    /// Tears down a mapping made by [`ComputeChannel::map_vram`].
    pub(crate) fn unmap(&mut self, mm: &mut GpuMm<'_>, mapped: MappedRange) -> Result {
        self.channel.unmap(mm, mapped)
    }

    /// Returns the work descriptor of `grid`, running the program at GPU virtual address
    /// `program`, and doing `release` once done. It must be at a GPU virtual address aligned to its
    /// size.
    pub(crate) fn qmd(
        &self,
        program: u64,
        grid: &Grid,
        release: Option<Release>,
    ) -> Result<KVec<u32>> {
        self.channel.hal().qmd(program, grid, release)
    }

    /// Returns the version of the instruction set of the shaders the channel runs, as in `sm_86`.
    pub(crate) fn shader_model(&self) -> u8 {
        self.channel.hal().shader_model()
    }

    /// Runs the compute work described by the work descriptor at GPU virtual address `qmd`, and
    /// returns once it is done.
    pub(crate) fn launch(&mut self, bar: Bar0<'_>, mm: &mut GpuMm<'_>, qmd: u64) -> Result {
        let mut push = KVec::new();
        self.channel.hal().push_compute_launch(&mut push, qmd)?;

        self.channel.run(bar, mm, &push)
    }

    /// Frees the channel and its context buffers.
    pub(crate) fn free(&mut self, dev: &device::Device, cmdq: &Cmdq<'_>, mm: &mut GpuMm<'_>) {
        // The context buffers stay mapped until the channel is freed.
        self.channel.release(dev, cmdq);
        while let Some(mapped) = self.mappings.pop() {
            let _ = self.channel.unmap(mm, mapped);
        }
        self.channel.free(dev, cmdq, mm);
    }
}

// SPDX-License-Identifier: GPL-2.0

use core::ops::Range;

use kernel::{
    device,
    dma::Device,
    fmt,
    gpu::buddy::GpuBuddyParams,
    io::Io,
    new_mutex,
    num::Bounded,
    pci,
    prelude::*,
    ptr::Alignment,
    sizes::{
        SizeConstants,
        SZ_4K, //
    },
    sync::{
        new_spinlock_irq,
        Arc,
        Mutex, //
    },
    uapi, //
};

use crate::{
    boot_display::BootDisplay,
    bounded_enum,
    channel::{
        self,
        gr::GrContext, //
    },
    display::{
        self,
        Display,
        DisplayEngine,
        DisplayId,
        InternalHandles,
        Scanout,
        ScanoutMode,
        VblankHandler,
        VblankHandlerSlot, //
    },
    driver::{
        Bar0,
        Bar1, //
    },
    falcon::{
        gsp::Gsp as GspFalcon,
        sec2::Sec2 as Sec2Falcon,
        Falcon, //
    },
    fb::SysmemFlush,
    fsp::Fsp,
    gsp::{
        self,
        cmdq::Cmdq,
        commands::GetGspStaticInfoReply,
        rm::{
            commands::{
                IntrGetKernelTable,
                RmControl, //
            },
            device::RmDevice,
        },
        ConnectorType,
        EngineInfo,
        Gsp,
        GspBootContext, //
    },
    irq::{
        self,
        display::DisplayIrq,
        gsp::GspIrq,
        interrupt_tree::{
            TopEnableGuard,
            Tree, //
        }, //
    },
    mm::{
        bar_user::BarUser,
        pagetable::MmuVersion,
        vmm::MappedRange,
        GpuMm,
        VramAddress,
        PAGE_SIZE, //
    },
    num::IntoSafeCast,
    vgpu::VgpuManager, //
};

mod hal;
mod regs;

macro_rules! define_chipset {
    ({ $($variant:ident),* $(,)* }) =>
    {
        ::kernel::macros::paste!(
        /// Enum representation of the GPU chipset.
        #[derive(fmt::Debug, Copy, Clone, PartialOrd, Ord, PartialEq, Eq)]
        #[repr(u32)]
        #[allow(missing_docs)]
        pub enum Chipset {
            $($variant = uapi::[<drm_nova_chipid_NOVA_DRM_CHIPID_ $variant:upper>]),*,
        }

        impl Chipset {
            /// All chipsets known to the driver.
            pub const ALL: &'static [Chipset] = &[
                $( Chipset::$variant, )*
            ];

            /// Returns the name of this chipset, in lowercase.
            ///
            /// # Examples
            ///
            /// ```
            /// let chipset = Chipset::GA102;
            /// assert_eq!(chipset.name(), "ga102");
            /// ```
            pub(crate) const fn name(&self) -> &'static str {
                match *self {
                $(
                    Chipset::$variant => stringify!([<$variant:lower>]),
                )*
                }
            }
        }

        // TODO[FPRI]: replace with something like derive(FromPrimitive)
        impl TryFrom<u32> for Chipset {
            type Error = kernel::error::Error;

            fn try_from(value: u32) -> Result<Self, Self::Error> {
                match value {
                    $(
                        uapi::[<drm_nova_chipid_NOVA_DRM_CHIPID_ $variant:upper>] => {
                            Ok(Chipset::$variant)
                        }
                    )*
                    _ => Err(ENODEV),
                }
            }
        }
    );
    }
}

define_chipset!({
    // Turing
    TU102,
    TU104,
    TU106,
    TU117,
    TU116,
    // Ampere
    GA100,
    GA102,
    GA103,
    GA104,
    GA106,
    GA107,
    // Hopper
    GH100,
    // Ada
    AD102,
    AD103,
    AD104,
    AD106,
    AD107,
    // Blackwell GB10x
    GB100,
    GB102,
    // Blackwell GB20x
    GB202,
    GB203,
    GB205,
    GB206,
    GB207,
});

impl Chipset {
    /// Returns the [`Architecture`] generation of this chipset.
    pub const fn arch(self) -> Architecture {
        match self {
            Self::TU102 | Self::TU104 | Self::TU106 | Self::TU117 | Self::TU116 => {
                Architecture::Turing
            }
            Self::GA100 | Self::GA102 | Self::GA103 | Self::GA104 | Self::GA106 | Self::GA107 => {
                Architecture::Ampere
            }
            Self::GH100 => Architecture::Hopper,
            Self::AD102 | Self::AD103 | Self::AD104 | Self::AD106 | Self::AD107 => {
                Architecture::Ada
            }
            Self::GB100 | Self::GB102 => Architecture::BlackwellGB10x,
            Self::GB202 | Self::GB203 | Self::GB205 | Self::GB206 | Self::GB207 => {
                Architecture::BlackwellGB20x
            }
        }
    }

    /// Returns the address range of the PCI config mirror space.
    pub(crate) fn pci_config_mirror_range(self) -> Range<u32> {
        hal::gpu_hal(self).pci_config_mirror_range()
    }

    /// Returns the MMU version for this chipset.
    pub(crate) fn mmu_version(self) -> MmuVersion {
        MmuVersion::from(self.arch())
    }
}

impl From<Chipset> for u32 {
    #[inline]
    fn from(value: Chipset) -> Self {
        // CAST: `Chipset` is `repr(u32)` and can thus be cast losslessly.
        value as u32
    }
}

// TODO
//
// The resulting strings are used to generate firmware paths, hence the
// generated strings have to be stable.
//
// Hence, replace with something like strum_macros derive(Display).
//
// For now, redirect to fmt::Debug for convenience.
impl fmt::Display for Chipset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

bounded_enum! {
    /// Enum representation of the GPU generation.
    #[derive(fmt::Debug, Copy, Clone)]
    #[repr(u32)]
    pub enum Architecture with TryFrom<Bounded<u32, 6>> {
        /// Turing (TU1xx).
        Turing = uapi::drm_nova_architecture_NOVA_DRM_ARCHITECTURE_TURING,
        /// Ampere (GA10x).
        Ampere = uapi::drm_nova_architecture_NOVA_DRM_ARCHITECTURE_AMPERE,
        /// Hopper (GH100).
        Hopper = uapi::drm_nova_architecture_NOVA_DRM_ARCHITECTURE_HOPPER,
        /// Ada Lovelace (AD10x).
        Ada = uapi::drm_nova_architecture_NOVA_DRM_ARCHITECTURE_ADA,
        /// Blackwell (GB10x).
        BlackwellGB10x =
            uapi::drm_nova_architecture_NOVA_DRM_ARCHITECTURE_BLACKWELL_GB10X,
        /// Blackwell (GB20x).
        BlackwellGB20x =
            uapi::drm_nova_architecture_NOVA_DRM_ARCHITECTURE_BLACKWELL_GB20X,
    }
}

impl From<Architecture> for u32 {
    #[inline]
    fn from(value: Architecture) -> Self {
        // CAST: `Architecture` is `repr(u32)` and can thus be cast losslessly.
        value as u32
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Revision {
    major: Bounded<u8, 4>,
    minor: Bounded<u8, 4>,
}

impl From<regs::NV_PMC_BOOT_42> for Revision {
    fn from(boot0: regs::NV_PMC_BOOT_42) -> Self {
        Self {
            major: boot0.major_revision().cast(),
            minor: boot0.minor_revision().cast(),
        }
    }
}

impl fmt::Display for Revision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:x}.{:x}", self.major, self.minor)
    }
}

/// Structure holding a basic description of the GPU: `Chipset` and `Revision`.
#[derive(Clone, Copy)]
pub struct Spec {
    /// The GPU chipset.
    pub chipset: Chipset,
    revision: Revision,
}

impl Spec {
    pub(crate) fn new(dev: &device::Device, bar: Bar0<'_>) -> Result<Spec> {
        // Some brief notes about boot0 and boot42, in chronological order:
        //
        // NV04 through NV50:
        //
        //    Not supported by Nova. boot0 is necessary and sufficient to identify these GPUs.
        //    boot42 may not even exist on some of these GPUs.
        //
        // Fermi through Volta:
        //
        //     Not supported by Nova. boot0 is still sufficient to identify these GPUs, but boot42
        //     is also guaranteed to be both present and accurate.
        //
        // Turing and later:
        //
        //     Supported by Nova. Identified by first checking boot0 to ensure that the GPU is not
        //     from an earlier (pre-Fermi) era, and then using boot42 to precisely identify the GPU.
        //     Somewhere in the Rubin timeframe, boot0 will no longer have space to add new GPU IDs.

        let boot0 = bar.read(regs::NV_PMC_BOOT_0);

        if boot0.is_older_than_fermi() {
            return Err(ENODEV);
        }

        let boot42 = bar.read(regs::NV_PMC_BOOT_42);
        Spec::try_from(boot42).inspect_err(|_| {
            dev_err!(dev, "Unsupported chipset: {}\n", boot42);
        })
    }
}

impl TryFrom<regs::NV_PMC_BOOT_42> for Spec {
    type Error = Error;

    fn try_from(boot42: regs::NV_PMC_BOOT_42) -> Result<Self> {
        Ok(Self {
            chipset: boot42.chipset()?,
            revision: boot42.into(),
        })
    }
}

impl fmt::Display for Spec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_fmt(fmt!(
            "Chipset: {}, Architecture: {:?}, Revision: {}",
            self.chipset,
            self.chipset.arch(),
            self.revision
        ))
    }
}

/// Self-contained resources to operate and drop the GSP.
#[pin_data(PinnedDrop)]
struct GspResources<'gpu> {
    /// Device owning the GPU.
    device: &'gpu pci::Device<device::Bound>,
    /// Details about the chipset.
    spec: Spec,
    /// MMIO mapping of PCI BAR 0.
    bar: Bar0<'gpu>,
    /// GSP falcon instance, used for GSP boot up and cleanup.
    gsp_falcon: Falcon<'gpu, GspFalcon>,
    /// SEC2 falcon instance, used for GSP boot up and cleanup.
    sec2_falcon: Falcon<'gpu, Sec2Falcon>,
    /// FSP instance, if on an arch that supports it.
    // TODO: use different resource types for each boot method, and make the relevant Gsp methods
    // generic against them.
    fsp: Option<Fsp<'gpu>>,
    /// vGPU state detected before GSP boot.
    vgpu: VgpuManager,
    /// GSP runtime data.
    #[pin]
    gsp: Gsp<'gpu>,
    /// GSP unload firmware bundle, if any.
    unload_bundle: Option<gsp::UnloadBundle<'gpu>>,
}

/// The GSP event handler's registration and the enable of its subtree at `TOP`.
///
/// The two drop as a unit, the handler first, on the drop of [`Gpu`] and on the error path of
/// its constructor alike, so that the subtree is disabled only after the handler is freed.
#[pin_data]
struct GspSubtree<'a> {
    #[pin]
    irq: GspIrq<'a>,
    /// Must be kept declared *after* `irq`. A handler still in flight enables the subtree again
    /// through its rearm.
    _top: TopEnableGuard<'a>,
}

impl<'a> GspSubtree<'a> {
    /// Returns an initializer that registers the GSP event handler and then enables its subtree at
    /// `TOP`.
    ///
    /// # Safety
    ///
    /// Callers must not `mem::forget()` the initialized `GspSubtree` or otherwise prevent its
    /// [`Drop`] implementation, which runs `free_irq`, from running.
    unsafe fn new(
        pdev: &'a pci::Device<device::Bound>,
        tree: &'a Tree<'a>,
        falcon: &'a Falcon<'a, GspFalcon>,
        cmdq: &'a Cmdq<'a>,
    ) -> impl PinInit<Self, Error> + 'a {
        try_pin_init!(Self {
            // SAFETY: this function's caller must not leak the `GspSubtree` that owns this
            // registration, so the registration's `Drop` runs.
            irq <- unsafe { GspIrq::new(pdev, tree, falcon, cmdq) },
            _top: tree.enable_top_guarded(),
        })
    }
}

/// Structure holding the resources required to operate the GPU.
#[pin_data(PinnedDrop)]
pub(crate) struct Gpu<'gpu> {
    pub(crate) spec: Spec,
    /// The display the system firmware set up on this GPU, if any.
    pub(crate) boot_display: Option<BootDisplay>,
    /// BAR1 mapping of the boot display's framebuffer, for the CPU to draw into.
    ///
    /// Unmapped in [`Gpu`]'s `PinnedDrop`, while `mm` and `bar_user` are still alive.
    #[pin]
    boot_scanout: Mutex<Option<MappedRange>>,
    /// GSP event interrupt registration, and the enable of its subtree.
    ///
    /// Must be kept declared *before* `gsp_resources`, so that the handler is unregistered, and
    /// any in-flight run of it has finished, before the command queue that it drains and the
    /// falcon that it reads are freed, and before the GSP is unloaded.
    #[pin]
    _gsp_subtree: GspSubtree<'gpu>,
    /// Display interrupt registration, if the GPU has a display engine whose interrupt the CPU
    /// services.
    ///
    /// Must be kept declared *before* `gsp_resources`, `vblank_handler` and `tree`, which its
    /// handler uses.
    display_irq: Option<Pin<KBox<DisplayIrq<'gpu>>>>,
    /// Static GPU information as provided by the GSP.
    pub(crate) gsp_static_info: GetGspStaticInfoReply,
    /// Nova's RM client, which its other RM objects are allocated under, if it could be allocated.
    ///
    /// Freed in [`Gpu`]'s `PinnedDrop`, while the GSP is still running.
    rm: Option<RmDevice>,
    /// RM objects for the display engine, if they could be allocated.
    ///
    /// Freed in [`Gpu`]'s `PinnedDrop`, while the GSP is still running.
    display: Option<Display>,
    /// The display the boot display is shown on, if the display engine reports one as active.
    boot_output: Option<DisplayId>,
    /// The display engine, once taken over from GSP-RM by [`Gpu::init_display_engine()`], and
    /// Nova's own scanout through it.
    ///
    /// Its RM objects are freed and the scanout unmapped in [`Gpu`]'s `PinnedDrop`, while the GSP
    /// is still running.
    #[pin]
    display_engine: Mutex<Option<DisplayOutput<'gpu>>>,
    /// The context of the graphics engine, once set up by [`Gpu::init_graphics()`].
    gr: Option<GrContext>,
    /// GPU memory manager owning memory management resources.
    ///
    /// Must be kept declared *before* `gsp_resources`, so that its components are dropped while
    /// the GSP is still operational.
    mm: GpuMm<'gpu>,
    /// BAR1 user interface for CPU access to GPU virtual memory.
    #[pin]
    bar_user: BarUser<'gpu>,
    /// GSP and its resources.
    #[pin]
    gsp_resources: GspResources<'gpu>,
    /// System memory page required for flushing all pending GPU-side memory writes done through
    /// PCIE into system memory, via sysmembar (A GPU-initiated HW memory-barrier operation).
    ///
    /// Must be kept declared *after* `gsp_resources`, as the latter's `PinnedDrop` implementation
    /// requires the sysmem flush page to be in place.
    sysmem_flush: SysmemFlush<'gpu>,
    /// The handler of vblanks that a client registered, which the display interrupt calls.
    ///
    /// Must be kept declared *after* `display_irq`, which holds a borrow of it.
    #[pin]
    vblank_handler: VblankHandlerSlot,
    /// Borrow of `vblank_handler` that `display_irq` holds, taken by hand like `tree_ref`.
    vblank_handler_ref: &'gpu VblankHandlerSlot,
    /// Borrow of `tree` that `_gsp_subtree` holds. A field that borrows a sibling field is
    /// self-referential, which `pin_init` cannot express, so the borrow is taken by hand.
    tree_ref: &'gpu Tree<'gpu>,
    /// The GIN CPU interrupt tree and the PCI vectors that deliver it.
    ///
    /// Must be kept declared *after* `_gsp_subtree`, which holds a borrow of it.
    #[pin]
    tree: Tree<'gpu>,
}

#[pinned_drop]
impl PinnedDrop for GspResources<'_> {
    fn drop(self: Pin<&mut Self>) {
        let this = self.project();
        let device = *this.device;
        let bar = *this.bar;
        let bundle = this.unload_bundle.take();

        let _ = this
            .gsp
            .as_ref()
            .get_ref()
            .unload(
                GspBootContext {
                    pdev: device,
                    bar,
                    chipset: this.spec.chipset,
                    gsp_falcon: &*this.gsp_falcon,
                    sec2_falcon: &*this.sec2_falcon,
                    fsp: this.fsp.as_mut(),
                    vgpu: &*this.vgpu,
                },
                bundle,
            )
            .inspect_err(|e| dev_err!(device, "failed to unload GSP: {:?}\n", e));
    }
}

impl<'gpu> Gpu<'gpu> {
    pub(crate) fn new<'a>(
        pdev: &'gpu pci::Device<device::Core<'a>>,
        bar: Bar0<'gpu>,
        bar1: Bar1<'gpu>,
    ) -> impl PinInit<Self, Error> + use<'gpu, 'a> {
        let dev = pdev.as_ref();

        try_pin_init!(Self {
            spec: Spec::new(dev, bar).inspect(|spec| {
                dev_info!(dev,"NVIDIA ({})\n", spec);
            })?,

            // Before the GSP boots: `BootDisplay::find()` relies on BAR1 still mapping VRAM 1:1.
            boot_display: BootDisplay::find(pdev).unwrap_or_else(|err| {
                dev_warn!(dev, "ignoring the firmware display: {:?}\n", err);
                None
            }),

            // Mapped by `Gpu::map_boot_display()` once the GPU is up.
            boot_scanout <- new_mutex!(None, "nova_boot_scanout"),

            tree: Tree::new(pdev, bar, spec.chipset, irq::gsp::GSP_SUBTREE.into())?,

            // SAFETY: `tree` is initialized above, is pinned at a stable address, and is dropped
            // after every field that uses `tree_ref` (struct field drop order).
            tree_ref: unsafe { &*core::ptr::from_ref(tree.as_ref().get_ref()) },

            vblank_handler <- new_spinlock_irq!(None, "nova_vblank_handler"),

            // SAFETY: `vblank_handler` is initialized above, is pinned at a stable address, and is
            // dropped after every field that uses `vblank_handler_ref` (struct field drop order).
            vblank_handler_ref: unsafe {
                &*core::ptr::from_ref(vblank_handler.as_ref().get_ref())
            },

            _: {
                let dma_mask = hal::gpu_hal(spec.chipset).dma_mask();

                // SAFETY: `Gpu` owns all DMA allocations for this device, and we are
                // still constructing it, so no concurrent DMA allocations can exist.
                unsafe { pdev.dma_set_mask_and_coherent(dma_mask)? };

                // Nova walks SG segments to build page tables, so their length is
                // irrelevant to the device.
                //
                // SAFETY: `Gpu` owns all DMA allocations for this device, and we are
                // still constructing it, so no concurrent DMA allocations can exist.
                unsafe { pdev.dma_set_max_seg_size(u32::MAX) };

                // We must wait for GFW_BOOT completion before doing any significant setup on the GPU.
                Self::wait_gfw_boot_completion(dev, bar, spec.chipset)?;
            },

            // Initialize this early because `gsp_resources` depends on it.
            sysmem_flush: SysmemFlush::register(dev, bar, spec.chipset)?,

            gsp_resources <- try_pin_init!(GspResources {
                device: pdev,

                spec: *spec,

                bar,

                gsp_falcon: Falcon::new(dev, spec.chipset, bar)?,

                sec2_falcon: Falcon::new(dev, spec.chipset, bar)?,

                fsp: Fsp::try_new(dev, bar, spec.chipset)?,

                vgpu: VgpuManager::new(pdev, spec.chipset, fsp.as_mut()),

                gsp <- Gsp::new(pdev, bar),

                // This member must be initialized last, so the `UnloadBundle` can never be dropped
                // from outside of the constructed `GspResources`, ensuring that the unload sequence
                // is properly run in case of failure.
                unload_bundle: gsp.boot(GspBootContext {
                    pdev,
                    bar,
                    chipset: spec.chipset,
                    gsp_falcon,
                    sec2_falcon,
                    fsp: fsp.as_mut(),
                    vgpu,
                })?,
            }),

            _: {
                irq::gsp::quiesce(tree_ref, &gsp_resources.gsp_falcon);
            },

            // SAFETY: the GSP falcon and the command queue are fields of `gsp_resources`, which
            // is initialized above and pinned, so both references outlive the registration. The
            // registration is a field of `Gpu` and is never leaked, so its `Drop` runs, and field
            // drop order runs it before either is freed.
            _gsp_subtree <- unsafe {
                GspSubtree::new(
                    pdev,
                    tree_ref,
                    &*core::ptr::from_ref(&gsp_resources.gsp_falcon),
                    &*core::ptr::from_ref(&gsp_resources.gsp.cmdq),
                )
            },

            // No interrupt announces the messages that the GSP posted during boot, before the
            // SWGEN0 latch was cleared.
            _: {
                gsp_resources.gsp.cmdq.drain()?;
            },

            gsp_static_info: {
                // Obtain and display basic GPU information.
                let info = gsp_resources.gsp.get_static_info()?;
                match info.gpu_name() {
                    Ok(name) => dev_info!(dev, "GPU name: {}\n", name),
                    Err(e) => dev_warn!(dev, "GPU name unavailable: {:?}\n", e),
                }

                if !info.usable_fb_regions.is_empty() {
                    dev_dbg!(dev, "Usable FB regions:\n");
                    for region in &info.usable_fb_regions {
                        dev_dbg!(dev, "  - {:#x?}\n", region);
                    }

                    dev_dbg!(
                        dev,
                        "Total usable VRAM: {} MiB\n",
                        info.vram_size() / u64::SZ_1M
                    );
                }

                info
            },

            // Not fatal: the RM client is only needed for the display engine and channels.
            rm: RmDevice::new(dev, &gsp_resources.gsp.cmdq)
                .inspect_err(|e| dev_warn!(dev, "can't allocate an RM client: {:?}\n", e))
                .ok(),

            // Not fatal: the display engine is only needed to describe the boot display.
            display: rm.as_ref().and_then(|rm| {
                Display::new(dev, &gsp_resources.gsp.cmdq, rm)
                    .inspect_err(|e| dev_warn!(dev, "can't access the display engine: {:?}\n", e))
                    .ok()
            }),

            boot_output: display.as_ref().and_then(|display| {
                let output = display
                    .active_display(dev, &gsp_resources.gsp.cmdq)
                    .inspect_err(|e| dev_warn!(dev, "can't find the active display: {:?}\n", e))
                    .ok()
                    .flatten();
                dev_dbg!(dev, "boot display output: {:?}\n", output);
                output
            }),

            // Not fatal: vblanks are only needed to pace what is shown.
            display_irq: display.as_ref().and_then(|_| {
                Self::register_display_irq(
                    dev,
                    bar,
                    &gsp_resources.gsp.cmdq,
                    gsp_resources.spec.chipset,
                    gsp_static_info,
                    tree_ref,
                    vblank_handler_ref,
                )
                .inspect_err(|e| dev_warn!(dev, "can't service display interrupts: {:?}\n", e))
                .ok()
            }),

            // Set up by `Gpu::init_display_engine()`, which needs to allocate VRAM.
            display_engine <- new_mutex!(None, "nova_display_engine"),

            // Set up by `Gpu::init_graphics()`, which needs to allocate VRAM.
            gr: None,

            // Create GPU memory manager owning memory management resources.
            mm: {
                let usable_vram = gsp_static_info.usable_fb_regions.first().ok_or(ENODEV)?;
                let buddy_params = GpuBuddyParams {
                    base_offset: usable_vram.start,
                    size: usable_vram.end - usable_vram.start,
                    chunk_size: Alignment::new::<SZ_4K>(),
                };

                GpuMm::new(
                    bar,
                    gsp_resources.spec.chipset,
                    buddy_params,
                    VramAddress::from_raw(gsp_static_info.total_fb_end),
                )?
            },

            // Create BAR1 user interface for CPU access to GPU virtual memory.
            bar_user <- {
                let pdb_addr = VramAddress::from_raw(gsp_static_info.bar1_pde_base);
                let bar1_idx = crate::driver::bar1_resource_index(pdev)?;
                let bar1_size = pdev.resource_len(bar1_idx)?;

                BarUser::new(
                    pdb_addr,
                    gsp_resources.spec.chipset,
                    bar1_size,
                    bar1,
                )
            },
        })
    }

    /// Registers the handler of the display interrupt, on the vector GSP-RM reports for it.
    #[expect(clippy::too_many_arguments)]
    fn register_display_irq(
        dev: &'gpu device::Device,
        bar: Bar0<'gpu>,
        cmdq: &Cmdq<'_>,
        chipset: Chipset,
        info: &GetGspStaticInfoReply,
        tree: &'gpu Tree<'gpu>,
        handler: &'gpu VblankHandlerSlot,
    ) -> Result<Pin<KBox<DisplayIrq<'gpu>>>> {
        let hal = display::hal::display_hal(chipset).ok_or(ENOTSUPP)?;
        let vector = RmControl::new(info.client(), info.subdevice(), IntrGetKernelTable::new())
            .send(dev, cmdq)?
            .ok_or(ENODEV)?;
        dev_dbg!(dev, "display: interrupt vector {}\n", vector);

        // SAFETY: the registration is a field of `Gpu`, which is never leaked, so its `Drop` runs.
        KBox::pin_init(
            unsafe { DisplayIrq::new(dev, bar, tree, hal, vector, handler)? },
            GFP_KERNEL,
        )
    }

    /// Waits for GFW, the GPU's boot firmware, to report completion.
    ///
    /// The driver must not program the GPU before then.
    ///
    /// # Errors
    ///
    /// `ETIMEDOUT` if GFW does not report completion in time.
    pub(crate) fn wait_gfw_boot_completion(
        dev: &device::Device<device::Bound>,
        bar: Bar0<'_>,
        chipset: Chipset,
    ) -> Result {
        hal::gpu_hal(chipset)
            .wait_gfw_boot_completion(bar)
            .inspect_err(|_| dev_err!(dev, "GFW boot did not complete\n"))
    }

    /// Maps the framebuffer of the boot display into BAR1, so the CPU can draw into it with
    /// [`Gpu::write_boot_display()`]. Does nothing without a boot display.
    pub(crate) fn map_boot_display(
        self: Pin<&mut Self>,
        #[cfg_attr(not(CONFIG_NOVA_CORE_SELFTESTS), allow(unused_variables))] pdev: &pci::Device<
            device::Bound,
        >,
    ) -> Result {
        let this = self.project();
        let Some(display) = this.boot_display.as_ref() else {
            return Ok(());
        };

        // BAR1 maps whole pages; the end of the last line may not be page-aligned.
        let page_size: u64 = PAGE_SIZE.into_safe_cast();
        let end = display
            .vram
            .end
            .into_raw()
            .checked_next_multiple_of(page_size)
            .ok_or(EOVERFLOW)?;
        let vram = display.vram.start..VramAddress::from_raw(end);

        let bar_user = this.bar_user.as_ref().get_ref();
        let mapped = bar_user.map_vram(this.mm, vram)?;

        #[cfg(CONFIG_NOVA_CORE_SELFTESTS)]
        if let Err(err) = crate::mm::boot_display_test::check_bar1_mapping(
            pdev.as_ref(),
            this.mm,
            bar_user,
            &mapped,
            display,
        ) {
            dev_err!(pdev, "boot display: BAR1 mapping check failed: {:?}\n", err);
        }

        *this.boot_scanout.lock() = Some(mapped);

        Ok(())
    }

    /// Writes `data` at `offset` into the framebuffer of the boot display, which shows on screen.
    ///
    /// `offset` and the length of `data` must be multiples of 4. Fails with `ENODEV` if the boot
    /// display isn't mapped.
    pub(crate) fn write_boot_display(&self, offset: usize, data: &[u8]) -> Result {
        let scanout = self.boot_scanout.lock();
        let mapped = scanout.as_ref().ok_or(ENODEV)?;

        self.bar_user.write(mapped, offset, data)
    }

    /// Sets up the context of the graphics engine, which channels on it need.
    ///
    /// Must be done before any other channel is allocated.
    pub(crate) fn init_graphics(self: Pin<&mut Self>) -> Result {
        let this = self.project();
        let gsp_resources = this.gsp_resources.as_ref().get_ref();
        let pdev = gsp_resources.device;
        let cmdq = &gsp_resources.gsp.cmdq;
        let rm = this.rm.as_ref().ok_or(ENODEV)?;
        let internal = InternalHandles {
            client: this.gsp_static_info.client(),
            subdevice: this.gsp_static_info.subdevice(),
        };

        let engine = channel::engines(pdev.as_ref(), cmdq, rm)?
            .into_iter()
            .find(EngineInfo::is_graphics)
            .ok_or(ENODEV)?;
        *this.gr = Some(GrContext::new(
            pdev.as_ref(),
            cmdq,
            this.mm,
            this.spec.chipset,
            rm,
            &internal,
            engine,
            channel::GOLDEN_CHID,
        )?);

        Ok(())
    }

    /// Takes over the display engine from GSP-RM, so that Nova can drive displays itself.
    ///
    /// Fails with `ENOTSUPP` on GPUs whose display engine isn't supported yet.
    pub(crate) fn init_display_engine(self: Pin<&mut Self>) -> Result {
        let this = self.project();
        let gsp_resources = this.gsp_resources.as_ref().get_ref();
        let pdev = gsp_resources.device;
        let display = this.display.as_ref().ok_or(ENODEV)?;
        let internal = InternalHandles {
            client: this.gsp_static_info.client(),
            subdevice: this.gsp_static_info.subdevice(),
        };

        let vram = VramAddress::ZERO..VramAddress::from_raw(this.gsp_static_info.total_fb_end);
        #[cfg_attr(not(CONFIG_NOVA_CORE_SELFTESTS), expect(unused_mut))]
        let mut engine = DisplayEngine::new(
            pdev.as_ref(),
            gsp_resources.bar,
            &gsp_resources.gsp.cmdq,
            this.mm,
            this.spec.chipset,
            display,
            &internal,
            vram,
        )?;
        dev_dbg!(
            pdev,
            "display engine taken over, windows {:#x}\n",
            engine.windows()
        );

        #[cfg(CONFIG_NOVA_CORE_SELFTESTS)]
        if let Some(boot) = this.boot_display.as_ref() {
            // A copy channel for the GPU to fill the test surface.
            let mut channel = crate::channel::copy::CopyChannel::new(
                pdev.as_ref(),
                &gsp_resources.gsp.cmdq,
                this.mm,
                this.spec.chipset,
                this.rm.as_ref().ok_or(ENODEV)?,
            )
            .inspect_err(|e| dev_err!(pdev, "channel: can't set up a copy channel: {:?}\n", e))
            .ok();

            // A compute channel for a shader to draw the test surface, once it passed its own
            // test.
            let mut compute = this.gr.as_ref().and_then(|gr| {
                crate::channel::gr::ComputeChannel::new(
                    pdev.as_ref(),
                    &gsp_resources.gsp.cmdq,
                    this.mm,
                    this.spec.chipset,
                    this.rm.as_ref()?,
                    gr,
                    crate::channel::COMPUTE_CHID,
                )
                .inspect_err(|e| {
                    dev_err!(pdev, "channel: can't set up a compute channel: {:?}\n", e)
                })
                .ok()
            });
            let computed = compute.as_mut().is_some_and(|compute| {
                crate::channel::compute_test::run(
                    pdev.as_ref(),
                    gsp_resources.bar,
                    this.mm,
                    compute,
                )
                .inspect_err(|e| dev_err!(pdev, "compute test failed: {:?}\n", e))
                .is_ok()
            });

            let _ = crate::display::scanout_test::run(
                pdev.as_ref(),
                gsp_resources.bar,
                this.mm,
                &mut engine,
                boot,
                channel.as_mut(),
                compute.as_mut().filter(|_| computed),
            );

            if let Some(mut compute) = compute {
                compute.free(pdev.as_ref(), &gsp_resources.gsp.cmdq, this.mm);
            }
            if let Some(mut channel) = channel {
                channel.free(pdev.as_ref(), &gsp_resources.gsp.cmdq, this.mm);
            }
        }

        // Nova's own scanout, of the boot display's mode: Nova can't set modes yet.
        let scanout = this.boot_display.as_ref().and_then(|boot| {
            Scanout::new(
                this.mm,
                this.bar_user.as_ref().get_ref(),
                boot.width,
                boot.height,
            )
            .inspect(|scanout| dev_info!(pdev, "display: scanout set up: {:?}\n", scanout.mode()))
            .inspect_err(|e| dev_err!(pdev, "display: can't set up the scanout: {:?}\n", e))
            .ok()
        });

        *this.display_engine.lock() = Some(DisplayOutput { engine, scanout });

        Ok(())
    }

    /// Returns the size and layout of the framebuffers of Nova's own scanout, if it is set up.
    pub(crate) fn scanout_mode(&self) -> Option<ScanoutMode> {
        self.display_engine
            .lock()
            .as_ref()
            .and_then(|output| output.scanout.as_ref())
            .map(Scanout::mode)
    }

    /// Writes `data` at `offset` into the back buffer of Nova's own scanout, which shows on
    /// screen from the next [`Gpu::flip_scanout()`].
    ///
    /// `offset` and the length of `data` must be multiples of 4. Fails with `ENODEV` if the
    /// scanout isn't set up.
    pub(crate) fn write_scanout(&self, offset: usize, data: &[u8]) -> Result {
        let output = self.display_engine.lock();
        let scanout = output
            .as_ref()
            .and_then(|output| output.scanout.as_ref())
            .ok_or(ENODEV)?;

        scanout.write(&self.bar_user, offset, data)
    }

    /// Shows the back buffer of Nova's own scanout from the next frame on, and returns once the
    /// framebuffer shown before can be drawn into.
    ///
    /// Fails with `ENODEV` if the scanout isn't set up.
    pub(crate) fn flip_scanout(&self) -> Result {
        let mut guard = self.display_engine.lock();
        let output = Option::as_mut(&mut guard).ok_or(ENODEV)?;
        let scanout = output.scanout.as_mut().ok_or(ENODEV)?;

        scanout.flip(&mut output.engine, self.gsp_resources.bar)
    }

    /// Enables the vblank interrupt of head `head` if `enable`, or disables it. Doesn't sleep.
    ///
    /// Fails with `ENODEV` if the display interrupt isn't serviced.
    pub(crate) fn set_vblank_interrupt(&self, head: u32, enable: bool) -> Result {
        if self.display_irq.is_none() {
            return Err(ENODEV);
        }
        let hal = display::hal::display_hal(self.spec.chipset).ok_or(ENOTSUPP)?;

        hal.set_vblank_interrupt(self.gsp_resources.bar, usize::try_from(head)?, enable)
    }

    /// Sets the handler that the vblanks of the heads whose vblank interrupt is enabled are passed
    /// to, replacing any, or removes it.
    pub(crate) fn set_vblank_handler(&self, handler: Option<Arc<dyn VblankHandler>>) {
        // The handler replaced is dropped once the lock is released, as dropping it may sleep.
        let _replaced = core::mem::replace(&mut *self.vblank_handler.lock(), handler);
    }

    /// Returns the display engine and the display the boot display is shown on.
    fn boot_output(&self) -> Result<(&Display, DisplayId)> {
        let display = self.display.as_ref().ok_or(ENODEV)?;
        let output = self.boot_output.ok_or(ENODEV)?;

        Ok((display, output))
    }

    /// Returns the type of connector the boot display is shown through.
    ///
    /// Fails with `ENODEV` if the display engine reports no active display.
    pub(crate) fn boot_display_connector(&self) -> Result<ConnectorType> {
        let (display, output) = self.boot_output()?;
        let dev = self.gsp_resources.device.as_ref();

        display.connector_type(dev, &self.gsp_resources.gsp.cmdq, output)
    }

    /// Reads the EDID of the monitor the boot display is shown on.
    ///
    /// Fails with `ENODEV` if the display engine reports no active display.
    pub(crate) fn boot_display_edid(&self) -> Result<KVec<u8>> {
        let (display, output) = self.boot_output()?;
        let dev = self.gsp_resources.device.as_ref();

        display.edid(dev, &self.gsp_resources.gsp.cmdq, output)
    }

    /// Runs self-tests on the constructed [`Gpu`], logging failures without failing probe.
    #[cfg(CONFIG_NOVA_CORE_SELFTESTS)]
    pub(crate) fn run_selftests(self: Pin<&mut Self>, pdev: &pci::Device<device::Bound>) {
        let this = self.project();
        let dev = pdev.as_ref();
        let regions = &this.gsp_static_info.usable_fb_regions;

        if let Err(err) = crate::mm::selftest::run(
            dev,
            this.mm,
            regions,
            &this.bar_user,
            this.gsp_static_info.bar1_pde_base,
            this.spec.chipset,
        ) {
            dev_err!(dev, "self-tests failed: {:?}\n", err);
        }

        if let Some(display) = this.boot_display {
            if let Err(err) = crate::mm::boot_display_test::draw(dev, this.mm, display, regions) {
                dev_err!(dev, "boot display test pattern failed: {:?}\n", err);
            }
        }
    }
}

/// The display engine, and Nova's own scanout through it.
struct DisplayOutput<'gpu> {
    engine: DisplayEngine<'gpu>,
    /// Nova's own scanout, if it could be set up.
    scanout: Option<Scanout>,
}

#[pinned_drop]
impl PinnedDrop for Gpu<'_> {
    fn drop(self: Pin<&mut Self>) {
        let this = self.project();

        // Before any field is dropped: unmapping needs `mm`, `bar_user` and a running GSP.
        let mapped = this.boot_scanout.lock().take();
        if let Some(mapped) = mapped {
            let _ = this
                .bar_user
                .as_ref()
                .get_ref()
                .unmap(this.mm, mapped)
                .inspect_err(|e| pr_err!("failed to unmap the boot display: {:?}\n", e));
        }

        // Before `gsp_resources` is dropped: freeing RM objects needs a running GSP.
        let gsp_resources = this.gsp_resources.as_ref().get_ref();
        let dev = gsp_resources.device.as_ref();

        // The display engine's objects first, as they live under those of `display`.
        let output = this.display_engine.lock().take();
        if let Some(DisplayOutput {
            mut engine,
            scanout,
        }) = output
        {
            if let Some(display) = this.display.as_ref() {
                engine.free(dev, &gsp_resources.gsp.cmdq, display);
            }

            // Once the window, which may show it, is freed.
            if let Some(scanout) = scanout {
                let _ = scanout
                    .unmap(this.mm, this.bar_user.as_ref().get_ref())
                    .inspect_err(|e| dev_err!(dev, "failed to unmap the scanout: {:?}\n", e));
            }
        }

        if let Some(display) = this.display.take() {
            let _ = display
                .free(dev, &gsp_resources.gsp.cmdq)
                .inspect_err(|e| dev_err!(dev, "failed to free the display objects: {:?}\n", e));
        }

        // Last, as every other RM object is allocated under it.
        if let Some(rm) = this.rm.take() {
            let _ = rm
                .free(dev, &gsp_resources.gsp.cmdq)
                .inspect_err(|e| dev_err!(dev, "failed to free the RM client: {:?}\n", e));
        }
    }
}

/// Reads the boot0 register and returns its raw value.
pub(crate) fn boot_0_raw(bar: Bar0<'_>) -> u32 {
    bar.read(regs::NV_PMC_BOOT_0).into_raw()
}

// SPDX-License-Identifier: GPL-2.0

//! Page table mapping operations for NVIDIA GPUs.

use core::{
    marker::PhantomData,
    ops::Range, //
};

use kernel::{
    gpu::buddy::{
        AllocatedBlocks,
        GpuBuddyAllocFlags,
        GpuBuddyAllocMode, //
    },
    io::io_write,
    prelude::*,
    ptr::Alignment,
    rbtree::{
        RBTree,
        RBTreeNode, //
    },
    sizes::SZ_4K, //
};

use super::{
    walk::{
        PtWalkInner,
        WalkPdeResult, //
    },
    Access,
    AperturePde,
    AperturePte,
    DualPdeOps,
    MmuConfig,
    MmuV2,
    MmuV3,
    MmuVersion,
    PageTableLevel,
    PdeOps,
    PteOps, //
};
use crate::{
    mm::{
        pramin,
        GpuMm,
        Pfn,
        Vfn,
        VramAddress,
        PAGE_SIZE, //
    },
    num::{
        IntoSafeCast, //
    },
};

/// A pre-allocated and zeroed page table page.
///
/// Created during the mapping prepare phase and consumed during the execute phase.
/// Stored in an [`RBTree`] keyed by the PDE slot address (`install_addr`).
pub(in crate::mm) struct PreparedPtPage {
    /// The allocated and zeroed page table page.
    pub(in crate::mm) alloc: Pin<KBox<AllocatedBlocks>>,
    /// Page table level -- needed to determine if this PT page is for a dual PDE.
    pub(in crate::mm) level: PageTableLevel,
}

/// Page table mapper.
pub(in crate::mm) struct PtMapInner<M: MmuConfig> {
    walker: PtWalkInner<M>,
    pdb_addr: VramAddress,
    _phantom: PhantomData<M>,
}

impl<M: MmuConfig> PtMapInner<M> {
    /// Create a new [`PtMapInner`].
    pub(super) fn new(pdb_addr: VramAddress) -> Self {
        Self {
            walker: PtWalkInner::<M>::new(pdb_addr),
            pdb_addr,
            _phantom: PhantomData,
        }
    }

    /// Allocate and zero a physical page table page.
    fn alloc_and_zero_page(mm: &mut GpuMm<'_>, level: PageTableLevel) -> Result<PreparedPtPage> {
        let blocks = KBox::pin_init(
            mm.buddy().alloc_blocks(
                GpuBuddyAllocMode::Simple,
                SZ_4K.into_safe_cast(),
                Alignment::new::<SZ_4K>(),
                GpuBuddyAllocFlags::default(),
            ),
            GFP_KERNEL,
        )?;

        let page_vram = VramAddress::from_raw(blocks.iter().next().ok_or(ENOMEM)?.offset());

        // Zero via PRAMIN.
        let window = mm
            .pramin_mut()
            .window_at::<[u64; PAGE_SIZE / 8]>(page_vram)?;
        for i in 0..PAGE_SIZE / 8 {
            io_write!(window.view(), [build: i], 0);
        }

        Ok(PreparedPtPage {
            alloc: blocks,
            level,
        })
    }

    /// Ensure all intermediate page table pages exist for a single VFN.
    ///
    /// The mutable PRAMIN borrow ends before each allocation.
    fn ensure_single_pte_path(
        &self,
        mm: &mut GpuMm<'_>,
        vfn: Vfn,
        pt_pages: &mut RBTree<VramAddress, PreparedPtPage>,
    ) -> Result {
        let max_iter = 2 * M::PDE_LEVELS.len();

        for _ in 0..max_iter {
            let result = self
                .walker
                .walk_pde_levels(mm.pramin_mut(), vfn, |install_addr| {
                    pt_pages.get(&install_addr).and_then(|p| {
                        p.alloc
                            .iter()
                            .next()
                            .map(|b| VramAddress::from_raw(b.offset()))
                    })
                })?;

            match result {
                WalkPdeResult::Complete { .. } => {
                    return Ok(());
                }
                WalkPdeResult::Missing {
                    install_addr,
                    level,
                } => {
                    let page = Self::alloc_and_zero_page(mm, level)?;
                    let node = RBTreeNode::new(install_addr, page, GFP_KERNEL)?;
                    let old = pt_pages.insert(node);
                    if old.is_some() {
                        kernel::pr_warn_once!(
                            "VMM: duplicate install_addr in pt_pages (internal consistency error)\n"
                        );
                        return Err(EIO);
                    }
                }
            }
        }

        kernel::pr_warn!(
            "VMM: ensure_pte_path: loop exhausted after {} iters (VFN {:?})\n",
            max_iter,
            vfn
        );
        Err(EIO)
    }

    /// Prepare page table resources for mapping `num_pages` pages starting at `vfn_start`.
    ///
    /// Reserves capacity in `page_table_allocs`, then walks the hierarchy
    /// per-VFN to prepare pages for all missing PDEs.
    pub(super) fn prepare_map(
        &self,
        mm: &mut GpuMm<'_>,
        vfn_start: Vfn,
        num_pages: usize,
        page_table_allocs: &mut KVec<Pin<KBox<AllocatedBlocks>>>,
        pt_pages: &mut RBTree<VramAddress, PreparedPtPage>,
    ) -> Result {
        // Pre-reserve so install_mappings() can use push_within_capacity (no alloc
        // in fence signalling critical path).
        let pt_upper_bound = M::pt_pages_upper_bound(num_pages);
        page_table_allocs.reserve(pt_upper_bound, GFP_KERNEL)?;

        // Walk the hierarchy to prepare pages for all missing PDEs, once per PTE-level table: the
        // pages of a table share the path to it.
        for i in 0..num_pages {
            let i_u64: u64 = i.into_safe_cast();
            let vfn = Vfn::new(vfn_start.raw() + i_u64);
            if i == 0 || PtWalkInner::<M>::starts_pte_table(vfn) {
                self.ensure_single_pte_path(mm, vfn, pt_pages)?;
            }
        }
        Ok(())
    }

    /// Install prepared PDEs and write PTEs, then flush TLB.
    ///
    /// Drains `pt_pages` and moves allocations into `page_table_allocs`.
    pub(super) fn install_mappings(
        &self,
        mm: &mut GpuMm<'_>,
        pt_pages: &mut RBTree<VramAddress, PreparedPtPage>,
        page_table_allocs: &mut KVec<Pin<KBox<AllocatedBlocks>>>,
        vfn_start: Vfn,
        pfns: &[Pfn],
        access: Access,
        aperture: AperturePte,
    ) -> Result {
        {
            let pramin = mm.pramin_mut();

            // Drain prepared PT pages, install all pending PDEs.
            let mut cursor = pt_pages.cursor_front_mut();
            while let Some(c) = cursor {
                let (next, node) = c.remove_current();
                let (install_addr, page) = node.to_key_value();
                let page_vram =
                    VramAddress::from_raw(page.alloc.iter().next().ok_or(ENOMEM)?.offset());

                if page.level == M::DUAL_PDE_LEVEL {
                    let new_dpde = M::DualPde::new_small(Pfn::from(page_vram));
                    new_dpde.write(pramin, install_addr)?;
                } else {
                    let new_pde = M::Pde::new(AperturePde::VideoMemory, Pfn::from(page_vram));
                    new_pde.write(pramin, install_addr)?;
                }

                page_table_allocs
                    .push_within_capacity(page.alloc)
                    .map_err(|_| ENOMEM)?;

                cursor = next;
            }

            // Write PTEs (all PDEs now installed in HW).
            let mut ptes = KVec::new();
            self.for_each_pte_run(
                pramin,
                vfn_start,
                pfns.len(),
                |pramin, run, vfn, pte_addr| {
                    let Some(pte_addr) = pte_addr else {
                        kernel::pr_warn_once!("VMM: page table missing for VFN {vfn:?}\n");
                        return Err(EIO);
                    };

                    ptes.clear();
                    for &pfn in &pfns[run] {
                        ptes.push(M::Pte::new(aperture, pfn, access).into(), GFP_KERNEL)?;
                    }
                    pramin.write_u64s(pte_addr, &ptes)
                },
            )?;
        }

        // Flush TLB.
        mm.tlb().flush(self.pdb_addr)
    }

    /// Splits the `num_pages` pages from `vfn_start` into runs whose PTEs follow each other in a
    /// PTE-level table, and calls `f` with the indices of each run's pages, its first page, and
    /// the address of its first PTE, if its page table is installed.
    ///
    /// The page directories are walked once per run: walks read them through PRAMIN, uncached,
    /// which costs far more than writing PTEs.
    fn for_each_pte_run(
        &self,
        pramin: &mut pramin::Pramin<'_>,
        vfn_start: Vfn,
        num_pages: usize,
        mut f: impl FnMut(&mut pramin::Pramin<'_>, Range<usize>, Vfn, Option<VramAddress>) -> Result,
    ) -> Result {
        let vfn = |i: usize| -> Vfn {
            let i_u64: u64 = i.into_safe_cast();
            Vfn::new(vfn_start.raw() + i_u64)
        };

        let mut start = 0;
        while start < num_pages {
            let mut end = start + 1;
            while end < num_pages && !PtWalkInner::<M>::starts_pte_table(vfn(end)) {
                end += 1;
            }

            let first = vfn(start);
            let pte_addr = self
                .walker
                .pte_table(pramin, first)?
                .map(|table| PtWalkInner::<M>::pte_addr(table, first));
            f(pramin, start..end, first, pte_addr)?;
            start = end;
        }

        Ok(())
    }

    /// Invalidate PTEs for a range and flush TLB.
    pub(super) fn invalidate_ptes(
        &self,
        mm: &mut GpuMm<'_>,
        vfn_start: Vfn,
        num_pages: usize,
    ) -> Result {
        let invalid_pte: u64 = M::Pte::invalid().into();

        {
            let pramin = mm.pramin_mut();
            let mut ptes = KVec::new();
            self.for_each_pte_run(pramin, vfn_start, num_pages, |pramin, run, _, pte_addr| {
                // Nothing is mapped without a page table.
                let Some(pte_addr) = pte_addr else {
                    return Ok(());
                };

                ptes.clear();
                ptes.extend_with(run.len(), invalid_pte, GFP_KERNEL)?;
                pramin.write_u64s(pte_addr, &ptes)
            })?;
        }

        mm.tlb().flush(self.pdb_addr)
    }
}

macro_rules! pt_map_dispatch {
    ($self:expr, $method:ident ( $($arg:expr),* $(,)? )) => {
        match $self {
            PtMap::V2(inner) => inner.$method($($arg),*),
            PtMap::V3(inner) => inner.$method($($arg),*),
        }
    };
}

/// Page table mapper dispatch.
pub(in crate::mm) enum PtMap {
    /// MMU v2 (Turing/Ampere/Ada).
    V2(PtMapInner<MmuV2>),
    /// MMU v3 (Hopper+).
    V3(PtMapInner<MmuV3>),
}

impl PtMap {
    /// Create a new page table mapper for the given MMU version.
    pub(in crate::mm) fn new(pdb_addr: VramAddress, version: MmuVersion) -> Self {
        match version {
            MmuVersion::V2 => Self::V2(PtMapInner::<MmuV2>::new(pdb_addr)),
            MmuVersion::V3 => Self::V3(PtMapInner::<MmuV3>::new(pdb_addr)),
        }
    }

    /// Prepare page table resources for a mapping.
    pub(in crate::mm) fn prepare_map(
        &self,
        mm: &mut GpuMm<'_>,
        vfn_start: Vfn,
        num_pages: usize,
        page_table_allocs: &mut KVec<Pin<KBox<AllocatedBlocks>>>,
        pt_pages: &mut RBTree<VramAddress, PreparedPtPage>,
    ) -> Result {
        pt_map_dispatch!(
            self,
            prepare_map(mm, vfn_start, num_pages, page_table_allocs, pt_pages)
        )
    }

    /// Install prepared PDEs and write PTEs, then flush TLB.
    pub(in crate::mm) fn install_mappings(
        &self,
        mm: &mut GpuMm<'_>,
        pt_pages: &mut RBTree<VramAddress, PreparedPtPage>,
        page_table_allocs: &mut KVec<Pin<KBox<AllocatedBlocks>>>,
        vfn_start: Vfn,
        pfns: &[Pfn],
        access: Access,
        aperture: AperturePte,
    ) -> Result {
        pt_map_dispatch!(
            self,
            install_mappings(
                mm,
                pt_pages,
                page_table_allocs,
                vfn_start,
                pfns,
                access,
                aperture
            )
        )
    }

    /// Invalidate PTEs for a range and flush TLB.
    pub(in crate::mm) fn invalidate_ptes(
        &self,
        mm: &mut GpuMm<'_>,
        vfn_start: Vfn,
        num_pages: usize,
    ) -> Result {
        pt_map_dispatch!(self, invalidate_ptes(mm, vfn_start, num_pages))
    }
}

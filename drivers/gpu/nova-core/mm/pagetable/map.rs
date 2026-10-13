// SPDX-License-Identifier: GPL-2.0

//! Page table mapping operations for NVIDIA GPUs.

use core::{
    marker::PhantomData,
    ops::Range, //
};

use kernel::{
    gpu::buddy::AllocatedBlocks,
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
    PteOps,
    PAGES_PER_BIG_PAGE, //
};
use crate::{
    mm::{
        pramin,
        tlb::Flush,
        vmm::VmmKind,
        GpuMm,
        Pages,
        Pfn,
        Vfn,
        VirtualAddress,
        VramAddress,
        PAGE_SIZE, //
    },
    num::{
        IntoSafeCast, //
    },
};

/// Pre-allocated and zeroed page table pages for a PDE slot.
///
/// Created during the mapping prepare phase and consumed during the execute phase.
/// Stored in an [`RBTree`] keyed by the PDE slot address (`install_addr`).
pub(in crate::mm) struct PreparedPtPage {
    /// The table the PDE is to point to: for a dual PDE, its small page table, `None` if it is
    /// installed already.
    pub(in crate::mm) alloc: Option<Pin<KBox<AllocatedBlocks>>>,
    /// For a dual PDE, the big page table it is to point to, `None` if it is installed already
    /// or not needed.
    pub(in crate::mm) big: Option<Pin<KBox<AllocatedBlocks>>>,
    /// Page table level -- needed to determine if this PT page is for a dual PDE.
    pub(in crate::mm) level: PageTableLevel,
}

/// Entries to write, gathered so that those following each other are written at once, posted:
/// see [`pramin::Pramin::write_u64s_posted()`].
struct EntryRun {
    start: VramAddress,
    values: KVec<u64>,
}

impl EntryRun {
    fn new() -> Self {
        Self {
            start: VramAddress::from_raw(0),
            values: KVec::new(),
        }
    }

    /// Adds the entry `value` at `addr`, writing those gathered first unless it follows them.
    fn push(&mut self, pramin: &mut pramin::Pramin<'_>, addr: VramAddress, value: u64) -> Result {
        let next = self.start.into_raw() + 8 * u64::try_from(self.values.len())?;
        if !self.values.is_empty() && addr.into_raw() != next {
            self.write(pramin)?;
        }
        if self.values.is_empty() {
            self.start = addr;
        }
        self.values.push(value, GFP_KERNEL)?;
        Ok(())
    }

    /// Writes the entries gathered.
    fn write(&mut self, pramin: &mut pramin::Pramin<'_>) -> Result {
        if !self.values.is_empty() {
            pramin.write_u64s_posted(self.start, &self.values)?;
            self.values.clear();
        }
        Ok(())
    }
}

/// Returns the VRAM address of the page table page `alloc`.
fn table_address(alloc: &Pin<KBox<AllocatedBlocks>>) -> Result<VramAddress> {
    Ok(VramAddress::from_raw(
        alloc.iter().next().ok_or(ENOMEM)?.offset(),
    ))
}

/// Number of blocks of pages, of a PTE-level table each (2 MiB with 4 KiB pages), whose page
/// tables a [`TableCache`] remembers.
const TABLE_CACHE_SIZE: usize = 256;

/// The page tables of a block of pages, as walked to.
#[derive(Clone, Copy)]
struct BlockTables {
    /// The block: the VFN of its first page divided by the pages of a PTE-level table.
    block: u64,
    /// Its PTE-level (small page) table.
    small: VramAddress,
    /// Its big page table, if it has one.
    big: Option<VramAddress>,
}

/// The page tables of the blocks of pages of an address space last walked to, each in the slot
/// of its block number modulo [`TABLE_CACHE_SIZE`]: walks read page directories, uncached, which
/// costs more than writing the PTEs of a mapping. Page tables stay as long as the address space,
/// so these stay right until a dual PDE is written, which forgets them all.
pub(in crate::mm) struct TableCache {
    /// [`TABLE_CACHE_SIZE`] slots.
    tables: KVec<Option<BlockTables>>,
}

impl TableCache {
    /// Create an empty [`TableCache`].
    pub(in crate::mm) fn new() -> Result<Self> {
        Ok(Self {
            tables: KVec::from_elem(None, TABLE_CACHE_SIZE, GFP_KERNEL)?,
        })
    }

    /// Returns the slot of `block`.
    fn slot(block: u64) -> usize {
        let slots: u64 = TABLE_CACHE_SIZE.into_safe_cast();
        // In range: the remainder of a division by the number of slots.
        (block % slots).into_safe_cast()
    }

    /// Returns the page tables of `block`, if remembered.
    fn get(&self, block: u64) -> Option<BlockTables> {
        self.tables[Self::slot(block)].filter(|tables| tables.block == block)
    }

    /// Remembers `tables`.
    fn set(&mut self, tables: BlockTables) {
        self.tables[Self::slot(tables.block)] = Some(tables);
    }

    /// Forgets all page tables remembered: a dual PDE is being written.
    fn forget(&mut self) {
        self.tables.fill(None);
    }
}

/// Page table mapper.
pub(in crate::mm) struct PtMapInner<M: MmuConfig> {
    walker: PtWalkInner<M>,
    pdb_addr: VramAddress,
    /// Whether the address space is set up for [`super::BIG_PAGE_SIZE`] big pages, which VRAM is
    /// then mapped with where it can be.
    big_pages: bool,
    /// Whether the address space is BAR1's, which only the HUB MMU translates.
    bar: bool,
    _phantom: PhantomData<M>,
}

impl<M: MmuConfig> PtMapInner<M> {
    /// Create a new [`PtMapInner`] for an address space of the kind `space`.
    pub(super) fn new(pdb_addr: VramAddress, space: VmmKind) -> Self {
        Self {
            walker: PtWalkInner::<M>::new(pdb_addr),
            pdb_addr,
            big_pages: space == VmmKind::Channel && M::BIG_PAGES,
            bar: space == VmmKind::Bar1,
            _phantom: PhantomData,
        }
    }

    /// Returns the block of pages, of a PTE-level table, `vfn` is in.
    fn block_of(vfn: Vfn) -> u64 {
        let pages_per_table: u64 = M::entries_per_page(M::PTE_LEVEL).into_safe_cast();
        vfn.raw() / pages_per_table
    }

    /// Returns the page tables of the block of pages `vfn` is in: as `tables` remembers them, or
    /// walked to, then remembered. `None` if the page directories don't lead to a PTE-level
    /// table.
    fn block_tables(
        &self,
        pramin: &mut pramin::Pramin<'_>,
        tables: &mut TableCache,
        vfn: Vfn,
    ) -> Result<Option<BlockTables>> {
        let block = Self::block_of(vfn);
        if let Some(remembered) = tables.get(block) {
            return Ok(Some(remembered));
        }
        let Some(small) = self.walker.pte_table(pramin, vfn)? else {
            return Ok(None);
        };
        let big = self.walker.big_table(pramin, vfn)?;
        let walked = BlockTables { block, small, big };
        tables.set(walked);
        Ok(Some(walked))
    }

    /// Returns the frame of the big page that the pages of `pages` of `aperture` from page `i`
    /// make up, if they make up one: contiguous VRAM, from a big-page aligned frame.
    fn big_page(aperture: AperturePte, pages: Pages<'_>, i: usize) -> Option<Pfn> {
        let pages_per_big: u64 = PAGES_PER_BIG_PAGE.into_safe_cast();
        if aperture != AperturePte::VideoMemory {
            return None;
        }

        pages
            .contiguous_from(i, PAGES_PER_BIG_PAGE)
            .filter(|first| first.raw() % pages_per_big == 0)
    }

    /// Returns whether the `num_pages` pages from `vfn` cover a whole big page in the PTE-level
    /// table of `vfn`.
    fn covers_big_page(vfn: Vfn, num_pages: u64) -> bool {
        let pages_per_big: u64 = PAGES_PER_BIG_PAGE.into_safe_cast();
        let pages_per_table: u64 = M::entries_per_page(M::PTE_LEVEL).into_safe_cast();
        let table_end = (vfn.raw() / pages_per_table + 1) * pages_per_table;
        let end = (vfn.raw() + num_pages).min(table_end);
        vfn.raw().next_multiple_of(pages_per_big) + pages_per_big <= end
    }

    /// Allocate and zero a physical page table page.
    fn alloc_and_zero_page(mm: &mut GpuMm<'_>) -> Result<Pin<KBox<AllocatedBlocks>>> {
        let blocks = mm.alloc_page_table(SZ_4K, Alignment::new::<SZ_4K>())?;

        let page_vram = VramAddress::from_raw(blocks.iter().next().ok_or(ENOMEM)?.offset());

        // Zero via PRAMIN, or BAR1 for the page table pool.
        let window = mm
            .pramin_mut()
            .window_at::<[u64; PAGE_SIZE / 8]>(page_vram)?;
        for i in 0..PAGE_SIZE / 8 {
            io_write!(window.view(), [build: i], 0);
        }

        Ok(blocks)
    }

    /// Returns the address of the prepared table for the PDE slot `install_addr` in `pt_pages`.
    fn prepared_table(
        pt_pages: &RBTree<VramAddress, PreparedPtPage>,
        install_addr: VramAddress,
    ) -> Option<VramAddress> {
        pt_pages
            .get(&install_addr)
            .and_then(|p| p.alloc.as_ref())
            .and_then(|alloc| table_address(alloc).ok())
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
                    Self::prepared_table(pt_pages, install_addr)
                })?;

            match result {
                WalkPdeResult::Complete { .. } => {
                    return Ok(());
                }
                WalkPdeResult::Missing {
                    install_addr,
                    level,
                } => {
                    let alloc = Self::alloc_and_zero_page(mm)?;
                    match pt_pages.get_mut(&install_addr) {
                        // A big page table prepared for the dual PDE before its small one.
                        Some(page) if page.alloc.is_none() => page.alloc = Some(alloc),
                        Some(_) => {
                            kernel::pr_warn_once!(
                                "VMM: duplicate install_addr in pt_pages (internal consistency error)\n"
                            );
                            return Err(EIO);
                        }
                        None => {
                            let page = PreparedPtPage {
                                alloc: Some(alloc),
                                big: None,
                                level,
                            };
                            pt_pages.insert(RBTreeNode::new(install_addr, page, GFP_KERNEL)?);
                        }
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

    /// Ensure the dual PDE of `vfn`, whose path [`Self::ensure_single_pte_path()`] ensured, has a
    /// big page table.
    fn ensure_big_table(
        &self,
        mm: &mut GpuMm<'_>,
        vfn: Vfn,
        pt_pages: &mut RBTree<VramAddress, PreparedPtPage>,
    ) -> Result {
        let pramin = mm.pramin_mut();
        let Some(install_addr) = self
            .walker
            .dual_pde_addr(pramin, vfn, |addr| Self::prepared_table(pt_pages, addr))?
        else {
            kernel::pr_warn_once!("VMM: no path to the dual PDE of VFN {vfn:?}\n");
            return Err(EIO);
        };
        let prepared = pt_pages.get(&install_addr);
        if prepared.is_some_and(|p| p.big.is_some())
            || M::DualPde::read(pramin, install_addr)?.has_big()
        {
            return Ok(());
        }

        let big = Self::alloc_and_zero_page(mm)?;
        match pt_pages.get_mut(&install_addr) {
            Some(page) => page.big = Some(big),
            None => {
                let page = PreparedPtPage {
                    alloc: None,
                    big: Some(big),
                    level: M::DUAL_PDE_LEVEL,
                };
                pt_pages.insert(RBTreeNode::new(install_addr, page, GFP_KERNEL)?);
            }
        }
        Ok(())
    }

    /// Prepare page table resources for mapping `num_pages` pages starting at `vfn_start`.
    ///
    /// Reserves capacity in `page_table_allocs`, then walks the hierarchy
    /// per-VFN to prepare pages for all missing PDEs, and big page tables where the range covers
    /// big pages.
    pub(super) fn prepare_map(
        &self,
        mm: &mut GpuMm<'_>,
        vfn_start: Vfn,
        num_pages: usize,
        page_table_allocs: &mut KVec<Pin<KBox<AllocatedBlocks>>>,
        pt_pages: &mut RBTree<VramAddress, PreparedPtPage>,
        tables: &TableCache,
    ) -> Result {
        // Pre-reserve so install_mappings() can use push_within_capacity (no alloc
        // in fence signalling critical path).
        let pt_upper_bound = M::pt_pages_upper_bound(num_pages);
        page_table_allocs.reserve(pt_upper_bound, GFP_KERNEL)?;

        // Walk the hierarchy to prepare pages for all missing PDEs, once per PTE-level table: the
        // pages of a table share the path to it.
        let num_pages_u64: u64 = num_pages.into_safe_cast();
        for i in 0..num_pages {
            let i_u64: u64 = i.into_safe_cast();
            let vfn = Vfn::new(vfn_start.raw() + i_u64);
            if i == 0 || PtWalkInner::<M>::starts_pte_table(vfn) {
                let needs_big = self.big_pages && Self::covers_big_page(vfn, num_pages_u64 - i_u64);
                // Remembered, so installed, with what it needs: nothing to prepare.
                if tables
                    .get(Self::block_of(vfn))
                    .is_some_and(|remembered| !needs_big || remembered.big.is_some())
                {
                    continue;
                }
                self.ensure_single_pte_path(mm, vfn, pt_pages)?;
                if needs_big {
                    self.ensure_big_table(mm, vfn, pt_pages)?;
                }
            }
        }
        Ok(())
    }

    /// Install prepared PDEs and write PTEs, then flush TLB.
    ///
    /// Drains `pt_pages` and moves allocations into `page_table_allocs`.
    #[expect(clippy::too_many_arguments)]
    pub(super) fn install_mappings(
        &self,
        mm: &mut GpuMm<'_>,
        pt_pages: &mut RBTree<VramAddress, PreparedPtPage>,
        page_table_allocs: &mut KVec<Pin<KBox<AllocatedBlocks>>>,
        tables: &mut TableCache,
        vfn_start: Vfn,
        pages: Pages<'_>,
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

                if page.level == M::DUAL_PDE_LEVEL {
                    tables.forget();
                    let mut dpde = M::DualPde::read(pramin, install_addr)?;
                    if let Some(alloc) = &page.alloc {
                        dpde = dpde.with_small(Pfn::from(table_address(alloc)?));
                    }
                    if let Some(big) = &page.big {
                        dpde = dpde.with_big(table_address(big)?)?;
                    }
                    dpde.write(pramin, install_addr)?;
                } else {
                    let alloc = page.alloc.as_ref().ok_or(EIO)?;
                    let new_pde =
                        M::Pde::new(AperturePde::VideoMemory, Pfn::from(table_address(alloc)?));
                    new_pde.write(pramin, install_addr)?;
                }

                for alloc in [page.alloc, page.big].into_iter().flatten() {
                    page_table_allocs
                        .push_within_capacity(alloc)
                        .map_err(|_| ENOMEM)?;
                }

                cursor = next;
            }

            // Write PTEs (all PDEs now installed in HW).
            let pages_per_big: u64 = PAGES_PER_BIG_PAGE.into_safe_cast();
            let invalid_pte: u64 = M::Pte::invalid().into();
            let mut ptes = KVec::new();
            let mut big_ptes = KVec::new();
            self.for_each_pte_run(
                pramin,
                tables,
                vfn_start,
                pages.len(),
                |pramin, run, vfn, tables| {
                    let Some((pte_addr, big_table)) = tables else {
                        kernel::pr_warn_once!("VMM: page table missing for VFN {vfn:?}\n");
                        return Err(EIO);
                    };
                    // Big PTEs, which hide the small ones under them unless unmapped, are
                    // looked at even where this address space maps no big pages: GSP-RM sets
                    // up BAR1 with some.

                    // Big PTEs for the whole big pages of physically contiguous VRAM, small
                    // ones (invalid under the big ones) for the rest.
                    ptes.clear();
                    big_ptes.clear();
                    let mut i = run.start;
                    while i < run.end {
                        let i_u64: u64 = i.into_safe_cast();
                        let page = Vfn::new(vfn_start.raw() + i_u64);
                        let big = big_table
                            .filter(|_| {
                                self.big_pages
                                    && page.raw() % pages_per_big == 0
                                    && i + PAGES_PER_BIG_PAGE <= run.end
                            })
                            .and_then(|table| Some((table, Self::big_page(aperture, pages, i)?)));
                        if let Some((big_table, first)) = big {
                            ptes.extend_with(PAGES_PER_BIG_PAGE, invalid_pte, GFP_KERNEL)?;
                            big_ptes.push(
                                (
                                    PtWalkInner::<M>::big_pte_addr(big_table, page),
                                    M::Pte::new(aperture, first, access),
                                ),
                                GFP_KERNEL,
                            )?;
                            i += PAGES_PER_BIG_PAGE;
                        } else {
                            let pfn = pages.get(i).ok_or(EINVAL)?;
                            ptes.push(M::Pte::new(aperture, pfn, access).into(), GFP_KERNEL)?;
                            i += 1;
                        }
                    }
                    pramin.write_u64s_posted(pte_addr, &ptes)?;

                    if let Some(big_table) = big_table {
                        // Big PTEs the small PTEs just written would be hidden under are
                        // unmapped, their other pages keeping what they mapped, with small PTEs.
                        let first = vfn;
                        let len: u64 = run.len().into_safe_cast();
                        let mapped_big = |chunk: Vfn| {
                            big_ptes.iter().any(|(addr, _)| {
                                *addr == PtWalkInner::<M>::big_pte_addr(big_table, chunk)
                            })
                        };
                        Self::split_big_pages(pramin, big_table, pte_addr, first, len, mapped_big)?;
                        let mut run = EntryRun::new();
                        for (addr, pte) in &big_ptes {
                            run.push(pramin, *addr, (*pte).into())?;
                        }
                        run.write(pramin)?;
                    }
                    Ok(())
                },
            )?;
        }

        // The entries reach VRAM before the TLBs are flushed.
        mm.pramin_mut().order()?;
        mm.tlb().flush(
            self.pdb_addr,
            Flush {
                bar: self.bar,
                downgrade: false,
            },
        )
    }

    /// Unmaps the big PTEs of `big_table` overlapping the `len` pages from `first`, whose small
    /// PTEs, from `first_pte` in their PTE-level table, are being rewritten: the MMU only looks at
    /// the small PTEs under a big PTE that is unmapped (all zeroes). A valid one maps its big page;
    /// an invalid one is sparse (`VOL`: reads return zeroes, writes are dropped) or blocks the
    /// small PTEs (`PRIV`): GSP-RM leaves some of both in BAR1. The pages of each outside the
    /// range get small PTEs mapping what it mapped, or for an invalid one, its own value, which
    /// means the same for small PTEs or is merely invalid, and the big PTE is unmapped. The big
    /// pages `skip` returns true for are left as they are: they are being rewritten too.
    fn split_big_pages(
        pramin: &mut pramin::Pramin<'_>,
        big_table: VramAddress,
        first_pte: VramAddress,
        first: Vfn,
        len: u64,
        skip: impl Fn(Vfn) -> bool,
    ) -> Result {
        let pages_per_big: u64 = PAGES_PER_BIG_PAGE.into_safe_cast();
        let entry_size: u64 = M::entry_size(M::PTE_LEVEL).into_safe_cast();
        let first_index = M::level_index(VirtualAddress::from(first), M::PTE_LEVEL.as_index());
        let pte_table = VramAddress::from_raw(first_pte.into_raw() - first_index * entry_size);
        let end = first.raw() + len;
        let invalid_pte: u64 = M::Pte::invalid().into();

        // Big pages wholly in the range keep no page outside it: their big PTEs are invalidated
        // without being read, which costs far more than writing them. Only those at its ends are
        // read, to keep their other pages mapped.
        let mut invalidated = EntryRun::new();
        let mut big_page = first.raw() / pages_per_big * pages_per_big;
        while big_page < end {
            let big_vfn = Vfn::new(big_page);
            let big_pte_addr = PtWalkInner::<M>::big_pte_addr(big_table, big_vfn);
            if skip(big_vfn) {
                // Left as it is.
            } else if big_page >= first.raw() && big_page + pages_per_big <= end {
                invalidated.push(pramin, big_pte_addr, invalid_pte)?;
            } else {
                let big_pte = M::Pte::read(pramin, big_pte_addr)?;
                let raw: u64 = big_pte.into();
                if raw != invalid_pte {
                    let mut kept = EntryRun::new();
                    for page in big_page..big_page + pages_per_big {
                        if (first.raw()..end).contains(&page) {
                            continue;
                        }
                        let value = if big_pte.is_valid() {
                            let pfn = Pfn::new(big_pte.frame_number().raw() + (page - big_page));
                            big_pte.with_pfn(pfn).into()
                        } else {
                            raw
                        };
                        kept.push(
                            pramin,
                            PtWalkInner::<M>::pte_addr(pte_table, Vfn::new(page)),
                            value,
                        )?;
                    }
                    kept.write(pramin)?;
                    invalidated.push(pramin, big_pte_addr, invalid_pte)?;
                }
            }
            big_page += pages_per_big;
        }
        invalidated.write(pramin)
    }

    /// Splits the `num_pages` pages from `vfn_start` into runs whose PTEs follow each other in a
    /// PTE-level table, and calls `f` with the indices of each run's pages, its first page, and,
    /// if its page table is installed, the address of its first PTE and its big page table, if
    /// any.
    ///
    /// The page tables of each run are those [`Self::block_tables()`] remembers or walks to:
    /// walks read page directories through PRAMIN, uncached, which costs far more than writing
    /// PTEs.
    fn for_each_pte_run(
        &self,
        pramin: &mut pramin::Pramin<'_>,
        tables: &mut TableCache,
        vfn_start: Vfn,
        num_pages: usize,
        mut f: impl FnMut(
            &mut pramin::Pramin<'_>,
            Range<usize>,
            Vfn,
            Option<(VramAddress, Option<VramAddress>)>,
        ) -> Result,
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
            let run_tables = self
                .block_tables(pramin, tables, first)?
                .map(|walked| (PtWalkInner::<M>::pte_addr(walked.small, first), walked.big));
            f(pramin, start..end, first, run_tables)?;
            start = end;
        }

        Ok(())
    }

    /// Invalidate PTEs for a range and flush TLB.
    pub(super) fn invalidate_ptes(
        &self,
        mm: &mut GpuMm<'_>,
        tables: &mut TableCache,
        vfn_start: Vfn,
        num_pages: usize,
    ) -> Result {
        let invalid_pte: u64 = M::Pte::invalid().into();

        {
            let pramin = mm.pramin_mut();
            let mut ptes = KVec::new();
            self.for_each_pte_run(
                pramin,
                tables,
                vfn_start,
                num_pages,
                |pramin, run, vfn, tables| {
                    // Nothing is mapped without a page table.
                    let Some((pte_addr, big_table)) = tables else {
                        return Ok(());
                    };

                    ptes.clear();
                    ptes.extend_with(run.len(), invalid_pte, GFP_KERNEL)?;
                    pramin.write_u64s_posted(pte_addr, &ptes)?;

                    // Big pages in the range are unmapped, those it shares with other mappings are
                    // split to keep their pages outside it mapped.
                    if let Some(big_table) = big_table {
                        let len: u64 = run.len().into_safe_cast();
                        Self::split_big_pages(pramin, big_table, pte_addr, vfn, len, |_| false)?;
                    }
                    Ok(())
                },
            )?;
        }

        mm.pramin_mut().order()?;
        mm.tlb().flush(
            self.pdb_addr,
            Flush {
                bar: self.bar,
                downgrade: true,
            },
        )
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
    /// Create a new page table mapper for the given MMU version, for an address space of the
    /// kind `space`.
    pub(in crate::mm) fn new(pdb_addr: VramAddress, version: MmuVersion, space: VmmKind) -> Self {
        match version {
            MmuVersion::V2 => Self::V2(PtMapInner::<MmuV2>::new(pdb_addr, space)),
            MmuVersion::V3 => Self::V3(PtMapInner::<MmuV3>::new(pdb_addr, space)),
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
        tables: &TableCache,
    ) -> Result {
        pt_map_dispatch!(
            self,
            prepare_map(
                mm,
                vfn_start,
                num_pages,
                page_table_allocs,
                pt_pages,
                tables
            )
        )
    }

    /// Install prepared PDEs and write PTEs, then flush TLB.
    #[expect(clippy::too_many_arguments)]
    pub(in crate::mm) fn install_mappings(
        &self,
        mm: &mut GpuMm<'_>,
        pt_pages: &mut RBTree<VramAddress, PreparedPtPage>,
        page_table_allocs: &mut KVec<Pin<KBox<AllocatedBlocks>>>,
        tables: &mut TableCache,
        vfn_start: Vfn,
        pages: Pages<'_>,
        access: Access,
        aperture: AperturePte,
    ) -> Result {
        pt_map_dispatch!(
            self,
            install_mappings(
                mm,
                pt_pages,
                page_table_allocs,
                tables,
                vfn_start,
                pages,
                access,
                aperture
            )
        )
    }

    /// Invalidate PTEs for a range and flush TLB.
    pub(in crate::mm) fn invalidate_ptes(
        &self,
        mm: &mut GpuMm<'_>,
        tables: &mut TableCache,
        vfn_start: Vfn,
        num_pages: usize,
    ) -> Result {
        pt_map_dispatch!(self, invalidate_ptes(mm, tables, vfn_start, num_pages))
    }
}

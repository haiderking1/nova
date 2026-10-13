// SPDX-License-Identifier: GPL-2.0

//! Page table walker implementation for NVIDIA GPUs.
//!
//! This module provides page table walking functionality for MMU v2 and v3.
//! The walker traverses the page table hierarchy to resolve virtual addresses
//! to physical addresses or to find PTE locations.
//!
//! # Page Table Hierarchy
//!
//! ## MMU v2 (Turing/Ampere/Ada) - 5 levels
//!
//! ```text
//!     +-------+     +-------+     +-------+     +---------+     +-------+
//!     | PDB   |---->|  L1   |---->|  L2   |---->| L3 Dual |---->|  L4   |
//!     | (L0)  |     |       |     |       |     | PDE     |     | (PTE) |
//!     +-------+     +-------+     +-------+     +---------+     +-------+
//!       64-bit        64-bit        64-bit        128-bit         64-bit
//!        PDE           PDE           PDE        (big+small)        PTE
//! ```
//!
//! ## MMU v3 (Hopper+) - 6 levels
//!
//! ```text
//!     +-------+     +-------+     +-------+     +-------+     +---------+     +-------+
//!     | PDB   |---->|  L1   |---->|  L2   |---->|  L3   |---->| L4 Dual |---->|  L5   |
//!     | (L0)  |     |       |     |       |     |       |     | PDE     |     | (PTE) |
//!     +-------+     +-------+     +-------+     +-------+     +---------+     +-------+
//!       64-bit        64-bit        64-bit        64-bit        128-bit         64-bit
//!        PDE           PDE           PDE           PDE        (big+small)        PTE
//! ```
//!
//! # Result of a page table walk
//!
//! The walker returns a [`WalkResult`] indicating the outcome.

use core::marker::PhantomData;

use kernel::prelude::*;

use super::{
    AperturePde,
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
        GpuMm,
        Pfn,
        Vfn,
        VirtualAddress,
        VramAddress, //
    },
    num::{
        IntoSafeCast, //
    },
};

/// Result of walking to a PTE.
#[derive(Debug, Clone, Copy)]
pub(in crate::mm) enum WalkResult {
    /// Intermediate page tables are missing (only returned in lookup mode).
    PageTableMissing,
    /// PTE exists but is invalid (page not mapped).
    Unmapped { pte_addr: VramAddress },
    /// PTE exists and is valid (page is mapped).
    Mapped { pte_addr: VramAddress, pfn: Pfn },
}

/// Result of walking PDE levels only.
///
/// Returned by [`PtWalkInner::walk_pde_levels()`] to indicate whether all PDE
/// levels resolved or a PDE is missing.
#[derive(Debug, Clone, Copy)]
pub(in crate::mm) enum WalkPdeResult {
    /// All PDE levels resolved -- returns PTE page table address.
    Complete {
        /// VRAM address of the PTE-level page table.
        pte_table: VramAddress,
    },
    /// A PDE is missing and no prepared page was provided by the closure.
    Missing {
        /// PDE slot address in the parent page table (where to install).
        install_addr: VramAddress,
        /// The page table level that is missing.
        level: PageTableLevel,
    },
}

/// Page table walker.
pub(in crate::mm) struct PtWalkInner<M: MmuConfig> {
    pdb_addr: VramAddress,
    _phantom: PhantomData<M>,
}

impl<M: MmuConfig> PtWalkInner<M> {
    /// Calculate the VRAM address of an entry within a page table.
    fn entry_addr(table: VramAddress, level: PageTableLevel, index: u64) -> VramAddress {
        let entry_size: u64 = M::entry_size(level).into_safe_cast();
        table + index * entry_size
    }

    /// Returns the address of the PTE of `vfn` in `pte_table`, the PTE-level table it is in.
    pub(super) fn pte_addr(pte_table: VramAddress, vfn: Vfn) -> VramAddress {
        let idx = M::level_index(VirtualAddress::from(vfn), M::PTE_LEVEL.as_index());
        Self::entry_addr(pte_table, M::PTE_LEVEL, idx)
    }

    /// Returns whether `vfn` has the first PTE of its PTE-level table.
    pub(super) fn starts_pte_table(vfn: Vfn) -> bool {
        M::level_index(VirtualAddress::from(vfn), M::PTE_LEVEL.as_index()) == 0
    }

    /// Returns the PTE-level table `vfn` is in, if the page directories leading to it are
    /// installed.
    pub(super) fn pte_table(
        &self,
        pramin: &mut pramin::Pramin<'_>,
        vfn: Vfn,
    ) -> Result<Option<VramAddress>> {
        Ok(match self.walk_pde_levels(pramin, vfn, |_| None)? {
            WalkPdeResult::Complete { pte_table } => Some(pte_table),
            WalkPdeResult::Missing { .. } => None,
        })
    }

    /// Returns the address of the dual PDE of `vfn`, if the page directories leading to it are
    /// installed or `resolve_prepared` (as for [`Self::walk_pde_levels()`]) gives them.
    pub(super) fn dual_pde_addr(
        &self,
        pramin: &mut pramin::Pramin<'_>,
        vfn: Vfn,
        resolve_prepared: impl Fn(VramAddress) -> Option<VramAddress>,
    ) -> Result<Option<VramAddress>> {
        let va = VirtualAddress::from(vfn);
        let mut table = self.pdb_addr;

        for &level in M::PDE_LEVELS {
            let addr = Self::entry_addr(table, level, M::level_index(va, level.as_index()));
            if level == M::DUAL_PDE_LEVEL {
                return Ok(Some(addr));
            }
            let pde = M::Pde::read(pramin, addr)?;
            table = if pde.is_valid_vram() {
                pde.table_vram_address()
            } else if let Some(prepared) = resolve_prepared(addr) {
                prepared
            } else {
                return Ok(None);
            };
        }

        Ok(None)
    }

    /// Returns the big page table `vfn` is in, if one is installed. Fails if it isn't in VRAM,
    /// as one GSP-RM set up may not be: its entries can't be seen to then.
    pub(super) fn big_table(
        &self,
        pramin: &mut pramin::Pramin<'_>,
        vfn: Vfn,
    ) -> Result<Option<VramAddress>> {
        if !M::BIG_PAGES {
            return Ok(None);
        }
        let Some(addr) = self.dual_pde_addr(pramin, vfn, |_| None)? else {
            return Ok(None);
        };
        let dpde = M::DualPde::read(pramin, addr)?;
        if !dpde.has_big() {
            return Ok(None);
        }
        if dpde.big_aperture() != AperturePde::VideoMemory {
            kernel::pr_warn_once!(
                "VMM: big page table of VFN {vfn:?} in {:?}, not VRAM\n",
                dpde.big_aperture()
            );
            return Err(ENOTSUPP);
        }
        Ok(Some(dpde.big_vram_address()))
    }

    /// Returns the address of the PTE of the big page `vfn` is in, in `big_table`, the big page
    /// table it is in.
    pub(super) fn big_pte_addr(big_table: VramAddress, vfn: Vfn) -> VramAddress {
        // A big page table covers what a PTE-level table does.
        let small_index = M::level_index(VirtualAddress::from(vfn), M::PTE_LEVEL.as_index());
        let pages_per_big: u64 = PAGES_PER_BIG_PAGE.into_safe_cast();
        Self::entry_addr(big_table, M::PTE_LEVEL, small_index / pages_per_big)
    }

    /// Create a new page table walker.
    pub(super) fn new(pdb_addr: VramAddress) -> Self {
        Self {
            pdb_addr,
            _phantom: PhantomData,
        }
    }

    /// Walk PDE levels with closure-based resolution for missing PDEs.
    ///
    /// Traverses all PDE levels for the MMU version. At each level, reads the PDE.
    /// If valid, extracts the child table address and continues. If missing, calls
    /// `resolve_prepared(install_addr)` to resolve the missing PDE.
    pub(super) fn walk_pde_levels(
        &self,
        pramin: &mut pramin::Pramin<'_>,
        vfn: Vfn,
        resolve_prepared: impl Fn(VramAddress) -> Option<VramAddress>,
    ) -> Result<WalkPdeResult> {
        let va = VirtualAddress::from(vfn);
        let mut cur_table = self.pdb_addr;

        for &level in M::PDE_LEVELS {
            let idx = M::level_index(va, level.as_index());
            let install_addr = Self::entry_addr(cur_table, level, idx);

            if level == M::DUAL_PDE_LEVEL {
                // 128-bit dual PDE with big+small page table pointers.
                let dpde = M::DualPde::read(pramin, install_addr)?;
                if dpde.has_small() {
                    cur_table = dpde.small_vram_address();
                    continue;
                }
            } else {
                // Regular 64-bit PDE. Use `is_valid_vram()` because
                // `table_vram_address()` only reads the VRAM frame-number
                // bitfield; system-memory PDEs store the address in a
                // different (wider) field and would be silently truncated.
                let pde = M::Pde::read(pramin, install_addr)?;
                if pde.is_valid_vram() {
                    cur_table = pde.table_vram_address();
                    continue;
                }
            }

            // PDE missing in HW. Ask caller for resolution.
            if let Some(prepared_addr) = resolve_prepared(install_addr) {
                cur_table = prepared_addr;
                continue;
            }

            return Ok(WalkPdeResult::Missing {
                install_addr,
                level,
            });
        }

        Ok(WalkPdeResult::Complete {
            pte_table: cur_table,
        })
    }

    /// Returns the address of the page directory `depth` levels below the root one used for
    /// `vfn`, if it exists.
    pub(super) fn directory_at(
        &self,
        mm: &mut GpuMm<'_>,
        vfn: Vfn,
        depth: usize,
    ) -> Result<Option<VramAddress>> {
        let va = VirtualAddress::from(vfn);
        let pramin = mm.pramin_mut();
        let mut table = self.pdb_addr;

        for &level in M::PDE_LEVELS.get(..depth).ok_or(EINVAL)? {
            if level == M::DUAL_PDE_LEVEL {
                return Err(EINVAL);
            }
            let idx = M::level_index(va, level.as_index());
            let pde = M::Pde::read(pramin, Self::entry_addr(table, level, idx))?;
            if !pde.is_valid_vram() {
                return Ok(None);
            }
            table = pde.table_vram_address();
        }

        Ok(Some(table))
    }

    /// Walk to PTE for lookup only (no allocation).
    ///
    /// Returns [`WalkResult::PageTableMissing`] if intermediate tables don't exist.
    pub(super) fn walk_to_pte_lookup(&self, mm: &mut GpuMm<'_>, vfn: Vfn) -> Result<WalkResult> {
        self.walk_to_pte_lookup_with_window(mm.pramin_mut(), vfn)
    }

    /// Walk to PTE using a caller-provided PRAMIN manager (lookup only).
    pub(super) fn walk_to_pte_lookup_with_window(
        &self,
        pramin: &mut pramin::Pramin<'_>,
        vfn: Vfn,
    ) -> Result<WalkResult> {
        // A valid big PTE maps the page whatever the small page table has.
        if let Some(big_table) = self.big_table(pramin, vfn)? {
            let pte_addr = Self::big_pte_addr(big_table, vfn);
            let pte = M::Pte::read(pramin, pte_addr)?;
            if pte.is_valid() {
                let pages_per_big: u64 = PAGES_PER_BIG_PAGE.into_safe_cast();
                return Ok(WalkResult::Mapped {
                    pte_addr,
                    pfn: Pfn::new(pte.frame_number().raw() + vfn.raw() % pages_per_big),
                });
            }
        }

        match self.walk_pde_levels(pramin, vfn, |_| None)? {
            WalkPdeResult::Complete { pte_table } => {
                Self::read_pte_at_level(pramin, vfn, pte_table)
            }
            WalkPdeResult::Missing { .. } => Ok(WalkResult::PageTableMissing),
        }
    }

    /// Read the PTE at the PTE level given the PTE table address.
    fn read_pte_at_level(
        pramin: &mut pramin::Pramin<'_>,
        vfn: Vfn,
        pte_table: VramAddress,
    ) -> Result<WalkResult> {
        let va = VirtualAddress::from(vfn);
        let pte_level = M::PTE_LEVEL;
        let pte_idx = M::level_index(va, pte_level.as_index());
        let pte_addr = Self::entry_addr(pte_table, pte_level, pte_idx);
        let pte = M::Pte::read(pramin, pte_addr)?;

        if pte.is_valid() {
            return Ok(WalkResult::Mapped {
                pte_addr,
                pfn: pte.frame_number(),
            });
        }
        Ok(WalkResult::Unmapped { pte_addr })
    }
}

macro_rules! pt_walk_dispatch {
    ($self:expr, $method:ident ( $($arg:expr),* $(,)? )) => {
        match $self {
            PtWalk::V2(inner) => inner.$method($($arg),*),
            PtWalk::V3(inner) => inner.$method($($arg),*),
        }
    };
}

/// Page table walker dispatch.
pub(in crate::mm) enum PtWalk {
    /// MMU v2 (Turing/Ampere/Ada).
    V2(PtWalkInner<MmuV2>),
    /// MMU v3 (Hopper+).
    V3(PtWalkInner<MmuV3>),
}

impl PtWalk {
    /// Create a new page table walker for the given MMU version.
    pub(in crate::mm) fn new(pdb_addr: VramAddress, version: MmuVersion) -> Self {
        match version {
            MmuVersion::V2 => Self::V2(PtWalkInner::<MmuV2>::new(pdb_addr)),
            MmuVersion::V3 => Self::V3(PtWalkInner::<MmuV3>::new(pdb_addr)),
        }
    }

    /// Walk to PTE for lookup.
    pub(in crate::mm) fn walk_to_pte(&self, mm: &mut GpuMm<'_>, vfn: Vfn) -> Result<WalkResult> {
        pt_walk_dispatch!(self, walk_to_pte_lookup(mm, vfn))
    }

    /// Returns the address of the page directory `depth` levels below the root one used for
    /// `vfn`, if it exists.
    pub(in crate::mm) fn directory_at(
        &self,
        mm: &mut GpuMm<'_>,
        vfn: Vfn,
        depth: usize,
    ) -> Result<Option<VramAddress>> {
        pt_walk_dispatch!(self, directory_at(mm, vfn, depth))
    }
}

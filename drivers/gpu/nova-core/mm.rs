// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

//! Memory management subsystems.

#![cfg_attr(not(CONFIG_NOVA_CORE_SELFTESTS), expect(dead_code))]

/// Implements `From` conversions between a frame-number type and `Bounded<u64, N>`.
///
/// Each MMU version module should invoke this for the specific bit widths used by that version's
/// PTE/PDE bitfield definitions.
macro_rules! impl_frame_number_bounded {
    ($type:ty, $bits:literal) => {
        impl From<Bounded<u64, $bits>> for $type {
            fn from(val: Bounded<u64, $bits>) -> Self {
                Self::new(val.get())
            }
        }

        impl From<$type> for Bounded<u64, $bits> {
            fn from(v: $type) -> Self {
                Bounded::from_expr(v.raw() & ::kernel::bits::genmask_u64(0..=($bits - 1)))
            }
        }
    };
}

/// Implements `From` conversions between [`Pfn`] and `Bounded<u64, N>` for bitfield interop.
macro_rules! impl_pfn_bounded {
    ($bits:literal) => {
        impl_frame_number_bounded!(Pfn, $bits);
    };
}

use core::{
    fmt::LowerHex,
    ops::{
        self,
        Range, //
    },
};

use kernel::{
    bitfield,
    fmt,
    gpu::buddy::{
        AllocatedBlocks,
        GpuBuddy,
        GpuBuddyAllocFlag,
        GpuBuddyAllocFlags,
        GpuBuddyAllocMode,
        GpuBuddyParams, //
    },
    io::{
        io_write,
        Io, //
    },
    num::Bounded,
    prelude::*,
    ptr::{
        Alignable,
        Alignment, //
    },
    sizes::SZ_4K,
    sync::Mutex, //
};

use crate::{
    driver::{
        Bar0,
        Bar1, //
    },
    gpu::Chipset,
    num::{
        self,
        IntoSafeCast, //
    },
};

pub(crate) use pagetable::BIG_PAGE_SIZE;
pub(crate) use tlb::Tlb;

pub(crate) mod bar_user;
mod hal;
pub(super) mod pagetable;
mod pramin;
mod regs;
pub(super) mod tlb;
pub(super) mod vmm;

/// GPU Memory Manager - owns all core MM components.
///
/// Provides centralized ownership of memory management resources:
/// - [`GpuBuddy`] allocator for VRAM page table allocation.
/// - [`pramin::Pramin`] for direct VRAM access.
/// - [`Tlb`] manager for translation buffer flush operations.
pub(crate) struct GpuMm<'gpu> {
    buddy: GpuBuddy,
    pramin: pramin::Pramin<'gpu>,
    tlb: Pin<KBox<Tlb<'gpu>>>,
    /// VRAM page tables are allocated from first, see [`PageTablePool`].
    pt_pool: Option<PageTablePool>,
}

/// VRAM mapped in BAR1, write-combined, which page tables are allocated from first, so that the CPU
/// writes them through BAR1: mapping or unmapping a buffer then streams its PTEs out, where through
/// the PRAMIN window, uncached, each costs about a microsecond (some 3 ms for a 12 MiB buffer).
/// Page tables are allocated from the rest of VRAM once it is full.
struct PageTablePool {
    /// Allocator of the pool's VRAM.
    buddy: GpuBuddy,
    /// Its BAR1 mapping, which [`GpuMm::take_page_table_pool()`] gives back to be unmapped.
    mapped: Option<vmm::MappedRange>,
    /// Whether writes through BAR1 were seen to land in VRAM: the pool is only used if so.
    usable: bool,
    /// The pool's VRAM.
    _vram: ContiguousVram,
}

/// Access to the GPU memory manager for code that needs it only now and then, between which it
/// may wait, such as for GSP-RM: the manager itself, which the caller already locked, or the mutex
/// it is behind, which is then locked for each use only.
///
/// The manager accesses VRAM through PRAMIN, a single window, so it is shared by locking it whole:
/// whatever holds it, other users of VRAM wait for, among them the display's per-frame copies.
pub(crate) trait MmAccess {
    /// Runs `f` with the memory manager.
    fn with<R>(&mut self, f: impl for<'g> FnOnce(&mut GpuMm<'g>) -> R) -> R;
}

impl MmAccess for GpuMm<'_> {
    fn with<R>(&mut self, f: impl for<'g> FnOnce(&mut GpuMm<'g>) -> R) -> R {
        f(self)
    }
}

impl MmAccess for &Mutex<GpuMm<'_>> {
    fn with<R>(&mut self, f: impl for<'g> FnOnce(&mut GpuMm<'g>) -> R) -> R {
        f(&mut self.lock())
    }
}

impl<'gpu> GpuMm<'gpu> {
    /// Creates the GPU memory manager.
    pub(crate) fn new(
        bar: Bar0<'gpu>,
        chipset: Chipset,
        buddy_params: GpuBuddyParams,
        total_fb_end: VramAddress,
    ) -> Result<Self> {
        // PRAMIN covers all physical VRAM (including GSP-reserved areas
        // above the usable region, e.g. the BAR1 page directory).
        let vram_region = VramAddress::ZERO..total_fb_end;

        Ok(Self {
            buddy: GpuBuddy::new(buddy_params)?,
            pramin: pramin::Pramin::new(bar, chipset, vram_region)?,
            tlb: KBox::pin_init(Tlb::new(bar), GFP_KERNEL)?,
            pt_pool: None,
        })
    }

    /// Has page tables allocated from `vram` first, which `mapped` maps in BAR1 from its offset
    /// `bar1_offset`, and accessed through BAR1: see [`PageTablePool`].
    pub(crate) fn set_page_table_pool(
        &mut self,
        vram: ContiguousVram,
        mapped: vmm::MappedRange,
        bar1: Bar1<'gpu>,
        bar1_offset: usize,
    ) -> Result {
        let range = vram.range();
        let buddy = GpuBuddy::new(GpuBuddyParams {
            base_offset: range.start.into_raw(),
            size: range.end - range.start,
            chunk_size: Alignment::new::<SZ_4K>(),
        })?;
        let check_addr = range.start;
        self.pramin.set_wc_region(Some(pramin::WcRegion {
            bar1,
            vram: range,
            bar1_offset,
        }));

        // Page tables written through BAR1 have to be what the GPU reads: check that writes
        // through it land in VRAM as the PRAMIN window sees it, before using it.
        let pattern = [
            0x0123_4567_89ab_cdef_u64,
            0xfedc_ba98_7654_3210,
            0,
            u64::MAX,
        ];
        let checked = self.pramin.write_u64s(check_addr, &pattern).and_then(|()| {
            let wc = self.pramin.set_wc_region(None);
            let mut read = [0u64; 4];
            let result = (|| {
                for (i, value) in read.iter_mut().enumerate() {
                    let addr = check_addr
                        .checked_add(u64::try_from(8 * i)?)
                        .ok_or(EINVAL)?;
                    *value = self.pramin.window_at::<u64>(addr)?.view().read_val();
                }
                Ok(())
            })();
            self.pramin.set_wc_region(wc);
            result.and_then(|()| if read == pattern { Ok(()) } else { Err(EIO) })
        });
        if checked.is_err() {
            self.pramin.set_wc_region(None);
        }

        // Kept even if unusable, for its mapping to be given back.
        self.pt_pool = Some(PageTablePool {
            buddy,
            mapped: Some(mapped),
            usable: checked.is_ok(),
            _vram: vram,
        });

        checked
    }

    /// Stops accessing the page table pool through BAR1, and gives back its BAR1 mapping, to be
    /// unmapped. Its page tables stay, and are accessed through the PRAMIN window from then on.
    pub(crate) fn take_page_table_pool(&mut self) -> Option<vmm::MappedRange> {
        self.pramin.set_wc_region(None);
        self.pt_pool.as_mut().and_then(|pool| pool.mapped.take())
    }

    /// Allocates `size` bytes of VRAM for page tables, aligned to `align`: from the page table
    /// pool if it has room, from the rest of VRAM otherwise.
    pub(super) fn alloc_page_table(
        &self,
        size: usize,
        align: Alignment,
    ) -> Result<Pin<KBox<AllocatedBlocks>>> {
        let alloc = |buddy: &GpuBuddy| {
            KBox::pin_init(
                buddy.alloc_blocks(
                    GpuBuddyAllocMode::Simple,
                    size.into_safe_cast(),
                    align,
                    GpuBuddyAllocFlags::default(),
                ),
                GFP_KERNEL,
            )
        };

        // Only while the pool is accessed through BAR1: its page tables are slower otherwise.
        if let Some(pool) = self
            .pt_pool
            .as_ref()
            .filter(|pool| pool.usable && pool.mapped.is_some())
        {
            if let Ok(blocks) = alloc(&pool.buddy) {
                return Ok(blocks);
            }
        }
        alloc(&self.buddy)
    }

    /// Access the [`GpuBuddy`] allocator.
    pub(crate) fn buddy(&self) -> &GpuBuddy {
        &self.buddy
    }

    /// Access the [`pramin::Pramin`].
    fn pramin_mut(&mut self) -> &mut pramin::Pramin<'gpu> {
        &mut self.pramin
    }

    /// Access the [`Tlb`] manager.
    pub(crate) fn tlb(&self) -> &Tlb<'gpu> {
        self.tlb.as_ref().get_ref()
    }

    /// Allocates `size` bytes of physically contiguous VRAM, aligned to `align`.
    pub(crate) fn alloc_contiguous(&self, size: usize, align: Alignment) -> Result<ContiguousVram> {
        let blocks = KBox::pin_init(
            self.buddy.alloc_blocks(
                GpuBuddyAllocMode::Simple,
                size.into_safe_cast(),
                align,
                GpuBuddyAllocFlag::Contiguous,
            ),
            GFP_KERNEL,
        )?;

        // The blocks of a contiguous allocation follow each other: the allocation starts at the
        // lowest one.
        let start = blocks
            .iter()
            .map(|block| block.offset())
            .min()
            .ok_or(ENOMEM)?;
        let start = VramAddress::from_raw(start);
        let end = start.checked_add(size.into_safe_cast()).ok_or(EOVERFLOW)?;

        Ok(ContiguousVram {
            _blocks: blocks,
            range: start..end,
        })
    }

    /// Fills the VRAM in `range` with zeroes. `range` must be 8-byte aligned.
    pub(crate) fn clear_vram(&mut self, range: Range<VramAddress>) -> Result {
        /// Number of `u64`s cleared per PRAMIN access.
        const CHUNK: usize = 64;
        const CHUNK_BYTES: u64 = num::usize_as_u64(CHUNK * size_of::<u64>());

        let pramin = self.pramin_mut();
        let mut addr = range.start;
        while addr < range.end {
            if range.end - addr >= CHUNK_BYTES {
                let access = pramin.window_at::<[u64; CHUNK]>(addr)?;
                let view = access.view();
                for i in 0..CHUNK {
                    io_write!(view, [try: i], 0u64);
                }
                addr = addr + CHUNK_BYTES;
            } else {
                pramin.window_at::<u64>(addr)?.view().write_val(0u64);
                addr = addr + num::usize_as_u64(size_of::<u64>());
            }
        }

        Ok(())
    }

    /// Reads the word of VRAM at `addr`, which must be 4-byte aligned.
    pub(crate) fn read_vram(&mut self, addr: VramAddress) -> Result<u32> {
        Ok(self.pramin_mut().window_at::<u32>(addr)?.view().read_val())
    }

    /// Writes `words` to the VRAM at `addr`, which must be 4-byte aligned.
    pub(crate) fn write_vram(&mut self, addr: VramAddress, words: &[u32]) -> Result {
        /// Number of words written per PRAMIN access.
        const CHUNK: usize = 64;
        const WORD: u64 = num::usize_as_u64(size_of::<u32>());

        let pramin = self.pramin_mut();
        let mut addr = addr;
        let mut chunks = words.chunks_exact(CHUNK);
        for chunk in &mut chunks {
            let access = pramin.window_at::<[u32; CHUNK]>(addr)?;
            let view = access.view();
            for (i, word) in chunk.iter().enumerate() {
                io_write!(view, [try: i], *word);
            }
            addr = addr + num::usize_as_u64(CHUNK) * WORD;
        }
        for word in chunks.remainder() {
            pramin.window_at::<u32>(addr)?.view().write_val(*word);
            addr = addr + WORD;
        }

        Ok(())
    }
}

/// A physically contiguous range of VRAM, freed when dropped.
pub(crate) struct ContiguousVram {
    _blocks: Pin<KBox<AllocatedBlocks>>,
    range: Range<VramAddress>,
}

impl ContiguousVram {
    /// Returns the VRAM range of the allocation.
    pub(crate) fn range(&self) -> Range<VramAddress> {
        self.range.clone()
    }
}

/// Page size in bytes (4 KiB).
pub(crate) const PAGE_SIZE: usize = SZ_4K;

/// Physical VRAM address in GPU video memory.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub(crate) struct VramAddress(u64);

impl VramAddress {
    /// The zero address.
    pub(crate) const ZERO: Self = Self::from_raw(0);

    /// Creates an address from a raw value.
    pub(crate) const fn from_raw(addr: u64) -> Self {
        Self(addr)
    }

    /// Returns the address as a raw value.
    pub(crate) const fn into_raw(self) -> u64 {
        self.0
    }

    /// Adds `rhs` to this address, returning [`None`] on overflow.
    pub(crate) const fn checked_add(self, rhs: u64) -> Option<Self> {
        match self.into_raw().checked_add(rhs) {
            Some(addr) => Some(Self::from_raw(addr)),
            None => None,
        }
    }
}

impl Alignable for VramAddress {
    fn align_down(self, alignment: Alignment) -> Self {
        Self::from_raw(self.into_raw().align_down(alignment))
    }

    fn align_up(self, alignment: Alignment) -> Option<Self> {
        self.into_raw().align_up(alignment).map(Self::from_raw)
    }
}

impl LowerHex for VramAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        LowerHex::fmt(&self.into_raw(), f)
    }
}

impl fmt::Debug for VramAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_fmt(fmt!("{:#x}", self))
    }
}

impl ops::Add<u64> for VramAddress {
    type Output = Self;

    fn add(self, rhs: u64) -> Self::Output {
        Self::from_raw(self.into_raw() + rhs)
    }
}

impl ops::Sub for VramAddress {
    type Output = u64;

    fn sub(self, rhs: Self) -> Self::Output {
        self.into_raw() - rhs.into_raw()
    }
}

impl From<Pfn> for VramAddress {
    fn from(pfn: Pfn) -> Self {
        Self::from_raw(pfn.raw() << 12)
    }
}

bitfield! {
    /// Virtual address in GPU address space.
    pub(crate) struct VirtualAddress(u64) {
        /// Offset within 4KB page.
        11:0    offset;
        /// Virtual frame number.
        63:12   frame_number => Vfn;
    }
}

impl VirtualAddress {
    /// Create a new virtual address from a raw value.
    #[expect(dead_code)]
    pub(crate) const fn new(addr: u64) -> Self {
        Self::from_raw(addr)
    }
}

impl From<Vfn> for VirtualAddress {
    fn from(vfn: Vfn) -> Self {
        Self::zeroed().with_frame_number(vfn)
    }
}

/// Physical Frame Number.
///
/// Represents a physical page in VRAM.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Pfn(u64);

impl Pfn {
    /// Create a new PFN from a frame number.
    pub(crate) const fn new(frame_number: u64) -> Self {
        Self(frame_number)
    }

    /// Get the raw frame number.
    pub(crate) const fn raw(self) -> u64 {
        self.0
    }
}

impl From<VramAddress> for Pfn {
    fn from(addr: VramAddress) -> Self {
        Self::new(addr.into_raw() >> 12)
    }
}

impl From<u64> for Pfn {
    fn from(val: u64) -> Self {
        Self(val)
    }
}

impl From<Pfn> for u64 {
    fn from(pfn: Pfn) -> Self {
        pfn.0
    }
}

impl_pfn_bounded!(52);

/// A run of physically contiguous pages, in [`PfnRuns`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct PfnRun {
    /// The frame of the first page.
    first: Pfn,
    /// How many pages the run has.
    len: usize,
    /// How many pages the runs before this one have.
    offset: usize,
}

/// The pages of a buffer, in order, as runs of contiguous ones: the blocks of a VRAM buffer, or
/// the entries of a scatter-gather table. Mapping them takes [`PfnRuns::pages`], which spares
/// listing them page by page.
pub(crate) struct PfnRuns {
    runs: KVVec<PfnRun>,
    len: usize,
}

impl PfnRuns {
    /// Creates an empty list, with room for `runs` runs.
    pub(crate) fn with_capacity(runs: usize) -> Result<Self> {
        Ok(Self {
            runs: KVVec::with_capacity(runs, GFP_KERNEL)?,
            len: 0,
        })
    }

    /// Appends the `len` pages from frame `first`, which extend the last run if they follow it.
    pub(crate) fn push(&mut self, first: Pfn, len: usize) -> Result {
        if len == 0 {
            return Ok(());
        }
        let total = self.len.checked_add(len).ok_or(EOVERFLOW)?;
        let follows = self.runs.last().is_some_and(|last| {
            u64::try_from(last.len)
                .is_ok_and(|n| last.first.raw().checked_add(n) == Some(first.raw()))
        });
        match self.runs.last_mut() {
            Some(last) if follows => last.len += len,
            _ => self.runs.push(
                PfnRun {
                    first,
                    len,
                    offset: self.len,
                },
                GFP_KERNEL,
            )?,
        }
        self.len = total;

        Ok(())
    }

    /// Drops the pages after the first `len`.
    pub(crate) fn truncate(&mut self, len: usize) {
        if len >= self.len {
            return;
        }
        let kept = self.runs.partition_point(|run| run.offset < len);
        self.runs.truncate(kept);
        if let Some(last) = self.runs.last_mut() {
            last.len = len - last.offset;
        }
        self.len = len;
    }

    /// Returns how many pages there are.
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// Returns the pages, for mapping.
    pub(crate) fn pages(&self) -> Pages<'_> {
        Pages::Runs {
            runs: &self.runs,
            skip: 0,
            len: self.len,
        }
    }
}

/// The physical pages of a mapping, in order.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Pages<'a> {
    /// Frames listed one by one.
    Each(&'a [Pfn]),
    /// `len` contiguous frames from `first`.
    Contiguous { first: Pfn, len: usize },
    /// The pages of `runs` from page `skip`, `len` pages in all.
    Runs {
        runs: &'a [PfnRun],
        skip: usize,
        len: usize,
    },
}

impl<'a> Pages<'a> {
    /// The page-aligned address `range`, as contiguous pages.
    pub(crate) fn of_range(range: Range<u64>) -> Result<Self> {
        let page_size: u64 = PAGE_SIZE.into_safe_cast();
        if range.start % page_size != 0 || range.end % page_size != 0 || range.start >= range.end {
            return Err(EINVAL);
        }

        Ok(Self::Contiguous {
            first: Pfn::new(range.start / page_size),
            len: usize::try_from((range.end - range.start) / page_size)?,
        })
    }

    /// Returns how many pages there are.
    pub(crate) fn len(&self) -> usize {
        match *self {
            Self::Each(pfns) => pfns.len(),
            Self::Contiguous { len, .. } | Self::Runs { len, .. } => len,
        }
    }

    /// Returns whether there are no pages.
    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the run that page `page` of `runs` is in.
    fn run_of(runs: &[PfnRun], page: usize) -> Option<&PfnRun> {
        runs.get(runs.partition_point(|run| run.offset + run.len <= page))
    }

    /// Returns the frame of page `i`, if there are more pages than `i`.
    pub(crate) fn get(&self, i: usize) -> Option<Pfn> {
        if i >= self.len() {
            return None;
        }
        match *self {
            Self::Each(pfns) => pfns.get(i).copied(),
            Self::Contiguous { first, .. } => Some(Pfn::new(first.raw() + u64::try_from(i).ok()?)),
            Self::Runs { runs, skip, .. } => {
                let page = skip + i;
                let run = Self::run_of(runs, page)?;
                Some(Pfn::new(
                    run.first.raw() + u64::try_from(page - run.offset).ok()?,
                ))
            }
        }
    }

    /// Returns the frame of page `i` if the `n` pages from it are physically contiguous.
    pub(crate) fn contiguous_from(&self, i: usize, n: usize) -> Option<Pfn> {
        let end = i.checked_add(n).filter(|&end| n > 0 && end <= self.len())?;
        let first = self.get(i)?;
        let contiguous = match *self {
            Self::Each(pfns) => pfns[i..end]
                .iter()
                .zip(first.raw()..)
                .all(|(pfn, want)| pfn.raw() == want),
            Self::Contiguous { .. } => true,
            Self::Runs { runs, skip, .. } => {
                let run = Self::run_of(runs, skip + i)?;
                skip + end <= run.offset + run.len
            }
        };

        contiguous.then_some(first)
    }

    /// Returns the pages in `range`, if there are that many.
    pub(crate) fn sub(&self, range: Range<usize>) -> Option<Self> {
        if range.start > range.end || range.end > self.len() {
            return None;
        }
        let len = range.end - range.start;
        Some(match *self {
            Self::Each(pfns) => Self::Each(pfns.get(range)?),
            Self::Contiguous { first, .. } => Self::Contiguous {
                first: Pfn::new(first.raw() + u64::try_from(range.start).ok()?),
                len,
            },
            Self::Runs { runs, skip, .. } => Self::Runs {
                runs,
                skip: skip + range.start,
                len,
            },
        })
    }
}

impl<'a> From<&'a [Pfn]> for Pages<'a> {
    fn from(pfns: &'a [Pfn]) -> Self {
        Self::Each(pfns)
    }
}

/// Virtual Frame Number.
///
/// Represents a virtual page in GPU address space.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Vfn(u64);

impl Vfn {
    /// Create a new VFN from a frame number.
    pub(crate) const fn new(frame_number: u64) -> Self {
        Self(frame_number)
    }

    /// Get the raw frame number.
    pub(crate) const fn raw(self) -> u64 {
        self.0
    }
}

impl From<VirtualAddress> for Vfn {
    fn from(addr: VirtualAddress) -> Self {
        addr.frame_number()
    }
}

impl From<u64> for Vfn {
    fn from(val: u64) -> Self {
        Self(val)
    }
}

impl From<Vfn> for u64 {
    fn from(vfn: Vfn) -> Self {
        vfn.0
    }
}

impl_frame_number_bounded!(Vfn, 52);

/// Test pattern for the display the firmware set up at boot.
///
/// Draws through PRAMIN, which reaches any VRAM address without a BAR1 mapping, to check on real
/// hardware that writing the [`BootDisplay`] framebuffer shows on screen.
#[cfg(CONFIG_NOVA_CORE_SELFTESTS)]
pub(crate) mod boot_display_test {
    use core::ops::Range;

    use kernel::{
        device,
        io::{
            io_write,
            Io, //
        },
        screen_info::{
            Channel,
            PixelFormat, //
        },
    };

    use super::*;
    use crate::{
        boot_display::BootDisplay,
        mm::{
            bar_user::BarUser,
            vmm::MappedRange, //
        },
        num::IntoSafeCast, //
    };

    /// Pixels written per PRAMIN access: amortizes the window check and ordering read.
    const CHUNK: usize = 64;

    /// Maps the framebuffer of `display` into BAR1, checks the mapping with
    /// [`check_bar1_mapping()`], and unmaps it.
    pub(crate) fn check_bar1(
        dev: &device::Device<device::Bound>,
        mm: &mut GpuMm<'_>,
        bar_user: &BarUser<'_>,
        display: &BootDisplay,
    ) -> Result {
        // BAR1 maps whole pages; the end of the last line may not be page-aligned.
        let page_size: u64 = PAGE_SIZE.into_safe_cast();
        let end = display
            .vram
            .end
            .into_raw()
            .checked_next_multiple_of(page_size)
            .ok_or(EOVERFLOW)?;
        let mapped = bar_user.map_vram(mm, display.vram.start..VramAddress::from_raw(end))?;

        let checked = check_bar1_mapping(dev, mm, bar_user, &mapped, display);
        bar_user.unmap(mm, mapped).and(checked)
    }

    /// Checks that `mapped`, the BAR1 mapping of `display`'s framebuffer, maps each page to the
    /// right VRAM: writes a marker into every page through BAR1 and reads VRAM back through
    /// PRAMIN. Logs the pages that map elsewhere, and for the first ones where their writes land.
    fn check_bar1_mapping(
        dev: &device::Device<device::Bound>,
        mm: &mut GpuMm<'_>,
        bar_user: &BarUser<'_>,
        mapped: &MappedRange,
        display: &BootDisplay,
    ) -> Result {
        // Probe an offset inside each page, away from the start.
        const PROBE: usize = 0x40;

        let num_pages = mapped.num_pages;
        let fb_pages = usize::try_from(display.vram.end - display.vram.start)? / PAGE_SIZE;
        let mut bad = 0;

        for page in 0..num_pages.min(fb_pages) {
            let offset = page * PAGE_SIZE + PROBE;
            let vram = display.vram.start + u64::try_from(offset)?;
            let marker = 0x5a00_0000 | u32::try_from(page)?;

            // Restore through BAR1 what BAR1 points at, wherever that is.
            let original = bar_user.read32(mapped, offset)?;
            bar_user.write(mapped, offset, &marker.to_le_bytes())?;
            // Reading back through BAR1 also flushes the posted write.
            let via_bar1 = bar_user.read32(mapped, offset)?;
            let in_vram = mm.pramin_mut().window_at::<u32>(vram)?.view().read_val();

            if in_vram != marker {
                if bad < 2 {
                    // Find where the write went within the framebuffer.
                    let mut found = None;
                    for other in 0..fb_pages {
                        let addr = display.vram.start + u64::try_from(other * PAGE_SIZE + PROBE)?;
                        if mm.pramin_mut().window_at::<u32>(addr)?.view().read_val() == marker {
                            found = Some(other);
                            break;
                        }
                    }
                    dev_warn!(
                        dev,
                        "boot display: BAR1 page {page} reads back {via_bar1:#x}, but VRAM page \
                         {page} has {in_vram:#x}; the write landed in framebuffer page {found:?}\n"
                    );
                } else if bad < 8 {
                    dev_warn!(dev, "boot display: BAR1 page {page} maps elsewhere\n");
                }
                bad += 1;
            }

            bar_user.write(mapped, offset, &original.to_le_bytes())?;
        }

        dev_info!(
            dev,
            "boot display: BAR1 mapping check: {} of {} pages map to the right VRAM\n",
            num_pages.min(fb_pages) - bad,
            num_pages.min(fb_pages)
        );

        // Where the page tables say each 2 MiB chunk goes.
        for page in (0..num_pages)
            .step_by(512)
            .flat_map(|p| [p, (p + 511).min(num_pages - 1)])
        {
            let (vfn, pte) = bar_user.walk_mapped(mm, mapped, page)?;
            match pte {
                Some((pte_addr, pfn)) => dev_info!(
                    dev,
                    "boot display: page {page}: VFN {:#x}, PTE at {:#x}, maps PFN {:?}\n",
                    vfn.raw(),
                    pte_addr.into_raw(),
                    pfn.map(|p| p.raw())
                ),
                None => dev_info!(
                    dev,
                    "boot display: page {page}: VFN {:#x}, no page table\n",
                    vfn.raw()
                ),
            }
        }
        Ok(())
    }

    /// Returns `rgb` (8 bits per channel, `0xRRGGBB`) encoded in `format`.
    fn encode(format: &PixelFormat, rgb: u32) -> u32 {
        let channel = |c: Channel, value: u32| {
            let value = value & 0xff;
            match c.length {
                0 => 0,
                l @ 1..=8 => (value >> (8 - l)) << c.offset,
                l => (value << (l - 8)) << c.offset,
            }
        };

        channel(format.red, rgb >> 16) | channel(format.green, rgb >> 8) | channel(format.blue, rgb)
    }

    /// Fills the boot display with eight vertical color bars.
    pub(crate) fn draw(
        dev: &device::Device<device::Bound>,
        mm: &mut GpuMm<'_>,
        display: &BootDisplay,
        usable_fb_regions: &[Range<u64>],
    ) -> Result {
        const BARS: [u32; 8] = [
            0xffffff, 0xffff00, 0x00ffff, 0x00ff00, 0xff00ff, 0xff0000, 0x0000ff, 0x202020,
        ];

        let (start, end) = (display.vram.start.into_raw(), display.vram.end.into_raw());
        // Don't draw over memory nova may hand out.
        if usable_fb_regions
            .iter()
            .any(|r| r.start < end && start < r.end)
        {
            dev_warn!(
                dev,
                "boot display: [{start:#x}, {end:#x}) is usable VRAM, not drawing\n"
            );
            return Err(EBUSY);
        }
        if display.format.bits_per_pixel != 32 {
            dev_info!(
                dev,
                "boot display: {} bpp not supported by the test pattern\n",
                display.format.bits_per_pixel
            );
            return Err(ENOTSUPP);
        }

        let width: usize = display.width.into_safe_cast();
        let bar = |x: usize| encode(&display.format, BARS[x * BARS.len() / width]);
        let pramin = mm.pramin_mut();

        for y in 0..u64::from(display.height) {
            let line = display.vram.start + y * u64::from(display.stride);
            let mut x = 0;
            while x < width {
                let addr = line + (x * size_of::<u32>()).into_safe_cast();
                if x + CHUNK <= width {
                    let access = pramin.window_at::<[u32; CHUNK]>(addr)?;
                    let view = access.view();
                    for i in 0..CHUNK {
                        io_write!(view, [try: i], bar(x + i));
                    }
                    x += CHUNK;
                } else {
                    pramin.window_at::<u32>(addr)?.view().write_val(bar(x));
                    x += 1;
                }
            }
        }

        dev_info!(
            dev,
            "boot display: drew test pattern ({}x{})\n",
            display.width,
            display.height
        );
        Ok(())
    }
}

#[cfg(CONFIG_NOVA_CORE_SELFTESTS)]
pub(crate) mod selftest {
    use core::ops::Range;

    use kernel::{
        device,
        sizes::SizeConstants, //
    };

    use super::*;

    /// Run MM subsystem self-tests during probe.
    pub(crate) fn run(
        dev: &device::Device<device::Bound>,
        mm: &mut GpuMm<'_>,
        usable_fb_regions: &[Range<u64>],
        bar_user: &bar_user::BarUser<'_>,
        bar1_pdb: u64,
        chipset: Chipset,
    ) -> Result {
        // VRAM span the self-tests are free to overwrite, from the chosen test base.
        const SELFTEST_SPAN: u64 = u64::SZ_64M;

        let base = usable_fb_regions.iter().find_map(|region| {
            // Tests rely on this being 8 byte aligned for checking misalignment handling.
            let base = region.start.align_up(Alignment::new::<8>())?;
            (base.checked_add(SELFTEST_SPAN)? <= region.end).then_some(base)
        });
        let Some(base) = base else {
            dev_warn!(
                dev,
                "PRAMIN: skipping self-tests, no usable VRAM region of {:#x} bytes\n",
                SELFTEST_SPAN
            );
            return Ok(());
        };

        pramin::selftest::run(dev, mm.pramin_mut(), VramAddress::from_raw(base))?;
        bar_user::run_self_test(dev, mm, bar_user, bar1_pdb, chipset)
    }
}

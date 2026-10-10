// SPDX-License-Identifier: GPL-2.0

//! GPU virtual address spaces for channels.
//!
//! Channels access memory through GPU virtual addresses. Nova owns the page tables of the
//! address spaces of its channels, as nouveau does: GSP-RM is told about an address space and its
//! page directory, but leaves the mappings to Nova.

use core::ops::Range;

use kernel::{
    device,
    prelude::*,
    ptr::Alignment,
    sizes::{
        SZ_2M,
        SZ_4K, //
    },
};

use crate::{
    gsp::{
        cmdq::Cmdq,
        commands::{
            Client,
            Device,
            Handle,
            VaSpace, //
        },
        rm::commands::{
            AllocVaSpace,
            DmaSetPageDirectory,
            DmaUnsetPageDirectory,
            RmAlloc,
            RmControl,
            RmFree,
            VaspaceCopyServerReservedPdes, //
        },
        ServerReservedLevel, RM_SERVER_VA,
    },
    mm::{
        pagetable::{
            Access,
            MmuVersion, //
        },
        vmm::{
            MappedRange,
            Memory,
            Vmm, //
        },
        ContiguousVram,
        GpuMm,
        MmAccess,
        Pfn,
        VirtualAddress,
        VramAddress,
        PAGE_SIZE, //
    },
    num,
};

/// Size of the address spaces of channels: plenty for kernel channels.
pub const VA_SIZE: u64 = 1 << 40;
/// Start of the addresses mapped: those below are left unmapped, so that null pointers fault.
const VA_START: u64 = 1 << 16;
/// Start of the addresses of aligned mappings, away from the other mappings, which take the
/// lowest free addresses, and from GSP-RM's part of split address spaces.
const ALIGNED_VA_START: u64 = 1 << 33;
static_assert!(RM_SERVER_VA.end <= ALIGNED_VA_START);

/// How GSP-RM sees an address space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Nova owns the page tables, and GSP-RM is only told about the root page directory.
    ClientOwned,
    /// The address space is split between Nova and GSP-RM, which maps buffers of its own in
    /// [`RM_SERVER_VA`], as building the golden context of the graphics engine needs. GSP-RM is
    /// given the page directories above its part with [`AddressSpace::share_with_rm`].
    Split,
}

/// Which addresses of an address space Nova maps its own buffers at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum VaLayout {
    /// All of them, as in the address spaces of kernel channels.
    Kernel,
    /// Those of the range, as in the address spaces of clients, which map at the other addresses
    /// themselves, with [`AddressSpace::map_at`].
    Client(Range<u64>),
}

/// A GPU virtual address space whose page tables Nova owns.
///
/// It must be released with [`AddressSpace::free`] while the GSP is running.
pub(crate) struct AddressSpace {
    client: Handle<Client>,
    device: Handle<Device>,
    handle: Handle<VaSpace>,
    mode: Mode,
    mmu_version: MmuVersion,
    /// The root page directory.
    _pdb: ContiguousVram,
    vmm: Vmm,
    /// Where aligned mappings start.
    aligned_start: u64,
    /// The addresses Nova maps its own buffers at.
    layout: VaLayout,
}

impl AddressSpace {
    /// Allocates an empty address space named `handle` in `mode` under `device` of `client`, in
    /// which Nova maps its own buffers as `layout` says.
    #[expect(clippy::too_many_arguments)]
    pub(crate) fn new(
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        mm: &mut impl MmAccess,
        mmu_version: MmuVersion,
        client: Handle<Client>,
        device: Handle<Device>,
        handle: u32,
        mode: Mode,
        layout: VaLayout,
    ) -> Result<Self> {
        let page_size = num::usize_as_u64(PAGE_SIZE);
        let aligned_start = match &layout {
            VaLayout::Kernel => ALIGNED_VA_START,
            VaLayout::Client(kernel) => {
                // GSP-RM's part of split address spaces is Nova's.
                if mode != Mode::ClientOwned
                    || kernel.start < VA_START
                    || kernel.end > VA_SIZE
                    || kernel.start >= kernel.end
                    || kernel.start % page_size != 0
                    || kernel.end % page_size != 0
                {
                    return Err(EINVAL);
                }

                // Aligned mappings in the upper half, away from the others.
                (kernel.start + (kernel.end - kernel.start) / 2)
                    .next_multiple_of(num::usize_as_u64(SZ_2M))
            }
        };

        // An empty root page directory: no mappings. The memory manager is only taken for it, not
        // while GSP-RM allocates the address space.
        let pdb = mm.with(|mm| -> Result<_> {
            let pdb = mm.alloc_contiguous(SZ_4K, Alignment::new::<SZ_4K>())?;
            mm.clear_vram(pdb.range())?;
            Ok(pdb)
        })?;
        let pdb_addr = pdb.range().start;
        let vmm = Vmm::new(pdb_addr, mmu_version, VA_SIZE)?;
        match &layout {
            VaLayout::Kernel => vmm.reserve(0..VA_START)?,
            // Nova's mappings stay in the client's kernel range.
            VaLayout::Client(kernel) => {
                vmm.reserve(0..kernel.start)?;
                if kernel.end < VA_SIZE {
                    vmm.reserve(kernel.end..VA_SIZE)?;
                }
            }
        }

        let handle = match mode {
            Mode::ClientOwned => {
                let handle = RmAlloc::new(
                    client,
                    device,
                    Handle::new(handle),
                    AllocVaSpace::externally_owned(),
                )
                .send(dev, cmdq)?;

                let num_entries = u32::try_from(mmu_version.pdb_entries())?;
                RmControl::new(
                    client,
                    device,
                    DmaSetPageDirectory::new(handle, pdb_addr, num_entries),
                )
                .send(dev, cmdq)
                .inspect_err(|_| {
                    let _ = RmFree::new(client, handle).send(dev, cmdq);
                })?;

                handle
            }
            Mode::Split => {
                // Nova mustn't map anything in GSP-RM's part.
                vmm.reserve(RM_SERVER_VA)?;

                RmAlloc::new(client, device, Handle::new(handle), AllocVaSpace::split())
                    .send(dev, cmdq)?
            }
        };

        Ok(Self {
            client,
            device,
            handle,
            mode,
            mmu_version,
            _pdb: pdb,
            vmm,
            aligned_start,
            layout,
        })
    }

    /// Gives GSP-RM the page directories above its part of the address space, which must be split.
    /// They must exist: something must be mapped below the first 512 GiB.
    pub(crate) fn share_with_rm(
        &self,
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        mm: &mut GpuMm<'_>,
    ) -> Result {
        if self.mode != Mode::Split {
            return Err(EINVAL);
        }

        // The levels down to the one whose entries cover GSP-RM's part.
        let rm_size = RM_SERVER_VA.end - RM_SERVER_VA.start;
        let rm_start = VirtualAddress::from_raw(RM_SERVER_VA.start);
        let mut levels = KVec::new();
        for depth in 0.. {
            let layout = self.mmu_version.directory_layout(depth).ok_or(EINVAL)?;
            let entry_size = 1u64 << layout.va_shift;
            if entry_size < rm_size {
                return Err(EINVAL);
            }
            let addr = self.vmm.directory_at(mm, rm_start, depth)?.ok_or(ENOENT)?;
            levels.push(
                ServerReservedLevel {
                    addr: addr.into_raw(),
                    size: num::usize_as_u64(layout.size),
                    va_shift: u8::try_from(layout.va_shift)?,
                },
                GFP_KERNEL,
            )?;

            if entry_size == rm_size {
                return Ok(RmControl::new(
                    self.client,
                    self.handle,
                    VaspaceCopyServerReservedPdes::new(&levels, entry_size)?,
                )
                .send(dev, cmdq)
                .inspect_err(|e| {
                    dev_err!(dev, "GSP-RM rejected the split address space: {:?}\n", e)
                })?);
            }
        }

        Err(EINVAL)
    }

    /// Returns the RM handle of the address space.
    pub(crate) fn handle(&self) -> Handle<VaSpace> {
        self.handle
    }

    /// Maps the pages `pfns` of `memory`, in order, with `access` at a free virtual address,
    /// returned by [`MappedRange::start`]. The mapping must be given back to
    /// [`AddressSpace::unmap`].
    pub(crate) fn map_pages(
        &mut self,
        mm: &mut GpuMm<'_>,
        pfns: &[Pfn],
        memory: Memory,
        access: Access,
    ) -> Result<MappedRange> {
        self.vmm.map_memory(mm, pfns, None, access, memory)
    }

    /// Maps the page-aligned VRAM `range` with `access` at a free virtual address, returned by
    /// [`MappedRange::start`]. The mapping must be given back to [`AddressSpace::unmap`].
    pub(crate) fn map_vram(
        &mut self,
        mm: &mut GpuMm<'_>,
        range: Range<VramAddress>,
        access: Access,
    ) -> Result<MappedRange> {
        self.vmm.map_pages(
            mm,
            &Self::pfns(range.start.into_raw()..range.end.into_raw())?,
            None,
            access,
        )
    }

    /// Like [`AddressSpace::map_vram`], but at a virtual address aligned to `alignment`, a power
    /// of two. Aligned mappings aren't meant to be torn down before the address space is freed:
    /// their addresses aren't reused.
    pub(crate) fn map_vram_aligned(
        &mut self,
        mm: &mut GpuMm<'_>,
        range: Range<VramAddress>,
        alignment: u64,
        access: Access,
    ) -> Result<MappedRange> {
        self.map_aligned(
            mm,
            range.start.into_raw()..range.end.into_raw(),
            alignment,
            access,
            Memory::Vram,
        )
    }

    /// Like [`AddressSpace::map_vram_aligned`], for the page-aligned range `dma` of DMA addresses
    /// of coherent system memory.
    pub(crate) fn map_sysmem_aligned(
        &mut self,
        mm: &mut GpuMm<'_>,
        dma: Range<u64>,
        alignment: u64,
        access: Access,
    ) -> Result<MappedRange> {
        self.map_aligned(mm, dma, alignment, access, Memory::SystemCoherent)
    }

    /// Maps the page-aligned `range` of addresses of `memory` at the lowest free virtual address
    /// aligned to `alignment`, so that the addresses of freed mappings are used again: a channel
    /// replacing another maps its buffers where the other had them.
    fn map_aligned(
        &mut self,
        mm: &mut GpuMm<'_>,
        range: Range<u64>,
        alignment: u64,
        access: Access,
        memory: Memory,
    ) -> Result<MappedRange> {
        if !alignment.is_power_of_two() {
            return Err(EINVAL);
        }
        let pfns = Self::pfns(range.clone())?;
        let mut start = self
            .aligned_start
            .checked_next_multiple_of(alignment)
            .ok_or(ENOSPC)?;
        loop {
            let end = start
                .checked_add(range.end - range.start)
                .filter(|&end| end <= VA_SIZE)
                .ok_or(ENOSPC)?;
            match self
                .vmm
                .map_memory(mm, &pfns, Some(start..end), access, memory)
            {
                // Mapped there already: the next aligned address.
                Err(err) if err == EEXIST => start = start.checked_add(alignment).ok_or(ENOSPC)?,
                result => return result,
            }
        }
    }

    /// Returns the frames of the page-aligned address `range`.
    fn pfns(range: Range<u64>) -> Result<KVec<Pfn>> {
        let (start, end) = (range.start, range.end);
        let page_size = u64::try_from(PAGE_SIZE)?;
        if start % page_size != 0 || end % page_size != 0 || start >= end {
            return Err(EINVAL);
        }

        let mut pfns = KVec::new();
        for pfn in (start / page_size)..(end / page_size) {
            pfns.push(Pfn::new(pfn), GFP_KERNEL)?;
        }

        Ok(pfns)
    }

    /// Tears down a mapping made by [`AddressSpace::map_vram`].
    pub(crate) fn unmap(&mut self, mm: &mut GpuMm<'_>, mapped: MappedRange) -> Result {
        self.vmm.unmap_pages(mm, mapped)
    }

    /// Maps the pages `pfns` of `memory`, in order, with `access` at the page-aligned virtual
    /// address `start`, replacing what the client mapped there.
    ///
    /// Fails with `EINVAL` unless the address space is a client's, and the pages are outside its
    /// kernel range.
    pub(crate) fn map_at(
        &mut self,
        mm: &mut GpuMm<'_>,
        start: u64,
        pfns: &[Pfn],
        memory: Memory,
        access: Access,
    ) -> Result {
        self.check_client_range(start, pfns.len())?;

        self.vmm
            .map_at(mm, VirtualAddress::from_raw(start), pfns, access, memory)
    }

    /// Tears down the `num_pages` pages the client mapped from the page-aligned virtual address
    /// `start`.
    pub(crate) fn unmap_at(&mut self, mm: &mut GpuMm<'_>, start: u64, num_pages: usize) -> Result {
        self.check_client_range(start, num_pages)?;

        self.vmm
            .unmap_at(mm, VirtualAddress::from_raw(start), num_pages)
    }

    /// Checks that the `num_pages` pages from `start` are the client's to map.
    fn check_client_range(&self, start: u64, num_pages: usize) -> Result {
        let VaLayout::Client(kernel) = &self.layout else {
            return Err(EINVAL);
        };
        let page_size = num::usize_as_u64(PAGE_SIZE);
        let end = num::usize_as_u64(num_pages)
            .checked_mul(page_size)
            .and_then(|size| start.checked_add(size))
            .ok_or(EINVAL)?;

        if num_pages == 0
            || start % page_size != 0
            || end > VA_SIZE
            || (start < kernel.end && kernel.start < end)
        {
            return Err(EINVAL);
        }

        Ok(())
    }

    /// Releases the address space from GSP-RM. Its page tables are freed when it is dropped, so
    /// no channel may use it anymore.
    pub(crate) fn free(&self, dev: &device::Device, cmdq: &Cmdq<'_>) -> Result {
        if self.mode == Mode::Split {
            return Ok(RmFree::new(self.client, self.handle).send(dev, cmdq)?);
        }

        let unset = RmControl::new(
            self.client,
            self.device,
            DmaUnsetPageDirectory::new(self.handle),
        )
        .send(dev, cmdq);
        let freed = RmFree::new(self.client, self.handle).send(dev, cmdq);

        unset.and(freed).map_err(Error::from)
    }
}

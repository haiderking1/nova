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
    sizes::SZ_4K, //
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
            RmFree, //
        },
    },
    mm::{
        pagetable::MmuVersion,
        vmm::{
            MappedRange,
            Vmm, //
        },
        ContiguousVram,
        GpuMm,
        Pfn,
        VramAddress,
        PAGE_SIZE, //
    },
};

/// Size of the address spaces of channels: plenty for kernel channels.
const VA_SIZE: u64 = 1 << 40;
/// Start of the addresses mapped: those below are left unmapped, so that null pointers fault.
const VA_START: u64 = 1 << 16;

/// A GPU virtual address space whose page tables Nova owns.
///
/// It must be released with [`AddressSpace::free`] while the GSP is running.
pub(crate) struct AddressSpace {
    client: Handle<Client>,
    device: Handle<Device>,
    handle: Handle<VaSpace>,
    /// The root page directory.
    _pdb: ContiguousVram,
    vmm: Vmm,
}

impl AddressSpace {
    /// Allocates an empty address space named `handle` under `device` of `client`.
    pub(crate) fn new(
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        mm: &mut GpuMm<'_>,
        mmu_version: MmuVersion,
        client: Handle<Client>,
        device: Handle<Device>,
        handle: u32,
    ) -> Result<Self> {
        // An empty root page directory: no mappings.
        let pdb = mm.alloc_contiguous(SZ_4K, Alignment::new::<SZ_4K>())?;
        mm.clear_vram(pdb.range())?;
        let pdb_addr = pdb.range().start;
        let vmm = Vmm::new(pdb_addr, mmu_version, VA_SIZE)?;
        vmm.reserve(0..VA_START)?;

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

        Ok(Self {
            client,
            device,
            handle,
            _pdb: pdb,
            vmm,
        })
    }

    /// Returns the RM handle of the address space.
    pub(crate) fn handle(&self) -> Handle<VaSpace> {
        self.handle
    }

    /// Maps the page-aligned VRAM `range` writable at a free virtual address, returned by
    /// [`MappedRange::start`]. The mapping must be given back to [`AddressSpace::unmap`].
    pub(crate) fn map_vram(
        &mut self,
        mm: &mut GpuMm<'_>,
        range: Range<VramAddress>,
    ) -> Result<MappedRange> {
        let (start, end) = (range.start.into_raw(), range.end.into_raw());
        let page_size = u64::try_from(PAGE_SIZE)?;
        if start % page_size != 0 || end % page_size != 0 || start >= end {
            return Err(EINVAL);
        }

        let mut pfns = KVec::new();
        for pfn in (start / page_size)..(end / page_size) {
            pfns.push(Pfn::new(pfn), GFP_KERNEL)?;
        }

        self.vmm.map_pages(mm, &pfns, None, true)
    }

    /// Tears down a mapping made by [`AddressSpace::map_vram`].
    pub(crate) fn unmap(&mut self, mm: &mut GpuMm<'_>, mapped: MappedRange) -> Result {
        self.vmm.unmap_pages(mm, mapped)
    }

    /// Releases the address space from GSP-RM. Its page tables are freed when it is dropped, so
    /// no channel may use it anymore.
    pub(crate) fn free(&self, dev: &device::Device, cmdq: &Cmdq<'_>) -> Result {
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

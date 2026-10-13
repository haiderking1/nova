// SPDX-License-Identifier: GPL-2.0

//! GPU virtual address spaces of nova-core's clients, such as the userspace drivers nova-drm
//! serves.
//!
//! A client maps its buffers itself, at the addresses it chooses, but in the kernel range it
//! gives when creating the address space: there, Nova maps the buffers it needs for the client,
//! such as those of the client's channels.

use core::ops::Range;

use kernel::{
    device,
    prelude::*, //
};

use crate::{
    buffer::{
        SystemBuffer,
        VramBuffer, //
    },
    channel::vaspace::{
        AddressSpace,
        Mode,
        VaLayout, //
    },
    gpu::Chipset,
    gsp::{
        cmdq::Cmdq,
        rm::device::RmDevice, //
    },
    mm::{
        pagetable::{
            kind,
            Access, //
        },
        vmm::Memory,
        GpuMm,
        MmAccess,
        PAGE_SIZE, //
    },
    num,
};

pub use crate::channel::vaspace::VA_SIZE;

/// Handle of the address space of a client, under the RM client of its own.
const VA_SPACE_HANDLE: u32 = 0x90f1_0000;

/// Number of address spaces clients may have at once: the IDs giving their RM clients.
pub(crate) const MAX_VMS: usize = 0x8000;

/// A buffer of a client, to map into a [`ClientVm`].
#[derive(Clone, Copy)]
pub enum BufferRef<'a> {
    /// A buffer in VRAM.
    Vram(&'a VramBuffer),
    /// A buffer in system memory.
    System(&'a SystemBuffer),
}

/// A GPU virtual address space of a client.
///
/// It has an RM client of its own, under whose device are the address space and the channels
/// running in it (see [`crate::gsp::rm::device`]).
///
/// It must be given back to [`NovaCoreApi::free_vm`](crate::api::NovaCoreApi::free_vm) while
/// nova-core is bound.
pub struct ClientVm {
    space: AddressSpace,
    rm: RmDevice,
    /// The ID giving the RM client.
    id: usize,
}

impl ClientVm {
    /// Allocates an empty address space whose kernel range is `kernel`, with the ID `id`, under
    /// an RM client of its own.
    pub(crate) fn new(
        dev: &device::Device,
        cmdq: &Cmdq<'_>,
        mm: &mut impl MmAccess,
        chipset: Chipset,
        kernel: Range<u64>,
        id: usize,
    ) -> Result<Self> {
        // Index 0 is Nova's own client.
        let rm = RmDevice::new(dev, cmdq, u32::try_from(id)?.checked_add(1).ok_or(EINVAL)?)?;
        let space = AddressSpace::new(
            dev,
            cmdq,
            mm,
            chipset.mmu_version(),
            rm.client(),
            rm.device(),
            VA_SPACE_HANDLE,
            Mode::ClientOwned,
            VaLayout::Client(kernel),
        );

        match space {
            Ok(space) => Ok(Self { space, rm, id }),
            Err(err) => {
                let _ = rm.free(dev, cmdq);
                Err(err)
            }
        }
    }

    /// Returns the ID giving the RM client.
    pub(crate) fn id(&self) -> usize {
        self.id
    }

    /// Returns the RM client of the address space, under whose device the channels running in
    /// it are, and the address space, in whose kernel range they map their memory.
    pub(crate) fn rm_and_space_mut(&mut self) -> (&RmDevice, &mut AddressSpace) {
        (&self.rm, &mut self.space)
    }

    /// Returns the address space, in whose kernel range the channels of the client map their
    /// memory.
    pub(crate) fn space_mut(&mut self) -> &mut AddressSpace {
        &mut self.space
    }

    /// Maps the `size` bytes of `buffer` from `offset` at the virtual address `addr`, as memory of
    /// the PTE kind `kind`, replacing what was mapped there. `addr`, `offset` and `size` must be
    /// page aligned, and the addresses outside the kernel range.
    ///
    /// Compressible kinds are mapped as their uncompressed equivalent. Fails with `EINVAL` if
    /// `kind` isn't a kind.
    pub(crate) fn map(
        &mut self,
        mm: &mut GpuMm<'_>,
        addr: u64,
        buffer: BufferRef<'_>,
        offset: u64,
        size: u64,
        kind: u8,
    ) -> Result {
        let kind = kind::uncompressed(kind).ok_or(EINVAL)?;
        let page_size = num::usize_as_u64(PAGE_SIZE);
        let buffer_size = num::usize_as_u64(match buffer {
            BufferRef::Vram(buffer) => buffer.size(),
            BufferRef::System(buffer) => buffer.size(),
        });
        let end = offset.checked_add(size).ok_or(EINVAL)?;
        if size == 0 || offset % page_size != 0 || size % page_size != 0 || end > buffer_size {
            return Err(EINVAL);
        }

        let (runs, memory) = match buffer {
            BufferRef::Vram(buffer) => (buffer.runs()?, Memory::Vram),
            BufferRef::System(buffer) => (buffer.dma_runs()?, Memory::SystemCoherent),
        };
        let pages = usize::try_from(offset / page_size)?..usize::try_from(end / page_size)?;
        let access = Access {
            writable: true,
            privileged: false,
            kind,
        };

        self.space.map_at(
            mm,
            addr,
            runs.pages().sub(pages).ok_or(EINVAL)?,
            memory,
            access,
        )
    }

    /// Tears down the `size` bytes mapped from the virtual address `addr`, which must be page
    /// aligned and outside the kernel range.
    pub(crate) fn unmap(&mut self, mm: &mut GpuMm<'_>, addr: u64, size: u64) -> Result {
        let page_size = num::usize_as_u64(PAGE_SIZE);
        if size % page_size != 0 {
            return Err(EINVAL);
        }

        self.space
            .unmap_at(mm, addr, usize::try_from(size / page_size)?)
    }

    /// Releases the address space from GSP-RM, then its RM client, and frees its page tables.
    pub(crate) fn free(self, dev: &device::Device, cmdq: &Cmdq<'_>) -> Result {
        let Self { space, rm, .. } = self;
        let released = space.free(dev, cmdq);
        let freed = rm.free(dev, cmdq);

        released.and(freed)
    }
}

// SPDX-License-Identifier: GPL-2.0

//! Buffers of nova-core's clients, such as the userspace drivers nova-drm serves.
//!
//! A buffer is in VRAM or in system memory. The GPU accesses it through the address spaces it is
//! mapped into. The CPU accesses a VRAM buffer through a window into it in BAR1, if the buffer was
//! given one, and a system memory buffer directly: its pages are mapped into the kernel's address
//! space, and the GPU accesses them coherently with the CPU's caches.
//!
//! New buffers are zeroed, so that no client sees what another one left in their memory: VRAM by
//! the copy engine, system memory as it is allocated.

use core::ptr::NonNull;

use kernel::{
    alloc::{
        allocator::VmallocPageIter,
        flags::__GFP_ZERO, //
    },
    device,
    dma::DataDirection,
    gpu::buddy::AllocatedBlocks,
    io::PhysAddr,
    mm::virt::VmaNew,
    prelude::*,
    scatterlist::{
        Owned,
        SGTable, //
    },
    sizes::SZ_64K,
};

use crate::{
    mm::{
        vmm::MappedRange,
        Pfn,
        VramAddress,
        PAGE_SIZE, //
    },
    num::{
        self,
        IntoSafeCast, //
    },
};

/// Alignment and size granularity of VRAM buffers: the size of the GPU's large pages, like the
/// VRAM buffers of nouveau.
pub const VRAM_BUFFER_ALIGN: usize = SZ_64K;

/// The window of a VRAM buffer in BAR1, through which the CPU accesses it.
struct BarWindow {
    /// The mapping of the buffer into BAR1, which must be given back to the BAR1 address space.
    mapped: MappedRange,
    /// Physical address of the window.
    phys: PhysAddr,
}

/// A buffer in VRAM.
///
/// It must be given back to [`NovaCoreApi::free_vram`](crate::api::NovaCoreApi::free_vram)
/// while nova-core is bound, to tear down its window in BAR1.
pub struct VramBuffer {
    /// The VRAM of the buffer: blocks of the buddy allocator, not necessarily contiguous.
    blocks: Pin<KBox<AllocatedBlocks>>,
    /// Size in bytes.
    size: usize,
    /// The window of the buffer in BAR1, if it has one.
    window: Option<BarWindow>,
}

impl VramBuffer {
    /// Creates a buffer of the `size` bytes of VRAM of `blocks`.
    pub(crate) fn new(blocks: Pin<KBox<AllocatedBlocks>>, size: usize) -> Self {
        Self {
            blocks,
            size,
            window: None,
        }
    }

    /// Returns the size of the buffer in bytes.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Returns whether the CPU can access the buffer, through a window in BAR1.
    pub fn cpu_visible(&self) -> bool {
        self.window.is_some()
    }

    /// Returns the VRAM address the buffer starts at, if it is contiguous: its blocks follow each
    /// other.
    pub(crate) fn contiguous_start(&self) -> Option<VramAddress> {
        let mut blocks = self.blocks.iter();
        let first = blocks.next()?;
        let start = first.offset();
        let needed = start.checked_add(num::usize_as_u64(self.size))?;

        let mut end = start + first.size();
        for block in blocks {
            if end >= needed {
                break;
            }
            if block.offset() != end {
                return None;
            }
            end += block.size();
        }

        (end >= needed).then(|| VramAddress::from_raw(start))
    }

    /// Returns the frame numbers of the VRAM pages of the buffer, in order.
    pub(crate) fn pfns(&self) -> Result<KVVec<Pfn>> {
        let page_size: u64 = PAGE_SIZE.into_safe_cast();
        let mut pfns = KVVec::with_capacity(self.size / PAGE_SIZE, GFP_KERNEL)?;
        for block in self.blocks.iter() {
            let start = block.offset() / page_size;
            for pfn in start..start + block.size() / page_size {
                pfns.push(Pfn::new(pfn), GFP_KERNEL)?;
            }
        }
        // Blocks may be larger than asked for: only the buffer's pages are its.
        pfns.truncate(self.size / PAGE_SIZE);

        Ok(pfns)
    }

    /// Gives the buffer the window `mapped` in BAR1, at physical address `phys`.
    pub(crate) fn set_window(&mut self, mapped: MappedRange, phys: PhysAddr) {
        self.window = Some(BarWindow { mapped, phys });
    }

    /// Returns the mapping of the window of the buffer in BAR1, if it has one.
    pub(crate) fn window(&self) -> Option<&MappedRange> {
        self.window.as_ref().map(|window| &window.mapped)
    }

    /// Takes the window of the buffer in BAR1, if it has one.
    pub(crate) fn take_window(&mut self) -> Option<MappedRange> {
        self.window.take().map(|window| window.mapped)
    }

    /// Maps the buffer into `vma`, which spans at most its size, write-combined through its
    /// window in BAR1.
    ///
    /// Fails with `EINVAL` if the buffer has no window.
    pub fn mmap(&self, vma: &VmaNew) -> Result {
        let window = self.window.as_ref().ok_or(EINVAL)?;
        let size = vma.end() - vma.start();
        if size > self.size {
            return Err(EINVAL);
        }

        vma.io_remap_writecombine(0, window.phys, size)
    }
}

impl Drop for VramBuffer {
    fn drop(&mut self) {
        // A buffer whose window is left was not given back while nova-core was bound: BAR1's
        // address space went away with nova-core, and the window with it.
        if let Some(window) = self.window.take() {
            window.mapped.forget();
        }
    }
}

/// A buffer in system memory, which the GPU accesses coherently with the CPU's caches.
pub struct SystemBuffer {
    /// The pages of the buffer, mapped for the GPU's DMA.
    sgt: Pin<KBox<SGTable<Owned<VVec<u8>>>>>,
    /// The kernel's mapping of the pages, owned by `sgt`.
    vaddr: NonNull<u8>,
    /// Size in bytes, a multiple of the page size.
    size: usize,
}

// SAFETY: `vaddr` points to memory `sgt` owns, which can be accessed from any thread.
unsafe impl Send for SystemBuffer {}

// SAFETY: `SystemBuffer` doesn't access the memory `vaddr` points to through `&self`.
unsafe impl Sync for SystemBuffer {}

impl SystemBuffer {
    /// Allocates a zeroed buffer of `size` bytes, a multiple of the page size, mapped for the DMA
    /// of `dev`.
    pub(crate) fn new(dev: &device::Device<device::Bound>, size: usize) -> Result<Self> {
        if size == 0 || size % PAGE_SIZE != 0 {
            return Err(EINVAL);
        }

        let pages = VVec::<u8>::with_capacity(size, GFP_KERNEL | __GFP_ZERO)?;
        // The allocation doesn't move with the vector.
        let vaddr = NonNull::new(pages.as_ptr().cast_mut()).ok_or(ENOMEM)?;
        let sgt = KBox::pin_init(
            SGTable::new(dev, pages, DataDirection::Bidirectional, GFP_KERNEL),
            GFP_KERNEL,
        )?;

        Ok(Self { sgt, vaddr, size })
    }

    /// Returns the size of the buffer in bytes.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Returns the kernel's mapping of the buffer, valid as long as it is.
    pub fn vaddr(&self) -> NonNull<u8> {
        self.vaddr
    }

    /// Returns the frame numbers of the pages of the buffer, as the GPU addresses them through
    /// DMA, in order.
    pub(crate) fn dma_pfns(&self) -> Result<KVVec<Pfn>> {
        let page_size: u64 = PAGE_SIZE.into_safe_cast();
        let mut pfns = KVVec::with_capacity(self.size / PAGE_SIZE, GFP_KERNEL)?;
        for entry in self.sgt.iter() {
            let start = entry.dma_address() / page_size;
            for pfn in start..start + u64::from(entry.dma_len()) / page_size {
                pfns.push(Pfn::new(pfn), GFP_KERNEL)?;
            }
        }
        if pfns.len() != self.size / PAGE_SIZE {
            return Err(EINVAL);
        }

        Ok(pfns)
    }

    /// Maps the buffer into `vma`, which spans at most its size, cached like the kernel's
    /// mapping.
    pub fn mmap(&self, vma: &VmaNew) -> Result {
        let size = vma.end() - vma.start();
        if size > self.size {
            return Err(EINVAL);
        }
        let vma = vma.set_mixedmap()?;

        // SAFETY: `vaddr` is the start of the vmalloc allocation of `size` bytes `sgt` owns, which
        // outlives the iterator.
        let pages = unsafe { VmallocPageIter::new(self.vaddr, self.size) };
        for (index, page) in pages.take(size / PAGE_SIZE).enumerate() {
            vma.vm_insert_page(vma.start() + index * PAGE_SIZE, &page)?;
        }

        Ok(())
    }
}

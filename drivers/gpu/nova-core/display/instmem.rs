// SPDX-License-Identifier: GPL-2.0

//! The display engine's instance memory.
//!
//! The display engine finds the memory its channels access through context DMAs: objects in its
//! instance memory describing a memory range, found by handle through a hash table at the start
//! of the instance memory (`NV_UDISP_HASH_*`, `NV_UDISP_OBJ_MEM_*` and `NV_DMA_*` in
//! `dev_disp.h`). Nova writes both, as NVIDIA's CPU-side RM does.

use core::ops::Range;

use kernel::{
    prelude::*,
    ptr::Alignment,
    sizes::SZ_64K, //
};

use crate::{
    gsp::commands::{
        Client,
        Handle, //
    },
    mm::{
        ContiguousVram,
        GpuMm,
        VramAddress, //
    },
    num, //
};

use super::channel::DmaChannel;

/// Size of the instance memory.
const SIZE: usize = SZ_64K;
/// Size of the hash table at the start of the instance memory.
const HASH_TABLE_SIZE: u64 = 0x2000;
/// Size of a hash table entry: the handle of a context DMA, and where and for whom it is.
const HASH_ENTRY_SIZE: u64 = 8;
/// Number of hash table entries.
const HASH_ENTRIES: usize = num::u64_as_usize(HASH_TABLE_SIZE / HASH_ENTRY_SIZE);
/// Room taken by a context DMA object (20 bytes, aligned to 32).
const CTXDMA_SIZE: u64 = 32;

/// `NV_DMA_TARGET_NODE_PHYSICAL_NVM`: the context DMA points to VRAM.
const CTXDMA_TARGET_VRAM: u32 = 1;
/// `NV_DMA_TARGET_NODE_PHYSICAL_PCI_COHERENT`: the context DMA points to coherent system memory.
const CTXDMA_TARGET_PCI_COHERENT: u32 = 3;
/// `NV_DMA_ACCESS_READ_AND_WRITE`.
const CTXDMA_ACCESS_READ_AND_WRITE: u32 = 1 << 2;

/// The display engine's instance memory, and the context DMAs bound in it.
pub(super) struct InstMem {
    vram: ContiguousVram,
    /// Offset of the next free context DMA object.
    next_object: u64,
    /// Which hash table entries are in use.
    used: KVec<bool>,
}

impl InstMem {
    /// Allocates empty instance memory.
    pub(super) fn new(mm: &mut GpuMm<'_>) -> Result<Self> {
        let vram = mm.alloc_contiguous(SIZE, Alignment::new::<SZ_64K>())?;
        // An empty hash table, and no objects.
        mm.clear_vram(vram.range())?;

        let mut used = KVec::with_capacity(HASH_ENTRIES, GFP_KERNEL)?;
        used.resize(HASH_ENTRIES, false, GFP_KERNEL)?;

        Ok(Self {
            vram,
            next_object: HASH_TABLE_SIZE,
            used,
        })
    }

    /// Returns the VRAM range of the instance memory.
    pub(super) fn range(&self) -> Range<VramAddress> {
        self.vram.range()
    }

    /// Binds a context DMA named `handle` to `channel`, giving it read and write access to the
    /// pitch-linear VRAM in `range`, which must be 256-byte aligned.
    ///
    /// `client` is the RM client the channel was allocated under.
    pub(super) fn bind_vram(
        &mut self,
        mm: &mut GpuMm<'_>,
        client: Handle<Client>,
        channel: DmaChannel,
        handle: u32,
        range: Range<VramAddress>,
    ) -> Result {
        let range = range.start.into_raw()..range.end.into_raw();

        self.bind(mm, client, channel, handle, CTXDMA_TARGET_VRAM, range)
    }

    /// Binds a context DMA named `handle` to `channel`, giving it read and write access to the
    /// coherent system memory at the DMA addresses in `range`, which must be 256-byte aligned.
    ///
    /// `client` is the RM client the channel was allocated under.
    pub(super) fn bind_coherent(
        &mut self,
        mm: &mut GpuMm<'_>,
        client: Handle<Client>,
        channel: DmaChannel,
        handle: u32,
        range: Range<u64>,
    ) -> Result {
        self.bind(
            mm,
            client,
            channel,
            handle,
            CTXDMA_TARGET_PCI_COHERENT,
            range,
        )
    }

    /// Binds a context DMA named `handle` to `channel`, giving it read and write access to the
    /// pitch-linear memory in `range` of `target`.
    fn bind(
        &mut self,
        mm: &mut GpuMm<'_>,
        client: Handle<Client>,
        channel: DmaChannel,
        handle: u32,
        target: u32,
        range: Range<u64>,
    ) -> Result {
        let (start, end) = (range.start, range.end);
        if start % 256 != 0 || end % 256 != 0 || start >= end {
            return Err(EINVAL);
        }

        let object = self.next_object;
        if object + CTXDMA_SIZE > num::usize_as_u64(SIZE) {
            return Err(ENOSPC);
        }
        let entry = self.free_entry(client, channel, handle)?;

        // The object: where the memory is and how it may be accessed, then its first and last
        // 256-byte blocks (`NV_DMA_*`).
        let base = self.vram.range().start + object;
        let first = start >> 8;
        let last = (end - 1) >> 8;
        // CAST: the low and high halves of block numbers are stored separately.
        let words = [
            target | CTXDMA_ACCESS_READ_AND_WRITE,
            first as u32,
            (first >> 32) as u32,
            last as u32,
            (last >> 32) as u32,
        ];
        mm.write_vram(base, &words)?;

        // The hash table entry: the handle, then the RM client (bits 13:0), the object in 32-byte
        // units (bits 24:14) and the channel (bits 31:25) (`NV_UDISP_HASH_TBL_*`).
        let context = (client.as_raw() & 0x3fff)
            | (u32::try_from(object / CTXDMA_SIZE)? << 14)
            | (channel.number() << 25);
        let entry_addr = self.vram.range().start + num::usize_as_u64(entry) * HASH_ENTRY_SIZE;
        mm.write_vram(entry_addr, &[handle, context])?;

        self.used[entry] = true;
        self.next_object += CTXDMA_SIZE;
        Ok(())
    }

    /// Returns the hash table entry for `handle` bound to `channel` under `client`: the first free
    /// one from where the hash function points, as the display engine looks them up.
    fn free_entry(
        &self,
        client: Handle<Client>,
        channel: DmaChannel,
        handle: u32,
    ) -> Result<usize> {
        let start = num::u32_as_usize(hash(client.as_raw(), handle, channel.number()));

        (start..start + HASH_ENTRIES)
            .map(|i| i % HASH_ENTRIES)
            .find(|&i| !self.used[i])
            .ok_or(ENOSPC)
    }
}

/// Hash function of the display engine's hash table (`instmemHashFunc_v03_00` in RM).
fn hash(client: u32, handle: u32, channel: u32) -> u32 {
    (handle & 0x3ff)
        ^ ((handle >> 10) & 0x3ff)
        ^ ((handle >> 20) & 0x3ff)
        ^ (((client & 0xff) << 2) | (handle >> 30))
        ^ (((channel & 0xf) << 6) | ((client >> 8) & 0x3f))
        ^ ((channel >> 4) & 0x7)
}

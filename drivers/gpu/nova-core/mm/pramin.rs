// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

//! Utilities for accessing VRAM through the PRAMIN window, or, for VRAM mapped write-combined in
//! BAR1, through BAR1.

use core::ops::Range;

use kernel::{
    io::{
        io_project,
        register,
        register::OffsetLoc,
        Io,
        IoBase,
        Mmio, //
    },
    prelude::*,
    ptr::{
        Alignable,
        Alignment, //
    },
    sizes::{
        SZ_1M,
        SZ_64K, //
    },
    sync::barrier::{
        mb,
        Write, //
    },
    transmute::AsBytes,
};

use crate::{
    driver::{
        Bar0,
        Bar1,
        NovaRegisters, //
    },
    gpu::Chipset,
    mm::{
        hal::{
            self,
            MmHal, //
        },
        VramAddress, //
    },
    num::IntoSafeCast, //
};

/// Size of the PRAMIN window (1 MiB).
const WINDOW_SIZE: usize = SZ_1M;

/// The PRAMIN window, which is a 1 MiB window into VRAM at a fixed BAR0 offset.
#[derive(FromBytes, IntoBytes)]
struct PraminWindow([u8; WINDOW_SIZE]);

register! {
    base: NovaRegisters;

    /// Location of the window inside BAR0.
    PRAMIN: PraminWindow @ 0x700000;
}

/// VRAM mapped in BAR1, which the CPU accesses through BAR1 rather than through the PRAMIN window.
///
/// BAR1 is write-combined: writes through it stream to the GPU, where writes through the PRAMIN
/// window, uncached, go one by one, each of them costing about a microsecond.
pub(super) struct WcRegion<'gpu> {
    pub(super) bar1: Bar1<'gpu>,
    /// The VRAM mapped.
    pub(super) vram: Range<VramAddress>,
    /// Offset in BAR1 of the start of `vram`, which follows it there.
    pub(super) bar1_offset: usize,
}

/// Owner of the PRAMIN window state.
///
/// [`Pramin::window_at()`] repositions the window as needed and returns a typed MMIO view into
/// it, holding the manager borrowed for the lifetime of the view. VRAM of the [`WcRegion`], if
/// any, is accessed through BAR1 instead.
pub(super) struct Pramin<'gpu> {
    bar: Bar0<'gpu>,
    hal: &'static dyn MmHal,
    /// MMIO view of the PRAMIN window in BAR0.
    window: Mmio<'gpu, PraminWindow>,
    /// VRAM range to keep the PRAMIN window inside.
    vram_range: Range<VramAddress>,
    /// Cached window position.
    window_range: Range<VramAddress>,
    /// VRAM accessed through BAR1.
    wc: Option<WcRegion<'gpu>>,
    /// Posted writes, through BAR1 and through the window, no read has ordered yet.
    pending: Pending,
}

/// Writes made without an ordering point after them: see [`Pramin::write_u64s_posted()`].
#[derive(Default)]
struct Pending {
    /// Through BAR1: buffered in the CPU, write-combined, until flushed out.
    wc: bool,
    /// Through the PRAMIN window.
    window: bool,
}

/// Typed view of VRAM through the PRAMIN window.
///
/// Inserts an ordering point after previous writes through the window on drop. Views returned
/// by [`PraminAccess::view()`] cannot outlive this access, so the ordering point covers every
/// write made through them.
pub(super) struct PraminAccess<'a, T>
where
    T: FromBytes + IntoBytes,
{
    view: Mmio<'a, T>,
    /// Whether `view` is in BAR1, write-combined.
    wc: bool,
}

impl<T> PraminAccess<'_, T>
where
    T: FromBytes + IntoBytes,
{
    /// Returns the MMIO view of the accessed location.
    pub(super) fn view(&self) -> Mmio<'_, T> {
        self.view
    }
}

impl<T> Drop for PraminAccess<'_, T>
where
    T: FromBytes + IntoBytes,
{
    fn drop(&mut self) {
        // Write-combined writes are buffered in the CPU until flushed.
        if self.wc {
            mb(Write);
        }
        // Insert an ordering point after previous writes through this window.
        self.view.cast::<u8>().read_val();
    }
}

impl<'gpu> Pramin<'gpu> {
    /// Alignment required by the PRAMIN window.
    const BASE_ALIGN: Alignment = Alignment::new::<SZ_64K>();

    /// Creates the window manager for the given VRAM region.
    pub(super) fn new(
        bar: Bar0<'gpu>,
        chipset: Chipset,
        vram_range: Range<VramAddress>,
    ) -> Result<Self> {
        let hal = hal::mm_hal(chipset);
        let window = io_project!(bar, build: PRAMIN);
        let base = vram_range.start.align_down(Self::BASE_ALIGN);
        let window_range = Self::window_range(base)?;
        hal.write_pramin_window_base(bar, base)?;

        Ok(Self {
            bar,
            hal,
            window,
            vram_range,
            window_range,
            wc: None,
            pending: Pending::default(),
        })
    }

    /// Has the VRAM of `wc` accessed through BAR1 from now on, or, with `None`, none, returning
    /// the region accessed so before.
    pub(super) fn set_wc_region(&mut self, wc: Option<WcRegion<'gpu>>) -> Option<WcRegion<'gpu>> {
        // Writes through the region given back are ordered while it is still accessed, which
        // can't fail: its BAR1 mapping holds its own start.
        if self.order().is_err() {
            kernel::pr_warn_once!("PRAMIN: ordering the writes through BAR1 failed\n");
        }
        core::mem::replace(&mut self.wc, wc)
    }

    /// Returns the BAR1 offset of the `len` bytes at `addr`, if they are in the [`WcRegion`].
    fn wc_offset(&self, addr: VramAddress, len: usize) -> Option<usize> {
        let wc = self.wc.as_ref()?;
        let end = addr.checked_add(len.into_safe_cast())?;
        if !(wc.vram.contains(&addr) && end <= wc.vram.end) {
            return None;
        }
        let offset: usize = (addr - wc.vram.start).into_safe_cast();
        wc.bar1_offset.checked_add(offset)
    }

    /// Returns the VRAM range a window based at `base` exposes.
    fn window_range(base: VramAddress) -> Result<Range<VramAddress>> {
        let end = base
            .checked_add(WINDOW_SIZE.into_safe_cast())
            .ok_or(EINVAL)?;
        Ok(base..end)
    }

    /// Check the window covers `len` bytes at `addr`, moving it if needed.
    ///
    /// Returns the window offset at which to perform the access.
    fn window_offset(&mut self, addr: VramAddress, len: usize) -> Result<usize> {
        let end = addr.checked_add(len.into_safe_cast()).ok_or(EINVAL)?;

        let inside = |r: &Range<VramAddress>| r.contains(&addr) && end <= r.end;
        if !inside(&self.vram_range) {
            return Err(EINVAL);
        }

        // Reposition the window if the access falls outside it.
        if !inside(&self.window_range) {
            let base = addr.align_down(Self::BASE_ALIGN);
            let window_range = Self::window_range(base)?;
            if !inside(&window_range) {
                return Err(EINVAL);
            }
            self.hal.write_pramin_window_base(self.bar, base)?;
            self.window_range = window_range;
        }

        Ok((addr - self.window_range.start).into_safe_cast())
    }

    /// Writes `values` into the consecutive 64-bit words of VRAM from `vram_addr`, 8-byte aligned,
    /// with a single ordering point after the last: reading back through the window is uncached,
    /// and costs far more than the writes.
    pub(super) fn write_u64s(&mut self, vram_addr: VramAddress, values: &[u64]) -> Result {
        self.write_u64s_posted(vram_addr, values)?;
        self.order()
    }

    /// Writes `values` as [`Pramin::write_u64s()`] does, without an ordering point: they may still
    /// be on their way to VRAM until [`Pramin::order()`], or a read through this manager.
    pub(super) fn write_u64s_posted(&mut self, vram_addr: VramAddress, values: &[u64]) -> Result {
        if values.is_empty() {
            return Ok(());
        }
        let bytes = AsBytes::as_bytes(values);
        if let (Some(offset), Some(wc)) = (self.wc_offset(vram_addr, bytes.len()), &self.wc) {
            let end = offset.checked_add(bytes.len()).ok_or(EINVAL)?;
            let bar1 = wc.bar1.as_view().as_bytes();
            io_project!(bar1, [try: offset..end]).copy_from_slice(bytes);
            self.pending.wc = true;
            return Ok(());
        }

        let mut addr = vram_addr;
        for &value in values {
            let offset = self.window_offset(addr, size_of::<u64>())?;
            let view: Mmio<'_, u64> = io_project!(self.window, try: OffsetLoc::new(offset));
            view.write_val(value);
            addr = addr
                .checked_add(size_of::<u64>().into_safe_cast())
                .ok_or(EINVAL)?;
        }
        self.pending.window = true;
        Ok(())
    }

    /// Makes the writes made since the last ordering point reach VRAM before what follows, such
    /// as a TLB invalidation: write-combined ones are flushed out of the CPU, then a read through
    /// each path they took waits for them (PCIe reads don't pass the writes before them).
    pub(super) fn order(&mut self) -> Result {
        self.order_wc()?;
        self.order_window();
        Ok(())
    }

    /// The write-combined part of [`Pramin::order()`].
    fn order_wc(&mut self) -> Result {
        if core::mem::take(&mut self.pending.wc) {
            if let Some(wc) = &self.wc {
                mb(Write);
                let view: Mmio<'_, u8> =
                    io_project!(wc.bar1.as_view(), try: OffsetLoc::new(wc.bar1_offset));
                view.read_val();
            }
        }
        Ok(())
    }

    /// The PRAMIN window part of [`Pramin::order()`].
    fn order_window(&mut self) {
        if core::mem::take(&mut self.pending.window) {
            self.window.cast::<u8>().read_val();
        }
    }

    /// Reads the 64-bit word of VRAM at `vram_addr`, 8-byte aligned, after the writes made before
    /// it: without an ordering point of its own, which reads don't need.
    pub(super) fn read_u64(&mut self, vram_addr: VramAddress) -> Result<u64> {
        // The read orders the writes before it through its own path; those through the other
        // one, which may have written the word, need their ordering point.
        if let Some(offset) = self.wc_offset(vram_addr, size_of::<u64>()) {
            self.order_window();
            if core::mem::take(&mut self.pending.wc) {
                mb(Write);
            }
            let wc = self.wc.as_ref().ok_or(EINVAL)?;
            let view: Mmio<'_, u64> = io_project!(wc.bar1.as_view(), try: OffsetLoc::new(offset));
            return Ok(view.read_val());
        }
        self.order_wc()?;
        self.pending.window = false;
        let offset = self.window_offset(vram_addr, size_of::<u64>())?;
        let view: Mmio<'_, u64> = io_project!(self.window, try: OffsetLoc::new(offset));
        Ok(view.read_val())
    }

    /// Return a typed MMIO view of a `T` at `vram_addr`.
    ///
    /// Returns an error if `vram_addr` is not aligned to `T`'s alignment, or if
    /// a `T` at `vram_addr` does not fit within the VRAM region.
    pub(super) fn window_at<'a, T>(
        &'a mut self,
        vram_addr: VramAddress,
    ) -> Result<PraminAccess<'a, T>>
    where
        T: FromBytes + IntoBytes,
    {
        if let (Some(offset), Some(wc)) = (self.wc_offset(vram_addr, size_of::<T>()), &self.wc) {
            let view = io_project!(wc.bar1.as_view(), try: OffsetLoc::new(offset));
            return Ok(PraminAccess { view, wc: true });
        }

        let offset = self.window_offset(vram_addr, size_of::<T>())?;
        let view = io_project!(self.window, try: OffsetLoc::new(offset));

        Ok(PraminAccess { view, wc: false })
    }
}

#[cfg(CONFIG_NOVA_CORE_SELFTESTS)]
pub(super) mod selftest {
    use kernel::{
        device,
        io::io_read,
        sizes::SizeConstants, //
    };

    use super::*;
    use crate::{
        selftest_assert,
        selftest_assert_eq, //
    };

    /// Test read/write at byte granularity, at unaligned addresses.
    fn test_byte_readwrite(
        dev: &device::Device<device::Bound>,
        pramin: &mut Pramin<'_>,
        base: VramAddress,
    ) -> Result {
        for i in 0u8..4 {
            let addr = base + 1 + u64::from(i);
            pramin.window_at::<u8>(addr)?.view().write_val(0xA0 + i);
        }

        for i in 0u8..4 {
            let addr = base + 1 + u64::from(i);
            selftest_assert_eq!(
                dev,
                pramin.window_at::<u8>(addr)?.view().read_val(),
                0xA0 + i
            );
        }
        Ok(())
    }

    /// Test writing a `u32` and reading back as individual `u8`s.
    fn test_u32_as_bytes(
        dev: &device::Device<device::Bound>,
        pramin: &mut Pramin<'_>,
        base: VramAddress,
    ) -> Result {
        let addr = base + 0x10;
        let val: u32 = 0xDEADBEEF;
        pramin.window_at::<u32>(addr)?.view().write_val(val);

        let window = pramin.window_at::<[u8; 4]>(addr)?;
        for (i, &expected) in val.to_le_bytes().iter().enumerate() {
            selftest_assert_eq!(dev, io_read!(window.view(), [build: i]), expected);
        }
        Ok(())
    }

    /// Test window repositioning across 1 MiB boundaries.
    fn test_window_reposition(
        dev: &device::Device<device::Bound>,
        pramin: &mut Pramin<'_>,
        base: VramAddress,
    ) -> Result {
        let addr_a = base;
        let addr_b = base + u64::SZ_2M; // base + 2 MiB (different 1 MiB region).
        let val_a: u32 = 0x11111111;
        let val_b: u32 = 0x22222222;

        pramin.window_at::<u32>(addr_a)?.view().write_val(val_a);
        pramin.window_at::<u32>(addr_b)?.view().write_val(val_b);

        selftest_assert_eq!(
            dev,
            pramin.window_at::<u32>(addr_a)?.view().read_val(),
            val_a
        );
        selftest_assert_eq!(
            dev,
            pramin.window_at::<u32>(addr_b)?.view().read_val(),
            val_b
        );
        Ok(())
    }

    /// Test that a `u64` access moves both halves: nouveau and RM only ever access BAR0 32 bits
    /// at a time, so check that the window takes 64-bit accesses whole, in both directions.
    fn test_u64_halves(
        dev: &device::Device<device::Bound>,
        pramin: &mut Pramin<'_>,
        base: VramAddress,
    ) -> Result {
        let addr = base + 0x30;
        let val: u64 = 0x1122_3344_5566_7788;

        // A `u64` write, read back as two `u32`s.
        pramin.window_at::<u32>(addr)?.view().write_val(0xffff_ffff);
        pramin
            .window_at::<u32>(addr + 4)?
            .view()
            .write_val(0xffff_ffff);
        pramin.window_at::<u64>(addr)?.view().write_val(val);
        selftest_assert_eq!(
            dev,
            pramin.window_at::<u32>(addr)?.view().read_val(),
            0x5566_7788
        );
        selftest_assert_eq!(
            dev,
            pramin.window_at::<u32>(addr + 4)?.view().read_val(),
            0x1122_3344
        );

        // Two `u32` writes, read back as a `u64`.
        pramin.window_at::<u32>(addr)?.view().write_val(0x8877_6655);
        pramin
            .window_at::<u32>(addr + 4)?
            .view()
            .write_val(0x4433_2211);
        selftest_assert_eq!(
            dev,
            pramin.window_at::<u64>(addr)?.view().read_val(),
            0x4433_2211_8877_6655
        );

        // The batched writes that clear page tables, over set upper halves.
        pramin.write_u64s(addr, &[0, val])?;
        selftest_assert_eq!(dev, pramin.window_at::<u32>(addr + 4)?.view().read_val(), 0);
        selftest_assert_eq!(
            dev,
            pramin.window_at::<u32>(addr + 12)?.view().read_val(),
            0x1122_3344
        );
        Ok(())
    }

    /// Test that offsets outside the VRAM region are rejected.
    fn test_invalid_offset(
        dev: &device::Device<device::Bound>,
        pramin: &mut Pramin<'_>,
        vram_end: VramAddress,
    ) -> Result {
        selftest_assert!(dev, pramin.window_at::<u32>(vram_end).is_err());
        Ok(())
    }

    /// Test that misaligned accesses are rejected.
    fn test_misaligned_access(
        dev: &device::Device<device::Bound>,
        pramin: &mut Pramin<'_>,
        base: VramAddress,
    ) -> Result {
        // `u16` at odd offset (not 2-byte aligned).
        selftest_assert!(dev, pramin.window_at::<u16>(base + 0x21).is_err());

        // `u32` at 2-byte-aligned (not 4-byte-aligned) offset.
        selftest_assert!(dev, pramin.window_at::<u32>(base + 2).is_err());

        // `u64` at a 4-byte-aligned (not 8-byte-aligned) address.
        selftest_assert!(dev, pramin.window_at::<u64>(base + 0x44).is_err());

        // A `u16` view at an even address is allowed.
        pramin.window_at::<u16>(base + 0x22)?;
        Ok(())
    }

    /// Run PRAMIN self-tests during probe.
    ///
    /// `base` is the start of a driver-usable VRAM span that the tests are free to
    /// overwrite.
    pub(crate) fn run(
        dev: &device::Device<device::Bound>,
        pramin: &mut Pramin<'_>,
        base: VramAddress,
    ) -> Result {
        dev_dbg!(dev, "PRAMIN: starting self-tests\n");

        let vram_end = pramin.vram_range.end;

        test_byte_readwrite(dev, pramin, base)?;
        test_u32_as_bytes(dev, pramin, base)?;
        test_u64_halves(dev, pramin, base)?;
        test_window_reposition(dev, pramin, base)?;
        test_invalid_offset(dev, pramin, vram_end)?;
        test_misaligned_access(dev, pramin, base)?;

        dev_info!(dev, "PRAMIN: self-tests passed\n");
        Ok(())
    }
}

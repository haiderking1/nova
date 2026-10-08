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
    ops, //
};

use kernel::{
    bitfield,
    fmt,
    gpu::buddy::{
        GpuBuddy,
        GpuBuddyParams, //
    },
    num::Bounded,
    prelude::*,
    ptr::{
        Alignable,
        Alignment, //
    },
    sizes::SZ_4K, //
};

use crate::{
    driver::Bar0,
    gpu::Chipset, //
};

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
        })
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

    /// Checks that `mapped`, the BAR1 mapping of `display`'s framebuffer, maps each page to the
    /// right VRAM: writes a marker into every page through BAR1 and reads VRAM back through
    /// PRAMIN. Logs the pages that map elsewhere, and for the first ones where their writes land.
    pub(crate) fn check_bar1_mapping(
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
        for page in (0..num_pages).step_by(512).flat_map(|p| [p, (p + 511).min(num_pages - 1)]) {
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
        if usable_fb_regions.iter().any(|r| r.start < end && start < r.end) {
            dev_warn!(dev, "boot display: [{start:#x}, {end:#x}) is usable VRAM, not drawing\n");
            return Err(EBUSY);
        }
        if display.format.bits_per_pixel != 32 {
            dev_info!(dev, "boot display: {} bpp not supported by the test pattern\n",
                display.format.bits_per_pixel);
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

        dev_info!(dev, "boot display: drew test pattern ({}x{})\n", display.width, display.height);
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

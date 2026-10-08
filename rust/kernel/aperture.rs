// SPDX-License-Identifier: GPL-2.0

//! Ownership of graphics memory apertures.
//!
//! Firmware framebuffer drivers (e.g. `efifb`, `simpledrm`) keep the display the system firmware
//! set up running, and claim the graphics memory they scan out from. A native driver for the
//! same hardware has to take that memory over before it can use it.
//!
//! C header: [`include/linux/aperture.h`](srctree/include/linux/aperture.h)

use crate::{
    bindings,
    device::Bound,
    error::to_result,
    pci,
    prelude::*, //
};

/// Removes the firmware framebuffers and the legacy VGA device that use the memory BARs of
/// `pdev`.
///
/// Call this before reserving the BARs of `pdev`, which would otherwise fail with `EBUSY` while
/// a firmware framebuffer still owns them. The firmware console stops working.
pub fn remove_conflicting_pci_devices(pdev: &pci::Device<Bound>, name: &CStr) -> Result {
    // SAFETY: `pdev.as_raw()` is a valid `struct pci_dev` by the type invariants of
    // `pci::Device`, and `name` is a valid NUL-terminated string.
    to_result(unsafe {
        bindings::aperture_remove_conflicting_pci_devices(pdev.as_raw(), name.as_char_ptr())
    })
}

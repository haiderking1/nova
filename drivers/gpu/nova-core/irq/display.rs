// SPDX-License-Identifier: GPL-2.0

//! The display engine's interrupt.
//!
//! GSP-RM services the display engine's interrupts but those of head timing, which it leaves to
//! the CPU on the display vector it reports (`NV2080_CTRL_CMD_INTERNAL_INTR_GET_KERNEL_TABLE`).
//! The handler acknowledges the vblanks of the heads whose vblank interrupt is enabled, and passes
//! them on to the registered [`VblankHandler`](crate::display::VblankHandler), in the top half:
//! DRM handles vblanks in interrupt context.
//!
//! The display vector shares its subtree, and thus its PCI vector, with the GSP event: both
//! handlers are registered as shared, and each services its own vector only.

use kernel::{
    device,
    irq,
    prelude::*,
    sync::atomic::{
        Atomic,
        Relaxed, //
    }, //
};

use super::interrupt_tree::{
    GinVector,
    LeafEnableGuard,
    Tree, //
};
use crate::{
    display::{
        hal::DisplayHal,
        VblankHandlerSlot, //
    },
    driver::Bar0,
};

/// Hard IRQ handler for the display vector.
pub(crate) struct DisplayInterrupt<'a> {
    /// For logging.
    dev: &'a device::Device,
    bar: Bar0<'a>,
    tree: &'a Tree<'a>,
    hal: &'static dyn DisplayHal,
    vector: GinVector,
    handler: &'a VblankHandlerSlot,
    /// Whether a vblank was received yet, which is logged once.
    vblank_seen: Atomic<bool>,
}

impl irq::Handler for DisplayInterrupt<'_> {
    fn handle(&self) -> irq::IrqReturn {
        let subtree = self.vector.subtree();
        let leaf = self.tree.read_pending(self.vector.leaf_index());
        if !leaf.vectors().contains(self.vector.leaf_mask()) {
            self.tree.rearm_pci_irq(subtree);
            return irq::IrqReturn::None;
        }
        // Cleared before the heads' events, like RM: an event after those cleared latches the
        // vector again.
        leaf.clear_vectors(self.vector.leaf_mask());

        let heads = self.hal.take_vblanks(self.bar);
        if heads != 0 && !self.vblank_seen.xchg(true, Relaxed) {
            dev_info!(
                self.dev,
                "display: vblank interrupts received, heads {:#x}\n",
                heads
            );
        }
        if heads != 0 {
            if let Some(handler) = self.handler.lock().as_ref() {
                for head in (0..u32::BITS).filter(|head| heads & 1 << head != 0) {
                    handler.vblank(head);
                }
            }
        }

        self.tree.rearm_pci_irq(subtree);
        irq::IrqReturn::Handled
    }
}

/// The registered display interrupt handler and the enable of its vector.
///
/// The declaration order is the drop order, and it is required: the vector is disabled before
/// `free_irq` runs, as for the GSP event.
#[pin_data]
pub(crate) struct DisplayIrq<'a> {
    _leaf_guard: LeafEnableGuard<'a>,
    #[pin]
    reg: irq::Registration<'a, DisplayInterrupt<'a>>,
}

impl<'a> DisplayIrq<'a> {
    /// Returns an initializer that registers the handler of the display vector `vector`, which
    /// passes the vblanks of the heads to the handler in `handler`, and then enables the vector at
    /// its leaf.
    ///
    /// # Errors
    ///
    /// `EINVAL` if `vector` is not a vector of a subtree that `tree` services, right away.
    /// Otherwise, from the initializer, the error from `request_irq`.
    ///
    /// # Safety
    ///
    /// Callers must not `mem::forget()` the initialized `DisplayIrq` or otherwise prevent its
    /// [`Drop`] implementation, which runs `free_irq`, from running.
    pub(crate) unsafe fn new(
        dev: &'a device::Device,
        bar: Bar0<'a>,
        tree: &'a Tree<'a>,
        hal: &'static dyn DisplayHal,
        vector: u32,
        handler: &'a VblankHandlerSlot,
    ) -> Result<impl PinInit<Self, Error> + 'a> {
        let vector = GinVector::try_new(vector).ok_or(EINVAL)?;
        let request = tree.request_for(vector.subtree())?;

        Ok(try_pin_init!(Self {
            // SAFETY: this function's caller must not leak the `DisplayIrq` that owns this
            // registration, so the registration's `Drop` runs.
            reg <- unsafe {
                irq::Registration::new(
                    request,
                    irq::Flags::SHARED,
                    c"nova-core-display",
                    Ok(DisplayInterrupt {
                        dev,
                        bar,
                        tree,
                        hal,
                        vector,
                        handler,
                        vblank_seen: Atomic::new(false),
                    }),
                )
            },
            _leaf_guard: tree.enable_leaf_guarded(vector.leaf_index(), vector.leaf_mask()),
        }))
    }
}

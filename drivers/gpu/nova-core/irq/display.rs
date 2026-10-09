// SPDX-License-Identifier: GPL-2.0

//! The display engine's interrupt.
//!
//! GSP-RM services the display engine's interrupts but those of head timing, which it leaves to
//! the CPU on the display vector it reports (`NV2080_CTRL_CMD_INTERNAL_INTR_GET_KERNEL_TABLE`).
//! Servicing it acknowledges the vblanks of the heads whose vblank interrupt is enabled, and passes
//! them on to the registered [`VblankHandler`](crate::display::VblankHandler), in the top half:
//! DRM handles vblanks in interrupt context.
//!
//! The display vector is in the GSP event's subtree, and thus on its PCI vector: the GSP event's
//! handler services it too, so that the PCI vector is rearmed once per interrupt, once both
//! vectors are serviced. Rearming it while the display vector is still pending would deliver a
//! second interrupt right away.

use kernel::{
    device,
    prelude::*,
    sync::atomic::{
        Atomic,
        Relaxed, //
    }, //
};

use super::{
    gsp::GSP_SUBTREE,
    interrupt_tree::{
        GinVector,
        LeafEnableGuard,
        LeafPending,
        Tree, //
    },
};
use crate::{
    display::{
        hal::DisplayHal,
        VblankHandlerSlot, //
    },
    driver::Bar0,
};

/// The display vector, which the GSP event's handler services.
pub(crate) struct DisplayVector<'a> {
    /// For logging.
    dev: &'a device::Device,
    bar: Bar0<'a>,
    hal: &'static dyn DisplayHal,
    vector: GinVector,
    handler: &'a VblankHandlerSlot,
    /// Whether a vblank was received yet, which is logged once.
    vblank_seen: Atomic<bool>,
}

impl<'a> DisplayVector<'a> {
    /// Returns the display vector `vector`, whose vblanks are passed to the handler in `handler`.
    ///
    /// # Errors
    ///
    /// `ENOTSUPP` if `vector` is not in the GSP event's subtree, the one nova-core services.
    pub(crate) fn new(
        dev: &'a device::Device,
        bar: Bar0<'a>,
        hal: &'static dyn DisplayHal,
        vector: u32,
        handler: &'a VblankHandlerSlot,
    ) -> Result<Self> {
        let vector = GinVector::try_new(vector).ok_or(EINVAL)?;
        if vector.subtree() != GSP_SUBTREE {
            return Err(ENOTSUPP);
        }

        Ok(Self {
            dev,
            bar,
            hal,
            vector,
            handler,
            vblank_seen: Atomic::new(false),
        })
    }

    /// Enables the vector at its leaf, until the returned guard drops: from then on, the GSP
    /// event's handler must service it.
    pub(crate) fn enable(&self, tree: &Tree<'a>) -> LeafEnableGuard<'a> {
        tree.enable_leaf_guarded(self.vector.leaf_index(), self.vector.leaf_mask())
    }

    pub(super) fn vector(&self) -> GinVector {
        self.vector
    }

    /// Services the display vector, pending in `leaf`, without rearming the PCI vector.
    pub(super) fn service(&self, leaf: &LeafPending<'_>) {
        // Cleared before the heads' events, like RM: an event after those cleared latches the
        // vector again.
        leaf.clear_vectors(self.vector.leaf_mask());

        let heads = self.hal.take_vblanks(self.bar);
        if heads == 0 {
            return;
        }
        if !self.vblank_seen.xchg(true, Relaxed) {
            dev_info!(
                self.dev,
                "display: vblank interrupts received, heads {:#x}\n",
                heads
            );
        }
        if let Some(handler) = self.handler.lock().as_ref() {
            for head in (0..u32::BITS).filter(|head| heads & 1 << head != 0) {
                handler.vblank(head);
            }
        }
    }
}

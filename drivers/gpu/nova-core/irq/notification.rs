// SPDX-License-Identifier: GPL-2.0

//! The engines' notification interrupts.
//!
//! A channel raises the notification (non-stall) interrupt of its engine with the
//! `NON_STALL_INTERRUPT` method, after releasing the semaphore of a submission, so that the CPU
//! signals the submission's fence. GSP-RM programs each channel with the vector of its engine,
//! and reports the vectors (`NV2080_CTRL_CMD_INTERNAL_INTR_GET_KERNEL_TABLE`). RM keeps them all in
//! subtree 0 on every chipset nova-core supports (`intrInitSubtreeMap_TU102()` and
//! `intrInitSubtreeMap_GH100()` map `NV2080_INTR_CATEGORY_ESCHED_DRIVEN_ENGINE_NOTIFICATION`
//! there), apart from the GSP event's subtree.
//!
//! With MSI, every subtree raises the one PCI vector, whose handler, the GSP event's, services the
//! notification vectors too, so that the PCI vector is rearmed once per interrupt. With MSI-X,
//! subtree 0 has an entry of its own, with a handler of its own: [`NotificationIrq`].
//!
//! The handlers signal fences in the top half: signaling a fence takes no sleeping lock, and
//! waking up its waiters early matters.

use kernel::{
    device,
    irq,
    prelude::*,
    sync::{
        atomic::{
            Atomic,
            Relaxed, //
        },
        SetOnce, //
    }, //
};

use super::interrupt_tree::{
    GinVector,
    LeafEnableGuard,
    LeafIndex,
    LeafMask,
    Subtree,
    Tree, //
};
use crate::channel::fence::Timelines;

/// The subtree of the engines' notification vectors.
pub(crate) const NOTIFICATION_SUBTREE: Subtree = GinVector::new::<0>().subtree();

/// The notification vectors of the engines that channels run on, grouped by leaf.
pub(crate) struct NotificationVectors<'a> {
    /// For logging.
    dev: &'a device::Device,
    /// Each leaf with its notification vectors, at most one entry per leaf of the subtree.
    leaves: KVec<(LeafIndex, LeafMask)>,
    /// The timelines whose fences a notification may have completed.
    timelines: &'a Timelines<'a>,
    /// Whether a notification was received yet, which is logged once.
    notified: Atomic<bool>,
}

impl<'a> NotificationVectors<'a> {
    /// Returns the notification vectors `vectors`, on which the fences of `timelines` complete.
    ///
    /// # Errors
    ///
    /// `ENODEV` if `vectors` is empty, `EINVAL` if a vector is beyond every tree, and `ENOTSUPP` if
    /// one is not in [`NOTIFICATION_SUBTREE`].
    pub(crate) fn new(
        dev: &'a device::Device,
        vectors: &[u32],
        timelines: &'a Timelines<'a>,
    ) -> Result<Self> {
        if vectors.is_empty() {
            return Err(ENODEV);
        }

        let mut leaves: KVec<(LeafIndex, LeafMask)> = KVec::new();
        for &vector in vectors {
            let vector = GinVector::try_new(vector).ok_or(EINVAL)?;
            if vector.subtree() != NOTIFICATION_SUBTREE {
                return Err(ENOTSUPP);
            }

            match leaves
                .iter_mut()
                .find(|(leaf, _)| *leaf == vector.leaf_index())
            {
                Some((_, mask)) => *mask = mask.union(vector.leaf_mask()),
                None => leaves.push((vector.leaf_index(), vector.leaf_mask()), GFP_KERNEL)?,
            }
        }

        Ok(Self {
            dev,
            leaves,
            timelines,
            notified: Atomic::new(false),
        })
    }

    /// Enables the vectors at their leaves, until the returned guards drop: from then on, the
    /// handler of [`NOTIFICATION_SUBTREE`] must service them.
    pub(crate) fn enable(&self, tree: &Tree<'a>) -> Result<KVec<LeafEnableGuard<'a>>> {
        let mut guards = KVec::with_capacity(self.leaves.len(), GFP_KERNEL)?;
        for &(leaf, mask) in self.leaves.iter() {
            guards.push(tree.enable_leaf_guarded(leaf, mask), GFP_KERNEL)?;
        }

        Ok(guards)
    }

    /// Services the vectors that are pending, without rearming the PCI vector.
    ///
    /// Returns whether one was.
    pub(super) fn service(&self, tree: &Tree<'_>) -> bool {
        let mut notified = false;
        for &(leaf, mask) in self.leaves.iter() {
            let pending = tree.read_pending(leaf);
            let vectors = pending.vectors().intersection(mask);
            if !vectors.is_empty() {
                // Cleared before the fences are signaled: a notification after this one latches
                // the vector again.
                pending.clear_vectors(vectors);
                notified = true;
            }
        }
        if !notified {
            return false;
        }

        if !self.notified.xchg(true, Relaxed) {
            dev_info!(self.dev, "channels: notification interrupts received\n");
        }
        self.timelines.signal_completed();

        true
    }
}

/// Handler of the notification vectors, on the PCI vector of their subtree of their own: MSI-X.
pub(crate) struct NotificationInterrupt<'a> {
    tree: &'a Tree<'a>,
    /// The notification vectors, once set up.
    vectors: &'a SetOnce<NotificationVectors<'a>>,
}

impl irq::Handler for NotificationInterrupt<'_> {
    fn handle(&self) -> irq::IrqReturn {
        let serviced = self
            .vectors
            .as_ref()
            .is_some_and(|vectors| vectors.service(self.tree));
        self.tree.rearm_pci_irq(NOTIFICATION_SUBTREE);

        if serviced {
            irq::IrqReturn::Handled
        } else {
            irq::IrqReturn::None
        }
    }
}

/// The registered handler of the notification vectors, with MSI-X.
#[pin_data]
pub(crate) struct NotificationIrq<'a> {
    #[pin]
    reg: irq::Registration<'a, NotificationInterrupt<'a>>,
}

impl<'a> NotificationIrq<'a> {
    /// Returns an initializer that registers the handler of the notification vectors, if their
    /// subtree has a PCI vector of its own: `None` with MSI, whose one PCI vector the GSP event's
    /// handler serves.
    ///
    /// # Errors
    ///
    /// The error from `request_irq`.
    ///
    /// # Safety
    ///
    /// Callers must not `mem::forget()` the initialized `NotificationIrq` or otherwise prevent its
    /// [`Drop`] implementation, which runs `free_irq`, from running.
    pub(crate) unsafe fn new(
        tree: &'a Tree<'a>,
        vectors: &'a SetOnce<NotificationVectors<'a>>,
    ) -> Result<Option<Pin<KBox<Self>>>> {
        if !tree.has_own_pci_vector(NOTIFICATION_SUBTREE) {
            return Ok(None);
        }

        let init = try_pin_init!(Self {
            // SAFETY: this function's caller must not leak the `NotificationIrq` that owns this
            // registration, so the registration's `Drop` runs.
            reg <- unsafe {
                irq::Registration::new(
                    tree.request_for(NOTIFICATION_SUBTREE)?,
                    irq::Flags::TRIGGER_NONE,
                    c"nova-core-notify",
                    Ok(NotificationInterrupt { tree, vectors }),
                )
            },
        });

        Ok(Some(KBox::pin_init(init, GFP_KERNEL)?))
    }
}

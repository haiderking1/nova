// SPDX-License-Identifier: GPL-2.0

//! Vblanks: the starts of the vertical blanking of the heads, which the display engine signals
//! with an interrupt.

use kernel::sync::{
    Arc,
    SpinLockIrq, //
};

/// Receives the vblanks of the heads whose vblank interrupt is enabled.
pub trait VblankHandler: Send + Sync {
    /// Called in interrupt context at the start of the vertical blanking of head `head`.
    fn vblank(&self, head: u32);
}

/// The vblank handler, if one is registered, as the display interrupt finds it.
pub(crate) type VblankHandlerSlot = SpinLockIrq<Option<Arc<dyn VblankHandler>>>;

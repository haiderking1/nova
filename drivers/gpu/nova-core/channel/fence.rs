// SPDX-License-Identifier: GPL-2.0

//! Fences of the work of channels.
//!
//! A channel releases a semaphore in system memory once a submission's work is done, with the
//! submission's sequence number, then raises the notification (non-stall) interrupt of its engine.
//! The fence of a submission signals once the semaphore reached its sequence number: the
//! interrupt's handler checks the semaphores of the channels with fences pending, see
//! [`Timelines::signal_completed`].
//!
//! Each channel ID has a fence context of its own, created on first use and kept until the GPU
//! goes away: a fence context must outlive the fences on it, which can outlive their channel, and
//! fences of the channels that later get the same ID come after them on the same timeline.

use core::pin::Pin;

use kernel::{
    device,
    dma::Coherent,
    dma_buf::{
        DriverFence,
        Fence,
        FenceContext,
        FenceContextOps, //
    },
    new_spinlock_irq,
    page::PAGE_SIZE,
    prelude::*,
    str::CString,
    sync::{
        aref::ARef,
        Arc,
        SetOnce,
        SpinLockIrq, //
    }, //
};

use crate::num;

/// The fences of channels, which carry no data of their own.
pub(crate) struct ChannelFences;

impl FenceContextOps for ChannelFences {
    type FenceDataType = ();
}

/// The fence context of a channel ID.
pub(crate) type ChannelFenceContext = FenceContext<ChannelFences>;

/// The fence contexts of channels, one per channel ID.
pub(crate) struct FenceContexts {
    contexts: KVec<SetOnce<Pin<KBox<ChannelFenceContext>>>>,
}

impl FenceContexts {
    /// Creates the table for `count` channel IDs, without any context yet.
    pub(crate) fn new(count: usize) -> Result<Self> {
        let mut contexts = KVec::with_capacity(count, GFP_KERNEL)?;
        for _ in 0..count {
            contexts.push(SetOnce::new(), GFP_KERNEL)?;
        }

        Ok(Self { contexts })
    }

    /// Returns the fence context of channel `chid`, created if it has none yet.
    ///
    /// Fails with `EINVAL` if `chid` is beyond the table.
    pub(crate) fn get(&self, chid: u32) -> Result<&ChannelFenceContext> {
        let slot = self.contexts.get(num::u32_as_usize(chid)).ok_or(EINVAL)?;
        if slot.as_ref().is_none() {
            let name = CString::try_from_fmt(fmt!("channel {}", chid))?;
            let context = KBox::pin_init(
                FenceContext::new(0, c"nova-core", &name, ChannelFences),
                GFP_KERNEL,
            )?;
            // Another caller may have created it meanwhile: either context is fine.
            slot.populate(context);
        }

        Ok(slot.as_ref().ok_or(EINVAL)?)
    }
}

/// The fences of a channel's submissions, pending in order, and whether it still runs work.
struct Pending<'a> {
    /// Each with the sequence number its submission releases the semaphore with.
    fences: KVec<(u32, DriverFence<'a, ChannelFences>)>,
    /// Set once the channel runs no more work: the error its fences signal with.
    error: Option<Error>,
}

/// The fences of a channel, and the semaphore that its submissions release.
#[pin_data]
pub(crate) struct Timeline<'a> {
    context: &'a ChannelFenceContext,
    /// The page whose first 4 bytes are the semaphore, which the GPU writes coherently with the
    /// CPU's caches.
    semaphore: Coherent<'a, [u8]>,
    #[pin]
    pending: SpinLockIrq<Pending<'a>>,
}

impl<'a> Timeline<'a> {
    /// Creates the timeline of a channel, on `context`, with a new semaphore, which holds 0.
    pub(crate) fn new(
        dev: &'a device::Device<device::Bound>,
        context: &'a ChannelFenceContext,
    ) -> Result<Arc<Self>> {
        Arc::pin_init(
            try_pin_init!(Self {
                context,
                semaphore: Coherent::<u8>::zeroed_slice(dev, PAGE_SIZE, GFP_KERNEL)?,
                pending <- new_spinlock_irq!(
                    Pending {
                        fences: KVec::new(),
                        error: None,
                    },
                    "nova_channel_fences"
                ),
            }? Error),
            GFP_KERNEL,
        )
    }

    /// Returns the semaphore page, to map it for the GPU.
    pub(crate) fn semaphore(&self) -> &Coherent<'a, [u8]> {
        &self.semaphore
    }

    /// Returns the value the semaphore holds.
    pub(crate) fn completed(&self) -> u32 {
        // SAFETY: The semaphore is the first 4 bytes of the page `semaphore` owns, which the GPU
        // writes coherently with the CPU's caches.
        unsafe { self.semaphore.as_ptr().cast::<u32>().read_volatile() }
    }

    /// Returns the fence of the submission that releases the semaphore with `sequence`, which
    /// must be added before the submission is: so that its notification can't come first.
    ///
    /// Fails with `ENODEV` once the channel runs no more work.
    pub(crate) fn add(&self, sequence: u32) -> Result<ARef<Fence>> {
        // Allocated with the lock not held.
        let fence = self.context.new_fence_allocation(())?.new_fence();
        let consumer = ARef::from(fence.as_fence());

        let mut pending = self.pending.lock();
        if let Some(err) = pending.error {
            drop(pending);
            fence.signal(Err(err));
            return Err(ENODEV);
        }
        if let Err(err) = pending.fences.reserve(1, GFP_ATOMIC) {
            drop(pending);
            fence.signal(Err(ENOMEM));
            return Err(err.into());
        }
        if pending
            .fences
            .push_within_capacity((sequence, fence))
            .is_err()
        {
            // Unreachable after the reservation above: the fence drops signaled with an error.
            return Err(ENOMEM);
        }

        Ok(consumer)
    }

    /// Signals the fences of the submissions whose sequence number the semaphore reached.
    ///
    /// Takes no sleeping lock, for the notification interrupt's handler.
    pub(crate) fn signal_completed(&self) {
        let value = self.completed();
        let mut pending = self.pending.lock();

        // Sequence numbers wrap: one is reached when it is at most 2^31 before the value.
        let done = pending
            .fences
            .iter()
            .take_while(|(sequence, _)| value.wrapping_sub(*sequence) < 1 << 31)
            .count();
        for _ in 0..done {
            if let Ok((_, fence)) = pending.fences.remove(0) {
                fence.signal(Ok(()));
            }
        }
    }

    /// Has the fence of the submission that releases the semaphore with `sequence`, the last one
    /// added, signal `err`: the submission wasn't made.
    pub(crate) fn cancel(&self, sequence: u32, err: Error) {
        let mut pending = self.pending.lock();
        if pending
            .fences
            .last()
            .is_some_and(|(last, _)| *last == sequence)
        {
            if let Some((_, fence)) = pending.fences.pop() {
                fence.signal(Err(err));
            }
        }
    }

    /// Returns whether the channel runs no more work, see [`Timeline::fail`].
    pub(crate) fn failed(&self) -> bool {
        self.pending.lock().error.is_some()
    }

    /// Has the fences pending, and those added later, signal `err`: the channel runs no more work.
    pub(crate) fn fail(&self, err: Error) {
        let mut pending = self.pending.lock();
        pending.error = Some(err);
        for (_, fence) in pending.fences.drain_all() {
            fence.signal(Err(err));
        }
    }
}

/// The timelines of the channels whose fences the notification interrupt's handler signals.
#[pin_data]
pub(crate) struct Timelines<'a> {
    #[pin]
    active: SpinLockIrq<KVec<Arc<Timeline<'a>>>>,
}

impl<'a> Timelines<'a> {
    /// Creates the set, empty.
    pub(crate) fn new() -> impl PinInit<Self> {
        pin_init!(Self {
            active <- new_spinlock_irq!(KVec::new(), "nova_timelines"),
        })
    }

    /// Adds `timeline`, whose fences the notification interrupt's handler signals from now on.
    pub(crate) fn add(&self, timeline: Arc<Timeline<'a>>) -> Result {
        let mut active = self.active.lock();
        active.reserve(1, GFP_ATOMIC)?;
        active.push_within_capacity(timeline).map_err(|_| ENOMEM)
    }

    /// Removes `timeline`.
    pub(crate) fn remove(&self, timeline: &Arc<Timeline<'a>>) {
        self.active
            .lock()
            .retain(|active| !Arc::ptr_eq(active, timeline));
    }

    /// Signals the fences whose work is done, of every timeline.
    ///
    /// Takes no sleeping lock, for the notification interrupt's handler.
    pub(crate) fn signal_completed(&self) {
        for timeline in self.active.lock().iter() {
            timeline.signal_completed();
        }
    }
}

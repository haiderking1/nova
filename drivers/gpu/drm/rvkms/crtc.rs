// SPDX-License-Identifier: GPL-2.0
//! RVKMS's DRM CRTC implementation.
use super::RvkmsDriver;
use core::marker::*;
use kernel::{
    drm::{
        Device,
        Normal,
        kms::{
            KmsProbe,
            KmsRef,
            crtc::{
                self,
                RawCrtcState, //
            },
            vblank::*,
            ModeObject, //
        },
    },
    impl_has_hr_timer,
    interrupt::LocalInterruptDisabled,
    new_spinlock_irq,
    prelude::*,
    sync::{Arc, SpinLockIrq},
    time::{hrtimer::*, Monotonic, Instant, Delta},
};

pub(crate) type Crtc<C = Normal> = crtc::Crtc<RvkmsCrtc, C>;
//pub(crate) type CrtcState = crtc::CrtcState<RvkmsCrtcState>;

/// Vblank state that is only relevant when a Crtc is enabled.
pub(crate) struct VblankArmState {
    /// The instant in time when we first enabled vblanks.
    ///
    /// We use this to account for vblank events we may have missed as a result of vblank interrupts
    /// being disabled, in order to give the illusion that the scanline has continued running even
    /// without them.
    started: Instant<Monotonic>,

    /// The current duration between frames in nanoseconds.
    ///
    /// Stored separately here so it can be read safely without the main DRM vblank lock.
    frame_duration: i32,

    /// The current start of the vblank sync period for the adjusted display mode for this Crtc. It
    /// is measured as a time delta from the beginning of the frame.
    vbl_start: Delta,
}

/// State for emulating vblank events.
#[derive(Default)]
pub(crate) struct VblankState {
    /// The timer
    timer: Option<Arc<VblankTimer>>,

    /// Vblank state that is only relevant when a Crtc is enabled.
    armed: Option<VblankArmState>,

    /// A reference to a handle for the current VblankTimer if reporting is currently enabled.
    handle: Option<ArcHrTimerHandle<VblankTimer>>,

    /// Whether the vblank timer is about to report a vblank event.
    ///
    /// Due to lock ordering between the DRM vblank core and rvkms, we can't cancel the vblank timer
    /// when disabling vblank reporting if the vblank timer is about to acquire the DRM event lock
    /// in order to report a vblank. If the timer is about to report a vblank, this will be set to
    /// true.
    reporting: bool,

    /// Whether we need to ask the timer to cancel itself.
    ///
    /// This is set to true if we want to cancel the timer, but cannot due so as a result of it
    /// being in the process of reporting a vblank event.
    cancelling: bool,
}

#[pin_data]
pub(crate) struct RvkmsCrtc {
    /// The current vblank emulation state.
    #[pin]
    vblank_state: SpinLockIrq<VblankState>,
}

#[vtable]
impl crtc::DriverCrtc for RvkmsCrtc {
    type Args = ();
    type State = RvkmsCrtcState;
    type Driver = RvkmsDriver;
    type VblankImpl = Self;

    fn new(
        _device: &Device<Self::Driver, KmsProbe>,
        _args: &Self::Args
    ) -> impl PinInit<Self, Error> {
        try_pin_init!(Self {
            vblank_state <- new_spinlock_irq!(VblankState::default(), "vblank_handle_lock")
        })
    }

    fn atomic_check(check: crtc::CrtcAtomicCheck<'_, RvkmsCrtc>) -> Result {
        let crtc = check.crtc();
        let (state, old_state, mut new_state) = check.take_all();

        state.add_affected_planes(crtc)?;

        // Create a vblank timer when enabling a CRTC, and destroy said timer when disabling to
        // resolve the circular reference to CRTC it creates
        if old_state.active() != new_state.active() {
            new_state.vblank_timer = if new_state.active() {
                Some(VblankTimer::new(crtc)?)
            } else {
                None
            };
        }

        Ok(())
    }

    fn atomic_flush(commit: crtc::CrtcAtomicCommit<'_, RvkmsCrtc>) {
        let crtc = commit.crtc();
        let mut new_state = commit.take_new_state();

        if let Some(event) = new_state.get_pending_vblank_event() {
            if let Ok(vbl_ref) = crtc.vblank_get() {
                event.arm(vbl_ref);
            } else {
                event.send();
            }
        }
    }

    fn atomic_enable(commit: crtc::CrtcAtomicCommit<'_, RvkmsCrtc>) {
        let crtc = commit.crtc();
        let new_state = commit.take_new_state();

        // Store a reference to the newly created vblank timer for this CRTC
        crtc.vblank_state.lock().timer = new_state.vblank_timer.clone();

        crtc.vblank_on();
    }

    fn atomic_disable(commit: crtc::CrtcAtomicCommit<'_, RvkmsCrtc>) {
        let crtc = commit.crtc();

        crtc.vblank_off();

        let mut guard = crtc.vblank_state.lock();
        let handle = guard.handle.take();

        // Stop the timer, do so while not holding vbl_state to avoid deadlocking on the timer
        // callback waiting for the same lock.
        drop(guard);
        drop(handle);

        let mut vbl_state = crtc.vblank_state.lock();
        vbl_state.timer = None;
        vbl_state.armed = None;
    }
}

impl VblankSupport for RvkmsCrtc {
    type Crtc = Self;

    fn enable_vblank(
        crtc: &Crtc,
        vblank: &VblankGuard<'_, Self::Crtc>,
        irq: &LocalInterruptDisabled,
    ) -> Result {
        let mut vbl_state = crtc.vblank_state.lock_with(irq);

        let next_vbl_timestamp;
        let started;

        if let Some(ref armed) = vbl_state.armed {
            // Check if we still have a vblank timer we can't cancel. This could happen if the
            // same vblank timer wasn't interruptible when vblanks were last disabled and it still
            // hasn't completed its callback yet.
            //
            // Since that also means the timer hasn't noticed we asked it to cancel itself, we can
            // just uncancel it instead of rescheduling it.
            if vbl_state.reporting {
                debug_assert!(vbl_state.handle.is_some());
                vbl_state.cancelling = false;
                return Ok(());
            }

            started = armed.started;

            let elapsed = started.elapsed();
            let scanline_pos = elapsed.rem_nanos(armed.frame_duration);
            let frame_duration = Delta::from_nanos(armed.frame_duration.into());

            // Figure out the timestamp for the start of the vblank sync period for the current
            // frame.
            let vbl_start_for_this_frame = (elapsed - scanline_pos) + armed.vbl_start;

            // If we've already passed the start of the vblank sync period for this frame, then we
            // need to schedule the timer for the next frame.
            if scanline_pos >= armed.vbl_start {
                next_vbl_timestamp = vbl_start_for_this_frame + frame_duration;
            } else {
                next_vbl_timestamp = vbl_start_for_this_frame
            }
        } else {
            // "Arm" the vblank timer by calculating and saving the constants we need to ensure
            // proper timing
            let hwmode = vblank.hwmode().expect("Vblank hardware mode should be cached by now");
            let frame_duration = vblank.frame_duration();

            // Copied from drm_crtc_next_vblank_start() in drm_vblank.c
            let vbl_start = Delta::from_nanos(
                (i64::from(frame_duration) * i64::from(hwmode.crtc_vblank_start()))
                / i64::from(hwmode.crtc_vtotal())
            );

            started = Instant::now();
            next_vbl_timestamp = vbl_start;

            vbl_state.armed = Some(VblankArmState {
                started,
                frame_duration,
                vbl_start,
            });
        }

        let old_handle = vbl_state.handle.take();

        // Temporarily drop the vbl_state lock, in case we have old timer handles to cancel
        drop(vbl_state);

        // We've checked if the vblank timer could be blocked on earlier, so we can cancel any
        // leftover timer handles now.
        if let Some(mut old_handle) = old_handle {
            old_handle.cancel();
        }

        let mut vbl_state = crtc.vblank_state.lock_with(irq);

        // Schedule the timer to activate on the start of the next vertical sync period.
        let timer = vbl_state.timer.clone().expect("VBL HrTimer should be setup by now");
        vbl_state.handle = Some(timer.start(started + next_vbl_timestamp));

        Ok(())
    }

    fn disable_vblank(
        crtc: &Crtc,
        _vbl_guard: &VblankGuard<'_, Self::Crtc>,
        irq: &LocalInterruptDisabled,
    ) {
        let mut vbl_state = crtc.vblank_state.lock_with(irq);

        vbl_state.cancelling = true;

        // Check if it's safe to block on disabling the timer. If it's not, we need to let the timer
        // know it needs to halt at it's soonest opportunity.
        if vbl_state.reporting {
            return;
        }

        let handle = vbl_state.handle.take();

        // We need to temporarily drop vbl_state to ensure we don't deadlock when cancelling the
        // timer.
        drop(vbl_state);

        if let Some(mut handle) = handle {
            handle.cancel();
        }

        crtc.vblank_state.lock_with(irq).cancelling = false;
    }

    fn get_vblank_timestamp(crtc: &Crtc, handling_vblank_irq: bool) -> Option<VblankTimestamp> {
        let vbl_state = crtc.vblank_state.lock();
        let armed = vbl_state.armed.as_ref()?;

        let time;

        // If we're getting called from the hrtimer, we can just use the expiration time instead of
        // calculating all this.
        if handling_vblank_irq {
            let timer = vbl_state.timer.as_ref().expect("The timer should be ready if we're in its callback");

            time = timer.timer.expires() - Delta::from_nanos(armed.frame_duration.into());
        } else {
            let elapsed = armed.started.elapsed();
            let scanline_pos = elapsed.rem_nanos(armed.frame_duration);

            // Figure out when the scanout for the current frame started
            let frame_start = armed.started + (elapsed - scanline_pos);

            // If we're currently within the vblank, the DRM core will expect us to return the start of
            // the next scanout period.
            if scanline_pos >= armed.vbl_start {
                time = frame_start + Delta::from_nanos(armed.frame_duration.into())
            } else {
                time = frame_start
            };
        }

        Some(VblankTimestamp {
            // To prevent races we roll the hrtimer forward before we do any interrupt processing -
            // this is how real hw works (the interrupt is only generated after all the vblank
            // registers are updated) and what the vblank core expects. Therefore we need to always
            // correct the timestamp by one frame.
            time: Delta::from_nanos(time.as_nanos()),
            max_error: 0,
        })
    }
}

#[derive(Clone, Default)]
pub(crate) struct RvkmsCrtcState {
    /// The current hrtimer used for emulating vblank events, if there is one.
    vblank_timer: Option<Arc<VblankTimer>>,
}

impl crtc::DriverCrtcState for RvkmsCrtcState {
    type Crtc = RvkmsCrtc;
}

/// The main hrtimer structure for emulating vblanks.
#[pin_data]
pub(crate) struct VblankTimer {
    /// The actual hrtimer used for sending out vblanks
    #[pin]
    timer: HrTimer<Self>,

    /// An owned reference to the CRTC that this [`VblankTimer`] belongs to
    crtc: KmsRef<Crtc>,
}

impl_has_hr_timer! {
    impl HasHrTimer<Self> for VblankTimer {
        mode: AbsoluteMode<Monotonic>,
        field: self.timer
    }
}

impl VblankTimer {
    pub(crate) fn new(crtc: &Crtc) -> Result<Arc<Self>> {
        Arc::pin_init(
            pin_init!(Self {
                timer <- HrTimer::<Self>::new(),
                crtc: crtc.into(),
            }),
            GFP_KERNEL,
        )
    }
}

impl HrTimerCallback for VblankTimer {
    type Pointer<'a> = Arc<Self>;

    fn run(
        this: <Self::Pointer<'_> as RawHrTimerCallback>::CallbackTarget<'_>,
        mut context: HrTimerCallbackContext<'_, Self>,
    ) -> HrTimerRestart
    where
        Self: Sized + HasHrTimer<Self>,
    {
        let mut vbl_state = this.crtc.vblank_state.lock();

        // Check if we're being asked to stop before continuing
        if vbl_state.cancelling {
            vbl_state.cancelling = false;
            return HrTimerRestart::NoRestart;
        }

        let armed = vbl_state.armed.as_ref().expect("VBL state should be armed by now");

        // XXX: Why is this _timer? I feel like I would have used the timer if I left it like that,
        // we should check against some of the older versions of this.
        if let Some(ref _timer) = vbl_state.timer {
            let frame_duration = Delta::from_nanos(armed.frame_duration.into());

            let overrun = context.forward_now(frame_duration);
            if overrun != 1 {
                dev_warn!(
                    this.crtc.drm_dev().as_ref().as_ref(),
                    "vblank timer overrun (expected 1, got {overrun})\n"
                );
            }
        }

        // Indicate we're about to report the vblank, e.g. enable/disable can't block on this timer
        vbl_state.reporting = true;

        // Drop vbl_state temporarily to prevent a circular locking dependency
        drop(vbl_state);

        this.crtc.handle_vblank();

        let mut vbl_state = this.crtc.vblank_state.lock();

        vbl_state.reporting = false;

        if vbl_state.cancelling {
            vbl_state.cancelling = false;
            HrTimerRestart::NoRestart
        } else {
            HrTimerRestart::Restart
        }
    }
}

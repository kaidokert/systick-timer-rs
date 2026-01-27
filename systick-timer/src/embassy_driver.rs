// SPDX-License-Identifier: Apache-2.0

use core::cell::RefCell;
use cortex_m::{
    interrupt::{self, Mutex},
    peripheral::SYST,
};

struct Wakeup {
    wakeup_at: u64,
    waker: core::task::Waker,
}

/// Core scheduling logic extracted for testability.
/// Returns true if the wakeup was scheduled (or already exists), false if no slots available.
fn schedule_wake_into_slots<const N: usize>(
    slots: &mut [Option<Wakeup>; N],
    at: u64,
    waker: &core::task::Waker,
) -> bool {
    let mut empty_idx: Option<usize> = None;

    // Single pass: scan all slots for duplicates while tracking first empty
    for (i, slot) in slots.iter().enumerate() {
        if let Some(wakeup) = slot {
            if wakeup.waker.will_wake(waker) && wakeup.wakeup_at == at {
                // Duplicate found - already scheduled
                return true;
            }
        } else if empty_idx.is_none() {
            empty_idx = Some(i);
        }
    }

    // No duplicate found - insert into first empty slot
    if let Some(i) = empty_idx {
        slots[i] = Some(Wakeup {
            wakeup_at: at,
            waker: waker.clone(),
        });
        true
    } else {
        false
    }
}

/// Very basic Embassy time driver that uses the SysTick timer.
///
/// Wakeups are stored in a fixed-size array
///
/// The driver has to be a static instance, create it with:
///
/// ```
/// embassy_time_driver::time_driver_impl!(static DRIVER: SystickDriver<4>
///     = SystickDriver::new(48_000_000, 47999));
/// ```
///
pub struct SystickDriver<const N: usize> {
    wakeup_at: Mutex<RefCell<[Option<Wakeup>; N]>>,
    timer: crate::Timer,
}

impl<const N: usize> SystickDriver<N> {
    /// SystickDriver constructor.
    ///
    /// # Arguments
    ///
    /// * `systick_freq` - The frequency of the SysTick timer in Hz.
    /// * `reload_value` - The reload value for the SysTick timer.
    ///
    ///  Note the tick frequency is configured to embassy_time_driver::TICK_HZ.
    ///
    pub const fn new(systick_freq: u64, reload_value: u32) -> Self {
        let timer = crate::Timer::new(embassy_time_driver::TICK_HZ, reload_value, systick_freq);
        Self {
            wakeup_at: Mutex::new(RefCell::new([const { None }; N])),
            timer: timer,
        }
    }

    /// Create a new SystickDriver with a default reload value that matches
    /// the interrupt frequency to embassy_time_driver::TICK_HZ.
    ///
    /// The default reload value is calculated as (systick_freq / embassy_time_driver::TICK_HZ) - 1.
    ///
    /// # Arguments
    ///
    /// * `systick_freq` - The frequency of the SysTick timer in Hz.
    ///
    pub const fn new_default(systick_freq: u64) -> Self {
        let reload = (systick_freq / embassy_time_driver::TICK_HZ) - 1;
        Self::new(systick_freq, reload as u32)
    }

    fn maybe_wake(&self) {
        interrupt::free(|cs| {
            let mutex_borrow = &self.wakeup_at.borrow(cs);
            for slot in mutex_borrow.borrow_mut().iter_mut() {
                let mut cleared = false;
                if let Some(wakeup) = slot {
                    if self.timer.now() >= wakeup.wakeup_at {
                        wakeup.waker.wake_by_ref();
                        cleared = true;
                    }
                }
                if cleared {
                    *slot = None;
                }
            }
        })
    }

    pub fn start(&self, syst: &mut SYST) {
        self.timer.start(syst);
    }

    /// Call this from the SysTick interrupt handler.
    pub fn systick_interrupt(&self) {
        self.timer.systick_handler();
        self.maybe_wake();
    }
}

impl<const N: usize> embassy_time_driver::Driver for SystickDriver<N> {
    fn now(&self) -> u64 {
        self.timer.now()
    }

    fn schedule_wake(&self, at: u64, waker: &core::task::Waker) {
        interrupt::free(|cs| {
            let mutex_borrow = self.wakeup_at.borrow(cs);
            let mut wakeups = mutex_borrow.borrow_mut();
            if !schedule_wake_into_slots(&mut *wakeups, at, waker) {
                panic!("No free wakeup slots");
            }
        })
    }
}

#[cfg(feature = "embassy-defaults")]
embassy_time_driver::time_driver_impl!(static DRIVER: SystickDriver<4> = SystickDriver::new(8_000_000, 7_999));

#[cfg(feature = "embassy-defaults")]
#[cortex_m_rt::exception]
fn SysTick() {
    DRIVER.systick_interrupt();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::task::Wake;

    /// A simple Wake implementation for testing
    struct TestWake;

    impl Wake for TestWake {
        fn wake(self: Arc<Self>) {}
    }

    fn make_waker() -> core::task::Waker {
        Arc::new(TestWake).into()
    }

    fn make_distinct_waker() -> core::task::Waker {
        // Each call creates a new Arc, so wakers won't be equal
        Arc::new(TestWake).into()
    }

    fn count_occupied<const N: usize>(slots: &[Option<Wakeup>; N]) -> usize {
        slots.iter().filter(|s| s.is_some()).count()
    }

    #[test]
    fn test_basic_insert() {
        let mut slots: [Option<Wakeup>; 4] = [const { None }; 4];
        let waker = make_waker();

        let result = schedule_wake_into_slots(&mut slots, 100, &waker);
        assert!(result);
        assert_eq!(count_occupied(&slots), 1);
    }

    #[test]
    fn test_duplicate_detection_same_waker_same_time() {
        let mut slots: [Option<Wakeup>; 4] = [const { None }; 4];
        let waker = make_waker();

        // First insert succeeds
        assert!(schedule_wake_into_slots(&mut slots, 100, &waker));
        assert_eq!(count_occupied(&slots), 1);

        // Second insert with same waker and time - duplicate detected, still returns true
        assert!(schedule_wake_into_slots(&mut slots, 100, &waker));
        assert_eq!(count_occupied(&slots), 1); // Still only one slot used
    }

    #[test]
    fn test_different_times_same_waker_creates_separate_slots() {
        let mut slots: [Option<Wakeup>; 4] = [const { None }; 4];
        let waker = make_waker();

        schedule_wake_into_slots(&mut slots, 100, &waker);
        schedule_wake_into_slots(&mut slots, 200, &waker);

        assert_eq!(count_occupied(&slots), 2);
    }

    #[test]
    fn test_different_wakers_same_time_creates_separate_slots() {
        let mut slots: [Option<Wakeup>; 4] = [const { None }; 4];
        let waker1 = make_distinct_waker();
        let waker2 = make_distinct_waker();

        schedule_wake_into_slots(&mut slots, 100, &waker1);
        schedule_wake_into_slots(&mut slots, 100, &waker2);

        assert_eq!(count_occupied(&slots), 2);
    }

    /// CRITICAL TEST: This is the bug that the original PR fix missed.
    /// When slots = [None, Some(existing)], scheduling the same wakeup again
    /// should detect the duplicate in slot[1], NOT fill slot[0].
    #[test]
    fn test_duplicate_detection_with_hole_before_existing() {
        let mut slots: [Option<Wakeup>; 4] = [const { None }; 4];
        let waker = make_waker();

        // Manually create a "hole" scenario: [None, Some(wakeup), None, None]
        slots[1] = Some(Wakeup {
            wakeup_at: 100,
            waker: waker.clone(),
        });
        assert_eq!(count_occupied(&slots), 1);

        // Try to schedule the same waker/time - should detect duplicate
        let result = schedule_wake_into_slots(&mut slots, 100, &waker);
        assert!(result); // Returns true (already scheduled)
        assert_eq!(count_occupied(&slots), 1); // Must NOT have added another slot

        // Verify slot[0] is still empty (the flawed implementation would fill it)
        assert!(slots[0].is_none());
    }

    /// Another hole scenario: duplicate exists at the end
    #[test]
    fn test_duplicate_detection_with_multiple_holes() {
        let mut slots: [Option<Wakeup>; 4] = [const { None }; 4];
        let waker = make_waker();

        // Create: [None, None, None, Some(wakeup)]
        slots[3] = Some(Wakeup {
            wakeup_at: 100,
            waker: waker.clone(),
        });

        let result = schedule_wake_into_slots(&mut slots, 100, &waker);
        assert!(result);
        assert_eq!(count_occupied(&slots), 1);
    }

    /// Test that holes get filled correctly when no duplicate exists
    #[test]
    fn test_fills_first_available_hole() {
        let mut slots: [Option<Wakeup>; 4] = [const { None }; 4];
        let waker1 = make_distinct_waker();
        let waker2 = make_distinct_waker();

        // Create: [None, Some(waker1@100), None, None]
        slots[1] = Some(Wakeup {
            wakeup_at: 100,
            waker: waker1.clone(),
        });

        // Insert different waker - should fill slot[0]
        let result = schedule_wake_into_slots(&mut slots, 100, &waker2);
        assert!(result);
        assert_eq!(count_occupied(&slots), 2);
        assert!(slots[0].is_some()); // First empty slot was filled
    }

    #[test]
    fn test_returns_false_when_full() {
        let mut slots: [Option<Wakeup>; 2] = [const { None }; 2];
        let waker1 = make_distinct_waker();
        let waker2 = make_distinct_waker();
        let waker3 = make_distinct_waker();

        assert!(schedule_wake_into_slots(&mut slots, 100, &waker1));
        assert!(schedule_wake_into_slots(&mut slots, 100, &waker2));
        assert!(!schedule_wake_into_slots(&mut slots, 100, &waker3)); // Returns false
    }

    /// Simulate the select() polling scenario that triggered the original bug
    #[test]
    fn test_repeated_polling_scenario() {
        let mut slots: [Option<Wakeup>; 4] = [const { None }; 4];
        let waker = make_waker();

        // Simulate multiple polls with same waker/time (what select() does)
        for _ in 0..100 {
            schedule_wake_into_slots(&mut slots, 1000, &waker);
        }

        // Should only have used ONE slot, not exhausted all slots
        assert_eq!(count_occupied(&slots), 1);
    }
}

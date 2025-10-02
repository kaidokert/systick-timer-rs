// SPDX-License-Identifier: Apache-2.0

use core::sync::atomic::{AtomicU32, Ordering};

#[cfg(test)]
use core::sync::atomic::AtomicBool;

/// A 64-bit timer based on SysTick.
///
/// Stores wraparounds in 2 32-bit atomics. Scales the systick counts
/// to arbitrary frequency.
pub struct Timer {
    inner_wraps: AtomicU32, // Counts SysTick interrupts (lower 32 bits)
    outer_wraps: AtomicU32, // Counts overflows of inner_wraps (upper 32 bits)
    reload_value: u32,      // SysTick reload value (max 2^24 - 1)
    multiplier: u64,        // Precomputed for scaling cycles to ticks
    shift: u32,             // Precomputed for scaling efficiency
    #[cfg(test)]
    current_systick: AtomicU32,
    #[cfg(test)]
    systick_has_wrapped: AtomicBool, // emulated COUNTFLAG (read-to-clear)
    #[cfg(test)]
    after_v1_hook: Option<fn(&Timer)>, // injected nested call site
    #[cfg(test)]
    pendst_is_pending: AtomicBool, // emulated SCB->ICSR PENDSTSET bit
}

impl Timer {
    /// SysTick handler.
    ///
    /// Call this from the SysTick interrupt handler.
    pub fn systick_handler(&self) {
        // Guarantee the COUNTFLAG is cleared
        self.read_systick_countflag();

        // Increment inner_wraps and check for overflow
        let inner = self.inner_wraps.load(Ordering::Relaxed);
        // Check for overflow (inner was u32::MAX)
        // Store the incremented value
        self.inner_wraps
            .store(inner.wrapping_add(1), Ordering::SeqCst);
        if inner == u32::MAX {
            // Increment outer_wraps
            let outer = self.outer_wraps.load(Ordering::Relaxed).wrapping_add(1);
            self.outer_wraps.store(outer, Ordering::SeqCst);
        }
    }

    /// Robust `now()` (VAL-jump tie-breaker, no COUNTFLAG dependency).
    ///
    /// ## Design: One-Wrap Compensation via PendST Detection
    /// This implementation is designed to handle exactly **one missed SysTick wrap**.
    ///
    /// **How it works:**
    /// 1. Uses PendST bit to detect when the SysTick ISR is pending (hardware wrapped but ISR hasn't run)
    /// 2. If PendST is set, adds +1 to the wrap count to compensate for the missed wrap
    /// 3. This allows monotonic time even when the ISR is delayed by up to one full wrap period
    ///
    /// **Design limit:**
    /// If the SysTick ISR is starved for MORE than one complete wrap period, this compensation
    /// becomes insufficient and monotonic violations occur. The ISR starvation detection logic
    /// in `diagnose_timing_violation()` identifies these as catastrophic "N+1 missed wraps".
    pub fn now(&self) -> u64 {
        let reload = self.reload_value as u64;

        loop {
            // The order of these reads is critical to preventing race conditions.
            // 1. Read the high-level state (wrap counters).
            let in1 = self.inner_wraps.load(Ordering::SeqCst) as u64;
            let out1 = self.outer_wraps.load(Ordering::SeqCst) as u64;
            let wraps_pre = (out1 << 32) | in1;

            // 2. Read the low-level hardware value.
            let val_before = self.get_syst() as u64;

            // 3. Re-read the high-level state to detect if an ISR ran during our reads.
            let in2 = self.inner_wraps.load(Ordering::SeqCst) as u64;
            let out2 = self.outer_wraps.load(Ordering::SeqCst) as u64;
            let wraps_post = (out2 << 32) | in2;

            // If the wrap counters changed, an ISR ran. Our snapshot is inconsistent,
            // so we must loop again to get a clean read.
            if wraps_pre != wraps_post {
                continue;
            }

            // If we're here, the wrap counters are stable, but we need to handle the race
            // where PendST could have flipped after wraps_pre but before wraps_post.
            // Re-sample both PendST and VAL now that we know the counters are consistent.
            let is_pending = self.is_systick_pending();
            let val_after = self.get_syst() as u64;

            // Double-check that no ISR ran during our PendST/VAL re-sampling.
            let in3 = self.inner_wraps.load(Ordering::SeqCst) as u64;
            let out3 = self.outer_wraps.load(Ordering::SeqCst) as u64;
            let wraps_final = (out3 << 32) | in3;

            if wraps_final != wraps_pre {
                // Wrap counters changed during our final reads - loop again
                continue;
            }

            // Now we have a truly stable snapshot. Determine if a wrap occurred:
            // 1. PendST is set (hardware detected a wrap)
            // 2. VAL increased (SysTick counts down, so increase means wrap occurred)
            let wrap_occurred = is_pending || val_after > val_before;

            //
            // KEY DESIGN DECISION: This +1 compensation handles exactly ONE missed wrap.
            // If the ISR is starved for 2+ wrap periods, we get monotonic violations.
            let (wraps_u64, final_val) = if wrap_occurred {
                // A wrap occurred after we read wraps_pre. Use the post-wrap VAL reading.
                (wraps_pre + 1, val_after)
            } else {
                // No wrap occurred. Use the most recent VAL reading for consistency.
                (wraps_pre, val_after)
            };

            // Calculate final time.
            let total_cycles = wraps_u64
                .saturating_mul(reload + 1)
                .saturating_add(reload - final_val);

            // Scale to ticks.
            let (result, overflow) = total_cycles.overflowing_mul(self.multiplier);
            if !overflow {
                return result >> self.shift;
            } else {
                let wide = (total_cycles as u128) * (self.multiplier as u128);
                return (wide >> self.shift) as u64;
            }
        }
    }

    /// Returns the current SysTick counter value.
    pub fn get_syst(&self) -> u32 {
        #[cfg(test)]
        return self.current_systick.load(Ordering::SeqCst);

        #[cfg(all(not(test), feature = "cortex-m"))]
        return cortex_m::peripheral::SYST::get_current();

        #[cfg(all(not(test), not(feature = "cortex-m")))]
        panic!("This module requires the cortex-m crate to be available");
    }

    #[inline(always)]
    pub fn read_systick_countflag(&self) -> bool {
        #[cfg(test)]
        {
            return self
                .systick_has_wrapped
                .swap(false, core::sync::atomic::Ordering::SeqCst);
        }

        // # Safety
        // Not safe in any way - it's mutating the flag register without having & mut
        #[cfg(all(not(test), feature = "cortex-m"))]
        unsafe {
            // COUNTFLAG is bit 16. Read clears it.
            const COUNTFLAG: u32 = 1 << 16;
            let csr = (*cortex_m::peripheral::SYST::PTR).csr.read();
            (csr & COUNTFLAG) != 0
        }

        #[cfg(all(not(test), not(feature = "cortex-m")))]
        {
            panic!("This module requires the cortex-m crate");
        }
    }

    /// Checks if the SysTick interrupt is pending.
    pub fn is_systick_pending(&self) -> bool {
        #[cfg(test)]
        return self.pendst_is_pending.load(Ordering::SeqCst);

        #[cfg(all(not(test), feature = "cortex-m"))]
        return cortex_m::peripheral::SCB::is_pendst_pending();

        #[cfg(all(not(test), not(feature = "cortex-m")))]
        return false; // Or panic, depending on desired behavior without cortex-m
    }

    // Figure out a shift that leads to less precision loss
    const fn compute_shift(tick_hz: u64, systick_freq: u64) -> u32 {
        let mut shift = 32;
        let mut multiplier = (tick_hz << shift) / systick_freq;
        while multiplier == 0 && shift < 64 {
            shift += 1;
            multiplier = (tick_hz << shift) / systick_freq;
        }
        shift
    }

    /// Creates a new timer that converts SysTick cycles to ticks at a specified frequency.
    ///
    /// # Arguments
    ///
    /// * `tick_hz` - The desired output frequency in Hz (e.g., 1000 for millisecond ticks)
    /// * `reload_value` - The SysTick reload value. Must be between 1 and 2^24-1.
    ///   This determines how many cycles occur between interrupts.
    /// * `systick_freq` - The frequency of the SysTick counter in Hz (typically CPU frequency)
    ///
    /// # Panics
    ///
    /// * If `reload_value` is 0 or greater than 2^24-1 (16,777,215)
    /// * If `systick_freq` is 0
    ///
    /// # Examples
    ///
    /// ```
    /// # use systick_timer::Timer;
    /// // Create a millisecond-resolution timer on a 48MHz CPU with reload value of 47,999
    /// let timer = Timer::new(1000, 47_999, 48_000_000);
    /// ```
    pub const fn new(tick_hz: u64, reload_value: u32, systick_freq: u64) -> Self {
        if reload_value > (1 << 24) - 1 {
            panic!("Reload value too large");
        }
        if reload_value == 0 {
            panic!("Reload value cannot be 0");
        }

        // Use a shift to maintain precision and keep multiplier within u64
        let shift = Self::compute_shift(tick_hz, systick_freq);
        let multiplier = (tick_hz << shift) / systick_freq;

        Timer {
            inner_wraps: AtomicU32::new(0),
            outer_wraps: AtomicU32::new(0),
            reload_value,
            multiplier,
            shift,
            #[cfg(test)]
            current_systick: AtomicU32::new(0),
            #[cfg(test)]
            systick_has_wrapped: AtomicBool::new(false),
            #[cfg(test)]
            after_v1_hook: None,
            #[cfg(test)]
            pendst_is_pending: AtomicBool::new(false),
        }
    }

    /// Call this if you haven't already started the timer.
    #[cfg(feature = "cortex-m")]
    pub fn start(&self, syst: &mut cortex_m::peripheral::SYST) {
        syst.set_clock_source(cortex_m::peripheral::syst::SystClkSource::Core);
        syst.set_reload(self.reload_value);
        syst.clear_current();
        syst.enable_interrupt();
        syst.enable_counter();
    }

    /// Check if a time difference indicates ISR starvation beyond design limits.
    ///
    /// This timer implementation compensates for exactly one missed SysTick wrap using
    /// the PendST bit detection mechanism. If the SysTick ISR is starved longer than
    /// one complete wrap period, monotonic violations will occur.
    ///
    /// **Key insight**: A backwards jump of N wrap periods indicates N+1 total missed wraps,
    /// because the implementation already compensated for the first missed wrap via PendST.
    ///
    /// Returns `Some(total_missed_wraps)` if the backwards jump matches the pattern of
    /// ISR starvation (N+1 total missed wraps). Returns `None` for other timing issues.
    pub fn diagnose_timing_violation(
        &self,
        current_time: u64,
        previous_time: u64,
        systick_freq: u64,
    ) -> Option<u32> {
        if current_time >= previous_time {
            return None; // Not a backwards jump
        }

        let backwards_jump = previous_time - current_time;
        let wrap_period_ns = ((self.reload_value as u64 + 1) * 1_000_000_000) / systick_freq;

        // Check if backwards jump is close to N complete wrap periods
        // If so, this indicates N+1 total missed wraps (since PendST already compensated for 1)
        for observed_periods in 1..=3 {
            let expected_jump = observed_periods * wrap_period_ns;
            let tolerance = wrap_period_ns / 100; // 1% tolerance

            if backwards_jump >= expected_jump.saturating_sub(tolerance)
                && backwards_jump <= expected_jump + tolerance
            {
                return Some(observed_periods as u32 + 1); // +1 because PendST compensated for first wrap
            }
        }

        None
    }
}

impl Timer {
    // -------- test-only helpers ----------
    #[cfg(test)]
    pub fn set_syst(&self, value: u32) {
        debug_assert!(
            value <= self.reload_value,
            "set_syst: value {} exceeds reload {}",
            value,
            self.reload_value
        );
        self.current_systick.store(value, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub fn set_systick_has_wrapped(&self, val: bool) {
        self.systick_has_wrapped.store(val, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub fn set_after_v1_hook(&mut self, hook: Option<fn(&Timer)>) {
        self.after_v1_hook = hook;
    }

    #[cfg(test)]
    pub fn set_pendst_pending(&self, val: bool) {
        self.pendst_is_pending.store(val, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic]
    fn test_zero_systick_freq() {
        Timer::new(1000, 5, 0);
    }

    #[test]
    fn test_timer_new() {
        let mut timer = Timer::new(1000, 5, 12_000);
        timer.inner_wraps.store(4, Ordering::Relaxed); // 4 interrupts = 24 cycles
        timer.set_syst(3); // Start of next period
        assert_eq!(timer.now(), 2); // Should be ~2 ticks
    }

    #[test]
    fn test_compute_shift() {
        assert_eq!(Timer::compute_shift(1000, 12_000), 32);
        // This ratio overflows 32bit, so we shift
        assert_eq!(Timer::compute_shift(3, 16_000_000_000), 33);
    }

    #[test]
    fn test_timer_initial_state() {
        let timer = Timer::new(1000, 5, 12_000);
        assert_eq!(timer.now(), 0);
    }

    struct TestTimer<const RELOAD: u32> {
        timer: Timer,
    }
    impl<const RELOAD: u32> TestTimer<RELOAD> {
        fn new(tick_hz: u64, systick_freq: u64) -> Self {
            Self {
                timer: Timer::new(tick_hz, RELOAD, systick_freq),
            }
        }
        fn interrupt(&mut self) {
            self.timer.systick_handler();
            self.timer.set_syst(RELOAD);
        }
        fn set_tick(&mut self, tick: u32) -> u64 {
            assert!(tick <= RELOAD);
            self.timer.set_syst(tick);
            self.timer.now()
        }
    }

    #[test]
    fn test_timer_matching_rates() {
        let mut timer = TestTimer::<5>::new(1000, 1000);
        assert_eq!(timer.set_tick(5), 0);
        assert_eq!(timer.set_tick(4), 1);
        assert_eq!(timer.set_tick(0), 5);
        timer.interrupt();
        assert_eq!(timer.set_tick(5), 6);
    }

    #[test]
    fn test_timer_tick_rate_2x() {
        let mut timer = TestTimer::<5>::new(2000, 1000);
        assert_eq!(timer.set_tick(5), 0);
        assert_eq!(timer.set_tick(4), 2);
        assert_eq!(timer.set_tick(0), 10);
        timer.interrupt();
        assert_eq!(timer.set_tick(5), 12);
        timer.interrupt();
        assert_eq!(timer.set_tick(5), 24);
    }

    #[test]
    fn test_systick_rate_2x() {
        let mut timer = TestTimer::<5>::new(1000, 2000);
        assert_eq!(timer.set_tick(5), 0);
        assert_eq!(timer.set_tick(4), 0);
        assert_eq!(timer.set_tick(3), 1);
        assert_eq!(timer.set_tick(2), 1);
        assert_eq!(timer.set_tick(0), 2);
        timer.interrupt();
        assert_eq!(timer.set_tick(5), 3);
        timer.interrupt();
        assert_eq!(timer.set_tick(5), 6);
    }

    #[test]
    fn test_outer_wraps_wrapping() {
        let mut timer = TestTimer::<5>::new(1000, 1000);
        // Set up for outer_wraps overflow
        timer.timer.inner_wraps.store(u32::MAX, Ordering::Relaxed);
        timer.timer.outer_wraps.store(u32::MAX, Ordering::Relaxed);
        timer.timer.set_syst(5);

        // One more interrupt should wrap outer_wraps
        timer.interrupt();
        // Should still count correctly despite wrapping
        // With matching rates, we expect total_cycles * (1000/1000) ticks
        assert_eq!(timer.set_tick(5), ((1u128 << 64) * 1000 / 1000) as u64);
    }

    #[test]
    fn test_extreme_rates() {
        // Test with very high tick rate vs systick rate (1000:1)
        let mut timer = TestTimer::<5>::new(1_000_000, 1000);
        assert_eq!(timer.set_tick(5), 0);
        timer.interrupt(); // One interrupt = 6 cycles, each cycle = 1000 ticks
        assert_eq!(timer.set_tick(5), 6000); // 6 cycles * 1000 ticks/cycle

        // Test with very low tick rate vs systick rate (1:1000)
        let mut timer = TestTimer::<5>::new(1000, 1_000_000);
        // With 1000:1 ratio and reload of 5 (6 cycles per interrupt)
        // We need (1_000_000/1000 * 6) = 6000 cycles for 6 ticks
        // So we need 1000 interrupts for 6 ticks
        for _ in 0..1000 {
            timer.interrupt();
        }
        assert_eq!(timer.set_tick(5), 5); // Should get 5 complete ticks
    }

    #[test]
    fn test_boundary_conditions() {
        // Test with minimum reload value
        let mut timer = TestTimer::<1>::new(1000, 1000);
        assert_eq!(timer.set_tick(1), 0);
        assert_eq!(timer.set_tick(0), 1);
        timer.interrupt();
        assert_eq!(timer.set_tick(1), 2);

        // Test with maximum reload value
        let mut timer = TestTimer::<0xFFFFFF>::new(1000, 1000);
        assert_eq!(timer.set_tick(0xFFFFFF), 0);
        assert_eq!(timer.set_tick(0xFFFF00), 255);
        assert_eq!(timer.set_tick(0), 0xFFFFFF);
    }

    #[test]
    fn test_partial_tick_accuracy() {
        // With matching rates, test partial periods
        let mut timer = TestTimer::<100>::new(1000, 1000);
        assert_eq!(timer.set_tick(100), 0); // Start of period
        assert_eq!(timer.set_tick(75), 25); // 25% through period = 25 ticks
        assert_eq!(timer.set_tick(50), 50); // 50% through period = 50 ticks
        assert_eq!(timer.set_tick(25), 75); // 75% through period = 75 ticks
        assert_eq!(timer.set_tick(0), 100); // End of period = 100 ticks
    }

    #[test]
    fn test_interrupt_race() {
        let mut timer = TestTimer::<5>::new(1000, 1000);
        timer.interrupt();
        timer.timer.set_syst(3);
        let t1 = timer.timer.now();
        timer.interrupt();
        let t2 = timer.timer.now();
        assert!(t2 > t1); // Monotonicity
    }

    #[test]
    fn test_rapid_interrupts() {
        let mut timer = TestTimer::<5>::new(1000, 1000);
        // With matching rates, each interrupt = 6 cycles = 6 ticks
        for _ in 0..10 {
            timer.interrupt();
        }
        // 10 interrupts * 6 cycles/interrupt * (1000/1000) = 60 ticks
        assert_eq!(timer.set_tick(5), 60);

        // At position 2, we're 3 cycles in = 3 more ticks
        assert_eq!(timer.set_tick(2), 63);
    }

    #[test]
    fn test_u64_overflow_scenario() {
        // Timer configuration from the real application:
        // TICK_RESOLUTION: 10_000_000 (tick_hz)
        // reload_value: 0xFFFFFF (16,777,215)
        // systick_freq: 100_000_000
        let timer = Timer::new(10_000_000, 0xFFFFFF, 100_000_000);

        let total_interrupts = 2560u64;
        let outer = (total_interrupts >> 32) as u32;
        let inner = total_interrupts as u32;

        timer.outer_wraps.store(outer, Ordering::Relaxed);
        timer.inner_wraps.store(inner, Ordering::Relaxed);

        // This call should take the u128 fallback path.
        let expected_ticks = 4_296_645_011;
        assert_eq!(timer.now(), expected_ticks);
    }

    #[test]
    fn test_monotonicity_around_wrap() {
        const RELOAD: u32 = 100;
        let timer = Timer::new(1_000, RELOAD, 1_000);

        // 1. Time right before the wrap
        timer.set_syst(1);
        let t1 = timer.now();

        // 2. Simulate the hardware wrap:
        //    - The ISR has NOT run yet, but the pending bit is set.
        timer.set_syst(RELOAD);
        timer.set_pendst_pending(true);

        // 3. Time right after the wrap
        let t2 = timer.now();

        // The key assertion: time must not go backward.
        // The new logic reads the COUNTFLAG and virtually adds a wrap,
        // preventing the non-monotonic jump.
        assert!(
            t2 >= t1,
            "Timer is not monotonic: t1 was {}, t2 was {}",
            t1,
            t2
        );

        // For sanity, let's check the values.
        // t1 should be close to the end of a period.
        // t2 should be at the beginning of the *next* period.
        assert_eq!(t1, 99);
        assert_eq!(t2, 101);
    }

    #[test]
    fn test_monotonicity_between_interrupts() {
        const RELOAD: u32 = 100;
        let timer = Timer::new(1_000, RELOAD, 1_000);

        // Set the counter to the reload value, no wraps yet.
        timer.set_syst(RELOAD);
        let t1 = timer.now();

        // Simulate time passing by decrementing the hardware counter.
        timer.set_syst(RELOAD / 2);
        let t2 = timer.now();

        // Decrement again.
        timer.set_syst(0);
        let t3 = timer.now();

        // Assert that time is always moving forward.
        assert!(t2 > t1, "t2 ({}) should be > t1 ({})", t2, t1);
        assert!(t3 > t2, "t3 ({}) should be > t2 ({})", t3, t2);

        // Also check the specific values for correctness.
        assert_eq!(t1, 0);
        assert_eq!(t2, 50);
        assert_eq!(t3, 100);
    }

    const RELOAD: u32 = 100; // small for easy arithmetic; period = 101 cycles

    #[test]
    fn test_monotonicity_with_starved_isr() {
        // This test simulates the "hardest path" scenario:
        // 1. A wrap occurs, setting the PENDST bit.
        // 2. The SysTick ISR is "starved" by a higher-priority interrupt and does not run.
        // 3. Multiple calls to now() are made from the higher-priority context.
        // 4. All calls must see the pending wrap and report monotonic time.

        let timer = Timer::new(1_000, RELOAD, 1_000); // 1 tick per cycle

        // State 1: Right before a wrap.
        timer.set_syst(1);
        let t1 = timer.now();
        assert_eq!(t1, 100 - 1);

        // State 2: Hardware wraps, ISR is pended but does not run.
        // We manually simulate this state.
        timer.set_pendst_pending(true);
        timer.set_syst(RELOAD - 10); // Timer has wrapped and counted down a bit.

        // First call to now() after the wrap. It must see the pending bit.
        let t2 = timer.now();
        let expected_t2 = (0 + 1) * (RELOAD as u64 + 1) + (RELOAD as u64 - (RELOAD as u64 - 10));
        assert_eq!(t2, expected_t2);
        assert!(
            t2 > t1,
            "Time must advance after wrap. t1={}, t2={}",
            t1,
            t2
        );

        // State 3: More time passes, ISR is still starved.
        timer.set_syst(RELOAD - 20);

        // Second call to now(). It must still see the pending bit.
        let t3 = timer.now();
        let expected_t3 = (0 + 1) * (RELOAD as u64 + 1) + (RELOAD as u64 - (RELOAD as u64 - 20));
        assert_eq!(t3, expected_t3);
        assert!(
            t3 > t2,
            "Time must advance even if ISR is starved. t2={}, t3={}",
            t2,
            t3
        );

        // State 4: The ISR finally runs, clearing the pending bit and incrementing wraps.
        timer.set_pendst_pending(false);
        timer.systick_handler(); // This increments inner_wraps to 1.

        // Third call to now(). It should now use the updated wrap counter.
        let t4 = timer.now();
        let expected_t4 = 1 * (RELOAD as u64 + 1) + (RELOAD as u64 - (RELOAD as u64 - 20));
        assert_eq!(t4, expected_t4);
        assert_eq!(
            t4, t3,
            "Time should be consistent after ISR runs. t3={}, t4={}",
            t3, t4
        );
    }

    // The old tests for value-jump and COUNTFLAG are no longer relevant
    // as the core logic has been replaced. The new test above provides
    // superior coverage for the most critical race condition.
}

#[cfg(test)]
mod stress_test;

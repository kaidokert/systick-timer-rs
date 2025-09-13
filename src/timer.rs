// SPDX-License-Identifier: Apache-2.0

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

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
    hook_enabled: AtomicBool, // guard to avoid recursion
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

    /// Interrupt handler for nested interrupts.
    ///
    /// Call this instead of systick_handler from the interrupt handler, if
    /// you have nested interrupts enabled.
    #[cfg(feature = "cortex-m")]
    pub fn systick_interrupt_for_nested(&self) {
        cortex_m::interrupt::free(|_| {
            self.systick_handler();
        })
    }

    /// Returns the current 64-bit tick count, scaled to the configured frequency `tick_hz`.
    pub fn now(&self) -> u64 {
        let reload = self.reload_value as u64;

        // 1) SW counters (pre)
        let in1 = self.inner_wraps.load(Ordering::SeqCst) as u64;
        let out1 = self.outer_wraps.load(Ordering::SeqCst) as u64;
        let wraps_pre = (out1 << 32) | in1;

        // 2) HW down-counter
        let v1 = self.get_syst() as u64;

        // 3) SW counters (post)
        let in2 = self.inner_wraps.load(Ordering::SeqCst) as u64;
        let out2 = self.outer_wraps.load(Ordering::SeqCst) as u64;
        let wraps_post = (out2 << 32) | in2;

        // Coherent (wraps, val)
        let (wraps_u64, final_val_u64) = if wraps_pre == wraps_post {
            // No ISR in window → VAL matches wraps_pre. COUNTFLAG may indicate a pending wrap.
            if self.read_systick_countflag() {
                (wraps_pre + 1, self.get_syst() as u64)
            } else {
                (wraps_pre, v1)
            }
        } else {
            // ISR ran → use post counters and refresh VAL to match them.
            (wraps_post, self.get_syst() as u64)
        };

        // total_cycles = wraps*(reload+1) + (reload - final_val)
        // This cannot overflow u64 in any realistic uptime.
        let total_cycles = wraps_u64
            .saturating_mul(reload + 1)
            .saturating_add(reload - final_val_u64);

        // Scale to ticks (e.g., microseconds) using precomputed multiplier and shift
        let (result, overflow) = total_cycles.overflowing_mul(self.multiplier);
        if !overflow {
            result >> self.shift
        } else {
            // Slow path: Overflow occurred, fall back to u128 for correctness.
            let wide = (total_cycles as u128) * (self.multiplier as u128);
            (wide >> self.shift) as u64
        }
    }

    /// Returns the current SysTick counter value.
    fn get_syst(&self) -> u32 {
        #[cfg(test)]
        return self.current_systick.load(Ordering::SeqCst);

        #[cfg(all(not(test), feature = "cortex-m"))]
        return cortex_m::peripheral::SYST::get_current();

        #[cfg(all(not(test), not(feature = "cortex-m")))]
        panic!("This module requires the cortex-m crate to be available");
    }

    #[inline(always)]
    fn read_systick_countflag(&self) -> bool {
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
    ///                    This determines how many cycles occur between interrupts.
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
            hook_enabled: AtomicBool::new(false),
        }
    }

    // -------- test-only helpers ----------
    #[cfg(test)]
    pub fn set_syst(&self, value: u32) {
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

    /// Call this if you haven't already started the timer.
    #[cfg(feature = "cortex-m")]
    pub fn start(&self, syst: &mut cortex_m::peripheral::SYST) {
        syst.set_clock_source(cortex_m::peripheral::syst::SystClkSource::Core);
        syst.set_reload(self.reload_value);
        syst.clear_current();
        syst.enable_interrupt();
        syst.enable_counter();
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
        let mut timer = Timer::new(1_000, RELOAD, 1_000);

        // 1. Time right before the wrap
        timer.set_syst(1);
        let t1 = timer.now();

        // 2. Simulate the hardware wrap:
        //    - The hardware counter wraps from 0 to RELOAD.
        //    - The COUNTFLAG gets set.
        //    - The ISR has NOT run yet.
        timer.set_syst(RELOAD);
        timer.set_systick_has_wrapped(true);

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
        let mut timer = Timer::new(1_000, RELOAD, 1_000);

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
}

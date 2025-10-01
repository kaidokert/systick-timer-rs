#![no_std]

use rtt_target::rprintln;
use systick_timer::Timer;

/// Generic timer ID for monotonic checking
pub enum TimerId {
    Timer1,
    Timer2,
}

impl From<TimerId> for &'static str {
    fn from(val: TimerId) -> Self {
        match val {
            TimerId::Timer1 => "Timer1",
            TimerId::Timer2 => "Timer2",
        }
    }
}

/// Check timer monotonic property and panic if violated
///
/// This function takes a timer reference to avoid global static dependency
pub fn check_timer_monotonic<T: Into<TimerId>>(
    timer: &Timer,
    timer_id: T,
    last_now: &mut u64,
    core_frequency: u32,
) {
    let timer_name: &'static str = timer_id.into().into();
    let now = timer.now();
    if now < *last_now {
        // Diagnose the type of timing violation
        if let Some(total_missed_wraps) =
            timer.diagnose_timing_violation(now, *last_now, core_frequency as u64)
        {
            let observed_periods = total_missed_wraps - 1; // Subtract the 1 that PendST compensated for
            let wrap_period_ms = (0xFFFFFF as f32 / core_frequency as f32) * 1000.0;

            rprintln!(
                "Timer {} CATASTROPHIC ISR STARVATION: {} < {} (backwards jump = {} wrap periods)",
                timer_name,
                now,
                *last_now,
                observed_periods
            );
            rprintln!(
                "CAUSE: SysTick ISR was starved for {} TOTAL missed wraps (~{:.1}ms each wrap period)",
                total_missed_wraps,
                wrap_period_ms
            );
            rprintln!(
                "ANALYSIS: Implementation compensated for 1 missed wrap via PendST, but {} additional wraps were missed",
                observed_periods
            );
            rprintln!(
                "SOLUTION: This is CATASTROPHIC - increase SysTick priority or reduce critical section durations"
            );
            panic!(
                "CATASTROPHIC ISR starvation - {} total missed wraps (design limit exceeded)",
                total_missed_wraps
            );
        } else {
            rprintln!(
                "Timer {} monotonic violation: {} < {} (unknown cause)",
                timer_name,
                now,
                *last_now
            );
            panic!(
                "Timer monotonic violation: {} < {} (unknown cause)",
                now, *last_now
            );
        }
    }
    *last_now = now;
}

/// Report active configuration features
pub fn report_configuration() {
    // Report active configuration - use runtime detection
    if cfg!(feature = "freq-target-below") {
        rprintln!("Frequency config: Timer1=target, Timer2=target-below");
    } else if cfg!(feature = "freq-target-above") {
        rprintln!("Frequency config: Timer1=target, Timer2=target-above");
    } else {
        rprintln!("Frequency config: Timer1=target, Timer2=target");
    }

    if cfg!(feature = "block-both") {
        rprintln!("Blocking config: Both timers use critical_section");
    } else if cfg!(feature = "block-timer1") {
        rprintln!("Blocking config: Timer1 uses critical_section");
    } else if cfg!(feature = "block-timer2") {
        rprintln!("Blocking config: Timer2 uses critical_section");
    } else {
        rprintln!("Blocking config: No critical sections");
    }

    if cfg!(feature = "duration-full") {
        rprintln!("Duration config: Full test (overflow detection)");
    } else {
        rprintln!("Duration config: Short test");
    }

    if cfg!(feature = "reload-small") {
        rprintln!("Reload config: Small (accelerated overflow testing)");
    } else {
        rprintln!("Reload config: Normal (~51h to overflow)");
    }

    // Report interrupt priority configuration
    if cfg!(feature = "priority-equal") {
        rprintln!("Priority config: All equal (SysTick=1, Timer1=1, Timer2=1)");
    } else if cfg!(feature = "priority-systick-high") {
        rprintln!("Priority config: SysTick high (SysTick=0, Timer1=1, Timer2=1)");
    } else if cfg!(feature = "priority-timer1-high") {
        rprintln!("Priority config: Timer1 high (SysTick=1, Timer1=0, Timer2=1)");
    } else if cfg!(feature = "priority-timer2-high") {
        rprintln!("Priority config: Timer2 high (SysTick=1, Timer1=1, Timer2=0)");
    } else if cfg!(feature = "priority-mixed-1") {
        rprintln!("Priority config: SysTick high, mixed (SysTick=0, Timer1=0, Timer2=2)");
    } else if cfg!(feature = "priority-mixed-2") {
        rprintln!("Priority config: SysTick high, mixed (SysTick=0, Timer1=1, Timer2=2)");
    } else if cfg!(feature = "priority-mixed-3") {
        rprintln!("Priority config: SysTick high, mixed (SysTick=0, Timer1=2, Timer2=1)");
    } else if cfg!(feature = "priority-timers-high") {
        rprintln!("Priority config: Timers high, SysTick low (SysTick=2, Timer1=0, Timer2=0)");
    } else {
        rprintln!("Priority config: Default equal (SysTick=1, Timer1=1, Timer2=1)");
    }
}

/// Calculate test duration based on features (returns seconds)
pub const fn get_test_duration_seconds(full_duration: u64) -> u64 {
    if cfg!(feature = "duration-full") {
        full_duration
    } else {
        5 // Short duration for all platforms
    }
}

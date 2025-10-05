#![no_std]

use core::sync::atomic::{AtomicU32, Ordering};
use rtt_target::rprintln;
use systick_timer::Timer;

/// Standard tick resolution for all timer stress tests (1 GHz, 1 ns resolution)
pub const TICK_RESOLUTION: u64 = 1_000_000_000;

/// Convert seconds to nanosecond ticks using TICK_RESOLUTION
pub const fn seconds(s: u64) -> u64 {
    s * TICK_RESOLUTION // s * (ticks/second)
}

/// Global timer counters - shared across platforms for ISR access
pub static TIMER1_COUNTER: AtomicU32 = AtomicU32::new(0);
pub static TIMER2_COUNTER: AtomicU32 = AtomicU32::new(0);

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
            let wrap_period_ms = (timer.reload_value() as f32 / core_frequency as f32) * 1000.0;

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

/// Configure interrupt priorities and enable interrupts based on features
pub fn configure_interrupts<T: cortex_m::interrupt::InterruptNumber>(
    tim1_interrupt: T,
    tim2_interrupt: T,
    set_systick_priority: unsafe fn(u8),
    set_irq_priority: unsafe fn(T, u8),
) {
    unsafe {
        if cfg!(feature = "priority-equal") {
            // All equal priority (1,1,1)
            set_systick_priority(1);
            set_irq_priority(tim1_interrupt, 1);
            set_irq_priority(tim2_interrupt, 1);
        } else if cfg!(feature = "priority-systick-high") {
            // SysTick high, timers med (0,1,1)
            set_systick_priority(0);
            set_irq_priority(tim1_interrupt, 1);
            set_irq_priority(tim2_interrupt, 1);
        } else if cfg!(feature = "priority-timer1-high") {
            // Timer1 high, others med (1,0,1)
            set_systick_priority(1);
            set_irq_priority(tim1_interrupt, 0);
            set_irq_priority(tim2_interrupt, 1);
        } else if cfg!(feature = "priority-timer2-high") {
            // Timer2 high, others med (1,1,0)
            set_systick_priority(1);
            set_irq_priority(tim1_interrupt, 1);
            set_irq_priority(tim2_interrupt, 0);
        } else if cfg!(feature = "priority-mixed-1") {
            // SysTick high, Timer1 high, Timer2 low (0,0,2)
            set_systick_priority(0);
            set_irq_priority(tim1_interrupt, 0);
            set_irq_priority(tim2_interrupt, 2);
        } else if cfg!(feature = "priority-mixed-2") {
            // SysTick high, Timer1 med, Timer2 low (0,1,2)
            set_systick_priority(0);
            set_irq_priority(tim1_interrupt, 1);
            set_irq_priority(tim2_interrupt, 2);
        } else if cfg!(feature = "priority-mixed-3") {
            // SysTick high, Timer1 low, Timer2 med (0,2,1)
            set_systick_priority(0);
            set_irq_priority(tim1_interrupt, 2);
            set_irq_priority(tim2_interrupt, 1);
        } else if cfg!(feature = "priority-timers-high") {
            // Timers high, SysTick low (2,0,0)
            set_systick_priority(2);
            set_irq_priority(tim1_interrupt, 0);
            set_irq_priority(tim2_interrupt, 0);
        } else {
            // Default: all equal priority (1,1,1)
            set_systick_priority(1);
            set_irq_priority(tim1_interrupt, 1);
            set_irq_priority(tim2_interrupt, 1);
        }

        // Enable the interrupts after setting priorities
        cortex_m::peripheral::NVIC::unmask(tim1_interrupt);
        cortex_m::peripheral::NVIC::unmask(tim2_interrupt);
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

/// Run the main timer stress test loop
pub fn timer_stress_test(timer: &Timer, full_test_duration_secs: u64) {
    let start_time = timer.now();
    let mut last_log_time = start_time;
    let mut iteration_count = 0u64;
    let one_second = seconds(1);

    let duration_secs = get_test_duration_seconds(full_test_duration_secs);
    let test_duration = seconds(duration_secs);

    rprintln!("Starting timer loop at: {}", start_time);
    if cfg!(feature = "duration-full") {
        rprintln!(
            "Test will run for {} seconds (targeting 64-bit overflow)",
            duration_secs
        );
    } else {
        rprintln!("Test will run for {} seconds", duration_secs);
    }
    rprintln!(
        "one_second = {}, test_duration = {}",
        one_second,
        test_duration
    );

    loop {
        let current_time = timer.now();
        let elapsed_ns = current_time - start_time;

        // Log status every second
        if current_time - last_log_time >= one_second {
            let t1_count = TIMER1_COUNTER.load(Ordering::Relaxed);
            let t2_count = TIMER2_COUNTER.load(Ordering::Relaxed);
            rprintln!(
                "Elapsed: {}s, T1: {} ticks, T2: {} ticks, iterations: {}",
                elapsed_ns / 1_000_000_000,
                t1_count,
                t2_count,
                iteration_count
            );
            last_log_time = current_time;
        }

        // Exit after test duration
        if elapsed_ns >= test_duration {
            let t1_final = TIMER1_COUNTER.load(Ordering::Relaxed);
            let t2_final = TIMER2_COUNTER.load(Ordering::Relaxed);
            rprintln!(
                "Test completed after {:.3}s, total iterations: {}",
                duration_secs,
                iteration_count
            );
            rprintln!(
                "Final stats - TIMER1: {} ticks, TIMER2: {} ticks",
                t1_final,
                t2_final
            );
            break;
        }

        iteration_count += 1;
    }
}

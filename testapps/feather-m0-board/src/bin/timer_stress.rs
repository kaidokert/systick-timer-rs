#![no_main]
#![no_std]

use feather_m0_board as lib;

use core::sync::atomic::{AtomicU32, Ordering};
use cortex_m::peripheral::NVIC;
use hal::clock::GenericClockController;
use hal::pac::{Tc4, Tc5, interrupt};
use hal::time::Hertz;
use hal::timer::TimerCounter;
use hal::timer_traits::InterruptDrivenTimer;
use lib::hal::{self};
use rtt_target::{rprintln, rtt_init_log};
use systick_timer::Timer;
use timer_stress::{
    TimerId, check_timer_monotonic, get_test_duration_seconds, report_configuration,
};

const TICK_RESOLUTION: u64 = 1_000_000_000; // 1 GHz, 1 ns resolution
const CORE_FREQUENCY: u32 = 48_000_000;
const FULL_TEST_DURATION_SECS: u64 = 30; // SAMD21 at 48MHz

// SAMD21-specific timer frequencies (15kHz baseline - appropriate for Cortex-M0+)
const TIMER_TARGET_HZ: u32 = 15_000;
const TIMER_BELOW_HZ: u32 = 14_999;
const TIMER_ABOVE_HZ: u32 = 15_001;

// Global SysTick timer - accessible from ISRs and main code
#[cfg(feature = "reload-small")]
static TIMER: Timer = Timer::new(TICK_RESOLUTION, 0x1FFF, CORE_FREQUENCY as u64);
#[cfg(not(feature = "reload-small"))]
static TIMER: Timer = Timer::new(TICK_RESOLUTION, 0xFFFFFF, CORE_FREQUENCY as u64);
static TIMER1_COUNTER: AtomicU32 = AtomicU32::new(0);
static TIMER2_COUNTER: AtomicU32 = AtomicU32::new(0);

const fn seconds(s: u64) -> u64 {
    s * TICK_RESOLUTION // s * (ticks/second)
}

/// Encode priority level for ARM Cortex-M NVIC
/// ARM Cortex-M0+ uses only 2 bits for priority (4 levels: 0, 1, 2, 3)
/// Shift to upper bits: priority 0 = 0x00, priority 1 = 0x40, priority 2 = 0x80, etc.
const fn encode_priority(priority: u8) -> u8 {
    priority << 6
}

/// Set raw interrupt priority using direct register access (adapted for SAMD21)
unsafe fn set_irq_prio_raw(irq: hal::pac::Interrupt, priority: u8) {
    let irqn = irq as usize;
    unsafe {
        let nvic = &(*NVIC::PTR);
        // SAMD21 has limited interrupt count, check bounds
        if irqn < nvic.ipr.len() {
            nvic.ipr[irqn].write(encode_priority(priority) as u32);
        }
        // If out of bounds, interrupt priorities aren't supported for this IRQ on SAMD21
    }
}

/// Set SysTick priority using SCB (Cortex-M0+ has very limited priority support)
unsafe fn set_systick_priority(priority: u8) {
    // Cortex-M0+ only has shpr[0] and shpr[1], not shpr[11]
    // SysTick priority might not be configurable on M0+
    // For now, skip SysTick priority setting on this platform
    let _ = priority; // Silence unused parameter warning
}

/// Configure all interrupt priorities based on features
pub fn configure_interrupt_priorities(
    tim1_interrupt: hal::pac::Interrupt,
    tim2_interrupt: hal::pac::Interrupt,
) {
    unsafe {
        if cfg!(feature = "priority-equal") {
            // All equal priority (1,1,1)
            set_systick_priority(1);
            set_irq_prio_raw(tim1_interrupt, 1);
            set_irq_prio_raw(tim2_interrupt, 1);
        } else if cfg!(feature = "priority-systick-high") {
            // SysTick high, timers med (0,1,1)
            set_systick_priority(0);
            set_irq_prio_raw(tim1_interrupt, 1);
            set_irq_prio_raw(tim2_interrupt, 1);
        } else if cfg!(feature = "priority-timer1-high") {
            // Timer1 high, others med (1,0,1)
            set_systick_priority(1);
            set_irq_prio_raw(tim1_interrupt, 0);
            set_irq_prio_raw(tim2_interrupt, 1);
        } else if cfg!(feature = "priority-timer2-high") {
            // Timer2 high, others med (1,1,0)
            set_systick_priority(1);
            set_irq_prio_raw(tim1_interrupt, 1);
            set_irq_prio_raw(tim2_interrupt, 0);
        } else if cfg!(feature = "priority-mixed-1") {
            // SysTick high, Timer1 high, Timer2 low (0,0,2)
            set_systick_priority(0);
            set_irq_prio_raw(tim1_interrupt, 0);
            set_irq_prio_raw(tim2_interrupt, 2);
        } else if cfg!(feature = "priority-mixed-2") {
            // SysTick high, Timer1 med, Timer2 low (0,1,2)
            set_systick_priority(0);
            set_irq_prio_raw(tim1_interrupt, 1);
            set_irq_prio_raw(tim2_interrupt, 2);
        } else if cfg!(feature = "priority-mixed-3") {
            // SysTick high, Timer1 low, Timer2 med (0,2,1)
            set_systick_priority(0);
            set_irq_prio_raw(tim1_interrupt, 2);
            set_irq_prio_raw(tim2_interrupt, 1);
        } else if cfg!(feature = "priority-timers-high") {
            // Timers high, SysTick low (2,0,0)
            set_systick_priority(2);
            set_irq_prio_raw(tim1_interrupt, 0);
            set_irq_prio_raw(tim2_interrupt, 0);
        } else {
            // Default: all equal priority (1,1,1)
            set_systick_priority(1);
            set_irq_prio_raw(tim1_interrupt, 1);
            set_irq_prio_raw(tim2_interrupt, 1);
        }
    }
}

#[cortex_m_rt::entry]
fn main() -> ! {
    let mut cp = cortex_m::Peripherals::take().expect("Failed to take core peripherals");
    let mut dp = hal::pac::Peripherals::take().expect("Failed to take device peripherals");

    // Configure clocks - SAMD21 runs at 48MHz by default
    let mut clocks = GenericClockController::with_external_32kosc(
        dp.gclk,
        &mut dp.pm,
        &mut dp.sysctrl,
        &mut dp.nvmctrl,
    );

    let gclk0 = clocks.gclk0();

    rtt_init_log!(
        log::LevelFilter::Debug,
        rtt_target::ChannelMode::NoBlockTrim,
        1024
    );

    rprintln!("Hello from Feather M0 with Systick Timer!");

    // Report active configuration
    report_configuration();

    // Initialize the global SysTick timer
    TIMER.start(&mut cp.SYST);
    rprintln!(
        "SysTick timer initialized at {}MHz",
        CORE_FREQUENCY / 1_000_000
    );

    // Initialize TC4 and TC5 timers for high-frequency interrupts (~50kHz like STM32)
    // Configure a clock for the TC4 and TC5 peripherals
    let tc45 = &clocks.tc4_tc5(&gclk0).unwrap();

    // Initialize TC4 timer (Timer1) - always at target frequency
    let mut tc4_timer = TimerCounter::tc4_(tc45, dp.tc4, &mut dp.pm);
    InterruptDrivenTimer::start(&mut tc4_timer, Hertz::Hz(TIMER_TARGET_HZ).into_duration());
    tc4_timer.enable_interrupt();

    // Initialize TC5 timer (Timer2) - frequency based on features
    let mut tc5_timer = TimerCounter::tc5_(tc45, dp.tc5, &mut dp.pm);
    let tc5_frequency = if cfg!(feature = "freq-target-below") {
        TIMER_BELOW_HZ
    } else if cfg!(feature = "freq-target-above") {
        TIMER_ABOVE_HZ
    } else {
        TIMER_TARGET_HZ
    };
    InterruptDrivenTimer::start(&mut tc5_timer, Hertz::Hz(tc5_frequency).into_duration());
    tc5_timer.enable_interrupt();

    rprintln!(
        "TC4 and TC5 timers initialized - TC4: {}Hz, TC5: {}Hz",
        TIMER_TARGET_HZ,
        tc5_frequency
    );

    // Configure interrupt priorities before enabling interrupts
    configure_interrupt_priorities(hal::pac::Interrupt::TC4, hal::pac::Interrupt::TC5);

    // Enable TC4 and TC5 interrupts
    unsafe {
        NVIC::unmask(hal::pac::Interrupt::TC4);
        NVIC::unmask(hal::pac::Interrupt::TC5);
    }

    let start_time = TIMER.now();
    let mut last_log_time = start_time;
    let mut iteration_count = 0u64;
    let one_second = seconds(1);

    let duration_secs = get_test_duration_seconds(FULL_TEST_DURATION_SECS);
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
        let current_time = TIMER.now();
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

    feather_m0_board::exit()
}

// SysTick Timer Interrupt Handler
#[cortex_m_rt::exception]
fn SysTick() {
    TIMER.systick_handler();
}

// TC4 Timer Interrupt Handler (Timer1)
#[interrupt]
fn TC4() {
    static mut TIMER1_LAST_NOW: u64 = 0;
    TIMER1_COUNTER.store(
        TIMER1_COUNTER.load(Ordering::Relaxed) + 1,
        Ordering::Relaxed,
    );

    // Conditional critical section based on features
    #[cfg(any(feature = "block-timer1", feature = "block-both"))]
    critical_section::with(|_| {
        check_timer_monotonic(&TIMER, TimerId::Timer1, TIMER1_LAST_NOW, CORE_FREQUENCY);
    });

    #[cfg(not(any(feature = "block-timer1", feature = "block-both")))]
    check_timer_monotonic(&TIMER, TimerId::Timer1, TIMER1_LAST_NOW, CORE_FREQUENCY);

    // Clear the overflow interrupt flag
    unsafe {
        Tc4::ptr()
            .as_ref()
            .unwrap()
            .count16()
            .intflag()
            .modify(|_, w| w.ovf().set_bit());
    }
}

// TC5 Timer Interrupt Handler (Timer2)
#[interrupt]
fn TC5() {
    static mut TIMER2_LAST_NOW: u64 = 0;
    TIMER2_COUNTER.store(
        TIMER2_COUNTER.load(Ordering::Relaxed) + 1,
        Ordering::Relaxed,
    );

    // Conditional critical section based on features
    #[cfg(any(feature = "block-timer2", feature = "block-both"))]
    critical_section::with(|_| {
        check_timer_monotonic(&TIMER, TimerId::Timer2, TIMER2_LAST_NOW, CORE_FREQUENCY);
    });

    #[cfg(not(any(feature = "block-timer2", feature = "block-both")))]
    check_timer_monotonic(&TIMER, TimerId::Timer2, TIMER2_LAST_NOW, CORE_FREQUENCY);

    // Clear the overflow interrupt flag
    unsafe {
        Tc5::ptr()
            .as_ref()
            .unwrap()
            .count16()
            .intflag()
            .modify(|_, w| w.ovf().set_bit());
    }
}

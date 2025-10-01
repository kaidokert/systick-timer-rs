#![no_main]
#![no_std]

use stm32f412_nucleo as lib;

use core::sync::atomic::{AtomicU32, Ordering};
use cortex_m::peripheral::NVIC;
use hal::rcc::Config;
use lib::hal::{self, interrupt, prelude::*, timer::Timer as HalTimer};
use rtt_target::{rprintln, rtt_init_log};
use systick_timer::Timer;
use timer_stress::{
    TimerId, check_timer_monotonic, get_test_duration_seconds, report_configuration,
};

const TICK_RESOLUTION: u64 = 1_000_000_000; // 1 GHz, 1 ns resolution
const CORE_FREQUENCY: u32 = 100_000_000;
const FULL_TEST_DURATION_SECS: u64 = 50;

// STM32F412-specific timer frequencies (50kHz baseline - appropriate for Cortex-M4)
const TIMER_TARGET_HZ: u32 = 50_000;
const TIMER_BELOW_HZ: u32 = 49_999;
const TIMER_ABOVE_HZ: u32 = 50_001;

// Global SysTick timer - accessible from ISRs and main code
#[cfg(feature = "reload-small")]
static TIMER: Timer = Timer::new(TICK_RESOLUTION, 0x3FF, CORE_FREQUENCY as u64);
#[cfg(not(feature = "reload-small"))]
static TIMER: Timer = Timer::new(TICK_RESOLUTION, 0xFFFFFF, CORE_FREQUENCY as u64);
static TIMER1_COUNTER: AtomicU32 = AtomicU32::new(0);
static TIMER2_COUNTER: AtomicU32 = AtomicU32::new(0);

const fn seconds(s: u64) -> u64 {
    s * TICK_RESOLUTION // s * (ticks/second)
}

/// Encode priority level for ARM Cortex-M NVIC
/// ARM Cortex-M uses only the upper 4 bits for priority (on STM32F4)
/// So priority 0 = 0x00, priority 1 = 0x10, priority 2 = 0x20, etc.
const fn encode_priority(priority: u8) -> u8 {
    priority << 4
}

/// Set raw interrupt priority using direct register access
unsafe fn set_irq_prio_raw(irq: hal::pac::Interrupt, priority: u8) {
    let irqn = irq as usize;
    unsafe {
        let nvic = &(*NVIC::PTR);
        nvic.ipr[irqn].write(encode_priority(priority));
    }
}

/// Set SysTick priority using SCB
unsafe fn set_systick_priority(priority: u8) {
    unsafe {
        let scb = &(*cortex_m::peripheral::SCB::PTR);
        scb.shpr[11].write(encode_priority(priority)); // SysTick is exception 15, shpr[11]
    }
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
    let dp = hal::pac::Peripherals::take().expect("Failed to take device peripherals");
    let mut rcc = dp.RCC.freeze(Config::hse(8.MHz()).sysclk(100.MHz()));

    rtt_init_log!(
        log::LevelFilter::Debug,
        rtt_target::ChannelMode::NoBlockTrim,
        1024
    );

    rprintln!("Hello from STM32F412 with Systick Timer !");

    // Report active configuration
    report_configuration();

    // Initialize the global SysTick timer
    TIMER.start(&mut cp.SYST);
    rprintln!(
        "SysTick timer initialized at {}MHz",
        CORE_FREQUENCY / 1_000_000
    );

    // Configure TIM2 frequency based on features
    let mut tim2 = HalTimer::new(dp.TIM2, &mut rcc).counter_hz();
    tim2.start(TIMER_TARGET_HZ.Hz()).unwrap();
    tim2.listen(hal::timer::Event::Update);

    // Configure TIM5 frequency based on features
    let mut tim5 = HalTimer::new(dp.TIM5, &mut rcc).counter_hz();
    let tim5_frequency = if cfg!(feature = "freq-target-below") {
        TIMER_BELOW_HZ
    } else if cfg!(feature = "freq-target-above") {
        TIMER_ABOVE_HZ
    } else {
        TIMER_TARGET_HZ
    };
    tim5.start(tim5_frequency.Hz()).unwrap();
    tim5.listen(hal::timer::Event::Update);

    rprintln!(
        "TIM2 and TIM5 timers initialized - TIM2: {}Hz, TIM5: {}Hz",
        TIMER_TARGET_HZ,
        tim5_frequency
    );

    // Configure interrupt priorities before enabling interrupts
    configure_interrupt_priorities(hal::pac::Interrupt::TIM2, hal::pac::Interrupt::TIM5);

    // Enable TIM2 and TIM5 interrupts
    unsafe {
        NVIC::unmask(hal::pac::Interrupt::TIM2);
        NVIC::unmask(hal::pac::Interrupt::TIM5);
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

    stm32f412_nucleo::exit()
}

// SysTick Timer Interrupt Handler
#[cortex_m_rt::exception]
fn SysTick() {
    TIMER.systick_handler();
}

// TIM2 Timer Interrupt Handler (Timer1)
#[hal::interrupt]
fn TIM2() {
    static mut TIMER1_LAST_NOW: u64 = 0;
    TIMER1_COUNTER.fetch_add(1, Ordering::Relaxed);

    // Conditional critical section based on features
    #[cfg(any(feature = "block-timer1", feature = "block-both"))]
    critical_section::with(|_| {
        check_timer_monotonic(&TIMER, TimerId::Timer1, TIMER1_LAST_NOW, CORE_FREQUENCY);
    });

    #[cfg(not(any(feature = "block-timer1", feature = "block-both")))]
    check_timer_monotonic(&TIMER, TimerId::Timer1, TIMER1_LAST_NOW, CORE_FREQUENCY);

    unsafe {
        let tim2 = &*hal::pac::TIM2::ptr();
        if tim2.sr().read().uif().bit_is_set() {
            tim2.sr().write(|w| w.uif().clear());
        }
    }
}

// TIM5 Timer Interrupt Handler (Timer2)
#[hal::interrupt]
fn TIM5() {
    static mut TIMER2_LAST_NOW: u64 = 0;
    TIMER2_COUNTER.fetch_add(1, Ordering::Relaxed);

    // Conditional critical section based on features
    #[cfg(any(feature = "block-timer2", feature = "block-both"))]
    critical_section::with(|_| {
        check_timer_monotonic(&TIMER, TimerId::Timer2, TIMER2_LAST_NOW, CORE_FREQUENCY);
    });

    #[cfg(not(any(feature = "block-timer2", feature = "block-both")))]
    check_timer_monotonic(&TIMER, TimerId::Timer2, TIMER2_LAST_NOW, CORE_FREQUENCY);

    // Clear the overflow interrupt flag
    unsafe {
        let tim5 = &*hal::pac::TIM5::ptr();
        if tim5.sr().read().uif().bit_is_set() {
            tim5.sr().write(|w| w.uif().clear());
        }
    }
}

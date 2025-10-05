#![no_main]
#![no_std]

use stm32f412_nucleo as lib;

use core::sync::atomic::Ordering;
use hal::rcc::Config;
use lib::hal::{self, interrupt, prelude::*, timer::Timer as HalTimer};
use rtt_target::{rprintln, rtt_init_log};
use systick_timer::Timer;
use timer_stress::{
    TICK_RESOLUTION, TIMER1_COUNTER, TIMER2_COUNTER, TimerId, check_timer_monotonic,
    configure_interrupts, report_configuration, timer_stress_test,
};

const CORE_FREQUENCY: u32 = 100_000_000;
const FULL_TEST_DURATION_SECS: u64 = 30;

// STM32F412-specific timer frequencies (50kHz baseline - appropriate for Cortex-M4)
const TIMER_TARGET_HZ: u32 = 50_000;
const TIMER_BELOW_HZ: u32 = 49_999;
const TIMER_ABOVE_HZ: u32 = 50_001;

// Global SysTick timer - accessible from ISRs and main code
#[cfg(feature = "reload-small")]
static TIMER: Timer = Timer::new(TICK_RESOLUTION, 0x3FF, CORE_FREQUENCY as u64);
#[cfg(not(feature = "reload-small"))]
static TIMER: Timer = Timer::new(TICK_RESOLUTION, 0xFFFFFF, CORE_FREQUENCY as u64);

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
        let nvic = &(*cortex_m::peripheral::NVIC::PTR);
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

    // Configure interrupt priorities and enable interrupts
    configure_interrupts(
        hal::pac::Interrupt::TIM2,
        hal::pac::Interrupt::TIM5,
        set_systick_priority,
        set_irq_prio_raw,
    );

    timer_stress_test(&TIMER, FULL_TEST_DURATION_SECS);

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

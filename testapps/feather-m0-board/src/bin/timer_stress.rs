#![no_main]
#![no_std]

use feather_m0_board as lib;

use core::sync::atomic::Ordering;
use hal::clock::GenericClockController;
use hal::pac::{Tc4, Tc5, interrupt};
use hal::time::Hertz;
use hal::timer::TimerCounter;
use hal::timer_traits::InterruptDrivenTimer;
use lib::hal::{self};
use rtt_target::{rprintln, rtt_init_log};
use systick_timer::Timer;
use timer_stress::{
    TICK_RESOLUTION, TIMER1_COUNTER, TIMER2_COUNTER, TimerId, check_timer_monotonic,
    configure_interrupts, report_configuration, timer_stress_test,
};

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
        let nvic = &(*cortex_m::peripheral::NVIC::PTR);
        let ipr_index = irqn / 4;
        let byte_offset = (irqn % 4) * 8;
        if ipr_index < nvic.ipr.len() {
            let mask = 0xFF << byte_offset;
            let value = (encode_priority(priority) as u32) << byte_offset;
            nvic.ipr[ipr_index].modify(|r| (r & !mask) | value);
        }
    }
}

/// Set SysTick priority using SCB (Cortex-M0+ supports SysTick priority via SHPR1)
unsafe fn set_systick_priority(priority: u8) {
    unsafe {
        let scb = &(*cortex_m::peripheral::SCB::PTR);
        // SysTick is exception 15, on M0+ it's in SHPR1[31:24] (byte 3 of SHPR1)
        // M0+ only has shpr[0] and shpr[1], not shpr[2] or shpr[3]
        scb.shpr[1].write(encode_priority(priority) as u32);
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

    // Configure interrupt priorities and enable interrupts
    configure_interrupts(
        hal::pac::Interrupt::TC4,
        hal::pac::Interrupt::TC5,
        set_systick_priority,
        set_irq_prio_raw,
    );

    timer_stress_test(&TIMER, FULL_TEST_DURATION_SECS);

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

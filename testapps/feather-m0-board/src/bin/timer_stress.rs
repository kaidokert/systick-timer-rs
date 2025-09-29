#![no_main]
#![no_std]

use feather_m0_board as lib;

use core::sync::atomic::{AtomicU32, Ordering};
use lib::hal::clock::GenericClockController;
use rtt_target::{rprintln, rtt_init_log};
use systick_timer::Timer;
use lib::hal::pac::{interrupt, Tc4, Tc5};
use lib::hal::timer::TimerCounter;
use lib::hal::time::Hertz;
use lib::hal::timer_traits::InterruptDrivenTimer;
use cortex_m::peripheral::NVIC;

const TICK_RESOLUTION: u64 = 1_000_000_000; // 1 GHz, 1 ns resolution

// Global SysTick timer - accessible from ISRs and main code
static TIMER: Timer = Timer::new(TICK_RESOLUTION, 0xFFFFFF, 48_000_000);
static TC4_COUNTER: AtomicU32 = AtomicU32::new(0);
static TC5_COUNTER: AtomicU32 = AtomicU32::new(0);

const fn seconds(s: u64) -> u64 {
    s * TICK_RESOLUTION // s * (ticks/second)
}

#[cortex_m_rt::entry]
fn main() -> ! {
    let mut cp = cortex_m::Peripherals::take().expect("Failed to take core peripherals");
    let mut dp = lib::hal::pac::Peripherals::take().expect("Failed to take device peripherals");

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

    // Initialize the global SysTick timer
    TIMER.start(&mut cp.SYST);
    rprintln!("SysTick timer initialized at 48MHz");

    // Initialize TC4 and TC5 timers for high-frequency interrupts (~50kHz like STM32)
    // Configure a clock for the TC4 and TC5 peripherals
    let tc45 = &clocks.tc4_tc5(&gclk0).unwrap();

    // Initialize TC4 timer at 50kHz
    let mut tc4_timer = TimerCounter::tc4_(tc45, dp.tc4, &mut dp.pm);
    InterruptDrivenTimer::start(&mut tc4_timer, Hertz::Hz(50_000).into_duration());
    tc4_timer.enable_interrupt();

    // Initialize TC5 timer at 50kHz
    let mut tc5_timer = TimerCounter::tc5_(tc45, dp.tc5, &mut dp.pm);
    InterruptDrivenTimer::start(&mut tc5_timer, Hertz::Hz(50_000).into_duration());
    tc5_timer.enable_interrupt();

    rprintln!("TC4 and TC5 timers initialized at ~50kHz");

    // Enable TC4 and TC5 interrupts in NVIC
    unsafe {
        cp.NVIC.set_priority(interrupt::TC4, 2);
        cp.NVIC.set_priority(interrupt::TC5, 2);
        NVIC::unmask(interrupt::TC4);
        NVIC::unmask(interrupt::TC5);
        cortex_m::interrupt::enable();
    }

    let start_time = TIMER.now();
    let mut last_log_time = start_time;
    let mut iteration_count = 0u64;

    let one_second = seconds(1);
    let test_duration = seconds(5); // 5 second test like STM32 version

    rprintln!("Starting timer loop at: {}", start_time);
    rprintln!("Test will run for 5 seconds");
    rprintln!("one_second = {}, test_duration = {}", one_second, test_duration);

    loop {
        let current_time = TIMER.now();
        let elapsed_ns = current_time - start_time;

        // Log status every second
        if current_time - last_log_time >= one_second {
            let tc4_count = TC4_COUNTER.load(Ordering::Relaxed);
            let tc5_count = TC5_COUNTER.load(Ordering::Relaxed);
            rprintln!(
                "Elapsed: {}s, TC4: {} ticks, TC5: {} ticks, iterations: {}",
                elapsed_ns / 1_000_000_000,
                tc4_count,
                tc5_count,
                iteration_count
            );
            last_log_time = current_time;
        }

        // Check for test completion
        if elapsed_ns >= test_duration {
            let tc4_final = TC4_COUNTER.load(Ordering::Relaxed);
            let tc5_final = TC5_COUNTER.load(Ordering::Relaxed);

            rprintln!("Test completed!");
            rprintln!("Final stats - TC4: {} ticks, TC5: {} ticks, iterations: {}",
                     tc4_final, tc5_final, iteration_count);
            break;
        }

        iteration_count += 1;

        // Optional: yield to prevent overwhelming the system
        if iteration_count % 10000 == 0 {
            cortex_m::asm::nop();
        }
    }

    feather_m0_board::exit()
}

// SysTick Timer Interrupt Handler
#[cortex_m_rt::exception]
fn SysTick() {
    TIMER.systick_handler();
}

// TC4 Timer Interrupt Handler
#[interrupt]
fn TC4() {
    // Access global TIMER for timing measurements
    let _now = TIMER.now();

    // Increment counter for monitoring
    TC4_COUNTER.store(TC4_COUNTER.load(Ordering::Relaxed) + 1, Ordering::Relaxed);

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

// TC5 Timer Interrupt Handler
#[interrupt]
fn TC5() {
    // Access global TIMER for timing measurements
    let _now = TIMER.now();

    // Increment counter for monitoring
    TC5_COUNTER.store(TC5_COUNTER.load(Ordering::Relaxed) + 1, Ordering::Relaxed);

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

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
#[cfg(feature = "reload-small")]
static TIMER: Timer = Timer::new(TICK_RESOLUTION, 0x1FFF, 48_000_000);
#[cfg(not(feature = "reload-small"))]
static TIMER: Timer = Timer::new(TICK_RESOLUTION, 0xFFFFFF, 48_000_000);
static TC4_COUNTER: AtomicU32 = AtomicU32::new(0);
static TC5_COUNTER: AtomicU32 = AtomicU32::new(0);

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
unsafe fn set_irq_prio_raw(irq: lib::hal::pac::Interrupt, priority: u8) {
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
pub fn configure_interrupt_priorities() {
    unsafe {
        if cfg!(feature = "priority-equal") {
            // All equal priority (1,1,1)
            set_systick_priority(1);
            set_irq_prio_raw(lib::hal::pac::Interrupt::TC4, 1);
            set_irq_prio_raw(lib::hal::pac::Interrupt::TC5, 1);
        } else if cfg!(feature = "priority-systick-high") {
            // SysTick high, timers med (0,1,1)
            set_systick_priority(0);
            set_irq_prio_raw(lib::hal::pac::Interrupt::TC4, 1);
            set_irq_prio_raw(lib::hal::pac::Interrupt::TC5, 1);
        } else if cfg!(feature = "priority-timer1-high") {
            // Timer1 high, others med (1,0,1)
            set_systick_priority(1);
            set_irq_prio_raw(lib::hal::pac::Interrupt::TC4, 0);
            set_irq_prio_raw(lib::hal::pac::Interrupt::TC5, 1);
        } else if cfg!(feature = "priority-timer2-high") {
            // Timer2 high, others med (1,1,0)
            set_systick_priority(1);
            set_irq_prio_raw(lib::hal::pac::Interrupt::TC4, 1);
            set_irq_prio_raw(lib::hal::pac::Interrupt::TC5, 0);
        } else if cfg!(feature = "priority-mixed-1") {
            // SysTick high, Timer1 high, Timer2 low (0,0,2)
            set_systick_priority(0);
            set_irq_prio_raw(lib::hal::pac::Interrupt::TC4, 0);
            set_irq_prio_raw(lib::hal::pac::Interrupt::TC5, 2);
        } else if cfg!(feature = "priority-mixed-2") {
            // SysTick high, Timer1 med, Timer2 low (0,1,2)
            set_systick_priority(0);
            set_irq_prio_raw(lib::hal::pac::Interrupt::TC4, 1);
            set_irq_prio_raw(lib::hal::pac::Interrupt::TC5, 2);
        } else if cfg!(feature = "priority-mixed-3") {
            // SysTick high, Timer1 low, Timer2 med (0,2,1)
            set_systick_priority(0);
            set_irq_prio_raw(lib::hal::pac::Interrupt::TC4, 2);
            set_irq_prio_raw(lib::hal::pac::Interrupt::TC5, 1);
        } else if cfg!(feature = "priority-timers-high") {
            // Timers high, SysTick low (2,0,0)
            set_systick_priority(2);
            set_irq_prio_raw(lib::hal::pac::Interrupt::TC4, 0);
            set_irq_prio_raw(lib::hal::pac::Interrupt::TC5, 0);
        } else {
            // Default: all equal priority (1,1,1)
            set_systick_priority(1);
            set_irq_prio_raw(lib::hal::pac::Interrupt::TC4, 1);
            set_irq_prio_raw(lib::hal::pac::Interrupt::TC5, 1);
        }
    }
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

    // Configure interrupt priorities before enabling interrupts
    configure_interrupt_priorities();

    // Enable TC4 and TC5 interrupts
    unsafe {
        NVIC::unmask(lib::hal::pac::Interrupt::TC4);
        NVIC::unmask(lib::hal::pac::Interrupt::TC5);
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

#![no_main]
#![no_std]

use stm32f412_nucleo as lib;

use core::sync::atomic::{AtomicU32, Ordering};
use cortex_m::peripheral::NVIC;
use hal::rcc::Config;
use lib::hal::{self, interrupt, prelude::*, timer::Timer as HalTimer};
use rtt_target::{rprintln, rtt_init_log};
use systick_timer::Timer;

const TICK_RESOLUTION: u64 = 1_000_000_000; // 1 GHz, 1 ns resolution

#[cfg(feature = "reload-small")]
static TIMER: Timer = Timer::new(TICK_RESOLUTION, 0x3FF, 100_000_000);
#[cfg(not(feature = "reload-small"))]
static TIMER: Timer = Timer::new(TICK_RESOLUTION, 0xFFFFFF, 100_000_000);
static TIM2_COUNTER: AtomicU32 = AtomicU32::new(0);
static TIM5_COUNTER: AtomicU32 = AtomicU32::new(0);

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
pub fn configure_interrupt_priorities() {
    unsafe {
        if cfg!(feature = "priority-equal") {
            // All equal priority (1,1,1)
            set_systick_priority(1);
            set_irq_prio_raw(hal::pac::Interrupt::TIM2, 1);
            set_irq_prio_raw(hal::pac::Interrupt::TIM5, 1);
        } else if cfg!(feature = "priority-shh") {
            // SysTick high, TIM2&5 high (0,1,1)
            set_systick_priority(0);
            set_irq_prio_raw(hal::pac::Interrupt::TIM2, 1);
            set_irq_prio_raw(hal::pac::Interrupt::TIM5, 1);
        } else if cfg!(feature = "priority-smh") {
            // SysTick high, TIM2 med, TIM5 high (0,1,0)
            set_systick_priority(0);
            set_irq_prio_raw(hal::pac::Interrupt::TIM2, 1);
            set_irq_prio_raw(hal::pac::Interrupt::TIM5, 0);
        } else if cfg!(feature = "priority-shl") {
            // SysTick high, TIM2 high, TIM5 low (0,0,2)
            set_systick_priority(0);
            set_irq_prio_raw(hal::pac::Interrupt::TIM2, 0);
            set_irq_prio_raw(hal::pac::Interrupt::TIM5, 2);
        } else if cfg!(feature = "priority-sml") {
            // SysTick high, TIM2 med, TIM5 low (0,1,2)
            set_systick_priority(0);
            set_irq_prio_raw(hal::pac::Interrupt::TIM2, 1);
            set_irq_prio_raw(hal::pac::Interrupt::TIM5, 2);
        } else if cfg!(feature = "priority-slm") {
            // SysTick high, TIM2 low, TIM5 med (0,2,1)
            set_systick_priority(0);
            set_irq_prio_raw(hal::pac::Interrupt::TIM2, 2);
            set_irq_prio_raw(hal::pac::Interrupt::TIM5, 1);
        } else if cfg!(feature = "priority-sll") {
            // SysTick high, TIM2&5 low (0,2,2)
            set_systick_priority(0);
            set_irq_prio_raw(hal::pac::Interrupt::TIM2, 2);
            set_irq_prio_raw(hal::pac::Interrupt::TIM5, 2);
        } else if cfg!(feature = "priority-reverse") {
            // TIM2&5 high, SysTick low (2,0,0)
            set_systick_priority(2);
            set_irq_prio_raw(hal::pac::Interrupt::TIM2, 0);
            set_irq_prio_raw(hal::pac::Interrupt::TIM5, 0);
        } else {
            // Default: all equal priority (1,1,1)
            set_systick_priority(1);
            set_irq_prio_raw(hal::pac::Interrupt::TIM2, 1);
            set_irq_prio_raw(hal::pac::Interrupt::TIM5, 1);
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

    rprintln!("RTT Plus Test Starting with 100MHz clock...");

    // Report active configuration - use runtime detection
    if cfg!(feature = "freq-50-49999") {
        rprintln!("Frequency config: TIM2=50kHz, TIM5=49.999kHz");
    } else if cfg!(feature = "freq-50-50001") {
        rprintln!("Frequency config: TIM2=50kHz, TIM5=50.001kHz");
    } else {
        rprintln!("Frequency config: TIM2=50kHz, TIM5=50kHz");
    }

    if cfg!(feature = "block-both") {
        rprintln!("Blocking config: Both use critical_section");
    } else if cfg!(feature = "block-tim2") {
        rprintln!("Blocking config: TIM2 uses critical_section");
    } else if cfg!(feature = "block-tim5") {
        rprintln!("Blocking config: TIM5 uses critical_section");
    } else {
        rprintln!("Blocking config: No critical sections");
    }

    if cfg!(feature = "duration-full") {
        rprintln!("Duration config: 45+ second test (overflow detection)");
    } else {
        rprintln!("Duration config: 5 second test");
    }

    if cfg!(feature = "reload-small") {
        rprintln!("Reload config: Small (0x3FF, ~40s to overflow)");
    } else {
        rprintln!("Reload config: Normal (0xFFFFFF, ~51h to overflow)");
    }

    // Report interrupt priority configuration
    if cfg!(feature = "priority-equal") {
        rprintln!("Priority config: All equal (SysTick=1, TIM2=1, TIM5=1)");
    } else if cfg!(feature = "priority-shh") {
        rprintln!("Priority config: SysTick high (SysTick=0, TIM2=1, TIM5=1)");
    } else if cfg!(feature = "priority-smh") {
        rprintln!("Priority config: SysTick high, mixed (SysTick=0, TIM2=1, TIM5=0)");
    } else if cfg!(feature = "priority-shl") {
        rprintln!("Priority config: SysTick high, mixed (SysTick=0, TIM2=0, TIM5=2)");
    } else if cfg!(feature = "priority-sml") {
        rprintln!("Priority config: SysTick high, mixed (SysTick=0, TIM2=1, TIM5=2)");
    } else if cfg!(feature = "priority-slm") {
        rprintln!("Priority config: SysTick high, mixed (SysTick=0, TIM2=2, TIM5=1)");
    } else if cfg!(feature = "priority-sll") {
        rprintln!("Priority config: SysTick high, others low (SysTick=0, TIM2=2, TIM5=2)");
    } else if cfg!(feature = "priority-reverse") {
        rprintln!("Priority config: TIM2&5 high, SysTick low (SysTick=2, TIM2=0, TIM5=0)");
    } else {
        rprintln!("Priority config: Default equal (SysTick=1, TIM2=1, TIM5=1)");
    }

    // Configure TIM2 frequency based on features
    let mut tim2 = HalTimer::new(dp.TIM2, &mut rcc).counter_hz();
    tim2.start(50.kHz()).unwrap();
    tim2.listen(hal::timer::Event::Update);

    // Configure TIM5 frequency based on features
    let mut tim5 = HalTimer::new(dp.TIM5, &mut rcc).counter_hz();
    if cfg!(feature = "freq-50-49999") {
        tim5.start(49_999.Hz()).unwrap();
    } else if cfg!(feature = "freq-50-50001") {
        tim5.start(50_001.Hz()).unwrap();
    } else {
        // Default to freq-50-50
        tim5.start(50.kHz()).unwrap();
    }
    tim5.listen(hal::timer::Event::Update);

    // Configure interrupt priorities before enabling interrupts
    configure_interrupt_priorities();

    // Enable TIM2 and TIM5 interrupts
    unsafe {
        cortex_m::peripheral::NVIC::unmask(hal::pac::Interrupt::TIM2);
        cortex_m::peripheral::NVIC::unmask(hal::pac::Interrupt::TIM5);
    }

    TIMER.start(&mut cp.SYST);
    rprintln!("All timers started");

    let start_time = TIMER.now();
    let mut last_log_time = start_time;
    let mut iteration_count = 0;
    let one_second = seconds(1);

    let test_duration = if cfg!(feature = "duration-full") {
        seconds(50) // Go past 40s overflow point
    } else {
        seconds(5) // Default to short duration
    };

    rprintln!("Starting timer loop at: {}", start_time);

    if cfg!(feature = "duration-full") {
        rprintln!("Test will run for 50 seconds (targeting 64-bit overflow at ~40s)");
    } else {
        rprintln!("Test will run for 5 seconds");
    }

    loop {
        let current_time = TIMER.now();
        let elapsed_ns = current_time - start_time;

        // Log status every second
        if current_time - last_log_time >= one_second {
            let tim2_count = TIM2_COUNTER.load(Ordering::Relaxed);
            let tim5_count = TIM5_COUNTER.load(Ordering::Relaxed);
            rprintln!(
                "Elapsed: {}s, TIM2: {} ticks, TIM5: {} ticks, iterations: {}",
                elapsed_ns / 1_000_000_000,
                tim2_count,
                tim5_count,
                iteration_count
            );
            last_log_time = current_time;
        }

        // Exit after test duration
        if elapsed_ns >= test_duration {
            if cfg!(feature = "duration-full") {
                rprintln!(
                    "Test completed after 50.000s, total iterations: {}",
                    iteration_count
                );
            } else {
                rprintln!(
                    "Test completed after 5.000s, total iterations: {}",
                    iteration_count
                );
            }
            break;
        }

        iteration_count += 1;
        // asm::wfi();
    }

    stm32f412_nucleo::exit()
}

#[cortex_m_rt::exception]
fn SysTick() {
    TIMER.systick_handler();
}

enum TimerId {
    TIM2,
    TIM5,
}
impl From<TimerId> for &'static str {
    fn from(val: TimerId) -> Self {
        match val {
            TimerId::TIM2 => "TIM2",
            TimerId::TIM5 => "TIM5",
        }
    }
}

fn check_timer_monotonic<T: Into<TimerId>>(timer_id: T, last_now: &mut u64) {
    let timer_name: &'static str = timer_id.into().into();
    let now = TIMER.now();
    if now < *last_now {
        // Diagnose the type of timing violation
        if let Some(total_missed_wraps) =
            TIMER.diagnose_timing_violation(now, *last_now, 100_000_000)
        {
            let observed_periods = total_missed_wraps - 1; // Subtract the 1 that PendST compensated for
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
                167.77
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

#[hal::interrupt]
fn TIM2() {
    static mut TIM2_LAST_NOW: u64 = 0;
    TIM2_COUNTER.fetch_add(1, Ordering::Relaxed);

    // Conditional critical section based on features
    #[cfg(any(feature = "block-tim2", feature = "block-both"))]
    critical_section::with(|_| {
        check_timer_monotonic(TimerId::TIM2, TIM2_LAST_NOW);
    });

    #[cfg(not(any(feature = "block-tim2", feature = "block-both")))]
    check_timer_monotonic(TimerId::TIM2, TIM2_LAST_NOW);

    unsafe {
        let tim2 = &*hal::pac::TIM2::ptr();
        if tim2.sr().read().uif().bit_is_set() {
            tim2.sr().write(|w| w.uif().clear());
        }
    }
}

#[hal::interrupt]
fn TIM5() {
    static mut TIM5_LAST_NOW: u64 = 0;
    TIM5_COUNTER.fetch_add(1, Ordering::Relaxed);

    // Conditional critical section based on features
    #[cfg(any(feature = "block-tim5", feature = "block-both"))]
    critical_section::with(|_| {
        check_timer_monotonic(TimerId::TIM5, TIM5_LAST_NOW);
    });

    #[cfg(not(any(feature = "block-tim5", feature = "block-both")))]
    check_timer_monotonic(TimerId::TIM5, TIM5_LAST_NOW);

    unsafe {
        let tim5 = &*hal::pac::TIM5::ptr();
        if tim5.sr().read().uif().bit_is_set() {
            tim5.sr().write(|w| w.uif().clear());
        }
    }
}

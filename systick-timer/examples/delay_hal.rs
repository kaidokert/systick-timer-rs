// SPDX-License-Identifier: Apache-2.0
#![no_std]
#![no_main]

use cortex_m_semihosting::debug::{self, EXIT_SUCCESS};
use cortex_m_semihosting::hprintln;
use embedded_hal::delay::DelayNs;
use systick_timer::Timer;

// 1 kHz output resolution, SysTick reload every 1 ms at 8 MHz.
static INSTANCE: Timer = Timer::new(1_000, 7_999, 8_000_000);

#[cortex_m_rt::entry]
fn main() -> ! {
    hprintln!("Starting 500ms DelayNs demo");
    INSTANCE.start(&mut cortex_m::Peripherals::take().unwrap().SYST);

    let mut delay = &INSTANCE;
    delay.delay_ms(500);

    hprintln!("Finished 500ms delay");
    debug::exit(EXIT_SUCCESS);
    loop {}
}

#[cortex_m_rt::exception]
fn SysTick() {
    INSTANCE.systick_handler();
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

// SPDX-License-Identifier: Apache-2.0
#![no_std]
#![no_main]

use cortex_m_semihosting::hprintln;
use systick_timer::Timer;

// Set up for micro-second resolution, reload every 100 microseconds, 8 MHz clock
static INSTANCE: Timer = Timer::new(1_000_000, 799, 8_000_000);

#[cortex_m_rt::entry]
fn main() -> ! {
    hprintln!("Initializing ..");
    INSTANCE.start(&mut cortex_m::Peripherals::take().unwrap().SYST);
    let start = INSTANCE.now();
    let stop = start + 1_000_000;

    loop {
        // Small no-op busy loop
        for _ in 0..1_000_000 {
            cortex_m::asm::delay(100)
        }
        let time2 = INSTANCE.now();
        hprintln!("Time: {}", time2 - start);
        if time2 >= stop {
            break;
        }
    }
    hprintln!("Waited for a second");
    cortex_m_semihosting::debug::exit(cortex_m_semihosting::debug::EXIT_SUCCESS);
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

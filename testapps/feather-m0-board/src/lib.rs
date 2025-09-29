#![no_main]
#![no_std]

use panic_rtt_target as _;
pub use feather_m0::hal; // HAL modules

pub fn exit() -> ! {
    cortex_m_semihosting::debug::exit(cortex_m_semihosting::debug::EXIT_SUCCESS);
    #[allow(clippy::empty_loop)]
    loop {}
}

#[cortex_m_rt::exception]
unsafe fn HardFault(_frame: &cortex_m_rt::ExceptionFrame) -> ! {
    cortex_m_semihosting::debug::exit(cortex_m_semihosting::debug::EXIT_FAILURE);
    #[allow(clippy::empty_loop)]
    loop {}
}

# Test Applications

This directory contains stress test applications for verifying timer monotonicity on embedded hardware.

## Core Concept

- Configure the SysTick timer with nanosecond resolution
- Configure 2 hardware timers to run at high frequency (~15kHz on M0+, ~50kHz on M4)
- Verify that both timer ISRs always observe monotonically increasing time values

## Test Configurations

The stress tests run in various configurations to exercise different timing scenarios:

- **Interrupt blocking**: ISRs with and without critical sections
- **Frequency variations**: Slight frequency differences between timers
- **SysTick reload values**: Normal vs. accelerated overflow testing
- **Interrupt priorities**: Different relative priorities between SysTick and timer ISRs

## Hardware Platforms

Tests are designed for different Cortex-M architectures:
- **Cortex-M0+** (SAMD21): Limited interrupt priorities and atomics
- **Cortex-M4** (STM32F412): Full interrupt priority support

Each platform has platform-specific implementations while sharing common test logic via the `timer_stress` library.

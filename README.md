### 64-bit SysTick timer for Cortex-M0

[![crate](https://img.shields.io/crates/v/systick-timer.svg)](https://crates.io/crates/systick-timer)
[![documentation](https://docs.rs/systick-timer/badge.svg)](https://docs.rs/systick-timer/)
[![Build](https://github.com/kaidokert/systick-timer-rs/actions/workflows/rust.yml/badge.svg)](https://github.com/kaidokert/systick-timer-rs/actions/workflows/rust.yml)

Implements a 64-bit SysTick based timer, that tracks
overflows and provides as single monotonic 64-bit value
at the desired resolution. The only dependencies are cortex-m
and cortex-m-rt crates.

Optionally wraps this in an embassy-time-driver.

Example included for Qemu Cortex-M0

To run the demos with Qemu:

```
cargo runq --example basic_time
```

Embassy version:

```
cargo runq --example embassy_time
```

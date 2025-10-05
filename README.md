### 64-bit SysTick timer for Cortex-M0

[![crate](https://img.shields.io/crates/v/systick-timer.svg)](https://crates.io/crates/systick-timer)
[![documentation](https://docs.rs/systick-timer/badge.svg)](https://docs.rs/systick-timer/)
[![Build](https://github.com/kaidokert/systick-timer-rs/actions/workflows/rust.yml/badge.svg)](https://github.com/kaidokert/systick-timer-rs/actions/workflows/rust.yml)

Implements a 64-bit SysTick based timer, that tracks
overflows and provides as single monotonic 64-bit value
at the desired resolution. The only dependencies are [cortex-m](https://crates.io/crates/cortex-m)
and [cortex-m-rt](https://crates.io/crates/cortex-m-rt) crates.

Optionally wraps this in an [embassy-time-driver](https://crates.io/crates/embassy-time-driver).

## ⚠️ Critical Design Constraint

**The SysTick ISR must not be starved for more than one wrap period.**

This timer implementation is designed to handle exactly **one missed SysTick wrap** through its PendST bit detection mechanism. If the SysTick ISR is delayed by higher-priority interrupts for more than one complete wrap period, monotonic time violations **will occur**.

**Reminders:**
- Keep the SysTick ISR at higher or equal priority relative to application interrupts
- Ensure critical sections in higher-priority ISRs are brief, e.g. well shorter than SysTick reload interval

The implementation itself is non-locking and does not use critical sections.


[Examples included](https://github.com/kaidokert/systick-timer-rs/tree/main/examples) for QEMU Cortex-M0.

To run the demos with QEMU:

```
cargo runq --example basic_time
```

Embassy version:

```
cargo runq --example embassy_time
```

## Testing

To run the tests, run without default features:
```sh
cargo test --no-default-features

## Todo:

Add a constant-execution time option to now(). This would trade off
slower average access to deterministic, worst-case execution.

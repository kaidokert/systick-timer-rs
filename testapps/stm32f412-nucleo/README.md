# STM32F412 Nucleo Test Application

This test application provides comprehensive testing capabilities for the systick-timer library, specifically designed to reproduce and validate timer behavior under various stress conditions.

## Configuration Matrix

The test application supports multiple feature-gated configurations to systematically test different scenarios:

### Mutually Exclusive Configuration Groups

#### 1. Timer Frequency Configurations (3 options)
- `freq-50-50`: TIM2=50kHz, TIM5=50kHz (baseline)
- `freq-50-49999`: TIM2=50kHz, TIM5=49.999kHz (slight frequency mismatch)
- `freq-50-50001`: TIM2=50kHz, TIM5=50.001kHz (slight frequency mismatch)

#### 2. Interrupt Blocking Configurations (4 options)
- `block-none`: No critical sections (default behavior)
- `block-tim2`: TIM2 interrupt uses `critical_section::with()`
- `block-tim5`: TIM5 interrupt uses `critical_section::with()`
- `block-both`: Both TIM2 and TIM5 use critical sections

#### 3. Test Duration Configurations (2 options)
- `duration-short`: 5 second test (quick verification)
- `duration-full`: 50 second test (targets 64-bit overflow conditions)

#### 4. Timer Reload Configurations (2 options)
- `reload-normal`: 0xFFFFFF reload (~51 hours to overflow)
- `reload-small`: 0x3FF reload (~40 seconds to overflow at 100MHz)

### Total Combinations
**3 × 4 × 2 × 2 = 48 different test configurations**

## Usage

### Building with Specific Configurations

Use `--no-default-features` and explicitly specify desired features:

```bash
# Quick baseline test
cargo build --bin rtt_plus --no-default-features --features freq-50-50,block-none,duration-short,reload-normal

# Test critical section blocking
cargo build --bin rtt_plus --no-default-features --features freq-50-49999,block-both,duration-short,reload-normal

# Test overflow conditions (accelerated)
cargo build --bin rtt_plus --no-default-features --features freq-50-50,block-none,duration-full,reload-small

# Stress test: frequency mismatch + blocking + overflow
cargo build --bin rtt_plus --no-default-features --features freq-50-49999,block-both,duration-full,reload-small
```

### Running Tests

```bash
# Build and run specific configuration
cargo run --bin rtt_plus --no-default-features --features freq-50-50,block-none,duration-short,reload-normal

# Or build first, then run
cargo build --bin rtt_plus --no-default-features --features [your-features]
probe-rs run --chip STM32F412ZE target/thumbv7em-none-eabihf/debug/rtt_plus
```

## Test Objectives

This test suite is designed to reproduce conditions described in `bug.md`:

1. **Race conditions**: Multiple high-frequency interrupts accessing the systick timer
2. **Critical section interference**: Testing how interrupt blocking affects timer accuracy
3. **Frequency beat patterns**: Slightly mismatched timer frequencies creating interference
4. **64-bit overflow edge cases**: Using small reload values to accelerate overflow conditions
5. **Monotonic violations**: Detecting when timer values go backwards

## Test Output

The application provides real-time RTT output showing:
- Active configuration (frequency, blocking, duration, reload)
- Per-second status updates with timer tick counts
- Monotonic violation detection and reporting
- Final statistics after test completion

## Systematic Testing

For comprehensive validation, test key combinations focusing on:
- **Basic functionality**: All frequency configurations with baseline settings
- **Blocking behavior**: All blocking combinations with standard frequency
- **Overflow conditions**: Critical combinations with accelerated overflow
- **Stress scenarios**: Worst-case combinations likely to trigger bugs

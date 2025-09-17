use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

// Use a small reload value to ensure wraps happen very frequently,
// maximizing the chances of hitting a race condition.
const RELOAD: u32 = 1000;
const TEST_DURATION_MS: u64 = 2000; // 2 seconds

#[test]
fn monotonicity_stress_test() {
    // The timer is the central piece of state, shared across all threads.
    // The test-only fields `current_systick` and `systick_has_wrapped`
    // will serve as our mock hardware registers.
    let timer = Arc::new(Timer::new(1_000_000, RELOAD, 48_000_000));
    timer.set_syst(RELOAD);

    // A shared flag to signal all threads to stop.
    let stop_signal = Arc::new(AtomicBool::new(false));

    // --- Thread 1: The "Hardware Clock" Simulator ---
    // This thread's job is to make the clock tick down.
    let timer_hw = timer.clone();
    let stop_hw = stop_signal.clone();
    let hw_thread = thread::spawn(move || {
        while !stop_hw.load(Ordering::Relaxed) {
            let current_val = timer_hw.get_syst();
            if current_val > 0 {
                timer_hw.set_syst(current_val - 1);
            } else {
                // We've hit 0, time to wrap.
                timer_hw.set_syst(RELOAD);
                // Signal that a wrap occurred. The ISR thread will pick this up.
                timer_hw.set_systick_has_wrapped(true);
            }
            // Sleep for a tiny duration to simulate the clock speed.
            thread::sleep(Duration::from_nanos(100));
        }
    });

    // --- Thread 2: The "ISR" (Interrupt Service Routine) Simulator ---
    // This thread simulates the SysTick interrupt firing when it sees the COUNTFLAG.
    let timer_isr = timer.clone();
    let stop_isr = stop_signal.clone();
    let isr_thread = thread::spawn(move || {
        while !stop_isr.load(Ordering::Relaxed) {
            // In hardware, reading the COUNTFLAG clears it. The test helper does the same.
            // If it was true, it means we should run the handler.
            if timer_isr.read_systick_countflag() {
                timer_isr.systick_handler();
            }
            // Sleep for a tiny, slightly variable duration to make the timing unpredictable.
            thread::sleep(Duration::from_micros(1));
        }
    });

    // --- Thread 3: The "Application" / Monotonicity Checker ---
    // This is the actual test. It calls now() repeatedly and ensures
    // that the returned time never goes backward.
    let timer_app = timer.clone();
    let stop_app = stop_signal.clone();
    let app_thread = thread::spawn(move || {
        let mut last_seen_time = 0;
        let mut iterations = 0;
        while !stop_app.load(Ordering::Relaxed) {
            let current_time = timer_app.now();
            assert!(
                current_time >= last_seen_time,
                "Monotonicity failed! current: {}, last: {}",
                current_time,
                last_seen_time
            );
            last_seen_time = current_time;
            iterations += 1;
        }
        println!("Checker thread completed {} iterations.", iterations);
    });

    // Let the threads run for the specified duration.
    println!("Running stress test for {}ms...", TEST_DURATION_MS);
    thread::sleep(Duration::from_millis(TEST_DURATION_MS));

    // Signal all threads to stop and wait for them to finish.
    stop_signal.store(true, Ordering::Relaxed);
    hw_thread.join().unwrap();
    isr_thread.join().unwrap();
    app_thread.join().unwrap();

    println!("Stress test passed.");
}

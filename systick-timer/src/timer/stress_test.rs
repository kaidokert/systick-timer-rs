use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

// Use a small reload value to ensure wraps happen very frequently,
// maximizing the chances of hitting a race condition.
const RELOAD: u32 = 1000;
const TEST_DURATION_MS: u64 = 2000; // 2 seconds

#[test]
fn monotonicity_stress_test() {
    // The timer is the central piece of state, shared across all threads.
    // A Mutex is used here to simulate a critical section, preventing the
    // "ISR" from preempting the "Application" during a `now()` call.
    let timer = Arc::new(Mutex::new(Timer::new(1_000_000, RELOAD, 48_000_000)));
    timer.lock().unwrap().set_syst(RELOAD);

    // A shared flag to signal all threads to stop.
    let stop_signal = Arc::new(AtomicBool::new(false));

    // --- Thread 1: The "Hardware Clock" Simulator ---
    // This thread's job is to make the clock tick down.
    let timer_hw = timer.clone();
    let stop_hw = stop_signal.clone();
    let hw_thread = thread::spawn(move || {
        while !stop_hw.load(Ordering::Relaxed) {
            // Lock the timer to modify its state
            let timer_guard = timer_hw.lock().unwrap();
            let current_val = timer_guard.get_syst();
            if current_val > 0 {
                timer_guard.set_syst(current_val - 1);
            } else {
                // We've hit 0, time to wrap.
                timer_guard.set_syst(RELOAD);
                // Signal that the ISR is pending. The ISR thread will pick this up.
                timer_guard.set_pendst_pending(true);
            }
            // Drop the lock by letting timer_guard go out of scope
            drop(timer_guard);
            // Sleep for a tiny duration to simulate the clock speed.
            thread::sleep(Duration::from_nanos(100));
        }
    });

    // --- Thread 2: The "ISR" (Interrupt Service Routine) Simulator ---
    let timer_isr = timer.clone();
    let stop_isr = stop_signal.clone();
    let isr_thread = thread::spawn(move || {
        while !stop_isr.load(Ordering::Relaxed) {
            // Lock the timer to check and handle the pending interrupt atomically.
            let timer_guard = timer_isr.lock().unwrap();
            if timer_guard.is_systick_pending() {
                timer_guard.set_pendst_pending(false); // ISR clears the pending bit
                timer_guard.systick_handler();
            }
            drop(timer_guard);
            // Sleep for a tiny, slightly variable duration to make the timing unpredictable.
            thread::sleep(Duration::from_micros(1));
        }
    });

    // --- Thread 3: The "Application" / Monotonicity Checker ---
    let timer_app = timer.clone();
    let stop_app = stop_signal.clone();
    let app_thread = thread::spawn(move || {
        let mut last_seen_time = 0;
        let mut iterations = 0;
        while !stop_app.load(Ordering::Relaxed) {
            // Lock the timer to call now(), ensuring the ISR can't run in the middle.
            let current_time = timer_app.lock().unwrap().now();
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

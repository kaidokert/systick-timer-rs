#!/usr/bin/env python3
"""
Comprehensive test runner for STM32F412 Nucleo systick timer tests.
"""

import subprocess
import sys
import time
import logging
import os
import argparse
import threading
import queue
import select
from datetime import datetime
from itertools import product
from dataclasses import dataclass
from typing import List, Tuple, Optional

@dataclass
class TestResult:
    config: str
    test_id: str
    success: bool
    duration: float
    log_file: str = ""
    output: str = ""
    error: str = ""

class TestRunner:
    def __init__(self, board_dir: str, specific_test_id: Optional[str] = None, release_mode: bool = True, include_invalid: bool = False, build_only: bool = False):
        # Configuration options (mutually exclusive groups)
        self.freq_options = ["freq-target-target", "freq-target-below", "freq-target-above"]
        self.block_options = ["block-none", "block-timer1", "block-timer2", "block-both"]
        self.duration_options = ["duration-short", "duration-full"]
        self.reload_options = ["reload-normal", "reload-small"]
        self.priority_options = ["priority-equal", "priority-systick-high", "priority-timer1-high", "priority-timer2-high", "priority-mixed-1", "priority-mixed-2", "priority-mixed-3", "priority-timers-high"]

        # Invalid configurations that violate design constraints
        self.invalid_priorities = ["priority-timers-high"]

        self.results: List[TestResult] = []
        self.failed_tests: List[TestResult] = []
        self.board_dir = board_dir
        self.specific_test_id = specific_test_id
        self.release_mode = release_mode
        self.include_invalid = include_invalid
        self.build_only = build_only

        # Create logs directory
        os.makedirs("logs", exist_ok=True)

        # Setup logging
        self.setup_logging()

    def setup_logging(self):
        """Setup logging to both stdout and file."""
        timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
        log_filename = f"systick_test_results_{timestamp}.log"

        # Create logger
        logger = logging.getLogger()
        logger.setLevel(logging.INFO)

        # Create formatter
        formatter = logging.Formatter('%(asctime)s - %(levelname)s - %(message)s')

        # Console handler (stdout)
        console_handler = logging.StreamHandler(sys.stdout)
        console_handler.setLevel(logging.INFO)
        console_handler.setFormatter(formatter)

        # File handler
        file_handler = logging.FileHandler(log_filename)
        file_handler.setLevel(logging.INFO)
        file_handler.setFormatter(formatter)

        # Add handlers to logger
        logger.addHandler(console_handler)
        logger.addHandler(file_handler)

        self.logger = logger
        self.log_filename = log_filename

    def is_config_invalid(self, config: Tuple[str, str, str, str, str]) -> bool:
        """Check if a configuration violates design constraints."""
        freq, block, duration, reload, priority = config

        # Priority-reverse always invalid (SysTick lower priority)
        if priority in self.invalid_priorities:
            return True

        # Debug mode: critical sections cause excessive overhead
        return not self.release_mode and block != "block-none"

    def generate_all_configs(self) -> List[Tuple[str, str, str, str, str]]:
        """Generate all possible feature combinations, optionally filtering invalid ones."""
        all_configs = list(product(
            self.freq_options,
            self.block_options,
            self.duration_options,
            self.reload_options,
            self.priority_options
        ))

        if self.include_invalid or self.specific_test_id:
            # Include all configs when explicitly requested or testing specific ID
            return all_configs
        else:
            # Filter out invalid configs by default
            return [config for config in all_configs if not self.is_config_invalid(config)]

    def config_to_string(self, config: Tuple[str, str, str, str, str]) -> str:
        """Convert config tuple to feature string."""
        return ",".join(config)

    def config_to_id(self, config: Tuple[str, str, str, str, str]) -> str:
        """Convert config tuple to short meaningful ID."""
        freq, block, duration, reload, priority = config

        # Frequency mapping: target-target -> A, target-below -> B, target-above -> C
        freq_map = {"freq-target-target": "A", "freq-target-below": "B", "freq-target-above": "C"}

        # Block mapping: none -> N, timer1 -> 1, timer2 -> 2, both -> B
        block_map = {"block-none": "N", "block-timer1": "1", "block-timer2": "2", "block-both": "B"}

        # Duration mapping: short -> S, full -> F
        duration_map = {"duration-short": "S", "duration-full": "F"}

        # Reload mapping: normal -> N, small -> S
        reload_map = {"reload-normal": "N", "reload-small": "S"}

        # Priority mapping: compact single character codes
        priority_map = {
            "priority-equal": "E",         # All equal
            "priority-systick-high": "S",  # SysTick high, timers med
            "priority-timer1-high": "1",   # Timer1 high, others med
            "priority-timer2-high": "2",   # Timer2 high, others med
            "priority-mixed-1": "M",       # SysTick high, Timer1 high, Timer2 low
            "priority-mixed-2": "L",       # SysTick high, Timer1 med, Timer2 low
            "priority-mixed-3": "R",       # SysTick high, Timer1 low, Timer2 med
            "priority-timers-high": "X"     # Timers high, SysTick low (INVALID)
        }

        base_id = f"{freq_map[freq]}{block_map[block]}{duration_map[duration]}{reload_map[reload]}{priority_map[priority]}"
        return f"{base_id}-R" if self.release_mode else base_id

    def run_single_test(self, config: Tuple[str, str, str, str, str]) -> TestResult:
        """Run a single test configuration."""
        config_str = self.config_to_string(config)
        test_id = self.config_to_id(config)

        # Warn about invalid configurations
        if self.is_config_invalid(config):
            self.logger.warning(f"Testing INVALID configuration (violates design constraints): {config_str} [ID: {test_id}]")
            self.logger.warning("This test is expected to fail catastrophically - it demonstrates ISR starvation limits")
        else:
            self.logger.info(f"Testing: {config_str} [ID: {test_id}]")

        # Command selection based on build_only flag
        if self.build_only:
            run_cmd = [
                "cargo", "build", "--bin", "timer_stress",
                "--no-default-features",
                "--features", config_str
            ]
        else:
            run_cmd = [
                "cargo", "run", "--bin", "timer_stress",
                "--no-default-features",
                "--features", config_str
            ]
        if self.release_mode:
            run_cmd.append("--release")

        start_time = time.time()
        timestamp = datetime.now().strftime("%H_%M_%S")
        log_filename = f"logs/{timestamp}_{test_id}.log"
        result = TestResult(config_str, test_id, False, 0.0, log_filename)

        try:
            if self.build_only:
                self.logger.info("  Building configuration...")
            else:
                self.logger.info("  Running test...")
            self.logger.info(f"    Command: {' '.join(run_cmd)}")

            # Run phase with real-time output streaming
            stdout_lines = []
            stderr_lines = []
            return_code = None

            try:
                with subprocess.Popen(
                    run_cmd,
                    cwd=self.board_dir,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    text=True,
                    bufsize=1,  # Line buffered
                    universal_newlines=True
                ) as process:

                    # Create threads to read stdout and stderr
                    def read_stream(stream, line_list, stream_name):
                        try:
                            for line in iter(stream.readline, ''):
                                if line:
                                    line_clean = line.rstrip()
                                    line_list.append(line_clean)
                                    # Print with prefix to distinguish streams
                                    if stream_name == 'STDOUT':
                                        print(f"[{test_id}] {line_clean}")
                                    else:
                                        print(f"[{test_id}] ERROR: {line_clean}")
                        except Exception as e:
                            print(f"[{test_id}] Stream read error ({stream_name}): {e}")

                    stdout_thread = threading.Thread(
                        target=read_stream,
                        args=(process.stdout, stdout_lines, 'STDOUT')
                    )
                    stderr_thread = threading.Thread(
                        target=read_stream,
                        args=(process.stderr, stderr_lines, 'STDERR')
                    )

                    stdout_thread.start()
                    stderr_thread.start()

                    # Wait for process with timeout
                    try:
                        return_code = process.wait(timeout=120)  # Max 2 minutes per test
                    except subprocess.TimeoutExpired:
                        print(f"[{test_id}] TIMEOUT - Killing process...")
                        process.kill()
                        return_code = -1

                    # Wait for threads to finish reading remaining output
                    stdout_thread.join(timeout=1.0)
                    stderr_thread.join(timeout=1.0)

            except Exception as e:
                print(f"[{test_id}] Process error: {e}")
                return_code = -1

            result.duration = time.time() - start_time
            result.output = '\n'.join(stdout_lines)
            result.error = '\n'.join(stderr_lines)

            # Save full output to individual log file
            try:
                with open(log_filename, 'w') as f:
                    f.write(f"Test ID: {test_id}\n")
                    f.write(f"Config: {config_str}\n")
                    f.write(f"Build Mode: {'Release' if self.release_mode else 'Debug'}\n")
                    f.write(f"Duration: {result.duration:.1f}s\n")
                    f.write(f"Return code: {return_code}\n")
                    f.write("=" * 80 + "\n")
                    f.write("STDOUT:\n")
                    f.write(result.output)
                    f.write("\n" + "=" * 80 + "\n")
                    f.write("STDERR:\n")
                    f.write(result.error)
                self.logger.info(f"  Full log saved to: {log_filename}")
            except Exception as e:
                self.logger.warning(f"  Failed to save log file: {e}")

            # Check for success indicators
            if self.build_only:
                # For build-only, success is just a clean build
                if return_code == 0:
                    result.success = True
                    self.logger.info(f"  BUILD SUCCESS ({result.duration:.1f}s)")
                else:
                    result.success = False
                    self.logger.warning(f"  BUILD FAILED ({result.duration:.1f}s)")
                    if return_code != 0:
                        self.logger.warning(f"    Non-zero exit code: {return_code}")
            else:
                # For run mode, check for test completion
                if ("Test completed" in result.output and
                    "Timer monotonic violation" not in result.output and
                    return_code == 0):
                    result.success = True
                    self.logger.info(f"  SUCCESS ({result.duration:.1f}s)")
                else:
                    self.logger.warning(f"  COMPLETED WITH WARNINGS ({result.duration:.1f}s)")
                    if "Timer monotonic violation" in result.output:
                        self.logger.warning("    Monotonic violation detected!")
                    if return_code != 0:
                        self.logger.warning(f"    Non-zero exit code: {return_code}")
                    result.success = False

        except subprocess.TimeoutExpired as e:
            result.duration = time.time() - start_time
            result.error = "Test timed out (this shouldn't happen with new streaming implementation)"
            self.logger.error(f"  TIMEOUT after {result.duration:.1f}s")

            # Save timeout info to log file
            try:
                with open(log_filename, 'w') as f:
                    f.write(f"Test ID: {test_id}\n")
                    f.write(f"Config: {config_str}\n")
                    f.write(f"Build Mode: {'Release' if self.release_mode else 'Debug'}\n")
                    f.write(f"Duration: {result.duration:.1f}s (TIMEOUT)\n")
                    f.write("=" * 80 + "\n")
                    f.write("TIMEOUT - Test exceeded 120 seconds\n")
            except Exception as log_e:
                self.logger.warning(f"  Failed to save timeout log: {log_e}")

        except Exception as e:
            result.duration = time.time() - start_time
            result.error = f"Exception: {str(e)}"
            result.output = f"Exception occurred: {str(e)}"
            self.logger.error(f"  EXCEPTION: {str(e)}")

            # Save exception info to log file
            try:
                with open(log_filename, 'w') as f:
                    f.write(f"Test ID: {test_id}\n")
                    f.write(f"Config: {config_str}\n")
                    f.write(f"Build Mode: {'Release' if self.release_mode else 'Debug'}\n")
                    f.write(f"Duration: {result.duration:.1f}s (EXCEPTION)\n")
                    f.write(f"Return code: N/A\n")
                    f.write("=" * 80 + "\n")
                    f.write(f"EXCEPTION: {str(e)}\n")
            except Exception as log_e:
                self.logger.warning(f"  Failed to save exception log: {log_e}")

        return result

    def find_config_by_id(self, test_id: str) -> Optional[Tuple[str, str, str, str, str]]:
        """Find configuration by test ID."""
        all_configs = self.generate_all_configs()
        for config in all_configs:
            if self.config_to_id(config) == test_id:
                return config
        return None

    def run_all_tests(self):
        """Run all test configurations in order."""
        if self.specific_test_id:
            # Run only the specific test
            config = self.find_config_by_id(self.specific_test_id)
            if not config:
                self.logger.error(f"Test ID '{self.specific_test_id}' not found!")
                self.logger.info("Available test IDs:")
                all_configs = self.generate_all_configs()
                for cfg in sorted(all_configs):
                    test_id = self.config_to_id(cfg)
                    self.logger.info(f"  {test_id}: {self.config_to_string(cfg)}")
                return

            self.logger.info(f"Running single test: {self.specific_test_id}")
            self.logger.info(f"Config: {self.config_to_string(config)}")
            result = self.run_single_test(config)
            self.results.append(result)
            if not result.success:
                self.failed_tests.append(result)
            self.print_summary()
            return

        all_configs = self.generate_all_configs()

        # Show filtering information
        if not self.include_invalid:
            all_configs_with_invalid = list(product(
                self.freq_options, self.block_options, self.duration_options,
                self.reload_options, self.priority_options
            ))
            invalid_count = len([c for c in all_configs_with_invalid if self.is_config_invalid(c)])
            self.logger.info(f"Filtering out {invalid_count} invalid configurations (use --include-invalid to test them)")
            build_mode = "release" if self.release_mode else "debug"
            if not self.release_mode:
                self.logger.info(f"Debug mode: Excluding blocking configs (critical_section overhead causes ISR starvation)")
            self.logger.info(f"Always excluded: priority-timers-high (violates design constraints in {build_mode} mode)")

        # Sort to run short-duration tests first
        all_configs.sort(key=lambda x: (
            0 if x[2] == "duration-short" else 1,  # duration first
            x[0], x[1], x[3]  # then other options
        ))

        self.logger.info(f"Running {len(all_configs)} test configurations...")
        self.logger.info(f"Test order: short-duration tests first, then full-duration tests")
        self.logger.info(f"Log file: {self.log_filename}")

        for i, config in enumerate(all_configs, 1):
            self.logger.info(f"{'='*80}")
            self.logger.info(f"Test {i}/{len(all_configs)}")

            result = self.run_single_test(config)
            self.results.append(result)

            if not result.success:
                self.failed_tests.append(result)

            # Brief pause between tests
            time.sleep(1)

        self.print_summary()

    def print_summary(self):
        """Print test results summary."""
        self.logger.info(f"{'='*80}")
        self.logger.info("TEST SUMMARY")
        self.logger.info(f"{'='*80}")

        total = len(self.results)
        passed = sum(r.success for r in self.results)
        failed = total - passed

        self.logger.info(f"Total tests: {total}")
        self.logger.info(f"Passed: {passed}")
        self.logger.info(f"Failed: {failed}")
        self.logger.info(f"Success rate: {passed/total*100:.1f}%")

        if self.failed_tests:
            self.logger.info("FAILED TESTS:")
            for result in self.failed_tests:
                self.logger.info(f"  FAILED: {result.config} [ID: {result.test_id}]")
                if result.log_file:
                    self.logger.info(f"     Log: {result.log_file}")
                if result.error:
                    self.logger.info(f"     Error: {result.error[:100]}...")

        # Duration analysis
        short_tests = [r for r in self.results if "duration-short" in r.config]
        full_tests = [r for r in self.results if "duration-full" in r.config]

        if short_tests:
            avg_short = sum(r.duration for r in short_tests) / len(short_tests)
            self.logger.info(f"Average short test duration: {avg_short:.1f}s")

        if full_tests:
            avg_full = sum(r.duration for r in full_tests) / len(full_tests)
            self.logger.info(f"Average full test duration: {avg_full:.1f}s")

        total_time = sum(r.duration for r in self.results)
        self.logger.info(f"Total test time: {total_time/60:.1f} minutes")

        self.logger.info(f"Complete log saved to: {self.log_filename}")

def main():
    """Main entry point."""
    parser = argparse.ArgumentParser(
        description="Systick Timer - Comprehensive Test Runner (STM32F412/SAMD21)",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""Test ID Format: [FREQ][BLOCK][DURATION][RELOAD][PRIORITY]
  FREQ: A=target-target, B=target-below, C=target-above
  BLOCK: N=none, 1=timer1, 2=timer2, B=both
  DURATION: S=short, F=full
  RELOAD: N=normal, S=small
  PRIORITY: E=equal, S=systick-high, 1=timer1-high, 2=timer2-high, M/L/R=mixed, X=timers-high (INVALID)

Examples:
  ANSNE = freq-target-target,block-none,duration-short,reload-normal,priority-equal
  BFFSX = freq-target-below,block-both,duration-full,reload-small,priority-timers-high (INVALID - skipped by default)
  CBSNS = freq-target-above,block-both,duration-short,reload-normal,priority-systick-high

Total combinations: 3×4×2×2×8 = 384 tests
Valid combinations (release mode default): 3×4×2×2×7 = 336 tests
Valid combinations (debug mode): 3×1×2×2×7 = 84 tests (excludes blocking configs)
Invalid configurations are skipped by default. Use --include-invalid to test them anyway.
"""
    )
    parser.add_argument(
        "--test-id",
        help="Run specific test by ID (e.g., ANSN, BFFS)"
    )
    parser.add_argument(
        "--list-tests",
        action="store_true",
        help="List all available test IDs and exit"
    )
    parser.add_argument(
        "--debug",
        action="store_true",
        help="Build and run in debug mode (default: release mode)"
    )
    parser.add_argument(
        "--include-invalid",
        action="store_true",
        help="Include invalid configurations that violate design constraints (e.g., priority-timers-high)"
    )
    parser.add_argument(
        "--build-only",
        action="store_true",
        help="Only build configurations, don't run tests (faster verification of all combinations)"
    )
    parser.add_argument(
        "--board",
        choices=["stm32f412-nucleo", "feather-m0-board", "auto"],
        default="auto",
        help="Target board (auto-detect by default)"
    )

    args = parser.parse_args()

    # Initial setup without logging
    print("Systick Timer - Comprehensive Test Runner (STM32F412/SAMD21)")
    print("="*80)

    # Detect or validate board
    def detect_board():
        """Auto-detect which board we're working with."""
        if os.path.exists("stm32f412-nucleo/Cargo.toml"):
            return "stm32f412-nucleo"
        elif os.path.exists("feather-m0-board/Cargo.toml"):
            return "feather-m0-board"
        else:
            return None

    if args.board == "auto":
        board_dir = detect_board()
        if not board_dir:
            print("ERROR: Could not auto-detect board. Please run from testapps directory or specify --board")
            print("Available boards: stm32f412-nucleo, feather-m0-board")
            sys.exit(1)
    else:
        board_dir = args.board

    # Validate board directory exists
    if not os.path.exists(board_dir):
        print(f"ERROR: Board directory '{board_dir}' not found")
        sys.exit(1)

    board_cargo_toml = os.path.join(board_dir, "Cargo.toml")
    if not os.path.exists(board_cargo_toml):
        print(f"ERROR: Cargo.toml not found in {board_dir}")
        sys.exit(1)

    print(f"Using board: {board_dir}")
    print()

    runner = TestRunner(board_dir, args.test_id, not args.debug, args.include_invalid, args.build_only)

    if args.list_tests:
        print("Available test IDs:")
        all_configs = runner.generate_all_configs()
        for config in sorted(all_configs):
            test_id = runner.config_to_id(config)
            config_str = runner.config_to_string(config)
            print(f"  {test_id}: {config_str}")
        sys.exit(0)

    try:
        runner.run_all_tests()
    except KeyboardInterrupt:
        runner.logger.info("Test run interrupted by user")
        runner.print_summary()
        sys.exit(1)

if __name__ == "__main__":
    main()

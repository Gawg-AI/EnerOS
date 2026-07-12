//! EnerOS Time Service — RTC driver, monotonic clock, and high-resolution timers.
//!
//! This crate provides:
//! - **PL031 RTC driver** for wall-clock time (battery-backed)
//! - **Monotonic clock** based on HAL `HalClock` (ARM Generic Timer)
//! - **High-resolution timer wheel** (TimerWheel with 64 slots)
//! - **Time API**: `get_time()`, `get_monotonic_ns()`, `sleep_until()`, etc.
//!
//! # Usage
//!
//! ```ignore
//! use eneros_time::time_init;
//! use hal::arm64::timer::clock;
//!
//! // Initialize with ARM64 Generic Timer and PL031 RTC at 0x09010000
//! time_init(clock(), 0x09010000);
//!
//! // Now use time APIs
//! let mono_ns = eneros_time::get_monotonic_ns();
//! let wall = eneros_time::get_time();
//! ```

#![cfg_attr(not(test), no_std)]

pub mod api;
pub mod beidou;
pub mod holdover;
pub mod hrtimer;
pub mod monotonic;
pub mod redundancy;
pub mod rtc;

pub use api::{
    cancel_timer, get_monotonic_ns, get_time, register_periodic, register_timer, rtc_read,
    rtc_write, sleep_until, time_init, timer_expired_count,
};
pub use hrtimer::{HrTimer, TimerId, TimerWheel};
pub use monotonic::MonotonicClock;
pub use rtc::{Pl031Rtc, RtcTime, TimeStamp};

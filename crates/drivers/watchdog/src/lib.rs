//! EnerOS Watchdog Service — SP805 hardware watchdog and layered feeding.
//!
//! This crate provides:
//! - **SP805 hardware watchdog driver** for hard reset on timeout
//! - **Layered watchdog feeding** with per-layer periodic deadlines
//! - **Watchdog API**: `wdt_init()`, `wdt_register_layer()`, `wdt_feed_layer()`, `wdt_kick()`, etc.
//!
//! # Usage
//!
//! ```ignore
//! use eneros_watchdog::wdt_init;
//!
//! // Initialize SP805 watchdog at 0x09050000 with 10s timeout
//! wdt_init(10_000, 0x09050000);
//! ```

#![cfg_attr(not(test), no_std)]

pub mod api;
pub mod layered;
pub mod wdt;

pub use api::{
    wdt_check, wdt_feed_layer, wdt_init, wdt_kick, wdt_layer_count, wdt_register_layer, wdt_stop,
};
pub use layered::{FeedLayer, LayerId, Watchdog, WatchdogStatus};
pub use wdt::HwWatchdog;

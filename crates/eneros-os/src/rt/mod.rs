//! Real-time runtime with SCHED_FIFO and CPU isolation

pub mod runtime;
pub mod ipc;
pub mod watchdog;

pub use runtime::{RtRuntime, RtConfig};
pub use ipc::{RtCommandQueue, RtResultChannel};
pub use watchdog::{HardwareWatchdog, WatchdogError, WatchdogLogEntry, WatchdogLogger};

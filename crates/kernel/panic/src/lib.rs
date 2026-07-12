//! EnerOS Panic Handling Framework — Phase 0 P0-D (v0.14.0).
//!
//! This crate provides:
//! - **Panic context** capturing level, location, message, timestamp, core id
//! - **Panic strategies**: kernel reset and partition isolation
//! - **Reset policy**: immediate or delayed hard reset
//! - **Global strategy registry** for runtime selection
//!
//! Per the D1 design decision, this crate does *not* define
//! `#[panic_handler]` to avoid symbol conflicts with `kernel`/`hello`.
//! Callers wire `handle_panic` into their own panic handler.
//!
//! # Usage
//!
//! ```ignore
//! use eneros_panic::{KernelResetStrategy, PanicContext, PanicLevel, set_strategy};
//!
//! static KS: KernelResetStrategy = KernelResetStrategy;
//! set_strategy(&KS);
//!
//! // In your #[panic_handler]:
//! fn panic(info: &core::panic::PanicInfo) -> ! {
//!     eneros_panic::handle_panic(info)
//! }
//! ```

#![cfg_attr(not(test), no_std)]

pub mod isolation;
pub mod logger;

use core::hint::spin_loop;

use spin::Mutex;

/// Severity/scope of a panic event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanicLevel {
    /// Kernel-wide panic — requires full system reset.
    Kernel,
    /// Partition-scoped panic — isolate the offending partition.
    Partition(u32),
}

/// Immutable snapshot of a panic event.
#[derive(Debug)]
pub struct PanicContext {
    pub level: PanicLevel,
    pub location: &'static str,
    pub message: &'static str,
    pub timestamp_ns: u64,
    pub core_id: u32,
}

/// Strategy for reacting to a panic. Implementations must not return.
pub trait PanicStrategy {
    fn handle(&self, ctx: &PanicContext) -> !;
}

/// Policy controlling how `KernelResetStrategy` performs the reset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetPolicy {
    /// Reset the system immediately after flushing logs.
    Immediate,
    /// Delay the reset by the given number of milliseconds (e.g. to let a
    /// watchdog fire first or allow logs to drain).
    Delayed(u64),
}

/// Kernel-wide reset strategy: log, flush, then hard-reset the board.
pub struct KernelResetStrategy;

/// Partition isolation strategy: log, mark the partition dead, then halt.
pub struct PartitionIsolateStrategy {
    pub partition: u32,
}

// ---------------------------------------------------------------------------
// Globals
// ---------------------------------------------------------------------------

/// Globally registered panic strategy (defaults to `None` → KernelResetStrategy).
static STRATEGY: Mutex<Option<&'static (dyn PanicStrategy + Sync)>> = Mutex::new(None);

/// Reset policy consulted by `KernelResetStrategy` (defaults to `Immediate`).
static RESET_POLICY: Mutex<ResetPolicy> = Mutex::new(ResetPolicy::Immediate);

/// Pre-allocated per-partition isolate strategies so a `&'static` reference
/// can be handed out at runtime via `set_partition_strategy`.
static PARTITION_STRATEGIES: [PartitionIsolateStrategy; 8] = [
    PartitionIsolateStrategy { partition: 0 },
    PartitionIsolateStrategy { partition: 1 },
    PartitionIsolateStrategy { partition: 2 },
    PartitionIsolateStrategy { partition: 3 },
    PartitionIsolateStrategy { partition: 4 },
    PartitionIsolateStrategy { partition: 5 },
    PartitionIsolateStrategy { partition: 6 },
    PartitionIsolateStrategy { partition: 7 },
];

// ---------------------------------------------------------------------------
// PanicContext
// ---------------------------------------------------------------------------

impl PanicContext {
    /// Build a new context, stamping the current monotonic time and core id.
    pub fn new(level: PanicLevel, location: &'static str, message: &'static str) -> Self {
        Self {
            level,
            location,
            message,
            timestamp_ns: eneros_time::get_monotonic_ns(),
            core_id: read_core_id(),
        }
    }
}

// ---------------------------------------------------------------------------
// Strategy implementations
// ---------------------------------------------------------------------------

impl PanicStrategy for KernelResetStrategy {
    fn handle(&self, ctx: &PanicContext) -> ! {
        logger::panic_log(ctx);
        logger::flush();
        let policy = *RESET_POLICY.lock();
        match policy {
            ResetPolicy::Immediate => hard_reset(),
            ResetPolicy::Delayed(ms) => {
                let start = eneros_time::get_monotonic_ns();
                let target = start.saturating_add(ms.saturating_mul(1_000_000));
                while eneros_time::get_monotonic_ns() < target {
                    spin_loop();
                }
                hard_reset();
            }
        }
    }
}

impl PanicStrategy for PartitionIsolateStrategy {
    fn handle(&self, ctx: &PanicContext) -> ! {
        logger::panic_log(ctx);
        match isolation::mark_partition_dead(self.partition) {
            Ok(()) => loop {
                spin_loop();
            },
            // Isolation failed → escalate to full kernel reset.
            Err(_) => KernelResetStrategy.handle(ctx),
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Register a panic strategy used by `handle_panic`.
pub fn set_strategy(strategy: &'static (dyn PanicStrategy + Sync)) {
    *STRATEGY.lock() = Some(strategy);
}

/// Convenience: register the pre-allocated isolate strategy for `id`.
pub fn set_partition_strategy(id: u32) {
    if (id as usize) < 8 {
        *STRATEGY.lock() = Some(&PARTITION_STRATEGIES[id as usize]);
    }
}

/// Set the reset delay policy consulted by `KernelResetStrategy`.
pub fn set_reset_policy(policy: ResetPolicy) {
    *RESET_POLICY.lock() = policy;
}

/// Entry point intended to be called from a crate that owns `#[panic_handler]`.
///
/// Builds a `PanicContext` (level = `Kernel`) and dispatches to the registered
/// strategy, falling back to `KernelResetStrategy` when none is registered.
///
/// `location`/`message` use `"?"` because `PanicInfo` cannot yield `&'static str`
/// without allocation; the structured `PanicContext::new` path should be used
/// when static location/message strings are available.
pub fn handle_panic(_info: &core::panic::PanicInfo) -> ! {
    let ctx = PanicContext::new(PanicLevel::Kernel, "?", "?");
    let strategy = *STRATEGY.lock();
    match strategy {
        Some(s) => s.handle(&ctx),
        None => KernelResetStrategy.handle(&ctx),
    }
}

/// Read the current CPU core id.
///
/// On aarch64 this reads `MPIDR_EL1` and masks to Aff0. On host (test) it
/// returns `0`.
#[cfg(target_arch = "aarch64")]
pub fn read_core_id() -> u32 {
    let id: u64;
    unsafe {
        core::arch::asm!(
            "mrs {}, mpidr_el1",
            out(reg) id,
            options(nostack, preserves_flags),
        );
    }
    (id & 0xff) as u32
}

#[cfg(not(target_arch = "aarch64"))]
pub fn read_core_id() -> u32 {
    0
}

/// Hard-reset the board.
///
/// On aarch64 this branches to the reset vector `0x0`. On host it spins so
/// tests that (indirectly) reach it do not return.
#[cfg(target_arch = "aarch64")]
pub fn hard_reset() -> ! {
    unsafe {
        core::arch::asm!("b 0x0", options(noreturn));
    }
}

#[cfg(not(target_arch = "aarch64"))]
pub fn hard_reset() -> ! {
    loop {
        spin_loop();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn test_panic_level_kernel() {
        assert_eq!(PanicLevel::Kernel, PanicLevel::Kernel);
        assert_ne!(PanicLevel::Kernel, PanicLevel::Partition(0));
    }

    #[test]
    fn test_panic_level_partition() {
        assert_eq!(PanicLevel::Partition(3), PanicLevel::Partition(3));
        assert_ne!(PanicLevel::Partition(3), PanicLevel::Partition(4));
        assert_ne!(PanicLevel::Partition(0), PanicLevel::Kernel);
    }

    #[test]
    fn test_panic_context_new() {
        let ctx = PanicContext::new(PanicLevel::Kernel, "file.rs", "boom");
        assert_eq!(ctx.level, PanicLevel::Kernel);
        assert_eq!(ctx.location, "file.rs");
        assert_eq!(ctx.message, "boom");
        assert_eq!(ctx.core_id, 0); // read_core_id() == 0 on host
        assert_eq!(ctx.timestamp_ns, 0); // get_monotonic_ns() == 0 before time_init
    }

    #[test]
    fn test_reset_policy_immediate() {
        assert_eq!(ResetPolicy::Immediate, ResetPolicy::Immediate);
        assert_ne!(ResetPolicy::Immediate, ResetPolicy::Delayed(0));
    }

    #[test]
    fn test_reset_policy_delayed() {
        assert_eq!(ResetPolicy::Delayed(100), ResetPolicy::Delayed(100));
        assert_ne!(ResetPolicy::Delayed(100), ResetPolicy::Delayed(200));
    }

    #[test]
    fn test_set_strategy() {
        let _g = lock();
        static KS: KernelResetStrategy = KernelResetStrategy;
        set_strategy(&KS);
        assert!(STRATEGY.lock().is_some());
        // Restore default.
        *STRATEGY.lock() = None;
        assert!(STRATEGY.lock().is_none());
    }

    #[test]
    fn test_read_core_id_host_returns_zero() {
        assert_eq!(read_core_id(), 0);
    }
}

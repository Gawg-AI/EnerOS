//! EnerOS Control Bus — Phase 0 P0-J (v0.22.0, ★ bottleneck version).
//!
//! This crate provides the control bus for dispatching [`ControlCommand`]s
//! from the Agent plane to the RTOS plane, with TTL-based validity,
//! constraint checking, and a dual-plane fallback strategy.
//!
//! # v0.22.0 Deliverables
//!
//! - **ControlCommand** type with device targeting, action, setpoint,
//!   constraint pack, and cryptographic signature slot.
//! - **TTL checking** ([`ttl::ttl_check`]) — commands expire after `ttl_ms`
//!   milliseconds, preventing stale command execution.
//! - **Constraint checking** ([`constraint::constraint_check`]) — validates
//!   SOC, voltage, frequency limits and truncates power setpoints to safe
//!   ranges.
//! - **Dual-plane fallback** ([`fallback::execute_or_fallback`]) — when the
//!   Agent plane is alive, commands flow normally; when it crashes, the RTOS
//!   plane falls back to the last valid command (within TTL) or a safe
//!   default.
//! - **Integration simulation** ([`integration::integration_step`]) —
//!   simulates the agent-alive / agent-crashed / agent-recovery lifecycle,
//!   verifying Phase 0 exit criteria for dual-plane coordination.
//!
//! # Phase 0 Exit Verification
//!
//! This crate is the Phase 0 capstone — it demonstrates that the kernel
//! infrastructure (heap, scheduler, IPC, MM, panic isolation) can support
//! a real-time control bus with:
//! - Lock-free command queueing via `eneros-ipc::SpscRing`
//! - Global state serialized via `eneros-sched::Spinlock` + `UnsafeCell`
//! - Bounded command size (fits in a single 256-byte ring slot)
//! - Deterministic fallback when the Agent plane fails
//!
//! # Design Decisions
//!
//! - **D2**: All global state uses `Spinlock + UnsafeCell<T>` (NOT
//!   `static mut`), matching the pattern established in `eneros-sched::tcb`
//!   and `eneros-ipc::endpoint`.
//! - **no_std**: All production code uses `core::*` only.
//! - **Bottleneck version (★)**: Code is "skeleton usable", algorithms
//!   complete, no `todo!()`/`unimplemented!()` stubs.

#![cfg_attr(not(test), no_std)]
#![allow(dead_code)]

pub mod command;
pub mod constraint;
pub mod fallback;
pub mod integration;
pub mod ttl;

pub use command::{
    command_consume, command_send, control_bus_init, CbError, ConstraintPack, ControlAction,
    ControlCommand, DeviceId,
};
pub use constraint::{constraint_check, ConstraintResult, DeviceState};
pub use fallback::{execute_or_fallback, FallbackMode};
pub use integration::{
    integration_step, new_integration_state, simulate_agent_crash, simulate_agent_recovery,
    IntegrationState,
};
pub use ttl::{ttl_check, TtlStatus};

/// Crate-wide test serialization lock.
///
/// All test modules share this lock to prevent concurrent access to the
/// global `CMD_RING` / `LAST_CMD` state. Tests acquire it via
/// `crate::TEST_LOCK.lock()`.
#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

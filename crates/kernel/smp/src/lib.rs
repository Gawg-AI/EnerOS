//! EnerOS SMP Framework — Phase 0 P0-E.
//!
//! This crate provides:
//! - **Multi-core boot** (v0.15.0) via PSCI `CPU_ON`
//! - **Core state** tracking (Offline / Booting / Online / Halted)
//! - **IPI** (v0.15.0) via GICv3 SGI
//! - **Mailbox** (v0.15.0) per-core message channel
//! - **Memory coherence** (v0.17.0) — barriers, cache maintenance, atomic
//!   counter, DMA buffer management
//!
//! Per the D2 design decision, this crate does *not* depend on `eneros-hal` —
//! aarch64 inline assembly is implemented directly to keep the dependency
//! graph minimal and the crate self-contained.
//!
//! All aarch64-specific code is gated behind `#[cfg(target_arch = "aarch64")]`
//! so the crate compiles and tests on the host target (x86_64).

#![cfg_attr(not(test), no_std)]

pub mod atomic_ops;
pub mod boot;
pub mod channel;
pub mod coherence;
pub mod dma_coherent;
pub mod ipi;

pub use atomic_ops::AtomicCounter;
pub use boot::{
    core_count, core_state, read_core_id, secondary_entry, set_core_state, smp_init,
    wake_secondary, CoreInfo, CoreState,
};
pub use channel::{mailbox_clear, mailbox_drain, mailbox_pop, mailbox_push, MAILBOX_CAPACITY};
pub use coherence::{cache_clean, cache_invalidate, dmb, dsb, isb, CACHELINE_SIZE};
pub use dma_coherent::DmaBuffer;
pub use ipi::{ipi_broadcast, ipi_dispatch, ipi_send, register_ipi_handler, IpiMsg, SGI_IRQ_NUM};

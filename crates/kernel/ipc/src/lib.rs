//! EnerOS IPC Subsystem — Phase 0 P0-H (v0.20.0 + v0.21.0).
//!
//! This crate provides:
//! - **Synchronous IPC messaging** (v0.20.0): [`endpoint::send`],
//!   [`endpoint::recv`], [`endpoint::endpoint_create`],
//!   [`endpoint::endpoint_destroy`] — blocking send/recv over endpoints
//!   with thread block/resume integration via `eneros-sched`.
//! - **Notifications** (v0.20.0): [`notification::notify`],
//!   [`notification::wait_notification`] — bit-mask notifications with
//!   thread wake-up.
//! - **RPC channel** (v0.20.0): [`channel::call`] — combined send+recv
//!   for request/reply semantics.
//! - **Lock-free SPSC ring buffer** (v0.21.0): [`spsc_ring::SpscRing`] —
//!   single-producer single-consumer ring buffer using atomics for
//!   wait-free push/pop, suitable for ISR-to-thread and inter-core
//!   communication.
//! - **Shared memory grant** (v0.20.0 stub): [`shared_mem::grant_shared_mem`]
//!   — Phase 0 placeholder for shared memory region management.
//!
//! # Design Decisions
//!
//! - **D2**: All global state uses `Spinlock + UnsafeCell<T>` (NOT
//!   `static mut`), matching the pattern established in `eneros-sched::tcb`.
//!   `Sync` is implemented manually with a `// SAFETY:` justification.
//! - **no_std**: All production code uses `core::*` only. `alloc` is
//!   declared for future use but the current implementation does not
//!   require heap allocation.
//! - **Blocking semantics**: On host (test) builds, `thread_block` is
//!   effectively a no-op (the scheduler does not dispatch), so blocking
//!   send/recv return immediately. On aarch64, a real context switch
//!   would occur.

#![cfg_attr(not(test), no_std)]
#![allow(dead_code)]

extern crate alloc;

pub mod channel;
pub mod endpoint;
pub mod notification;
pub mod shared_mem;
pub mod spsc_ring;

pub use channel::call;
pub use endpoint::{
    endpoint_create, endpoint_destroy, recv, send, Endpoint, EndpointId, IpcError, Message,
    MAX_ENDPOINTS, MSG_SIZE,
};
pub use notification::{notify, wait_notification, Notification, MAX_NOTIFY_SLOTS};
pub use shared_mem::{grant_shared_mem, SharedMemRegion};
pub use spsc_ring::{RingError, SpscRing};

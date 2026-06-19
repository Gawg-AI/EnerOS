//! Event types re-exported from eneros-core.
//!
//! The canonical definitions live in `eneros_core::event` so they can be
//! shared as IPC schema across processes. This module re-exports them for
//! backward compatibility with existing eventbus code.

pub use eneros_core::event::{Event, EventPayload, EventType};

//! Inter-Processor Interrupt (IPI) support.
//!
//! On aarch64, IPIs are delivered as GICv3 Software Generated Interrupts
//! (SGIs). The sender pushes a message into the target core's mailbox and
//! then fires SGI 0; the receiving core drains its mailbox inside the SGI
//! handler and dispatches each message to a registered per-type handler.

use spin::Mutex;

use crate::boot::{core_count, read_core_id};
use crate::channel;

/// GICv3 SGI interrupt number used for IPI signalling.
pub const SGI_IRQ_NUM: u32 = 0;

/// Maximum number of distinct IPI message types (handler table size).
const MAX_IPI_TYPES: usize = 16;

/// Per-type handler function pointer.
type Handler = Option<fn(IpiMsg)>;

/// The full handler table indexed by `IpiMsg::msg_type()`.
type HandlerTable = [Handler; MAX_IPI_TYPES];

/// IPI message payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpiMsg {
    /// Request the target to reschedule.
    Reschedule,
    /// Request the target to shut down.
    Shutdown,
    /// Request a TLB shootdown for the given address-space identifier.
    TlbShootdown(u64),
    /// User-defined message with a 32-bit parameter.
    Custom(u32),
}

impl IpiMsg {
    /// Returns the handler-table index for this message type.
    ///
    /// `Reschedule`=0, `Shutdown`=1, `TlbShootdown`=2,
    /// `Custom(t)`=3 + (t % 13)  →  3..=15.
    pub fn msg_type(&self) -> u32 {
        match self {
            IpiMsg::Reschedule => 0,
            IpiMsg::Shutdown => 1,
            IpiMsg::TlbShootdown(_) => 2,
            IpiMsg::Custom(t) => 3 + (*t % 13),
        }
    }
}

static IPI_HANDLERS: Mutex<HandlerTable> = Mutex::new({
    const NONE: Handler = None;
    [NONE; MAX_IPI_TYPES]
});

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Send an IPI to `target`.
///
/// Pushes `msg` into the target's mailbox (ignoring overflow) and then
/// fires SGI 0 via `icc_sgi1r_el1`. On host the SGI is a no-op but the
/// mailbox push still executes, enabling testing.
pub fn ipi_send(target: u32, msg: IpiMsg) {
    let _ = channel::mailbox_push(target, msg);
    send_sgi(target, SGI_IRQ_NUM);
}

/// Broadcast an IPI to all cores except the caller.
pub fn ipi_broadcast(msg: IpiMsg) {
    let self_id = read_core_id();
    let count = core_count();
    for i in 0..count {
        if i != self_id {
            ipi_send(i, msg);
        }
    }
}

/// Register a handler for the given message type.
///
/// `msg_type` values >= `MAX_IPI_TYPES` are silently ignored.
pub fn register_ipi_handler(msg_type: u32, handler: fn(IpiMsg)) {
    if msg_type as usize >= MAX_IPI_TYPES {
        return;
    }
    IPI_HANDLERS.lock()[msg_type as usize] = Some(handler);
}

/// Drain the caller's mailbox and dispatch each message to its handler.
///
/// Messages with no registered handler are silently dropped.
pub fn ipi_dispatch() {
    let msgs = channel::mailbox_drain(read_core_id());
    // Copy out the handler table so we don't hold the lock while invoking
    // handlers (a handler may itself send an IPI, which would deadlock).
    let handlers: HandlerTable = *IPI_HANDLERS.lock();
    for msg in msgs.as_slice() {
        let idx = msg.msg_type() as usize;
        if idx < MAX_IPI_TYPES {
            if let Some(handler) = handlers[idx] {
                handler(*msg);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// aarch64 SGI generation
// ---------------------------------------------------------------------------

/// Fire a Software Generated Interrupt via `icc_sgi1r_el1`.
///
/// SGI encoding (simplified single-cluster):
/// `target_aff0 << 16 | sgi_num`
#[cfg(target_arch = "aarch64")]
fn send_sgi(target: u32, sgi_num: u32) {
    let val: u64 = ((target as u64) << 16) | (sgi_num as u64 & 0xf);
    unsafe {
        core::arch::asm!(
            "msr icc_sgi1r_el1, {}",
            in(reg) val,
            options(nostack, preserves_flags),
        );
    }
}

#[cfg(not(target_arch = "aarch64"))]
fn send_sgi(_target: u32, _sgi_num: u32) {}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel;

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn test_ipi_msg_variants() {
        let _a = IpiMsg::Reschedule;
        let _b = IpiMsg::Shutdown;
        let _c = IpiMsg::TlbShootdown(0xdead_beef);
        let _d = IpiMsg::Custom(42);
        assert_ne!(IpiMsg::Reschedule, IpiMsg::Shutdown);
        assert_ne!(IpiMsg::Shutdown, IpiMsg::TlbShootdown(0));
    }

    #[test]
    fn test_ipi_msg_msg_type() {
        assert_eq!(IpiMsg::Reschedule.msg_type(), 0);
        assert_eq!(IpiMsg::Shutdown.msg_type(), 1);
        assert_eq!(IpiMsg::TlbShootdown(0x1000).msg_type(), 2);
        assert_eq!(IpiMsg::Custom(0).msg_type(), 3);
        assert_eq!(IpiMsg::Custom(1).msg_type(), 4);
        assert_eq!(IpiMsg::Custom(12).msg_type(), 15);
        // 13 % 13 == 0 → wraps back to 3.
        assert_eq!(IpiMsg::Custom(13).msg_type(), 3);
        // All Custom types map into 3..=15.
        for t in 0..100u32 {
            let mt = IpiMsg::Custom(t).msg_type();
            assert!((3..MAX_IPI_TYPES as u32).contains(&mt));
        }
    }

    #[test]
    fn test_register_ipi_handler() {
        let _g = lock();
        fn handler(_msg: IpiMsg) {}
        register_ipi_handler(0, handler);
        assert!(IPI_HANDLERS.lock()[0].is_some());
        // Restore
        *IPI_HANDLERS.lock() = [const { None }; MAX_IPI_TYPES];
        // ^ type inferred from HandlerTable
    }

    #[test]
    fn test_register_ipi_handler_ignored() {
        let _g = lock();
        fn handler(_msg: IpiMsg) {}
        register_ipi_handler(16, handler);
        register_ipi_handler(100, handler);
        assert!(IPI_HANDLERS.lock().iter().all(|h| h.is_none()));
    }

    #[test]
    fn test_ipi_dispatch_empty_mailbox_no_panic() {
        let _g = lock();
        channel::mailbox_clear(0);
        ipi_dispatch();
    }
}

//! Per-core mailbox channel.
//!
//! Each core has a small bounded mailbox (`heapless::Vec<IpiMsg, 16>`) used
//! to queue IPI messages. The IPI sender pushes a message into the target
//! core's mailbox and then fires an SGI; the receiving core drains the
//! mailbox inside its IPI dispatch handler.

use spin::Mutex;

use crate::ipi::IpiMsg;

/// Maximum number of messages each mailbox can hold.
pub const MAILBOX_CAPACITY: usize = 16;

/// Maximum number of cores (must match `boot::MAX_CORES`).
const MAX_CORES: usize = 8;

type Mailbox = heapless::Vec<IpiMsg, MAILBOX_CAPACITY>;

static MAILBOXES: Mutex<[Mailbox; MAX_CORES]> = Mutex::new([
    heapless::Vec::new(),
    heapless::Vec::new(),
    heapless::Vec::new(),
    heapless::Vec::new(),
    heapless::Vec::new(),
    heapless::Vec::new(),
    heapless::Vec::new(),
    heapless::Vec::new(),
]);

/// Push a message into the target core's mailbox.
///
/// Returns `Err(msg)` if `core_id` is invalid or the mailbox is full.
pub fn mailbox_push(core_id: u32, msg: IpiMsg) -> Result<(), IpiMsg> {
    if core_id as usize >= MAX_CORES {
        return Err(msg);
    }
    let mut boxes = MAILBOXES.lock();
    boxes[core_id as usize].push(msg)
}

/// Pop the oldest message from the target core's mailbox.
///
/// Returns `None` if `core_id` is invalid or the mailbox is empty.
pub fn mailbox_pop(core_id: u32) -> Option<IpiMsg> {
    if core_id as usize >= MAX_CORES {
        return None;
    }
    let mut boxes = MAILBOXES.lock();
    let mbox = &mut boxes[core_id as usize];
    if mbox.is_empty() {
        None
    } else {
        Some(mbox.swap_remove(0))
    }
}

/// Drain all messages from the target core's mailbox.
///
/// Returns a new `Vec` containing all messages in FIFO order. Invalid
/// `core_id` values yield an empty `Vec`.
pub fn mailbox_drain(core_id: u32) -> Mailbox {
    let mut result = heapless::Vec::new();
    if core_id as usize >= MAX_CORES {
        return result;
    }
    let mut boxes = MAILBOXES.lock();
    let mbox = &mut boxes[core_id as usize];
    for &msg in mbox.as_slice() {
        let _ = result.push(msg);
    }
    mbox.clear();
    result
}

/// Clear all messages from the target core's mailbox.
///
/// Silently ignores invalid `core_id` values.
pub fn mailbox_clear(core_id: u32) {
    if core_id as usize >= MAX_CORES {
        return;
    }
    let mut boxes = MAILBOXES.lock();
    boxes[core_id as usize].clear();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipi::IpiMsg;

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn test_mailbox_push_pop() {
        let _g = lock();
        mailbox_clear(0);
        assert!(mailbox_push(0, IpiMsg::Reschedule).is_ok());
        assert_eq!(mailbox_pop(0), Some(IpiMsg::Reschedule));
        assert_eq!(mailbox_pop(0), None);
        mailbox_clear(0);
    }

    #[test]
    fn test_mailbox_push_full_returns_err() {
        let _g = lock();
        mailbox_clear(0);
        for i in 0..MAILBOX_CAPACITY {
            assert!(mailbox_push(0, IpiMsg::Custom(i as u32)).is_ok());
        }
        let result = mailbox_push(0, IpiMsg::Reschedule);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), IpiMsg::Reschedule);
        mailbox_clear(0);
    }

    #[test]
    fn test_mailbox_pop_empty_returns_none() {
        let _g = lock();
        mailbox_clear(0);
        assert_eq!(mailbox_pop(0), None);
    }

    #[test]
    fn test_mailbox_drain() {
        let _g = lock();
        mailbox_clear(0);
        mailbox_push(0, IpiMsg::Reschedule).unwrap();
        mailbox_push(0, IpiMsg::Shutdown).unwrap();
        let drained = mailbox_drain(0);
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0], IpiMsg::Reschedule);
        assert_eq!(drained[1], IpiMsg::Shutdown);
        // Mailbox should now be empty.
        assert_eq!(mailbox_pop(0), None);
    }

    #[test]
    fn test_mailbox_clear() {
        let _g = lock();
        mailbox_push(0, IpiMsg::Reschedule).unwrap();
        mailbox_push(0, IpiMsg::Shutdown).unwrap();
        mailbox_clear(0);
        assert_eq!(mailbox_pop(0), None);
    }

    #[test]
    fn test_mailbox_invalid_core_id() {
        let _g = lock();
        assert!(mailbox_push(8, IpiMsg::Reschedule).is_err());
        assert_eq!(mailbox_pop(8), None);
        assert_eq!(mailbox_drain(8).len(), 0);
        // Should not panic.
        mailbox_clear(8);
    }

    #[test]
    fn test_mailbox_cross_core() {
        let _g = lock();
        mailbox_clear(1);
        assert!(mailbox_push(1, IpiMsg::TlbShootdown(0x1000)).is_ok());
        assert_eq!(mailbox_pop(1), Some(IpiMsg::TlbShootdown(0x1000)));
        assert_eq!(mailbox_pop(1), None);
        mailbox_clear(1);
    }
}

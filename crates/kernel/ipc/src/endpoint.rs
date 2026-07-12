//! IPC endpoints — synchronous blocking send/recv (v0.20.0).
//!
//! Provides:
//! - [`EndpointId`] — endpoint identifier newtype
//! - [`Message`] — fixed-size IPC message (128 bytes)
//! - [`Endpoint`] — endpoint state (waiting sender/receiver, buffered msg)
//! - [`endpoint_create`]/[`endpoint_destroy`] — lifecycle management
//! - [`send`]/[`recv`] — blocking synchronous messaging
//!
//! Per D2, global state uses `Spinlock + UnsafeCell<T>` (NOT `static mut`).

use core::cell::UnsafeCell;

use eneros_sched::{current_tid, thread_block, thread_resume, Spinlock, Tid};

/// Maximum number of endpoints in the global table.
pub const MAX_ENDPOINTS: usize = 256;

/// Fixed message size (8-byte label + 120-byte payload = 128 bytes).
pub const MSG_SIZE: usize = 128;

/// Endpoint identifier (newtype over `u32`).
///
/// `EndpointId(0)` is reserved as "invalid" (analogous to `Tid(0)`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EndpointId(pub u32);

/// Fixed-size IPC message.
///
/// Total size: 8 (label) + 120 (payload) = 128 bytes = [`MSG_SIZE`].
#[derive(Clone, Copy, PartialEq)]
pub struct Message {
    /// Message label / opcode (identifies the request/reply type).
    pub label: u64,
    /// Inline payload (up to 120 bytes).
    pub payload: [u8; 120],
}

impl Default for Message {
    fn default() -> Self {
        Self {
            label: 0,
            payload: [0; 120],
        }
    }
}

impl core::fmt::Debug for Message {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Message")
            .field("label", &self.label)
            .field("payload_len", &self.payload.len())
            .finish()
    }
}

/// IPC error variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcError {
    /// The endpoint ID does not refer to a valid endpoint.
    InvalidEndpoint,
    /// The operation timed out (reserved for future timed send/recv).
    Timeout,
    /// The endpoint has been disconnected / destroyed.
    Disconnected,
}

/// Endpoint state.
///
/// Holds the endpoint ID, the currently waiting sender/receiver (if any),
/// and the last buffered message. Access is serialized through the global
/// [`ENDPOINTS`] table's spinlock.
pub struct Endpoint {
    pub id: EndpointId,
    pub waiting_sender: Option<Tid>,
    pub waiting_receiver: Option<Tid>,
    pub msg: Message,
}

// ---------------------------------------------------------------------------
// Global endpoint table
// ---------------------------------------------------------------------------

/// Wrapper around the global endpoint table.
///
/// Combines a [`Spinlock`] with an `UnsafeCell`-protected array of
/// `Option<Endpoint>`. The `UnsafeCell` provides interior mutability; the
/// spinlock serializes access.
///
/// # Safety
///
/// `Sync` is sound because all access to `entries` is gated by `lock`.
/// Callers must acquire `lock` before touching `entries`.
struct EndpointTable {
    lock: Spinlock,
    entries: UnsafeCell<[Option<Endpoint>; MAX_ENDPOINTS]>,
}

// SAFETY: Access to `entries` is serialized by `lock`. All public API
// functions acquire the spinlock before reading/writing `entries`.
unsafe impl Sync for EndpointTable {}

static ENDPOINTS: EndpointTable = EndpointTable {
    lock: Spinlock::new(),
    entries: UnsafeCell::new([const { None }; MAX_ENDPOINTS]),
};

/// Monotonic endpoint ID counter.
struct NextEpId {
    lock: Spinlock,
    value: UnsafeCell<u32>,
}

// SAFETY: Access to `value` is serialized by `lock`.
unsafe impl Sync for NextEpId {}

static NEXT_EP_ID: NextEpId = NextEpId {
    lock: Spinlock::new(),
    value: UnsafeCell::new(1),
};

/// Linear scan to find the slot index for `id`.
///
/// Must be called while holding the `ENDPOINTS` lock. The `entries`
/// reference avoids a second `UnsafeCell` dereference.
fn find_ep(entries: &[Option<Endpoint>; MAX_ENDPOINTS], id: EndpointId) -> Option<usize> {
    for (i, entry) in entries.iter().enumerate() {
        if let Some(ep) = entry {
            if ep.id == id {
                return Some(i);
            }
        }
    }
    None
}

/// Create a new endpoint.
///
/// Scans the global table for a free slot, assigns a monotonically
/// increasing ID, and inserts a new [`Endpoint`].
///
/// Returns the new `EndpointId` (≥ 1) on success, or `EndpointId(0)` if
/// the table is full.
pub fn endpoint_create() -> EndpointId {
    // Allocate an ID first.
    NEXT_EP_ID.lock.lock();
    // SAFETY: We hold the lock.
    let id_val = unsafe { *NEXT_EP_ID.value.get() };
    let new_id = EndpointId(id_val);
    // SAFETY: We hold the lock.
    unsafe {
        *NEXT_EP_ID.value.get() = id_val.wrapping_add(1);
    }
    NEXT_EP_ID.lock.unlock();

    ENDPOINTS.lock.lock();
    // SAFETY: We hold the lock.
    let entries = unsafe { &mut *ENDPOINTS.entries.get() };
    let result = {
        let mut found: Option<usize> = None;
        for (i, entry) in entries.iter().enumerate() {
            if entry.is_none() {
                found = Some(i);
                break;
            }
        }
        match found {
            Some(idx) => {
                entries[idx] = Some(Endpoint {
                    id: new_id,
                    waiting_sender: None,
                    waiting_receiver: None,
                    msg: Message::default(),
                });
                new_id
            }
            None => EndpointId(0),
        }
    };
    ENDPOINTS.lock.unlock();
    result
}

/// Destroy an endpoint.
///
/// Removes the endpoint from the global table. No-op if `ep` is invalid
/// or already destroyed.
pub fn endpoint_destroy(ep: EndpointId) {
    if ep.0 == 0 {
        return;
    }
    ENDPOINTS.lock.lock();
    // SAFETY: We hold the lock.
    let entries = unsafe { &mut *ENDPOINTS.entries.get() };
    if let Some(idx) = find_ep(entries, ep) {
        entries[idx] = None;
    }
    ENDPOINTS.lock.unlock();
}

/// Send a message on `ep`.
///
/// If a receiver is waiting on `ep`, the message is copied into the
/// endpoint, the receiver is resumed, and `Ok(())` is returned.
/// Otherwise, the current thread is recorded as the waiting sender and
/// blocked (on host, blocking is a no-op — the function returns
/// immediately).
///
/// Returns `Err(InvalidEndpoint)` if `ep` does not refer to a valid
/// endpoint.
pub fn send(ep: EndpointId, msg: &Message) -> Result<(), IpcError> {
    ENDPOINTS.lock.lock();
    // SAFETY: We hold the lock.
    let entries = unsafe { &mut *ENDPOINTS.entries.get() };
    let idx = match find_ep(entries, ep) {
        Some(idx) => idx,
        None => {
            ENDPOINTS.lock.unlock();
            return Err(IpcError::InvalidEndpoint);
        }
    };

    let receiver = entries[idx].as_ref().unwrap().waiting_receiver;
    let sender_tid = current_tid();

    if let Some(recv_tid) = receiver {
        // Deliver the message to the waiting receiver.
        let entry = entries[idx].as_mut().unwrap();
        entry.msg = *msg;
        entry.waiting_receiver = None;
        ENDPOINTS.lock.unlock();
        let _ = thread_resume(recv_tid);
        Ok(())
    } else {
        // No receiver — block the sender.
        let entry = entries[idx].as_mut().unwrap();
        entry.waiting_sender = Some(sender_tid);
        ENDPOINTS.lock.unlock();
        let _ = thread_block(sender_tid);
        Ok(())
    }
}

/// Receive a message on `ep`.
///
/// If a sender is waiting on `ep`, the buffered message is copied out,
/// the sender is resumed, and `Ok(msg)` is returned.
/// Otherwise, the current thread is recorded as the waiting receiver and
/// blocked (on host, blocking is a no-op — the function returns the
/// endpoint's last buffered message).
///
/// Returns `Err(InvalidEndpoint)` if `ep` does not refer to a valid
/// endpoint.
pub fn recv(ep: EndpointId) -> Result<Message, IpcError> {
    ENDPOINTS.lock.lock();
    // SAFETY: We hold the lock.
    let entries = unsafe { &mut *ENDPOINTS.entries.get() };
    let idx = match find_ep(entries, ep) {
        Some(idx) => idx,
        None => {
            ENDPOINTS.lock.unlock();
            return Err(IpcError::InvalidEndpoint);
        }
    };

    let sender = entries[idx].as_ref().unwrap().waiting_sender;
    let receiver_tid = current_tid();

    if let Some(send_tid) = sender {
        // A sender is waiting — copy out the message and resume the sender.
        let entry = entries[idx].as_mut().unwrap();
        let msg = entry.msg;
        entry.waiting_sender = None;
        ENDPOINTS.lock.unlock();
        let _ = thread_resume(send_tid);
        Ok(msg)
    } else {
        // No sender — block the receiver and return the current message.
        let entry = entries[idx].as_mut().unwrap();
        let msg = entry.msg;
        entry.waiting_receiver = Some(receiver_tid);
        ENDPOINTS.lock.unlock();
        let _ = thread_block(receiver_tid);
        Ok(msg)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use eneros_sched::{set_current_tid, thread_create, thread_state, ThreadState};

    use super::*;

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Test entry function that never returns (spins).
    fn test_entry() -> ! {
        loop {
            core::hint::spin_loop();
        }
    }

    /// Clear the global endpoint table and reset the ID counter.
    fn reset_endpoints() {
        ENDPOINTS.lock.lock();
        // SAFETY: We hold the lock.
        let entries = unsafe { &mut *ENDPOINTS.entries.get() };
        for entry in entries.iter_mut() {
            *entry = None;
        }
        ENDPOINTS.lock.unlock();

        NEXT_EP_ID.lock.lock();
        // SAFETY: We hold the lock.
        unsafe {
            *NEXT_EP_ID.value.get() = 1;
        }
        NEXT_EP_ID.lock.unlock();
    }

    // --- Test helpers to inspect/modify endpoint state ---

    fn get_endpoint_waiting_sender(id: EndpointId) -> Option<Tid> {
        ENDPOINTS.lock.lock();
        // SAFETY: We hold the lock.
        let entries = unsafe { &*ENDPOINTS.entries.get() };
        let result =
            find_ep(entries, id).and_then(|idx| entries[idx].as_ref().map(|ep| ep.waiting_sender));
        ENDPOINTS.lock.unlock();
        result.flatten()
    }

    fn get_endpoint_waiting_receiver(id: EndpointId) -> Option<Tid> {
        ENDPOINTS.lock.lock();
        // SAFETY: We hold the lock.
        let entries = unsafe { &*ENDPOINTS.entries.get() };
        let result = find_ep(entries, id)
            .and_then(|idx| entries[idx].as_ref().map(|ep| ep.waiting_receiver));
        ENDPOINTS.lock.unlock();
        result.flatten()
    }

    fn get_endpoint_msg(id: EndpointId) -> Message {
        ENDPOINTS.lock.lock();
        // SAFETY: We hold the lock.
        let entries = unsafe { &*ENDPOINTS.entries.get() };
        let result = find_ep(entries, id)
            .and_then(|idx| entries[idx].as_ref().map(|ep| ep.msg))
            .unwrap_or_default();
        ENDPOINTS.lock.unlock();
        result
    }

    fn set_endpoint_waiting_receiver(id: EndpointId, tid: Option<Tid>) {
        ENDPOINTS.lock.lock();
        // SAFETY: We hold the lock.
        let entries = unsafe { &mut *ENDPOINTS.entries.get() };
        if let Some(idx) = find_ep(entries, id) {
            if let Some(ep) = &mut entries[idx] {
                ep.waiting_receiver = tid;
            }
        }
        ENDPOINTS.lock.unlock();
    }

    fn set_endpoint_waiting_sender(id: EndpointId, tid: Option<Tid>) {
        ENDPOINTS.lock.lock();
        // SAFETY: We hold the lock.
        let entries = unsafe { &mut *ENDPOINTS.entries.get() };
        if let Some(idx) = find_ep(entries, id) {
            if let Some(ep) = &mut entries[idx] {
                ep.waiting_sender = tid;
            }
        }
        ENDPOINTS.lock.unlock();
    }

    fn set_endpoint_msg(id: EndpointId, msg: Message) {
        ENDPOINTS.lock.lock();
        // SAFETY: We hold the lock.
        let entries = unsafe { &mut *ENDPOINTS.entries.get() };
        if let Some(idx) = find_ep(entries, id) {
            if let Some(ep) = &mut entries[idx] {
                ep.msg = msg;
            }
        }
        ENDPOINTS.lock.unlock();
    }

    // --- Tests ---

    #[test]
    fn test_endpoint_create_returns_valid_id() {
        let _g = lock();
        reset_endpoints();

        let ep = endpoint_create();
        assert_ne!(
            ep,
            EndpointId(0),
            "endpoint_create should return non-zero ID"
        );
        assert!(ep.0 >= 1, "EndpointId should be >= 1");

        reset_endpoints();
    }

    #[test]
    fn test_endpoint_create_multiple() {
        let _g = lock();
        reset_endpoints();

        let ep1 = endpoint_create();
        let ep2 = endpoint_create();
        let ep3 = endpoint_create();

        assert_ne!(ep1, EndpointId(0));
        assert_ne!(ep2, EndpointId(0));
        assert_ne!(ep3, EndpointId(0));
        assert_ne!(ep1, ep2, "IDs must be unique");
        assert_ne!(ep2, ep3, "IDs must be unique");
        assert_ne!(ep1, ep3, "IDs must be unique");
        // IDs should be monotonically increasing.
        assert!(ep2.0 > ep1.0);
        assert!(ep3.0 > ep2.0);

        reset_endpoints();
    }

    #[test]
    fn test_endpoint_destroy() {
        let _g = lock();
        reset_endpoints();

        let ep = endpoint_create();
        assert_ne!(ep, EndpointId(0));

        // Destroy the endpoint.
        endpoint_destroy(ep);

        // Sending to a destroyed endpoint should fail.
        let msg = Message::default();
        let result = send(ep, &msg);
        assert_eq!(result, Err(IpcError::InvalidEndpoint));

        // After destroy, the slot is free — a new create should succeed.
        let ep2 = endpoint_create();
        assert_ne!(ep2, EndpointId(0));

        reset_endpoints();
    }

    #[test]
    fn test_send_invalid_endpoint() {
        let _g = lock();
        reset_endpoints();

        let msg = Message::default();
        let result = send(EndpointId(999), &msg);
        assert_eq!(result, Err(IpcError::InvalidEndpoint));

        reset_endpoints();
    }

    #[test]
    fn test_recv_invalid_endpoint() {
        let _g = lock();
        reset_endpoints();

        let result = recv(EndpointId(999));
        assert_eq!(result, Err(IpcError::InvalidEndpoint));

        reset_endpoints();
    }

    #[test]
    fn test_send_with_receiver_waiting() {
        let _g = lock();
        reset_endpoints();

        let ep = endpoint_create();
        assert_ne!(ep, EndpointId(0));

        // Set up: a receiver is waiting on this endpoint.
        let receiver_tid = Tid(5);
        set_endpoint_waiting_receiver(ep, Some(receiver_tid));

        // Sender sends.
        set_current_tid(Tid(6));
        let mut msg = Message {
            label: 0xBEEF,
            ..Default::default()
        };
        msg.payload[0] = 42;
        let result = send(ep, &msg);
        assert!(result.is_ok());

        // Verify: waiting_receiver is cleared.
        assert_eq!(get_endpoint_waiting_receiver(ep), None);
        // Verify: message was delivered to the endpoint.
        let delivered = get_endpoint_msg(ep);
        assert_eq!(delivered.label, 0xBEEF);
        assert_eq!(delivered.payload[0], 42);

        reset_endpoints();
        set_current_tid(Tid(0));
    }

    #[test]
    fn test_recv_with_sender_waiting() {
        let _g = lock();
        reset_endpoints();

        let ep = endpoint_create();
        assert_ne!(ep, EndpointId(0));

        // Set up: a sender is waiting with a message.
        let sender_tid = Tid(7);
        let mut msg = Message {
            label: 0xCAFE,
            ..Default::default()
        };
        msg.payload[3] = 99;
        set_endpoint_waiting_sender(ep, Some(sender_tid));
        set_endpoint_msg(ep, msg);

        // Receiver receives.
        set_current_tid(Tid(8));
        let result = recv(ep);
        assert!(result.is_ok());
        let received = result.unwrap();
        assert_eq!(received.label, 0xCAFE);
        assert_eq!(received.payload[3], 99);

        // Verify: waiting_sender is cleared.
        assert_eq!(get_endpoint_waiting_sender(ep), None);

        reset_endpoints();
        set_current_tid(Tid(0));
    }

    #[test]
    fn test_send_no_receiver_blocks_sender() {
        let _g = lock();
        reset_endpoints();

        let ep = endpoint_create();
        assert_ne!(ep, EndpointId(0));

        // Create a thread for the sender (so thread_block has a target).
        let sender_tid = thread_create(test_entry, 4096, 0);
        assert_ne!(sender_tid, Tid(0), "thread_create should succeed");
        set_current_tid(sender_tid);

        // No receiver waiting — send should set waiting_sender and block.
        let msg = Message::default();
        let result = send(ep, &msg);
        assert!(result.is_ok());

        // Verify: waiting_sender is set to current_tid.
        assert_eq!(
            get_endpoint_waiting_sender(ep),
            Some(sender_tid),
            "waiting_sender should be set to the sender's Tid"
        );

        // Verify: the thread still exists (not Dead).
        // On host, thread_block may not transition the state to Blocked
        // (requires Running→Blocked, but the thread is Ready), but the
        // thread must still be alive.
        let state = thread_state(sender_tid);
        assert_ne!(
            state,
            ThreadState::Dead,
            "sender thread should still exist after block"
        );

        reset_endpoints();
        set_current_tid(Tid(0));
    }
}

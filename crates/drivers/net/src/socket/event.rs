//! Event type — returned by `Poll::poll_once` / `SocketManager::poll_once`.
//!
//! An [`Event`] associates a [`SocketId`] with the current [`Readiness`] of
//! that socket. Events are produced by the poll loop and consumed by the
//! application's event dispatcher.

use crate::socket::api::SocketId;
use crate::socket::poll::Readiness;

/// A poll event — associates a socket id with its current readiness.
///
/// Produced by [`SocketManager::poll_once`](crate::socket::manager::SocketManager::poll_once)
/// for each registered socket that has a non-empty readiness. The application
/// reads `socket_id` to identify the socket and `readiness` to determine which
/// operations (read/write) are ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event {
    /// The socket id that triggered the event.
    pub socket_id: SocketId,
    /// Which operations are ready on the socket.
    pub readiness: Readiness,
}

impl Event {
    /// Create a new event for the given socket id and readiness.
    pub fn new(socket_id: SocketId, readiness: Readiness) -> Self {
        Self {
            socket_id,
            readiness,
        }
    }

    /// Returns `true` if the event has no readiness bits set.
    ///
    /// Events with empty readiness are typically filtered out by `poll_once`
    /// before being returned, but this method is useful for assertions.
    pub fn is_empty(&self) -> bool {
        self.readiness.is_empty()
    }

    /// Returns `true` if the event includes the readable bit.
    pub fn is_readable(&self) -> bool {
        self.readiness.contains(Readiness::READABLE)
    }

    /// Returns `true` if the event includes the writable bit.
    pub fn is_writable(&self) -> bool {
        self.readiness.contains(Readiness::WRITABLE)
    }

    /// Returns `true` if the event includes the error bit.
    pub fn is_error(&self) -> bool {
        self.readiness.contains(Readiness::ERROR)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_new() {
        let ev = Event::new(5, Readiness::READABLE);
        assert_eq!(ev.socket_id, 5);
        assert_eq!(ev.readiness, Readiness::READABLE);
    }

    #[test]
    fn test_event_new_writable() {
        let ev = Event::new(3, Readiness::WRITABLE);
        assert_eq!(ev.socket_id, 3);
        assert!(ev.is_writable());
        assert!(!ev.is_readable());
    }

    #[test]
    fn test_event_new_combined() {
        let mut r = Readiness::READABLE;
        r.insert(Readiness::WRITABLE);
        let ev = Event::new(1, r);
        assert!(ev.is_readable());
        assert!(ev.is_writable());
        assert!(!ev.is_error());
    }

    #[test]
    fn test_event_is_empty() {
        let ev_empty = Event::new(0, Readiness::EMPTY);
        assert!(ev_empty.is_empty());

        let ev_ready = Event::new(0, Readiness::READABLE);
        assert!(!ev_ready.is_empty());
    }

    #[test]
    fn test_event_is_error() {
        let ev = Event::new(2, Readiness::ERROR);
        assert!(ev.is_error());
        assert!(!ev.is_readable());
        assert!(!ev.is_writable());
    }

    #[test]
    fn test_event_eq() {
        let a = Event::new(1, Readiness::READABLE);
        let b = Event::new(1, Readiness::READABLE);
        assert_eq!(a, b);

        let c = Event::new(2, Readiness::READABLE);
        assert_ne!(a, c);

        let d = Event::new(1, Readiness::WRITABLE);
        assert_ne!(a, d);
    }

    #[test]
    fn test_event_copy() {
        let a = Event::new(1, Readiness::READABLE);
        let b = a; // Copy
        assert_eq!(a, b);
    }

    #[test]
    fn test_event_debug() {
        let ev = Event::new(7, Readiness::READABLE);
        let s = format!("{:?}", ev);
        assert!(s.contains("Event"));
        assert!(s.contains("7"));
    }
}

//! Poll-based multiplexing — `Readiness`, `Interest`, and `Poll`.
//!
//! Provides a registry-based poll mechanism for multiplexing socket IO. Callers
//! register [`Interest`] for each [`SocketId`] with [`Poll::register`], then
//! call [`Poll::check_readiness`] (or [`SocketManager::poll_once`](crate::socket::manager::SocketManager::poll_once))
//! to get the current [`Readiness`] of each registered socket.
//!
//! # Design
//!
//! - **`Readiness(u8)`** is a bitfield newtype with manual bit operations
//!   (no `bitflags` dependency, per "Simplicity First").
//! - **`Interest`** is a plain struct of three bools (readable/writable/error).
//! - **`Poll`** uses `BTreeMap<SocketId, Interest>` for the registry (no
//!   `hashbrown`/`HashMap` dependency).
//!
//! # Non-blocking model
//!
//! v0.29.0 uses non-blocking poll (`poll_once()`). There is no blocking
//! `poll(timeout)` — the application main loop is responsible for calling
//! `poll_once()` + `poll_interface(timestamp_ms)` + sleep/yield. This fits
//! the RTOS event-driven model (蓝图 §8.2).

use alloc::collections::BTreeMap;

use crate::socket::api::SocketId;

// ---------------------------------------------------------------------------
// Readiness
// ---------------------------------------------------------------------------

/// Bitfield indicating which IO operations are ready on a socket.
///
/// A `u8` newtype with manual bit operations. The three meaningful bits are
/// [`READABLE`](Self::READABLE), [`WRITABLE`](Self::WRITABLE), and
/// [`ERROR`](Self::ERROR). Multiple bits can be set simultaneously (e.g. a
/// socket can be both readable and writable).
///
/// # Example
///
/// ```
/// # use eneros_net::socket::Readiness;
/// let mut r = Readiness::READABLE;
/// r.insert(Readiness::WRITABLE);
/// assert!(r.contains(Readiness::READABLE));
/// assert!(r.contains(Readiness::WRITABLE));
/// assert!(!r.contains(Readiness::ERROR));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Readiness(u8);

impl Readiness {
    /// Readable bit — socket has data ready to read (or a connection to accept).
    pub const READABLE: Readiness = Readiness(0x01);
    /// Writable bit — socket can accept data for writing.
    pub const WRITABLE: Readiness = Readiness(0x02);
    /// Error bit — socket has an error condition.
    pub const ERROR: Readiness = Readiness(0x04);
    /// Empty readiness — no operations are ready.
    pub const EMPTY: Readiness = Readiness(0x00);

    /// Returns the empty readiness (no bits set).
    pub fn empty() -> Self {
        Self::EMPTY
    }

    /// Returns `true` if no bits are set.
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Returns `true` if all bits in `other` are set in `self`.
    pub fn contains(self, other: Readiness) -> bool {
        self.0 & other.0 == other.0
    }

    /// Inserts the bits from `other` into `self`.
    pub fn insert(&mut self, other: Readiness) {
        self.0 |= other.0;
    }

    /// Removes the bits in `other` from `self`.
    pub fn remove(&mut self, other: Readiness) {
        self.0 &= !other.0;
    }

    /// Returns the raw `u8` value (for internal use / debugging).
    pub fn bits(self) -> u8 {
        self.0
    }
}

// ---------------------------------------------------------------------------
// Interest
// ---------------------------------------------------------------------------

/// Registration interest — which readiness events the caller cares about.
///
/// Stored in [`Poll`]'s registry per [`SocketId`]. When checking readiness,
/// only events matching the registered interest are reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Interest {
    /// Interest in readable events.
    pub readable: bool,
    /// Interest in writable events.
    pub writable: bool,
    /// Interest in error events.
    pub error: bool,
}

impl Interest {
    /// Create a new interest with all fields set to `false`.
    pub fn none() -> Self {
        Self {
            readable: false,
            writable: false,
            error: false,
        }
    }

    /// Create an interest with only `readable` set.
    pub fn all_readable() -> Self {
        Self {
            readable: true,
            writable: false,
            error: false,
        }
    }

    /// Create an interest with only `writable` set.
    pub fn all_writable() -> Self {
        Self {
            readable: false,
            writable: true,
            error: false,
        }
    }

    /// Create an interest with all fields set to `true`.
    pub fn all() -> Self {
        Self {
            readable: true,
            writable: true,
            error: true,
        }
    }

    /// Builder: set the `readable` field.
    pub fn with_readable(mut self, on: bool) -> Self {
        self.readable = on;
        self
    }

    /// Builder: set the `writable` field.
    pub fn with_writable(mut self, on: bool) -> Self {
        self.writable = on;
        self
    }

    /// Builder: set the `error` field.
    pub fn with_error(mut self, on: bool) -> Self {
        self.error = on;
        self
    }
}

// ---------------------------------------------------------------------------
// Poll
// ---------------------------------------------------------------------------

/// Registry-based poll multiplexer.
///
/// Maintains a map of [`SocketId`] → [`Interest`]. The
/// [`SocketManager`](crate::socket::manager::SocketManager) owns a `Poll`
/// instance and delegates `register`/`deregister`/`modify_interest` to it.
///
/// `check_readiness` is a pure function: given the current socket state
/// (`is_readable`, `is_writable`) and the registered interest, it computes
/// the resulting [`Readiness`]. The actual socket state is queried by
/// `SocketManager::poll_once`, which iterates the registry and calls
/// `check_readiness` for each entry.
pub struct Poll {
    registry: BTreeMap<SocketId, Interest>,
}

impl Poll {
    /// Create a new empty poll registry.
    pub fn new() -> Self {
        Self {
            registry: BTreeMap::new(),
        }
    }

    /// Register interest for a socket.
    ///
    /// If the socket id is already registered, its interest is replaced.
    pub fn register(&mut self, id: SocketId, interest: Interest) {
        self.registry.insert(id, interest);
    }

    /// Deregister a socket from the poll registry.
    ///
    /// No-op if the id is not registered.
    pub fn deregister(&mut self, id: SocketId) {
        self.registry.remove(&id);
    }

    /// Modify the interest for an already-registered socket.
    ///
    /// Equivalent to `register` but semantically distinct: use `register` for
    /// initial registration and `modify` for subsequent changes. If the id is
    /// not yet registered, this registers it.
    pub fn modify(&mut self, id: SocketId, interest: Interest) {
        self.registry.insert(id, interest);
    }

    /// Returns `true` if the socket id is registered.
    pub fn is_registered(&self, id: SocketId) -> bool {
        self.registry.contains_key(&id)
    }

    /// Returns the registered interest for a socket, or `None` if not registered.
    pub fn interest(&self, id: SocketId) -> Option<Interest> {
        self.registry.get(&id).copied()
    }

    /// Returns the number of registered sockets.
    pub fn len(&self) -> usize {
        self.registry.len()
    }

    /// Returns `true` if no sockets are registered.
    pub fn is_empty(&self) -> bool {
        self.registry.is_empty()
    }

    /// Returns an iterator over all registered `(SocketId, Interest)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (SocketId, Interest)> + '_ {
        self.registry.iter().map(|(&id, &interest)| (id, interest))
    }

    /// Check the readiness of a socket given its current state and registered interest.
    ///
    /// - `id`: the socket id to check.
    /// - `is_readable`: whether the socket currently has data ready to read.
    /// - `is_writable`: whether the socket can currently accept data for writing.
    ///
    /// Returns the [`Readiness`] with bits set only for events that are both
    /// **registered** (in `Interest`) and **currently true**. If the socket is
    /// not registered, returns [`Readiness::EMPTY`].
    pub fn check_readiness(&self, id: SocketId, is_readable: bool, is_writable: bool) -> Readiness {
        let Some(interest) = self.registry.get(&id) else {
            return Readiness::EMPTY;
        };
        let mut readiness = Readiness::EMPTY;
        if interest.readable && is_readable {
            readiness.insert(Readiness::READABLE);
        }
        if interest.writable && is_writable {
            readiness.insert(Readiness::WRITABLE);
        }
        // Note: error readiness is not derivable from is_readable/is_writable.
        // SocketManager tracks error state separately (e.g. TcpState::Closed).
        // For now, we only set the error bit if the interest includes error and
        // both readable and writable are false but the socket is closed — this
        // is handled by SocketManager::poll_once which can pass a third flag.
        // Here we keep the API simple: error is never set by check_readiness.
        let _ = interest.error; // suppress unused field warning
        readiness
    }

    /// Clear all registered interests.
    pub fn clear(&mut self) {
        self.registry.clear();
    }
}

impl Default for Poll {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Debug for Poll {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Poll")
            .field("len", &self.registry.len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- Readiness tests ---

    #[test]
    fn test_readiness_default_is_empty() {
        let r = Readiness::default();
        assert!(r.is_empty());
        assert_eq!(r, Readiness::EMPTY);
    }

    #[test]
    fn test_readiness_empty() {
        let r = Readiness::empty();
        assert!(r.is_empty());
        assert_eq!(r.bits(), 0x00);
    }

    #[test]
    fn test_readiness_constants() {
        assert_eq!(Readiness::READABLE.bits(), 0x01);
        assert_eq!(Readiness::WRITABLE.bits(), 0x02);
        assert_eq!(Readiness::ERROR.bits(), 0x04);
        assert_eq!(Readiness::EMPTY.bits(), 0x00);
    }

    #[test]
    fn test_readiness_is_empty() {
        assert!(Readiness::EMPTY.is_empty());
        assert!(!Readiness::READABLE.is_empty());
        assert!(!Readiness::WRITABLE.is_empty());
    }

    #[test]
    fn test_readiness_contains() {
        let r = Readiness::READABLE;
        assert!(r.contains(Readiness::READABLE));
        assert!(!r.contains(Readiness::WRITABLE));
        assert!(!r.contains(Readiness::ERROR));

        let mut combined = Readiness::READABLE;
        combined.insert(Readiness::WRITABLE);
        assert!(combined.contains(Readiness::READABLE));
        assert!(combined.contains(Readiness::WRITABLE));
        assert!(!combined.contains(Readiness::ERROR));
    }

    #[test]
    fn test_readiness_insert() {
        let mut r = Readiness::EMPTY;
        r.insert(Readiness::READABLE);
        assert_eq!(r.bits(), 0x01);
        r.insert(Readiness::WRITABLE);
        assert_eq!(r.bits(), 0x03);
        r.insert(Readiness::ERROR);
        assert_eq!(r.bits(), 0x07);
    }

    #[test]
    fn test_readiness_insert_idempotent() {
        let mut r = Readiness::READABLE;
        r.insert(Readiness::READABLE);
        assert_eq!(r.bits(), 0x01);
    }

    #[test]
    fn test_readiness_remove() {
        let mut r = Readiness::READABLE;
        r.insert(Readiness::WRITABLE);
        r.insert(Readiness::ERROR);
        assert_eq!(r.bits(), 0x07);

        r.remove(Readiness::WRITABLE);
        assert_eq!(r.bits(), 0x05);
        assert!(!r.contains(Readiness::WRITABLE));
        assert!(r.contains(Readiness::READABLE));
        assert!(r.contains(Readiness::ERROR));

        r.remove(Readiness::READABLE);
        r.remove(Readiness::ERROR);
        assert!(r.is_empty());
    }

    #[test]
    fn test_readiness_remove_nonexistent() {
        let mut r = Readiness::READABLE;
        r.remove(Readiness::WRITABLE);
        assert_eq!(r.bits(), 0x01);
    }

    #[test]
    fn test_readiness_all_bits() {
        let mut r = Readiness::EMPTY;
        r.insert(Readiness::READABLE);
        r.insert(Readiness::WRITABLE);
        r.insert(Readiness::ERROR);
        assert!(r.contains(Readiness::READABLE));
        assert!(r.contains(Readiness::WRITABLE));
        assert!(r.contains(Readiness::ERROR));
    }

    #[test]
    fn test_readiness_eq() {
        assert_eq!(Readiness::READABLE, Readiness::READABLE);
        assert_ne!(Readiness::READABLE, Readiness::WRITABLE);
        let mut a = Readiness::READABLE;
        a.insert(Readiness::WRITABLE);
        let mut b = Readiness::WRITABLE;
        b.insert(Readiness::READABLE);
        assert_eq!(a, b);
    }

    // --- Interest tests ---

    #[test]
    fn test_interest_default_all_false() {
        let i = Interest::default();
        assert!(!i.readable);
        assert!(!i.writable);
        assert!(!i.error);
    }

    #[test]
    fn test_interest_none() {
        let i = Interest::none();
        assert_eq!(i, Interest::default());
    }

    #[test]
    fn test_interest_all_readable() {
        let i = Interest::all_readable();
        assert!(i.readable);
        assert!(!i.writable);
        assert!(!i.error);
    }

    #[test]
    fn test_interest_all_writable() {
        let i = Interest::all_writable();
        assert!(!i.readable);
        assert!(i.writable);
        assert!(!i.error);
    }

    #[test]
    fn test_interest_all() {
        let i = Interest::all();
        assert!(i.readable);
        assert!(i.writable);
        assert!(i.error);
    }

    #[test]
    fn test_interest_builder() {
        let i = Interest::none().with_readable(true).with_writable(true);
        assert!(i.readable);
        assert!(i.writable);
        assert!(!i.error);
    }

    #[test]
    fn test_interest_eq() {
        let a = Interest::all_readable();
        let b = Interest::all_readable();
        assert_eq!(a, b);
        assert_ne!(a, Interest::all_writable());
    }

    // --- Poll tests ---

    #[test]
    fn test_poll_new_empty() {
        let p = Poll::new();
        assert!(p.is_empty());
        assert_eq!(p.len(), 0);
    }

    #[test]
    fn test_poll_default_empty() {
        let p = Poll::default();
        assert!(p.is_empty());
    }

    #[test]
    fn test_poll_register() {
        let mut p = Poll::new();
        p.register(1, Interest::all_readable());
        assert!(!p.is_empty());
        assert_eq!(p.len(), 1);
        assert!(p.is_registered(1));
        assert!(!p.is_registered(2));
    }

    #[test]
    fn test_poll_register_multiple() {
        let mut p = Poll::new();
        p.register(1, Interest::all_readable());
        p.register(2, Interest::all_writable());
        p.register(3, Interest::all());
        assert_eq!(p.len(), 3);
    }

    #[test]
    fn test_poll_register_replaces() {
        let mut p = Poll::new();
        p.register(1, Interest::all_readable());
        p.register(1, Interest::all_writable()); // replace
        assert_eq!(p.len(), 1);
        assert_eq!(p.interest(1), Some(Interest::all_writable()));
    }

    #[test]
    fn test_poll_deregister() {
        let mut p = Poll::new();
        p.register(1, Interest::all_readable());
        p.register(2, Interest::all_writable());
        p.deregister(1);
        assert_eq!(p.len(), 1);
        assert!(!p.is_registered(1));
        assert!(p.is_registered(2));
    }

    #[test]
    fn test_poll_deregister_nonexistent() {
        let mut p = Poll::new();
        p.deregister(99); // no-op
        assert!(p.is_empty());
    }

    #[test]
    fn test_poll_modify() {
        let mut p = Poll::new();
        p.register(1, Interest::all_readable());
        p.modify(1, Interest::all());
        assert_eq!(p.interest(1), Some(Interest::all()));
    }

    #[test]
    fn test_poll_modify_unregistered() {
        let mut p = Poll::new();
        p.modify(5, Interest::all_readable());
        assert!(p.is_registered(5));
        assert_eq!(p.interest(5), Some(Interest::all_readable()));
    }

    #[test]
    fn test_poll_interest() {
        let mut p = Poll::new();
        p.register(1, Interest::all_readable());
        assert_eq!(p.interest(1), Some(Interest::all_readable()));
        assert_eq!(p.interest(2), None);
    }

    #[test]
    fn test_poll_clear() {
        let mut p = Poll::new();
        p.register(1, Interest::all_readable());
        p.register(2, Interest::all_writable());
        p.clear();
        assert!(p.is_empty());
    }

    #[test]
    fn test_poll_check_readiness_registered_readable() {
        let mut p = Poll::new();
        p.register(1, Interest::all_readable());
        let r = p.check_readiness(1, true, false);
        assert!(r.contains(Readiness::READABLE));
        assert!(!r.contains(Readiness::WRITABLE));
    }

    #[test]
    fn test_poll_check_readiness_registered_writable() {
        let mut p = Poll::new();
        p.register(1, Interest::all_writable());
        let r = p.check_readiness(1, false, true);
        assert!(!r.contains(Readiness::READABLE));
        assert!(r.contains(Readiness::WRITABLE));
    }

    #[test]
    fn test_poll_check_readiness_both() {
        let mut p = Poll::new();
        p.register(1, Interest::all());
        let r = p.check_readiness(1, true, true);
        assert!(r.contains(Readiness::READABLE));
        assert!(r.contains(Readiness::WRITABLE));
    }

    #[test]
    fn test_poll_check_readiness_not_ready() {
        let mut p = Poll::new();
        p.register(1, Interest::all());
        let r = p.check_readiness(1, false, false);
        assert!(r.is_empty());
    }

    #[test]
    fn test_poll_check_readiness_unregistered() {
        let p = Poll::new();
        let r = p.check_readiness(99, true, true);
        assert!(r.is_empty());
    }

    #[test]
    fn test_poll_check_readiness_interest_filter() {
        // Registered for readable only, but socket is writable -> no events.
        let mut p = Poll::new();
        p.register(1, Interest::all_readable());
        let r = p.check_readiness(1, false, true);
        assert!(r.is_empty());
    }

    #[test]
    fn test_poll_iter() {
        let mut p = Poll::new();
        p.register(1, Interest::all_readable());
        p.register(2, Interest::all_writable());
        p.register(3, Interest::all());
        let entries: Vec<(SocketId, Interest)> = p.iter().collect();
        assert_eq!(entries.len(), 3);
        // BTreeMap iterates in sorted key order
        assert_eq!(entries[0].0, 1);
        assert_eq!(entries[1].0, 2);
        assert_eq!(entries[2].0, 3);
    }

    #[test]
    fn test_poll_debug() {
        let p = Poll::new();
        let s = format!("{:?}", p);
        assert!(s.contains("Poll"));
    }
}

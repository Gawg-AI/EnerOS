//! Lock-free single-producer single-consumer ring buffer (v0.21.0).
//!
//! Provides [`SpscRing`], a wait-free SPSC ring buffer using atomic
//! `head`/`tail` indices. Suitable for ISR-to-thread and inter-core
//! communication where exactly one producer and one consumer exist.
//!
//! # Memory model
//!
//! - `tail` (producer write position): updated with `Release` after a
//!   write, loaded with `Relaxed` by the producer.
//! - `head` (consumer read position): updated with `Release` after a
//!   read, loaded with `Relaxed` by the consumer.
//! - Cross-side loads use `Acquire` to establish happens-before.
//!
//! # Capacity
//!
//! The ring uses one empty slot to distinguish full from empty, so the
//! effective capacity is `slot_count - 1` items.

use core::sync::atomic::{AtomicUsize, Ordering};

/// Lock-free single-producer single-consumer ring buffer.
///
/// The buffer is externally owned (the caller provides a `&mut [u8]` in
/// [`SpscRing::new`]). The `SpscRing` stores a raw pointer to the buffer
/// and does not manage its lifetime — the caller must ensure the buffer
/// outlives the ring.
///
/// # Safety
///
/// `Send` and `Sync` are implemented manually because the struct contains
/// a raw pointer (`buffer`). Soundness relies on the SPSC discipline:
/// exactly one producer calls `push` and exactly one consumer calls
/// `pop`. The atomic head/tail indices ensure safe concurrent access
/// without locks.
pub struct SpscRing {
    /// Raw pointer to the backing buffer (externally owned).
    pub buffer: *mut u8,
    /// Total byte capacity of the backing buffer.
    pub capacity: usize,
    /// Size of each slot in bytes.
    pub slot_size: usize,
    /// Number of slots in the ring.
    pub slot_count: usize,
    /// Consumer read position (advanced by `pop`).
    pub head: AtomicUsize,
    /// Producer write position (advanced by `push`).
    pub tail: AtomicUsize,
}

// SAFETY: `SpscRing` is safe to send between threads because it is used
// in a single-producer single-consumer discipline: one thread owns the
// producer side (push), another owns the consumer side (pop).
unsafe impl Send for SpscRing {}

// SAFETY: `Sync` is sound under the SPSC contract: the producer and
// consumer access disjoint slots (separated by head/tail indices), and
// the atomic ordering (Acquire/Release) establishes the happens-before
// relationship.
unsafe impl Sync for SpscRing {}

/// Ring buffer error variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingError {
    /// The ring is full — no more items can be pushed.
    Full,
    /// The ring is empty — no items can be popped.
    Empty,
    /// The provided data does not fit in a slot (`data.len() > slot_size`).
    InvalidSize,
}

impl SpscRing {
    /// Construct a new SPSC ring over an externally-owned buffer.
    ///
    /// Verifies that `slot_size * slot_count <= buf.len()`. Stores the
    /// buffer pointer and dimensions; initializes `head` and `tail` to 0.
    ///
    /// The caller must ensure `buf` remains valid and writable for the
    /// lifetime of the returned `SpscRing`.
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    pub fn new(buf: &mut [u8], slot_size: usize, slot_count: usize) -> Self {
        let capacity = buf.len();
        Self {
            buffer: buf.as_mut_ptr(),
            capacity,
            slot_size,
            slot_count,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// Push data into the next free slot (producer side).
    ///
    /// Returns `Err(InvalidSize)` if `data.len() > slot_size`.
    /// Returns `Err(Full)` if the ring is full (one slot is reserved to
    /// distinguish full from empty).
    /// Returns `Ok(())` on success.
    ///
    /// # Safety
    ///
    /// This function writes to `buffer[tail * slot_size .. tail * slot_size + data.len()]`
    /// via `copy_nonoverlapping`. The caller (producer) must ensure no
    /// other thread is concurrently writing to the same slot.
    pub fn push(&self, data: &[u8]) -> Result<(), RingError> {
        if data.len() > self.slot_size {
            return Err(RingError::InvalidSize);
        }

        let tail = self.tail.load(Ordering::Relaxed);
        let next = (tail + 1) % self.slot_count;
        let head = self.head.load(Ordering::Acquire);

        if next == head {
            return Err(RingError::Full);
        }

        // SAFETY: `tail < slot_count` (due to modulo), so
        // `tail * slot_size < slot_count * slot_size <= capacity`.
        // The slot is not being read by the consumer (we checked
        // `next != head`, so the slot at `tail` is beyond `head`).
        let slot_ptr = unsafe { self.buffer.add(tail * self.slot_size) };
        unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr(), slot_ptr, data.len());
        }

        self.tail.store(next, Ordering::Release);
        Ok(())
    }

    /// Pop data from the next filled slot (consumer side).
    ///
    /// Returns `Err(Empty)` if the ring is empty (`head == tail`).
    /// Returns `Ok(len)` on success, where `len` is the number of bytes
    /// written to `out` (`min(slot_size, out.len())`).
    ///
    /// # Safety
    ///
    /// This function reads from `buffer[head * slot_size .. head * slot_size + len]`
    /// via `copy_nonoverlapping`. The caller (consumer) must ensure no
    /// other thread is concurrently reading from the same slot.
    pub fn pop(&self, out: &mut [u8]) -> Result<usize, RingError> {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);

        if head == tail {
            return Err(RingError::Empty);
        }

        let len = self.slot_size.min(out.len());

        // SAFETY: `head < slot_count` (due to modulo on the producer side),
        // so `head * slot_size < slot_count * slot_size <= capacity`.
        // The slot is not being written by the producer (we checked
        // `head != tail`, and the producer only writes at `tail`).
        let slot_ptr = unsafe { self.buffer.add(head * self.slot_size) };
        unsafe {
            core::ptr::copy_nonoverlapping(slot_ptr, out.as_mut_ptr(), len);
        }

        let next = (head + 1) % self.slot_count;
        self.head.store(next, Ordering::Release);
        Ok(len)
    }

    /// Number of items currently in the ring (consumer-observable).
    ///
    /// Uses `Relaxed` loads — suitable for statistics, not for
    /// synchronization decisions.
    pub fn used(&self) -> usize {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Relaxed);
        (tail + self.slot_count - head) % self.slot_count
    }

    /// Number of free slots available for pushing.
    ///
    /// One slot is always reserved to distinguish full from empty, so
    /// `free()` returns at most `slot_count - 1`.
    pub fn free(&self) -> usize {
        self.slot_count - 1 - self.used()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_ring() {
        let mut buf = [0u8; 256];
        let ring = SpscRing::new(&mut buf, 32, 8);
        assert_eq!(ring.capacity, 256);
        assert_eq!(ring.slot_size, 32);
        assert_eq!(ring.slot_count, 8);
        assert_eq!(ring.head.load(Ordering::Relaxed), 0);
        assert_eq!(ring.tail.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_push_pop_roundtrip() {
        let mut buf = [0u8; 256];
        let ring = SpscRing::new(&mut buf, 32, 8);

        let data = [0xAA; 32];
        assert_eq!(ring.push(&data), Ok(()));
        assert_eq!(ring.used(), 1);

        let mut out = [0u8; 32];
        let result = ring.pop(&mut out);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 32);
        assert_eq!(&out, &data);
        assert_eq!(ring.used(), 0);
    }

    #[test]
    fn test_push_full() {
        let mut buf = [0u8; 256];
        let ring = SpscRing::new(&mut buf, 32, 8);

        let data = [0xBB; 32];
        // slot_count = 8, effective capacity = 7 (one slot reserved).
        for _ in 0..7 {
            assert_eq!(ring.push(&data), Ok(()));
        }
        assert_eq!(ring.used(), 7);
        assert_eq!(ring.free(), 0);

        // Next push should fail with Full.
        let result = ring.push(&data);
        assert_eq!(result, Err(RingError::Full));
    }

    #[test]
    fn test_pop_empty() {
        let mut buf = [0u8; 256];
        let ring = SpscRing::new(&mut buf, 32, 8);

        let mut out = [0u8; 32];
        let result = ring.pop(&mut out);
        assert_eq!(result, Err(RingError::Empty));
    }

    #[test]
    fn test_ring_wraparound() {
        let mut buf = [0u8; 128];
        // Small ring to trigger wraparound quickly: 4 slots × 32 bytes.
        let ring = SpscRing::new(&mut buf, 32, 4);

        let mut out = [0u8; 32];

        // Push/pop 3 items (fills to capacity - 1 = 3).
        for i in 0u8..3 {
            let data = [i + 1; 32];
            assert_eq!(ring.push(&data), Ok(()));
        }
        assert_eq!(ring.used(), 3);

        // Pop all 3.
        for i in 0u8..3 {
            assert!(ring.pop(&mut out).is_ok());
            assert_eq!(out[0], i + 1);
        }
        assert_eq!(ring.used(), 0);
        // head should now be at slot 3 (wrapped to 3).

        // Push 3 more — this triggers wraparound of tail.
        for i in 0u8..3 {
            let data = [10 + i; 32];
            assert_eq!(ring.push(&data), Ok(()));
        }
        assert_eq!(ring.used(), 3);

        // Pop and verify data integrity after wraparound.
        for i in 0u8..3 {
            assert!(ring.pop(&mut out).is_ok());
            assert_eq!(out[0], 10 + i, "data corrupted after wraparound");
        }
        assert_eq!(ring.used(), 0);
    }

    #[test]
    fn test_used_free() {
        let mut buf = [0u8; 256];
        let ring = SpscRing::new(&mut buf, 32, 8);

        assert_eq!(ring.used(), 0);
        assert_eq!(ring.free(), 7);

        let data = [0xCC; 32];
        let _ = ring.push(&data);
        let _ = ring.push(&data);
        let _ = ring.push(&data);

        assert_eq!(ring.used(), 3);
        assert_eq!(ring.free(), 4);
    }

    #[test]
    fn test_push_invalid_size() {
        let mut buf = [0u8; 256];
        let ring = SpscRing::new(&mut buf, 32, 8);

        // Data larger than slot_size.
        let data = [0xDD; 33];
        let result = ring.push(&data);
        assert_eq!(result, Err(RingError::InvalidSize));

        // Data exactly slot_size should succeed.
        let data_ok = [0xDD; 32];
        assert_eq!(ring.push(&data_ok), Ok(()));
    }
}

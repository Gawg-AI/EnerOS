use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Lock-free SPSC (Single Producer Single Consumer) queue for command
/// passing from the general domain to the RT domain.
///
/// Backed by a ring buffer of `UnsafeCell<MaybeUninit<T>>` with atomic
/// head/tail indices. The single producer owns `head`; the single
/// consumer owns `tail`. No locks, no CAS — just Release/Acquire loads
/// and stores. Usable capacity is `CAPACITY - 1` (one slot reserved to
/// distinguish full from empty).
pub struct RtCommandQueue<T, const CAPACITY: usize> {
    buffer: Box<[UnsafeCell<MaybeUninit<T>>]>,
    head: AtomicUsize,
    tail: AtomicUsize,
}

impl<T, const CAPACITY: usize> RtCommandQueue<T, CAPACITY> {
    pub fn new() -> Self {
        let buffer: Vec<_> = (0..CAPACITY)
            .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
            .collect();
        Self {
            buffer: buffer.into_boxed_slice(),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// Attempts to enqueue `item`. Returns `Err(item)` if the queue is full.
    pub fn try_push(&self, item: T) -> Result<(), T> {
        let head = self.head.load(Ordering::Relaxed);
        let next_head = (head + 1) % CAPACITY;
        let tail = self.tail.load(Ordering::Acquire);

        if next_head == tail {
            return Err(item); // Queue full
        }

        // SAFETY: `head` is exclusively owned by the single producer, so no
        // other thread writes to this slot concurrently. The slot is either
        // never-written or has been consumed by the single consumer (which
        // advanced `tail` with Release before we observed it via Acquire),
        // so it holds no live value. `get_unchecked` is safe because
        // `head < CAPACITY` (guaranteed by the modulo above).
        unsafe {
            (*self.buffer.get_unchecked(head))
                .get()
                .write(MaybeUninit::new(item));
        }

        self.head.store(next_head, Ordering::Release);
        Ok(())
    }

    /// Attempts to dequeue an item. Returns `None` if the queue is empty.
    pub fn try_pop(&self) -> Option<T> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);

        if tail == head {
            return None; // Queue empty
        }

        // SAFETY: `tail` is exclusively owned by the single consumer, so no
        // other thread reads from this slot concurrently. The slot was
        // written by the single producer, which advanced `head` with
        // Release before we observed it via Acquire, so it holds a live
        // value. `assume_init_read` moves the value out (bit-copy), leaving
        // the slot logically uninitialized — it must not be dropped again.
        // `get_unchecked` is safe because `tail < CAPACITY`.
        let item = unsafe { (*self.buffer.get_unchecked(tail).get()).assume_init_read() };

        let next_tail = (tail + 1) % CAPACITY;
        self.tail.store(next_tail, Ordering::Release);

        Some(item)
    }
}

impl<T, const CAPACITY: usize> Default for RtCommandQueue<T, CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, const CAPACITY: usize> Drop for RtCommandQueue<T, CAPACITY> {
    fn drop(&mut self) {
        // Drop any unconsumed elements. We have &mut self, so no atomics
        // are needed — use get_mut to access the indices directly.
        let tail = *self.tail.get_mut();
        let head = *self.head.get_mut();

        let mut i = tail;
        while i != head {
            // SAFETY: slots from `tail` to `head` (exclusive) contain live
            // values that were pushed but not yet popped. `get_unchecked_mut`
            // is safe because `i < CAPACITY` (maintained by the modulo).
            // `assume_init_drop` runs the destructor and marks the slot as
            // logically uninitialized.
            unsafe {
                (*self.buffer.get_unchecked_mut(i).get()).assume_init_drop();
            }
            i = (i + 1) % CAPACITY;
        }
    }
}

// SAFETY: RtCommandQueue is SPSC. The single producer exclusively owns
// `head` and the slots it writes; the single consumer exclusively owns
// `tail` and the slots it reads. The head/tail index protocol ensures
// producer and consumer never touch the same slot. `T: Send` is required
// because items are moved from the producer thread to the consumer thread.
unsafe impl<T: Send, const N: usize> Send for RtCommandQueue<T, N> {}

// SAFETY: `&RtCommandQueue` can be shared between threads because the
// producer and consumer only access disjoint slots (enforced by the
// index protocol). `T: Send` is required because items cross thread
// boundaries via push/pop.
unsafe impl<T: Send, const N: usize> Sync for RtCommandQueue<T, N> {}

/// Result channel for RT domain -> general domain communication.
///
/// Uses a seqlock-style publish/subscribe pattern: the publisher bumps
/// the version to odd before writing, then to even after writing; readers
/// retry until they observe a stable even version. `T: Clone + Default`
/// is required because readers clone the current value out.
pub struct RtResultChannel<T: Clone + Default> {
    value: UnsafeCell<T>,
    version: AtomicU64,
}

impl<T: Clone + Default> RtResultChannel<T> {
    pub fn new() -> Self {
        Self {
            value: UnsafeCell::new(T::default()),
            version: AtomicU64::new(0),
        }
    }

    /// Publishes a new result. The version is bumped to odd before the
    /// write and to even after, so concurrent readers can detect an
    /// in-progress write and retry.
    pub fn publish(&self, result: T) {
        // Bump to odd — signals write-in-progress to concurrent readers.
        self.version.fetch_add(1, Ordering::Release);

        // SAFETY: The publisher has exclusive write access to the value.
        // Concurrent readers will observe the odd version and retry until
        // the write completes (version becomes even again).
        unsafe {
            *self.value.get() = result;
        }

        // Bump to even — signals write-complete.
        self.version.fetch_add(1, Ordering::Release);
    }

    /// Reads the latest published value and its version. Retries until a
    /// consistent (non-torn) read is observed via the seqlock protocol.
    pub fn read(&self) -> (T, u64) {
        loop {
            let v1 = self.version.load(Ordering::Acquire);

            // SAFETY: We clone the value out. If a concurrent publish is
            // in progress (odd version) or completes during the clone
            // (version changes), the check below detects it and we retry,
            // discarding the potentially-torn clone. This is safe for T
            // where reading partially-written bits cannot cause UB (e.g.,
            // T: Copy or fixed-size plain-old-data).
            let value = unsafe { (*self.value.get()).clone() };

            let v2 = self.version.load(Ordering::Relaxed);

            if v1 == v2 && v1.is_multiple_of(2) {
                return (value, v1);
            }
            // Version changed or write in progress — retry.
        }
    }
}

impl<T: Clone + Default> Default for RtResultChannel<T> {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: RtResultChannel uses a seqlock for safe publication. The
// publisher writes the value between two version bumps (Release); readers
// load the version (Acquire), clone the value, and re-check. `T: Send`
// is required because the value crosses thread boundaries.
unsafe impl<T: Send + Clone + Default> Send for RtResultChannel<T> {}

// SAFETY: `&RtResultChannel` can be shared between threads. The seqlock
// protocol ensures readers see a consistent (value, version) pair.
// `T: Send + Clone + Default` is required for safe cross-thread access.
unsafe impl<T: Send + Clone + Default> Sync for RtResultChannel<T> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spsc_queue_basic() {
        let queue: RtCommandQueue<i32, 16> = RtCommandQueue::new();
        assert!(queue.try_push(42).is_ok());
        assert_eq!(queue.try_pop(), Some(42));
        assert_eq!(queue.try_pop(), None);
    }

    #[test]
    fn test_spsc_queue_full() {
        let queue: RtCommandQueue<i32, 2> = RtCommandQueue::new();
        assert!(queue.try_push(1).is_ok());
        assert!(queue.try_push(2).is_err()); // Full (CAPACITY-1 usable)
    }

    #[test]
    fn test_result_channel() {
        let channel: RtResultChannel<i32> = RtResultChannel::new();
        let (_, v1) = channel.read();
        channel.publish(42);
        let (val, v2) = channel.read();
        assert_eq!(val, 42);
        assert!(v2 > v1);
    }

    #[test]
    fn test_spsc_queue_sequential() {
        let queue: RtCommandQueue<i32, 16> = RtCommandQueue::new();
        for i in 0..1000i32 {
            assert!(queue.try_push(i).is_ok());
            assert_eq!(queue.try_pop(), Some(i));
        }
        assert_eq!(queue.try_pop(), None);
    }

    #[test]
    fn test_spsc_queue_drop_drops_remaining() {
        use std::sync::atomic::{AtomicUsize, Ordering as SeqOrd};
        use std::sync::Arc;

        #[derive(Debug)]
        struct DropCounter {
            counter: Arc<AtomicUsize>,
        }
        impl Drop for DropCounter {
            fn drop(&mut self) {
                self.counter.fetch_add(1, SeqOrd::SeqCst);
            }
        }

        let counter = Arc::new(AtomicUsize::new(0));
        {
            let queue: RtCommandQueue<DropCounter, 8> = RtCommandQueue::new();
            for _ in 0..3 {
                queue
                    .try_push(DropCounter {
                        counter: counter.clone(),
                    })
                    .unwrap();
            }
            let _popped = queue.try_pop().unwrap();
            // _popped is still alive; dropping the queue should drop only
            // the 2 remaining items.
            drop(queue);
            assert_eq!(counter.load(SeqOrd::SeqCst), 2);
        }
        // After the block, all 3 DropCounters have been dropped.
        assert_eq!(counter.load(SeqOrd::SeqCst), 3);
    }
}

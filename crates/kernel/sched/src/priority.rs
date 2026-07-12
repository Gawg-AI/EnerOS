//! Priority-based thread selection — Phase 0 P0-F (v0.18.0).
//!
//! Provides [`select_next_by_priority`] — given a slice of `Tid`s and a
//! priority lookup closure, returns the highest-priority (lowest `u8`)
//! thread. Ties broken by FIFO (first occurrence wins).
//!
//! Also provides [`PriorityQueue`] — a fixed-capacity, array-backed priority
//! queue with no external dependencies.

use crate::Tid;

/// Select the next thread by priority.
///
/// Iterates `tids`, looks up each thread's priority via `get_prio`,
/// and returns the `Tid` with the smallest priority value (0 = highest).
/// Ties are broken by input order (FIFO): the first thread at the
/// winning priority wins.
pub fn select_next_by_priority(tids: &[Tid], get_prio: impl Fn(Tid) -> u8) -> Option<Tid> {
    let mut best: Option<(Tid, u8)> = None;
    for &tid in tids {
        let prio = get_prio(tid);
        match best {
            None => best = Some((tid, prio)),
            Some((_, bp)) if prio < bp => best = Some((tid, prio)),
            _ => {}
        }
    }
    best.map(|(t, _)| t)
}

/// Capacity of the fixed-array [`PriorityQueue`].
pub const PRIO_QUEUE_CAPACITY: usize = 64;

/// Fixed-capacity, array-backed priority queue.
///
/// Stores up to [`PRIO_QUEUE_CAPACITY`] `(Tid, priority)` pairs. `push`
/// inserts in slot order; `pop` scans for the highest-priority (lowest `u8`)
/// entry and removes it. No external dependencies, no heap allocation.
#[derive(Debug, Clone)]
pub struct PriorityQueue {
    entries: [(Tid, u8); PRIO_QUEUE_CAPACITY],
    count: usize,
}

impl PriorityQueue {
    /// Construct an empty priority queue. `const fn` for static init.
    pub const fn new() -> Self {
        Self {
            entries: [(Tid(0), 0); PRIO_QUEUE_CAPACITY],
            count: 0,
        }
    }

    /// Push `(tid, prio)` onto the queue.
    ///
    /// Finds the first empty slot (where `tid == Tid(0)`, the reserved
    /// "invalid" sentinel) and inserts there. This preserves insertion
    /// order across push/pop cycles, so [`pop`](Self::pop) can break ties
    /// by FIFO.
    ///
    /// Returns `true` if inserted, `false` if the queue is full.
    pub fn push(&mut self, tid: Tid, prio: u8) -> bool {
        if self.count >= PRIO_QUEUE_CAPACITY {
            return false;
        }
        // Scan for the first empty slot (Tid(0) is the reserved sentinel
        // initialized in `new()` and written back by `pop()`).
        for i in 0..PRIO_QUEUE_CAPACITY {
            if self.entries[i].0 .0 == 0 {
                self.entries[i] = (tid, prio);
                self.count += 1;
                return true;
            }
        }
        false
    }

    /// Pop the highest-priority (lowest `u8`) entry.
    ///
    /// Returns `Some(Tid)` if the queue is non-empty, or `None` if empty.
    /// Ties are broken by FIFO: the first occupied slot at the winning
    /// priority wins, because slots are scanned in index order and `<`
    /// (strict less-than) keeps the earliest match.
    ///
    /// The freed slot is marked empty by writing back `(Tid(0), 0)` so
    /// future `push` calls can reuse it without disturbing the order of
    /// the remaining entries.
    pub fn pop(&mut self) -> Option<Tid> {
        if self.count == 0 {
            return None;
        }
        // Find the index of the highest-priority (lowest u8) occupied entry.
        // `best_prio` starts at `u8::MAX` so the first occupied slot always wins.
        let mut best_idx: Option<usize> = None;
        let mut best_prio = u8::MAX;
        for i in 0..PRIO_QUEUE_CAPACITY {
            let (tid, prio) = self.entries[i];
            if tid.0 != 0 && prio < best_prio {
                best_prio = prio;
                best_idx = Some(i);
            }
        }
        match best_idx {
            Some(idx) => {
                let tid = self.entries[idx].0;
                self.entries[idx] = (Tid(0), 0);
                self.count -= 1;
                Some(tid)
            }
            None => None,
        }
    }

    /// Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Number of entries currently in the queue.
    pub fn len(&self) -> usize {
        self.count
    }
}

impl Default for PriorityQueue {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_select_empty() {
        let tids: &[Tid] = &[];
        let result = select_next_by_priority(tids, |_| 0);
        assert_eq!(result, None);
    }

    #[test]
    fn test_select_single() {
        let tids = [Tid(5)];
        let result = select_next_by_priority(&tids, |_| 3);
        assert_eq!(result, Some(Tid(5)));
    }

    #[test]
    fn test_select_highest_priority() {
        // Tid(1): prio 5, Tid(2): prio 1, Tid(3): prio 9
        // Lowest u8 = highest priority → Tid(2) wins.
        let tids = [Tid(1), Tid(2), Tid(3)];
        let prios = [5u8, 1, 9];
        let result = select_next_by_priority(&tids, |tid| prios[(tid.0 - 1) as usize]);
        assert_eq!(result, Some(Tid(2)));
    }

    #[test]
    fn test_select_fifo_tie() {
        // All same priority → first (FIFO) wins.
        let tids = [Tid(10), Tid(20), Tid(30)];
        let result = select_next_by_priority(&tids, |_| 4);
        assert_eq!(result, Some(Tid(10)));
    }

    #[test]
    fn test_priority_queue_push_pop() {
        let mut q = PriorityQueue::new();
        assert!(q.is_empty());
        assert_eq!(q.len(), 0);

        assert!(q.push(Tid(1), 5));
        assert!(q.push(Tid(2), 1));
        assert!(q.push(Tid(3), 3));
        assert_eq!(q.len(), 3);
        assert!(!q.is_empty());

        // Pop returns highest priority (lowest u8) first.
        assert_eq!(q.pop(), Some(Tid(2))); // prio 1
        assert_eq!(q.pop(), Some(Tid(3))); // prio 3
        assert_eq!(q.pop(), Some(Tid(1))); // prio 5
        assert_eq!(q.pop(), None);
        assert!(q.is_empty());
    }

    #[test]
    fn test_priority_queue_full() {
        let mut q = PriorityQueue::new();
        for i in 0..PRIO_QUEUE_CAPACITY as u32 {
            assert!(q.push(Tid(i + 1), (i % 10) as u8));
        }
        assert_eq!(q.len(), PRIO_QUEUE_CAPACITY);
        // Next push fails (full).
        assert!(!q.push(Tid(999), 0));
        assert_eq!(q.len(), PRIO_QUEUE_CAPACITY);
    }

    #[test]
    fn test_priority_queue_fifo_tie() {
        let mut q = PriorityQueue::new();
        // Same priority — FIFO order on pop.
        q.push(Tid(10), 3);
        q.push(Tid(20), 3);
        q.push(Tid(30), 3);
        assert_eq!(q.pop(), Some(Tid(10)));
        assert_eq!(q.pop(), Some(Tid(20)));
        assert_eq!(q.pop(), Some(Tid(30)));
    }

    #[test]
    fn test_priority_queue_default() {
        let q = PriorityQueue::default();
        assert!(q.is_empty());
        assert_eq!(q.len(), 0);
    }
}

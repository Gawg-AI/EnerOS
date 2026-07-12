//! Load balancer for per-core run queues.
//!
//! The balancer periodically scans the per-core run queues and migrates a
//! single thread from the busiest core to the idlest core whenever their load
//! difference exceeds a configurable threshold. This implements the
//! work-stealing-style rebalancing referenced by the v0.16.0 spec scenarios
//! "负载均衡触发" and the blueprint §4.3 flowchart.
//!
//! Migration is best-effort: if dequeuing or enqueuing fails (e.g. the
//! source queue is concurrently drained), the balancer leaves the queues
//! untouched and returns — it never panics (D4: no_std scheduling paths must
//! not crash).
//!
//! Per the D2 design decision, this module depends only on `core::*`.

use crate::percore::PerCoreRq;

/// Maximum number of cores the balancer can manage (matches `Scheduler`).
const MAX_CORES: usize = 8;

/// Workload balancer.
///
/// Scans per-core run queues and migrates a thread from the busiest core to
/// the idlest core when their load difference exceeds `threshold`. The
/// `interval_ms` field records the desired period between balance passes
/// (the actual timer hookup is performed by the caller/kernel).
#[derive(Debug, Clone, Copy)]
pub struct Balancer {
    /// Migration threshold: migrate only when `max_load - min_load > threshold`.
    pub threshold: usize,
    /// Desired interval between balance passes, in milliseconds.
    pub interval_ms: u32,
}

impl Balancer {
    /// Construct a balancer with the given threshold and interval.
    pub fn new(threshold: usize, interval_ms: u32) -> Self {
        Self {
            threshold,
            interval_ms,
        }
    }

    /// Default balancer: threshold = 2, interval = 10 ms (per blueprint §4.5).
    pub fn default_balancer() -> Self {
        Self {
            threshold: 2,
            interval_ms: 10,
        }
    }
}

impl Default for Balancer {
    fn default() -> Self {
        Self::default_balancer()
    }
}

impl Balancer {
    /// Find the busiest and idlest cores among the first `core_count` queues.
    ///
    /// Returns `(max_core, min_core)` — both are indices into `rqs`. When
    /// `core_count == 0`, returns `(0, 0)`. When all cores have equal load,
    /// `max_core` and `min_core` are distinct indices only if `core_count > 1`
    /// (otherwise they coincide at index 0).
    pub fn find_busiest(&self, rqs: &[PerCoreRq; MAX_CORES], core_count: u32) -> (usize, usize) {
        if core_count == 0 {
            return (0, 0);
        }
        let n = core_count as usize;
        let mut max_load = 0usize;
        let mut min_load = usize::MAX;
        let mut max_core = 0usize;
        let mut min_core = 0usize;
        for (i, rq) in rqs.iter().enumerate().take(n) {
            let load = rq.load();
            if load > max_load {
                max_load = load;
                max_core = i;
            }
            if load < min_load {
                min_load = load;
                min_core = i;
            }
        }
        (max_core, min_core)
    }

    /// Attempt a single balance pass.
    ///
    /// Steps:
    /// 1. Scan cores `0..core_count` to find the busiest and idlest.
    /// 2. If `max_load - min_load > threshold` and `max_core != min_core`:
    ///    3. Lock the busiest core's RQ, dequeue one thread, unlock.
    ///    4. If a thread was dequeued, lock the idlest core's RQ, enqueue, unlock.
    /// 5. Migration failure (empty source RQ, etc.) is silently tolerated —
    ///    the balancer never panics.
    ///
    /// If `core_count < 2`, no balancing is possible and the call is a no-op.
    pub fn balance(&self, rqs: &mut [PerCoreRq; MAX_CORES], core_count: u32) {
        if core_count < 2 {
            return;
        }
        let n = core_count as usize;

        // Step 1: find busiest and idlest.
        let mut max_load = 0usize;
        let mut min_load = usize::MAX;
        let mut max_core = 0usize;
        let mut min_core = 0usize;
        for (i, rq) in rqs.iter().enumerate().take(n) {
            let load = rq.load();
            if load > max_load {
                max_load = load;
                max_core = i;
            }
            if load < min_load {
                min_load = load;
                min_core = i;
            }
        }

        // Step 2: check threshold and distinctness.
        if max_core == min_core {
            return;
        }
        // max_load >= min_load guaranteed when core_count >= 1, but guard the
        // subtraction defensively in case of a future refactor.
        let Some(diff) = max_load.checked_sub(min_load) else {
            return;
        };
        if diff <= self.threshold {
            return;
        }

        // Step 3: dequeue one thread from the busiest core (under its lock).
        let tid = {
            rqs[max_core].lock.lock();
            let t = rqs[max_core].dequeue();
            rqs[max_core].lock.unlock();
            t
        };

        // Step 4: enqueue it onto the idlest core (under its lock).
        // Step 5: if dequeue returned None, do nothing (no crash).
        if let Some(tid) = tid {
            rqs[min_core].lock.lock();
            rqs[min_core].enqueue(tid);
            rqs[min_core].lock.unlock();
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::percore::Tid;

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn test_balancer_default_values() {
        let b = Balancer::default();
        assert_eq!(b.threshold, 2);
        assert_eq!(b.interval_ms, 10);
    }

    #[test]
    fn test_balancer_new_custom_values() {
        let b = Balancer::new(5, 25);
        assert_eq!(b.threshold, 5);
        assert_eq!(b.interval_ms, 25);
    }

    #[test]
    fn test_find_busiest_identifies_extremes() {
        let _g = lock();
        let mut rqs = [
            PerCoreRq::new(0),
            PerCoreRq::new(1),
            PerCoreRq::new(2),
            PerCoreRq::new(3),
            PerCoreRq::new(4),
            PerCoreRq::new(5),
            PerCoreRq::new(6),
            PerCoreRq::new(7),
        ];
        // Core 0: 3 threads, Core 1: 1 thread, Core 2: 0 threads.
        rqs[0].enqueue(Tid(10));
        rqs[0].enqueue(Tid(11));
        rqs[0].enqueue(Tid(12));
        rqs[1].enqueue(Tid(20));

        let b = Balancer::default();
        let (max_core, min_core) = b.find_busiest(&rqs, 4);
        assert_eq!(max_core, 0); // 3 threads
        assert_eq!(min_core, 2); // 0 threads
    }

    #[test]
    fn test_find_busiest_zero_cores() {
        let _g = lock();
        let rqs = [
            PerCoreRq::new(0),
            PerCoreRq::new(1),
            PerCoreRq::new(2),
            PerCoreRq::new(3),
            PerCoreRq::new(4),
            PerCoreRq::new(5),
            PerCoreRq::new(6),
            PerCoreRq::new(7),
        ];
        let b = Balancer::default();
        let (max_core, min_core) = b.find_busiest(&rqs, 0);
        assert_eq!(max_core, 0);
        assert_eq!(min_core, 0);
    }

    #[test]
    fn test_balance_migrates_when_above_threshold() {
        let _g = lock();
        let mut rqs = [
            PerCoreRq::new(0),
            PerCoreRq::new(1),
            PerCoreRq::new(2),
            PerCoreRq::new(3),
            PerCoreRq::new(4),
            PerCoreRq::new(5),
            PerCoreRq::new(6),
            PerCoreRq::new(7),
        ];
        // Core 0: 4 threads, Core 1: 0 threads. diff = 4 > threshold 2.
        rqs[0].enqueue(Tid(1));
        rqs[0].enqueue(Tid(2));
        rqs[0].enqueue(Tid(3));
        rqs[0].enqueue(Tid(4));
        assert_eq!(rqs[0].load(), 4);
        assert_eq!(rqs[1].load(), 0);

        let b = Balancer::default(); // threshold = 2
        b.balance(&mut rqs, 2);

        // One thread migrated: core 0 has 3, core 1 has 1.
        assert_eq!(rqs[0].load(), 3);
        assert_eq!(rqs[1].load(), 1);
    }

    #[test]
    fn test_balance_no_migration_at_or_below_threshold() {
        let _g = lock();
        let mut rqs = [
            PerCoreRq::new(0),
            PerCoreRq::new(1),
            PerCoreRq::new(2),
            PerCoreRq::new(3),
            PerCoreRq::new(4),
            PerCoreRq::new(5),
            PerCoreRq::new(6),
            PerCoreRq::new(7),
        ];
        // Core 0: 2 threads, Core 1: 0 threads. diff = 2 == threshold 2 → no migrate.
        rqs[0].enqueue(Tid(1));
        rqs[0].enqueue(Tid(2));
        assert_eq!(rqs[0].load(), 2);
        assert_eq!(rqs[1].load(), 0);

        let b = Balancer::default(); // threshold = 2
        b.balance(&mut rqs, 2);

        // diff == threshold, condition is `diff > threshold` → no migration.
        assert_eq!(rqs[0].load(), 2);
        assert_eq!(rqs[1].load(), 0);
    }

    #[test]
    fn test_balance_no_migration_single_core() {
        let _g = lock();
        let mut rqs = [
            PerCoreRq::new(0),
            PerCoreRq::new(1),
            PerCoreRq::new(2),
            PerCoreRq::new(3),
            PerCoreRq::new(4),
            PerCoreRq::new(5),
            PerCoreRq::new(6),
            PerCoreRq::new(7),
        ];
        rqs[0].enqueue(Tid(1));
        rqs[0].enqueue(Tid(2));
        rqs[0].enqueue(Tid(3));

        let b = Balancer::default();
        // core_count = 1 → no balancing possible.
        b.balance(&mut rqs, 1);

        assert_eq!(rqs[0].load(), 3);
        // Other cores untouched.
        for rq in rqs.iter().take(MAX_CORES).skip(1) {
            assert_eq!(rq.load(), 0);
        }
    }

    #[test]
    fn test_balance_converges_after_multiple_passes() {
        let _g = lock();
        let mut rqs = [
            PerCoreRq::new(0),
            PerCoreRq::new(1),
            PerCoreRq::new(2),
            PerCoreRq::new(3),
            PerCoreRq::new(4),
            PerCoreRq::new(5),
            PerCoreRq::new(6),
            PerCoreRq::new(7),
        ];
        // Core 0: 5 threads, Core 1: 0 threads, threshold = 2.
        rqs[0].enqueue(Tid(1));
        rqs[0].enqueue(Tid(2));
        rqs[0].enqueue(Tid(3));
        rqs[0].enqueue(Tid(4));
        rqs[0].enqueue(Tid(5));

        let b = Balancer::default();
        // Each pass migrates one thread. Repeat until stable.
        for _ in 0..10 {
            b.balance(&mut rqs, 2);
        }
        // After convergence: diff <= threshold. With 5 threads over 2 cores
        // and threshold 2, the stable split is 3/2 (diff 1) — but note each
        // pass only moves one thread, so we verify the invariant directly:
        // the final difference must be <= threshold.
        let diff = rqs[0].load().abs_diff(rqs[1].load());
        assert!(
            diff <= b.threshold,
            "load difference {diff} should be <= threshold {}",
            b.threshold
        );
        // No threads lost.
        assert_eq!(rqs[0].load() + rqs[1].load(), 5);
    }

    #[test]
    fn test_balance_no_migration_when_max_equal_min_load() {
        let _g = lock();
        let mut rqs = [
            PerCoreRq::new(0),
            PerCoreRq::new(1),
            PerCoreRq::new(2),
            PerCoreRq::new(3),
            PerCoreRq::new(4),
            PerCoreRq::new(5),
            PerCoreRq::new(6),
            PerCoreRq::new(7),
        ];
        // Both cores have 2 threads — diff = 0, no migration.
        rqs[0].enqueue(Tid(1));
        rqs[0].enqueue(Tid(2));
        rqs[1].enqueue(Tid(3));
        rqs[1].enqueue(Tid(4));

        let b = Balancer::default();
        b.balance(&mut rqs, 2);

        assert_eq!(rqs[0].load(), 2);
        assert_eq!(rqs[1].load(), 2);
    }
}

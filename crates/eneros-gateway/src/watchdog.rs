use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{debug, error, info};

/// A pending operation being monitored by the watchdog.
struct PendingOp {
    /// When this operation expires
    deadline: Instant,
    /// Optional callback to invoke on timeout
    on_timeout: Option<Box<dyn FnOnce() + Send + Sync>>,
}

/// RAII guard that automatically cancels the watchdog registration when dropped.
pub struct WatchdogGuard {
    /// The operation ID this guard is protecting
    id: String,
    /// Reference to the watchdog timer
    watchdog: Arc<WatchdogTimerInner>,
    /// Whether this guard has been disarmed (manually cancelled or already timed out)
    disarmed: bool,
}

impl Drop for WatchdogGuard {
    fn drop(&mut self) {
        if !self.disarmed {
            // Remove from pending operations (operation completed before timeout)
            let mut ops = self.watchdog.operations.write();
            ops.remove(&self.id);
            debug!("Watchdog guard dropped, cancelled operation: {}", self.id);
        }
    }
}

/// Inner state shared between WatchdogTimer and WatchdogGuard
struct WatchdogTimerInner {
    operations: RwLock<HashMap<String, PendingOp>>,
    default_timeout: Duration,
    total_timeouts: AtomicU64,
    total_cancelled: AtomicU64,
    total_registered: AtomicU64,
    running: AtomicBool,
    stop_tx: watch::Sender<()>,
}

/// Watchdog timer for monitoring critical operations.
///
/// When an operation is registered, a deadline is set. If the operation
/// is not completed (guard dropped) before the deadline, a timeout
/// callback is invoked.
///
/// Usage:
/// ```ignore
/// let watchdog = WatchdogTimer::new(Duration::from_millis(500));
/// let guard = watchdog.register("cmd-123".to_string(), Duration::from_millis(200));
/// // ... do work ...
/// drop(guard); // operation completed, watchdog cancelled
/// ```
pub struct WatchdogTimer {
    inner: Arc<WatchdogTimerInner>,
    check_interval: Duration,
}

impl WatchdogTimer {
    /// Create a new watchdog timer with the specified default timeout.
    pub fn new(default_timeout: Duration) -> Self {
        let (stop_tx, _) = watch::channel(());
        Self {
            inner: Arc::new(WatchdogTimerInner {
                operations: RwLock::new(HashMap::new()),
                default_timeout,
                total_timeouts: AtomicU64::new(0),
                total_cancelled: AtomicU64::new(0),
                total_registered: AtomicU64::new(0),
                running: AtomicBool::new(false),
                stop_tx,
            }),
            check_interval: Duration::from_millis(50),
        }
    }

    /// Create with custom check interval.
    pub fn with_check_interval(default_timeout: Duration, check_interval: Duration) -> Self {
        let (stop_tx, _) = watch::channel(());
        Self {
            inner: Arc::new(WatchdogTimerInner {
                operations: RwLock::new(HashMap::new()),
                default_timeout,
                total_timeouts: AtomicU64::new(0),
                total_cancelled: AtomicU64::new(0),
                total_registered: AtomicU64::new(0),
                running: AtomicBool::new(false),
                stop_tx,
            }),
            check_interval,
        }
    }

    /// Register an operation with the default timeout.
    /// Returns a `WatchdogGuard` that cancels the registration when dropped.
    pub fn register(&self, id: String) -> WatchdogGuard {
        self.register_with_timeout(id, self.inner.default_timeout)
    }

    /// Register an operation with a custom timeout.
    /// Returns a `WatchdogGuard` that cancels the registration when dropped.
    pub fn register_with_timeout(&self, id: String, timeout: Duration) -> WatchdogGuard {
        let deadline = Instant::now() + timeout;
        let op = PendingOp {
            deadline,
            on_timeout: None,
        };
        self.inner.operations.write().insert(id.clone(), op);
        self.inner.total_registered.fetch_add(1, Ordering::Relaxed);
        debug!("Watchdog registered: {} (timeout: {:?})", id, timeout);
        WatchdogGuard {
            id,
            watchdog: self.inner.clone(),
            disarmed: false,
        }
    }

    /// Register an operation with a timeout callback.
    /// The callback is invoked if the operation times out.
    pub fn register_with_action(
        &self,
        id: String,
        timeout: Duration,
        on_timeout: Box<dyn FnOnce() + Send + Sync>,
    ) -> WatchdogGuard {
        let deadline = Instant::now() + timeout;
        let op = PendingOp {
            deadline,
            on_timeout: Some(on_timeout),
        };
        self.inner.operations.write().insert(id.clone(), op);
        self.inner.total_registered.fetch_add(1, Ordering::Relaxed);
        debug!(
            "Watchdog registered with action: {} (timeout: {:?})",
            id, timeout
        );
        WatchdogGuard {
            id,
            watchdog: self.inner.clone(),
            disarmed: false,
        }
    }

    /// Manually cancel a registered operation.
    pub fn cancel(&self, id: &str) -> bool {
        let mut ops = self.inner.operations.write();
        if ops.remove(id).is_some() {
            self.inner.total_cancelled.fetch_add(1, Ordering::Relaxed);
            debug!("Watchdog cancelled: {}", id);
            true
        } else {
            false
        }
    }

    /// Start the watchdog check loop as a background tokio task.
    pub fn start(self: &Arc<Self>) -> JoinHandle<()> {
        self.inner.running.store(true, Ordering::SeqCst);
        let inner = self.inner.clone();
        let check_interval = self.check_interval;
        let mut stop_rx = inner.stop_tx.subscribe();

        tokio::spawn(async move {
            info!(
                "WatchdogTimer started (check interval: {:?})",
                check_interval
            );
            loop {
                if !inner.running.load(Ordering::SeqCst) {
                    info!("WatchdogTimer stopping");
                    break;
                }

                let now = Instant::now();
                let mut timed_out = Vec::new();

                // Find expired operations
                {
                    let ops = inner.operations.read();
                    for (id, op) in ops.iter() {
                        if now >= op.deadline {
                            timed_out.push(id.clone());
                        }
                    }
                }

                // Process timeouts
                for id in timed_out {
                    let op = {
                        let mut ops = inner.operations.write();
                        ops.remove(&id)
                    };
                    if let Some(op) = op {
                        inner.total_timeouts.fetch_add(1, Ordering::Relaxed);
                        error!("Watchdog timeout: {}", id);
                        if let Some(on_timeout) = op.on_timeout {
                            on_timeout();
                        }
                    }
                }

                tokio::select! {
                    _ = tokio::time::sleep(check_interval) => {}
                    changed = stop_rx.changed() => {
                        if changed.is_err() || !inner.running.load(Ordering::SeqCst) {
                            info!("WatchdogTimer stopping");
                            break;
                        }
                    }
                }
            }
        })
    }

    /// Stop the watchdog check loop.
    pub fn stop(&self) {
        self.inner.running.store(false, Ordering::SeqCst);
        let _ = self.inner.stop_tx.send(());
    }

    /// Whether the watchdog is running.
    pub fn is_running(&self) -> bool {
        self.inner.running.load(Ordering::SeqCst)
    }

    /// Number of currently pending operations.
    pub fn pending_count(&self) -> usize {
        self.inner.operations.read().len()
    }

    /// Total number of timeouts that have occurred.
    pub fn total_timeouts(&self) -> u64 {
        self.inner.total_timeouts.load(Ordering::Relaxed)
    }

    /// Total number of operations that were cancelled (completed before timeout).
    pub fn total_cancelled(&self) -> u64 {
        self.inner.total_cancelled.load(Ordering::Relaxed)
    }

    /// Total number of operations ever registered.
    pub fn total_registered(&self) -> u64 {
        self.inner.total_registered.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// Register a guard, drop it, verify it's removed from pending.
    #[test]
    fn test_register_and_drop() {
        let watchdog = WatchdogTimer::new(Duration::from_secs(10));

        assert_eq!(watchdog.pending_count(), 0);

        {
            let _guard = watchdog.register("op-1".to_string());
            assert_eq!(watchdog.pending_count(), 1);
        }

        // Guard dropped, operation should be removed
        assert_eq!(watchdog.pending_count(), 0);
        assert_eq!(watchdog.total_registered(), 1);
    }

    /// Register with a very short timeout, wait, verify timeout callback fires.
    #[tokio::test]
    async fn test_timeout_fires() {
        let timeout_count = Arc::new(AtomicUsize::new(0));
        let timeout_count_clone = timeout_count.clone();

        let watchdog = Arc::new(WatchdogTimer::with_check_interval(
            Duration::from_millis(100),
            Duration::from_millis(10),
        ));

        let handle = watchdog.start();

        let _guard = watchdog.register_with_action(
            "op-timeout".to_string(),
            Duration::from_millis(20),
            Box::new(move || {
                timeout_count_clone.fetch_add(1, Ordering::SeqCst);
            }),
        );

        // Wait for the timeout to expire and the check loop to detect it
        tokio::time::sleep(Duration::from_millis(100)).await;

        assert_eq!(timeout_count.load(Ordering::SeqCst), 1);
        assert_eq!(watchdog.total_timeouts(), 1);

        watchdog.stop();
        handle.await.unwrap();
    }

    /// Register then cancel, verify it's removed.
    #[test]
    fn test_cancel_manually() {
        let watchdog = WatchdogTimer::new(Duration::from_secs(10));

        let guard = watchdog.register("op-cancel".to_string());
        assert_eq!(watchdog.pending_count(), 1);

        let cancelled = watchdog.cancel("op-cancel");
        assert!(cancelled);
        assert_eq!(watchdog.pending_count(), 0);
        assert_eq!(watchdog.total_cancelled(), 1);

        // Guard is still alive but operation is already removed.
        // Disarm the guard so drop doesn't try to remove again.
        // Since we can't set disarmed from outside, dropping the guard
        // will just do a no-op remove.
        drop(guard);
        assert_eq!(watchdog.total_cancelled(), 1); // Should not double-count
    }

    /// Register multiple operations, verify stats.
    #[test]
    fn test_stats() {
        let watchdog = WatchdogTimer::new(Duration::from_secs(10));

        let _g1 = watchdog.register("op-1".to_string());
        let _g2 = watchdog.register("op-2".to_string());
        let _g3 = watchdog.register("op-3".to_string());

        assert_eq!(watchdog.pending_count(), 3);
        assert_eq!(watchdog.total_registered(), 3);

        // Cancel one
        watchdog.cancel("op-2");
        assert_eq!(watchdog.pending_count(), 2);
        assert_eq!(watchdog.total_cancelled(), 1);

        // Drop one guard (operation completed)
        drop(_g1);
        assert_eq!(watchdog.pending_count(), 1);
        // Note: drop does not increment total_cancelled; only explicit cancel does.

        assert_eq!(watchdog.total_timeouts(), 0);
    }

    /// Register multiple concurrent operations, verify they all work.
    #[tokio::test]
    async fn test_multiple_operations() {
        let watchdog = Arc::new(WatchdogTimer::with_check_interval(
            Duration::from_secs(5),
            Duration::from_millis(10),
        ));

        let handle = watchdog.start();

        let timeout_count = Arc::new(AtomicUsize::new(0));

        // Register 3 operations with different timeouts
        let tc1 = timeout_count.clone();
        let _guard1 = watchdog.register_with_action(
            "fast-timeout".to_string(),
            Duration::from_millis(20),
            Box::new(move || {
                tc1.fetch_add(1, Ordering::SeqCst);
            }),
        );

        let tc2 = timeout_count.clone();
        let _guard2 = watchdog.register_with_action(
            "medium-timeout".to_string(),
            Duration::from_millis(40),
            Box::new(move || {
                tc2.fetch_add(1, Ordering::SeqCst);
            }),
        );

        let tc3 = timeout_count.clone();
        let _guard3 = watchdog.register_with_action(
            "slow-timeout".to_string(),
            Duration::from_millis(60),
            Box::new(move || {
                tc3.fetch_add(1, Ordering::SeqCst);
            }),
        );

        assert_eq!(watchdog.pending_count(), 3);
        assert_eq!(watchdog.total_registered(), 3);

        // Wait for all to time out
        tokio::time::sleep(Duration::from_millis(150)).await;

        assert_eq!(timeout_count.load(Ordering::SeqCst), 3);
        assert_eq!(watchdog.total_timeouts(), 3);
        assert_eq!(watchdog.pending_count(), 0);

        watchdog.stop();
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_stop_wakes_sleeping_loop_without_waiting_for_interval() {
        let watchdog = Arc::new(WatchdogTimer::with_check_interval(
            Duration::from_secs(30),
            Duration::from_secs(5),
        ));

        let handle = watchdog.start();
        tokio::task::yield_now().await;
        watchdog.stop();

        let stopped = tokio::time::timeout(Duration::from_millis(100), handle).await;
        assert!(
            stopped.is_ok(),
            "stop should wake the watchdog loop immediately"
        );
    }
}

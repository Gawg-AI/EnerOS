use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;
use tokio::sync::Notify;
use tracing::{error, info, warn};

use eneros_os::rt::{HardwareWatchdog, RtConfig, RtRuntime};

use crate::command::{Command, CommandPriority};
use crate::gateway::SafetyGateway;
use crate::priority_queue::SharedPriorityCommandQueue;

/// Watchdog keepalive interval for the RT executor loop.
const WATCHDOG_KEEPALIVE_INTERVAL: Duration = Duration::from_millis(100);

/// Result of executing a single command
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CommandResult {
    /// Command executed successfully
    Executed { latency_ms: u64 },
    /// Command rejected by safety checks
    Rejected { reason: String },
    /// Command timed out waiting for ACK
    Timeout { retries: u32 },
}

/// Statistics for the executor
#[derive(Debug, Default, Clone)]
pub struct ExecutorStats {
    pub total_executed: u64,
    pub total_rejected: u64,
    pub total_timeouts: u64,
    pub total_retries: u64,
    pub by_priority: [u64; 4], // [Low, Normal, High, Critical]
    pub total_latency_ms: u64,
}

impl ExecutorStats {
    pub fn avg_latency_ms(&self) -> u64 {
        self.total_latency_ms.checked_div(self.total_executed).unwrap_or(0)
    }
}

/// Configuration for the realtime executor
#[derive(Debug, Clone)]
pub struct ExecutorConfig {
    /// Timeout for command ACK (default: 500ms)
    pub ack_timeout: Duration,
    /// Maximum retries on timeout (default: 3)
    pub max_retries: u32,
    /// Whether the executor is running
    pub running: bool,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            ack_timeout: Duration::from_millis(500),
            max_retries: 3,
            running: false,
        }
    }
}

/// Realtime command executor that consumes from a priority queue
pub struct RealtimeExecutor {
    queue: Arc<SharedPriorityCommandQueue>,
    gateway: Arc<SafetyGateway>,
    config: Mutex<ExecutorConfig>,
    stats: Mutex<ExecutorStats>,
    stop_notify: Notify,
    /// Optional hardware watchdog for independent RT-thread keepalive.
    ///
    /// When set, `run_loop()` feeds the watchdog at most once per
    /// [`WATCHDOG_KEEPALIVE_INTERVAL`] so that a stalled main thread
    /// (eneros-init) does not trigger a hardware reset that would kill the
    /// RT execution domain. Shared via `Arc<Mutex<…>>` because
    /// `HardwareWatchdog::keepalive` requires `&mut self` and the same
    /// device may be fed from multiple threads.
    watchdog: Option<Arc<Mutex<HardwareWatchdog>>>,
}

impl RealtimeExecutor {
    pub fn new(
        queue: Arc<SharedPriorityCommandQueue>,
        gateway: Arc<SafetyGateway>,
    ) -> Self {
        Self {
            queue,
            gateway,
            config: Mutex::new(ExecutorConfig::default()),
            stats: Mutex::new(ExecutorStats::default()),
            stop_notify: Notify::new(),
            watchdog: None,
        }
    }

    pub fn with_config(
        queue: Arc<SharedPriorityCommandQueue>,
        gateway: Arc<SafetyGateway>,
        config: ExecutorConfig,
    ) -> Self {
        Self {
            queue,
            gateway,
            config: Mutex::new(config),
            stats: Mutex::new(ExecutorStats::default()),
            stop_notify: Notify::new(),
            watchdog: None,
        }
    }

    /// Attach a hardware watchdog so that `run_loop()` independently feeds it
    /// from the RT thread.
    ///
    /// This decouples RT-domain liveness from the eneros-init main loop: if
    /// the main thread stalls but the RT thread is still processing commands
    /// (or idle-but-alive), the watchdog is kept alive and no hardware reset
    /// is triggered, preserving dual-execution-domain isolation.
    ///
    /// The watchdog is shared via `Arc<Mutex<HardwareWatchdog>>` because
    /// `keepalive()` requires `&mut self` and the underlying `/dev/watchdog`
    /// file descriptor must not be duplicated.
    pub fn with_watchdog(
        mut self,
        watchdog: Arc<Mutex<HardwareWatchdog>>,
    ) -> Self {
        self.watchdog = Some(watchdog);
        self
    }

    /// Start the executor's consumption loop as a background tokio task.
    /// Returns a JoinHandle to allow aborting.
    pub fn start(self: &Arc<Self>) -> JoinHandle<()> {
        let executor = self.clone();
        {
            let mut cfg = executor.config.lock();
            cfg.running = true;
        }
        tokio::spawn(async move {
            info!("RealtimeExecutor started");
            Self::run_loop(executor).await;
        })
    }

    /// Start the executor's consumption loop on a dedicated RT-configured thread.
    ///
    /// The thread is configured via `RtRuntime` (SCHED_FIFO, mlockall, CPU affinity,
    /// huge pages) before entering a single-threaded tokio runtime. On non-Linux
    /// platforms the RT configuration is a no-op but the dedicated thread is still
    /// spawned.
    pub fn start_rt(self: &Arc<Self>, rt_config: RtConfig) -> std::thread::JoinHandle<()> {
        let executor = self.clone();
        {
            let mut cfg = executor.config.lock();
            cfg.running = true;
        }
        std::thread::Builder::new()
            .name("rt-executor".into())
            .spawn(move || {
                let runtime = RtRuntime::new(rt_config);
                if let Err(e) = runtime.configure_current_thread() {
                    error!("Failed to configure RT thread: {}", e);
                    return;
                }

                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        error!("Failed to build tokio runtime: {}", e);
                        return;
                    }
                };

                rt.block_on(async move {
                    info!("RealtimeExecutor (RT) started");
                    Self::run_loop(executor).await;
                });
            })
            .expect("failed to spawn rt-executor thread")
    }

    /// Shared consumption loop used by both `start()` and `start_rt()`.
    ///
    /// When a watchdog is attached via [`with_watchdog`](Self::with_watchdog),
    /// the loop independently feeds it at most once per
    /// [`WATCHDOG_KEEPALIVE_INTERVAL`] (100ms). A `sleep` branch is added to
    /// the idle `select!` so that the watchdog is still fed when the queue is
    /// empty — without it, `select!` could block indefinitely and starve the
    /// watchdog, defeating the purpose of independent RT-thread keepalive.
    async fn run_loop(executor: Arc<RealtimeExecutor>) {
        let mut last_keepalive = Instant::now();
        loop {
            if !executor.config.lock().running {
                info!("RealtimeExecutor stopping");
                break;
            }

            // Try to dequeue synchronously first
            if let Some(cmd) = executor.queue.dequeue() {
                let result = executor.execute_one(cmd).await;
                match &result {
                    CommandResult::Executed { latency_ms } => {
                        info!("Command executed in {}ms", latency_ms);
                    }
                    CommandResult::Rejected { reason } => {
                        warn!("Command rejected: {}", reason);
                    }
                    CommandResult::Timeout { retries } => {
                        error!("Command timed out after {} retries", retries);
                    }
                }
                Self::maybe_keepalive(&executor, &mut last_keepalive);
                continue;
            }

            // Queue is empty: wait for a new command, a stop signal, or the
            // watchdog keepalive interval (whichever comes first). The sleep
            // branch guarantees the watchdog is fed even during prolonged idle
            // periods, preventing a hardware reset that would kill the RT
            // domain while it is still healthy.
            tokio::select! {
                cmd = executor.queue.dequeue_async() => {
                    let result = executor.execute_one(cmd).await;
                    match &result {
                        CommandResult::Executed { latency_ms } => {
                            info!("Command executed in {}ms", latency_ms);
                        }
                        CommandResult::Rejected { reason } => {
                            warn!("Command rejected: {}", reason);
                        }
                        CommandResult::Timeout { retries } => {
                            error!("Command timed out after {} retries", retries);
                        }
                    }
                }
                _ = executor.stop_notify.notified() => {
                    if !executor.config.lock().running {
                        info!("RealtimeExecutor stopping");
                        break;
                    }
                }
                _ = tokio::time::sleep(WATCHDOG_KEEPALIVE_INTERVAL) => {
                    // Keepalive timer expired; feed the watchdog below.
                }
            }
            Self::maybe_keepalive(&executor, &mut last_keepalive);
        }
    }

    /// Feed the attached watchdog if [`WATCHDOG_KEEPALIVE_INTERVAL`] has
    /// elapsed since the last keepalive. No-op when no watchdog is attached
    /// (e.g. non-RT `start()` path or tests).
    fn maybe_keepalive(executor: &RealtimeExecutor, last_keepalive: &mut Instant) {
        if last_keepalive.elapsed() < WATCHDOG_KEEPALIVE_INTERVAL {
            return;
        }
        if let Some(ref wd) = executor.watchdog {
            if let Err(e) = wd.lock().keepalive() {
                warn!("RT executor watchdog keepalive failed: {}", e);
            }
        }
        *last_keepalive = Instant::now();
    }

    /// Stop the executor loop
    pub fn stop(&self) {
        self.config.lock().running = false;
        self.stop_notify.notify_one();
    }

    /// Execute a single command with safety validation and optional retry.
    ///
    /// The ACK verification is now performed by the `CommandExecutor` inside
    /// `SafetyGateway::execute_command()`. If the command has device routing
    /// information, the executor will write to the device and read back to
    /// verify. If verification fails, it retries up to `max_retries` times.
    ///
    /// For commands without device routing, the `LoggingExecutor` is used,
    /// which always succeeds immediately (backward-compatible behavior).
    pub async fn execute_one(&self, cmd: Command) -> CommandResult {
        let start = Instant::now();
        let priority_idx = priority_index(&cmd.priority);

        // Validate through safety gateway
        if let Err(e) = self.gateway.validate_command(&cmd) {
            let mut stats = self.stats.lock();
            stats.total_rejected += 1;
            return CommandResult::Rejected {
                reason: e.to_string(),
            };
        }

        // Execute command through gateway (includes real device dispatch + ACK verification)
        let exec_result = self.gateway.execute_command(cmd).await;

        let elapsed = start.elapsed();
        let latency_ms = elapsed.as_millis() as u64;

        match exec_result {
            Ok(()) => {
                // Check the execution result for retry information
                let retries = self.gateway.last_execution_result()
                    .map(|r| r.retries)
                    .unwrap_or(0);

                // Single lock acquisition to update all stats
                let mut stats = self.stats.lock();
                if retries > 0 {
                    stats.total_retries += retries as u64;
                }
                stats.total_executed += 1;
                stats.by_priority[priority_idx] += 1;
                stats.total_latency_ms += latency_ms;

                CommandResult::Executed { latency_ms }
            }
            Err(e) => {
                // Distinguish between safety rejection and execution failure
                let err_str = e.to_string();
                if err_str.contains("execution failed") || err_str.contains("ACK verification failed") {
                    // Device execution failure — treat as timeout (device didn't respond properly)
                    let max_retries = self.config.lock().max_retries;
                    let mut stats = self.stats.lock();
                    stats.total_timeouts += 1;
                    stats.total_retries += max_retries as u64;
                    CommandResult::Timeout { retries: max_retries }
                } else {
                    // Safety check rejection
                    let mut stats = self.stats.lock();
                    stats.total_rejected += 1;
                    CommandResult::Rejected { reason: err_str }
                }
            }
        }
    }

    /// Get current executor statistics
    pub fn stats(&self) -> ExecutorStats {
        self.stats.lock().clone()
    }

    /// Whether the executor is running
    pub fn is_running(&self) -> bool {
        self.config.lock().running
    }
}

fn priority_index(priority: &CommandPriority) -> usize {
    match priority {
        CommandPriority::Low => 0,
        CommandPriority::Normal => 1,
        CommandPriority::High => 2,
        CommandPriority::Critical => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::CommandType;
    use crate::safety::SafetyCheck;
    use eneros_core::EnerOSError;

    fn make_cmd(priority: CommandPriority, id_suffix: &str) -> Command {
        Command::new(
            CommandType::SwitchOperation,
            1,
            priority,
            &format!("test-{}", id_suffix),
        )
    }

    /// A safety check that always rejects
    struct RejectAllCheck;

    impl SafetyCheck for RejectAllCheck {
        fn validate(&self, _command: &Command) -> eneros_core::Result<()> {
            Err(EnerOSError::Gateway("rejected by RejectAllCheck".into()))
        }
        fn name(&self) -> &str {
            "RejectAllCheck"
        }
        fn description(&self) -> &str {
            "Always rejects"
        }
    }

    #[tokio::test]
    async fn test_execute_critical_first() {
        let queue = Arc::new(SharedPriorityCommandQueue::new());
        let gateway = Arc::new(SafetyGateway::new(100));

        let executor = RealtimeExecutor::new(queue.clone(), gateway);

        // Enqueue Low first, then Critical
        let low_cmd = make_cmd(CommandPriority::Low, "low");
        let critical_cmd = make_cmd(CommandPriority::Critical, "critical");
        queue.enqueue(low_cmd);
        queue.enqueue(critical_cmd);

        // Dequeue should give Critical first
        let first = queue.dequeue().unwrap();
        assert_eq!(first.priority, CommandPriority::Critical);

        let second = queue.dequeue().unwrap();
        assert_eq!(second.priority, CommandPriority::Low);

        // Execute them and verify results
        let result1 = executor.execute_one(first).await;
        let result2 = executor.execute_one(second).await;

        assert!(matches!(result1, CommandResult::Executed { .. }));
        assert!(matches!(result2, CommandResult::Executed { .. }));
    }

    #[tokio::test]
    async fn test_execute_rejected() {
        let queue = Arc::new(SharedPriorityCommandQueue::new());
        let gateway = Arc::new(SafetyGateway::new(100));
        gateway.register_safety_check(Box::new(RejectAllCheck));

        let executor = RealtimeExecutor::new(queue, gateway);

        let cmd = make_cmd(CommandPriority::Normal, "rejected");
        let result = executor.execute_one(cmd).await;

        assert!(matches!(result, CommandResult::Rejected { .. }));
        if let CommandResult::Rejected { reason } = result {
            assert!(reason.contains("RejectAllCheck"));
        }

        let stats = executor.stats();
        assert_eq!(stats.total_rejected, 1);
        assert_eq!(stats.total_executed, 0);
    }

    #[tokio::test]
    async fn test_executor_stats() {
        let queue = Arc::new(SharedPriorityCommandQueue::new());
        let gateway = Arc::new(SafetyGateway::new(100));

        let executor = RealtimeExecutor::new(queue.clone(), gateway);

        // Execute multiple commands of different priorities
        let cmd_low = make_cmd(CommandPriority::Low, "low");
        let cmd_normal = make_cmd(CommandPriority::Normal, "normal");
        let cmd_high = make_cmd(CommandPriority::High, "high");
        let cmd_critical = make_cmd(CommandPriority::Critical, "critical");

        executor.execute_one(cmd_low).await;
        executor.execute_one(cmd_normal).await;
        executor.execute_one(cmd_high).await;
        executor.execute_one(cmd_critical).await;

        let stats = executor.stats();
        assert_eq!(stats.total_executed, 4);
        assert_eq!(stats.total_rejected, 0);
        assert_eq!(stats.total_timeouts, 0);
        assert_eq!(stats.by_priority[0], 1); // Low
        assert_eq!(stats.by_priority[1], 1); // Normal
        assert_eq!(stats.by_priority[2], 1); // High
        assert_eq!(stats.by_priority[3], 1); // Critical
        // total_latency_ms is u64, always >= 0; just verify it's present
        let _ = stats.total_latency_ms;
    }

    #[tokio::test]
    async fn test_start_stop() {
        let queue = Arc::new(SharedPriorityCommandQueue::new());
        let gateway = Arc::new(SafetyGateway::new(100));

        let executor = Arc::new(RealtimeExecutor::new(queue.clone(), gateway));

        // Start the executor loop
        let handle = executor.start();
        assert!(executor.is_running());

        // Enqueue a command so the loop processes something
        queue.enqueue(make_cmd(CommandPriority::Normal, "while-running"));

        // Give the executor time to process
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Stop the executor
        executor.stop();
        assert!(!executor.is_running());

        // Wait for the task to finish
        let _ = handle.await;

        // Verify the command was processed
        let stats = executor.stats();
        assert!(stats.total_executed >= 1);
    }
}

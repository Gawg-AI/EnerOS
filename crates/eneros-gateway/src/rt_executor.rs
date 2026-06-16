use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;
use tokio::sync::Notify;
use tracing::{error, info, warn};

use crate::command::{Command, CommandPriority};
use crate::gateway::SafetyGateway;
use crate::priority_queue::SharedPriorityCommandQueue;

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
        }
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
                    continue;
                }

                // Queue is empty: wait for either a new command or a stop signal
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
                }
            }
        })
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

                if retries > 0 {
                    let mut stats = self.stats.lock();
                    stats.total_retries += retries as u64;
                }

                let mut stats = self.stats.lock();
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
        assert!(stats.total_latency_ms >= 0);
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

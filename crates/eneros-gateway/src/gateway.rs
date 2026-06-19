use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use eneros_core::Result;
use parking_lot::RwLock;
use tokio::sync::Mutex;

use super::command::Command;
use super::executor::{CommandExecutor, ExecutionResult, LoggingExecutor};
use super::priority_queue::SharedPriorityCommandQueue;
use super::rt_executor::RealtimeExecutor;
use super::safety::SafetyCheck;

/// Real-time safety gateway for cross-domain communication
pub struct SafetyGateway {
    safety_checks: RwLock<Vec<Box<dyn SafetyCheck>>>,
    // VecDeque gives O(1) eviction of the oldest command; the previous Vec
    // used `remove(0)` which is O(n) on every accepted realtime command.
    command_history: RwLock<VecDeque<Command>>,
    /// Per-device lock pool: each `device_id` gets its own async mutex so
    /// that commands targeting different devices can execute concurrently
    /// (e.g., a slow Modbus RTU retry no longer blocks a fast IEC 61850
    /// command). The outer `parking_lot::RwLock` is read-heavy (locks are
    /// created lazily on first use of a device).
    device_locks: RwLock<HashMap<String, Arc<Mutex<()>>>>,
    /// Fallback lock for commands without a `device_id` (e.g., logging-only
    /// commands). Wrapped in `Arc` so `get_device_lock` can return a uniform
    /// `Arc<Mutex<()>>` type.
    global_lock: Arc<Mutex<()>>,
    /// Short-held async mutex that serializes only the `command_history`
    /// push (not the device I/O), so the global history order matches the
    /// logical commit order across devices.
    history_lock: Mutex<()>,
    max_history: usize,
    /// Optional priority command queue for realtime execution
    queue: Option<Arc<SharedPriorityCommandQueue>>,
    /// Optional realtime executor
    executor: RwLock<Option<Arc<RealtimeExecutor>>>,
    /// Command execution backend (defaults to LoggingExecutor)
    command_executor: Arc<dyn CommandExecutor>,
    /// Last execution result (for ACK verification in rt_executor)
    last_execution_result: RwLock<Option<ExecutionResult>>,
}

impl SafetyGateway {
    /// Create a new safety gateway
    pub fn new(max_history: usize) -> Self {
        Self {
            safety_checks: RwLock::new(Vec::new()),
            command_history: RwLock::new(VecDeque::new()),
            device_locks: RwLock::new(HashMap::new()),
            global_lock: Arc::new(Mutex::new(())),
            history_lock: Mutex::new(()),
            max_history,
            queue: None,
            executor: RwLock::new(None),
            command_executor: Arc::new(LoggingExecutor),
            last_execution_result: RwLock::new(None),
        }
    }

    /// Create a safety gateway with a custom command executor (for real device execution).
    pub fn with_executor(max_history: usize, command_executor: Arc<dyn CommandExecutor>) -> Self {
        Self {
            safety_checks: RwLock::new(Vec::new()),
            command_history: RwLock::new(VecDeque::new()),
            device_locks: RwLock::new(HashMap::new()),
            global_lock: Arc::new(Mutex::new(())),
            history_lock: Mutex::new(()),
            max_history,
            queue: None,
            executor: RwLock::new(None),
            command_executor,
            last_execution_result: RwLock::new(None),
        }
    }

    /// Create a safety gateway with a priority command queue.
    pub fn with_queue(max_history: usize, queue: Arc<SharedPriorityCommandQueue>) -> Self {
        Self {
            safety_checks: RwLock::new(Vec::new()),
            command_history: RwLock::new(VecDeque::new()),
            device_locks: RwLock::new(HashMap::new()),
            global_lock: Arc::new(Mutex::new(())),
            history_lock: Mutex::new(()),
            max_history,
            queue: Some(queue),
            executor: RwLock::new(None),
            command_executor: Arc::new(LoggingExecutor),
            last_execution_result: RwLock::new(None),
        }
    }

    /// Create a safety gateway with both a priority queue and a command executor.
    pub fn with_queue_and_executor(
        max_history: usize,
        queue: Arc<SharedPriorityCommandQueue>,
        command_executor: Arc<dyn CommandExecutor>,
    ) -> Self {
        Self {
            safety_checks: RwLock::new(Vec::new()),
            command_history: RwLock::new(VecDeque::new()),
            device_locks: RwLock::new(HashMap::new()),
            global_lock: Arc::new(Mutex::new(())),
            history_lock: Mutex::new(()),
            max_history,
            queue: Some(queue),
            executor: RwLock::new(None),
            command_executor,
            last_execution_result: RwLock::new(None),
        }
    }

    /// Register a safety check
    pub fn register_safety_check(&self, check: Box<dyn SafetyCheck>) {
        let mut checks = self.safety_checks.write();
        checks.push(check);
    }

    /// Validate a command through all safety checks
    pub fn validate_command(&self, command: &Command) -> Result<()> {
        let checks = self.safety_checks.read();
        for check in checks.iter() {
            check.validate(command)?;
        }
        Ok(())
    }

    /// Resolve the async mutex guarding a specific device.
    ///
    /// Commands with a `device_id` are serialized per-device via a lazily
    /// populated lock pool (`device_locks`): the fast path takes a
    /// `parking_lot` read lock and clones the existing `Arc`; the slow path
    /// upgrades to a write lock only to insert a fresh mutex for a
    /// first-seen device. Commands without a `device_id` fall back to the
    /// shared `global_lock` so legacy/logging commands remain serialized.
    fn get_device_lock(&self, device_id: &Option<String>) -> Arc<Mutex<()>> {
        match device_id {
            Some(id) => {
                // Fast path: read lock + clone existing Arc.
                if let Some(lock) = self.device_locks.read().get(id) {
                    return lock.clone();
                }
                // Slow path: write lock to insert a new per-device mutex.
                self.device_locks
                    .write()
                    .entry(id.clone())
                    .or_insert_with(|| Arc::new(Mutex::new(())))
                    .clone()
            }
            None => self.global_lock.clone(),
        }
    }

    /// Execute a command after safety validation (synchronous direct execution).
    /// For priority-queue-based execution, use `submit_command()` instead.
    ///
    /// Concurrency model:
    /// - Commands targeting **different devices** run concurrently (each
    ///   `device_id` has its own async mutex in `device_locks`), so a slow
    ///   Modbus RTU retry no longer blocks a fast IEC 61850 command.
    /// - Commands targeting the **same device** (or with no `device_id`,
    ///   which share `global_lock`) serialize, preserving per-device commit
    ///   order.
    /// - The `command_history` push is serialized by a short-held
    ///   `history_lock` (acquired **after** releasing the device lock) so
    ///   that the global history remains consistent without holding device
    ///   locks during history eviction.
    ///
    /// If the command has device routing information (`device_id`, `device_address`,
    /// `device_value`), it will be dispatched to the real device via the configured
    /// `CommandExecutor`. Otherwise, it falls back to the logging executor.
    pub async fn execute_command(&self, command: Command) -> Result<()> {
        // 1. Acquire per-device lock (or global fallback). Same-device
        //    commands serialize here; different devices proceed concurrently.
        let device_lock = self.get_device_lock(&command.device_id);
        let _guard = device_lock.lock().await;

        // 2. Validate (rejects never enter history). Held under the device
        //    lock so same-device commands validate/commit in order.
        self.validate_command(&command)?;

        // 3. Execute through the backend (async device I/O; may be slow but
        //    only blocks other commands on the same device).
        let exec_result = self.command_executor.execute(&command).await?;

        // 4. Store execution result for ACK verification by rt_executor
        //    (short-held write lock, updated while still holding the device
        //    lock since the result is tied to this device's execution).
        *self.last_execution_result.write() = Some(exec_result.clone());

        // 5. Release the device lock before recording history so a slow
        //    device doesn't block unrelated devices from acquiring their
        //    locks while we push to the shared history. The history_lock
        //    serializes the push globally to keep history order consistent.
        drop(_guard);
        let _history_guard = self.history_lock.lock().await;
        {
            let mut history = self.command_history.write();
            history.push_back(command);
            while history.len() > self.max_history {
                history.pop_front();
            }
        }

        if !exec_result.success {
            return Err(eneros_core::EnerOSError::Gateway(
                format!("Command execution failed: {}", exec_result.description),
            ));
        }

        Ok(())
    }

    /// Submit a command to the priority queue for realtime execution.
    /// Returns an error if no queue is configured.
    pub fn submit_command(&self, command: Command) -> Result<()> {
        // Validate first
        self.validate_command(&command)?;

        match &self.queue {
            Some(queue) => {
                tracing::info!(
                    "Submitting command to priority queue: {:?} (priority: {:?})",
                    command.command_type,
                    command.priority
                );
                queue.enqueue(command);
                Ok(())
            }
            None => Err(eneros_core::EnerOSError::Gateway(
                "No priority command queue configured".to_string(),
            )),
        }
    }

    /// Start the realtime executor with this gateway and its queue.
    /// Returns the executor Arc for reference.
    /// Returns an error if no queue is configured.
    pub fn start_executor(self: &Arc<Self>) -> Result<Arc<RealtimeExecutor>> {
        let queue = match &self.queue {
            Some(q) => q.clone(),
            None => {
                return Err(eneros_core::EnerOSError::Gateway(
                    "No priority command queue configured".to_string(),
                ));
            }
        };

        let executor = Arc::new(RealtimeExecutor::new(queue, self.clone()));
        executor.start();
        *self.executor.write() = Some(executor.clone());
        Ok(executor)
    }

    /// Get a reference to the priority command queue, if configured.
    pub fn queue(&self) -> Option<&Arc<SharedPriorityCommandQueue>> {
        self.queue.as_ref()
    }

    /// Get a reference to the realtime executor, if started.
    pub fn executor(&self) -> Option<Arc<RealtimeExecutor>> {
        self.executor.read().clone()
    }

    /// Get command history
    pub fn command_history(&self) -> Vec<Command> {
        self.command_history.read().iter().cloned().collect()
    }

    /// Get the last execution result (for ACK verification)
    pub fn last_execution_result(&self) -> Option<ExecutionResult> {
        self.last_execution_result.read().clone()
    }

    /// Get safety check count
    pub fn safety_check_count(&self) -> usize {
        self.safety_checks.read().len()
    }
}

impl Default for SafetyGateway {
    fn default() -> Self {
        Self::new(1000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eneros_core::EnerOSError;
    use std::sync::mpsc;
    use std::sync::Mutex as StdMutex;
    use std::time::Duration;

    struct BlockingCheck {
        entered: StdMutex<Option<mpsc::Sender<()>>>,
        release: StdMutex<mpsc::Receiver<()>>,
    }

    impl SafetyCheck for BlockingCheck {
        fn validate(&self, command: &Command) -> Result<()> {
            if command.id == "slow" {
                if let Some(tx) = self.entered.lock().unwrap().take() {
                    tx.send(()).unwrap();
                }
                self.release
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(1))
                    .map_err(|e| EnerOSError::Gateway(e.to_string()))?;
            }
            Ok(())
        }

        fn name(&self) -> &str {
            "BlockingCheck"
        }

        fn description(&self) -> &str {
            "Blocks one command to expose validate/history ordering"
        }
    }

    fn command_with_id(id: &str) -> Command {
        let mut command = Command::new(
            super::super::command::CommandType::SwitchOperation,
            1,
            super::super::command::CommandPriority::Normal,
            "test",
        );
        command.id = id.to_string();
        command
    }

    #[tokio::test]
    async fn test_execute_command_records_history_in_validation_commit_order() {
        let gateway = Arc::new(SafetyGateway::new(10));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();

        gateway.register_safety_check(Box::new(BlockingCheck {
            entered: StdMutex::new(Some(entered_tx)),
            release: StdMutex::new(release_rx),
        }));

        // Execute two commands sequentially (the blocking check ensures ordering)
        let rt = tokio::runtime::Handle::current();

        let slow_gateway = gateway.clone();
        let (slow_done_tx, slow_done_rx) = mpsc::channel();
        let rt_slow = rt.clone();
        std::thread::spawn(move || {
            rt_slow.block_on(async {
                slow_gateway
                    .execute_command(command_with_id("slow"))
                    .await
                    .unwrap();
            });
            slow_done_tx.send(()).unwrap();
        });

        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        let fast_gateway = gateway.clone();
        let (fast_done_tx, fast_done_rx) = mpsc::channel();
        std::thread::spawn(move || {
            rt.block_on(async {
                fast_gateway
                    .execute_command(command_with_id("fast"))
                    .await
                    .unwrap();
            });
            fast_done_tx.send(()).unwrap();
        });

        std::thread::sleep(Duration::from_millis(20));
        release_tx.send(()).unwrap();

        slow_done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        fast_done_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        let ids: Vec<String> = gateway
            .command_history()
            .into_iter()
            .map(|command| command.id)
            .collect();
        assert_eq!(ids, vec!["slow".to_string(), "fast".to_string()]);
    }

    #[tokio::test]
    async fn test_execute_command_with_device_executor() {
        use eneros_device::adapter::{ConnectionConfig, ProtocolConfig, DeviceInfo};
        use eneros_device::mock_adapter::mock::MockAdapter;
        use eneros_device::DeviceManager;
        use eneros_device::protocol::ProtocolType;
        use super::super::executor::DeviceCommandExecutor;
        use super::super::command::DeviceValue;

        let dm = Arc::new(DeviceManager::new());
        let adapter = Box::new(MockAdapter::new("mock-rtu"));
        let config = ConnectionConfig {
            host: "127.0.0.1".into(), port: 502, timeout_ms: 3000,
            credentials: None, protocol_config: ProtocolConfig::Modbus { slave_id: 1, baud_rate: None },
        };
        let info = DeviceInfo {
            device_id: "rtu-1".into(), name: "rtu-1".into(), protocol: ProtocolType::Modbus,
            manufacturer: "Test".into(), model: "Mock-100".into(), firmware_version: "1.0.0".into(),
            ip_address: "127.0.0.1".into(), port: 502, capabilities: vec!["read".into(), "write".into()],
        };
        dm.register_device("rtu-1", adapter, config, info).await;
        dm.connect("rtu-1").await.unwrap();

        let executor = Arc::new(DeviceCommandExecutor::new(dm));
        let gateway = SafetyGateway::with_executor(100, executor);

        let cmd = Command::new(
            super::super::command::CommandType::SwitchToggle,
            42,
            super::super::command::CommandPriority::Normal,
            "test",
        ).with_device("rtu-1", "coil:1", DeviceValue::Bool(true));

        let result = gateway.execute_command(cmd).await;
        assert!(result.is_ok(), "Expected Ok, got {:?}", result);

        let history = gateway.command_history();
        assert_eq!(history.len(), 1);

        let exec_result = gateway.last_execution_result().unwrap();
        assert!(exec_result.success);
    }

    #[tokio::test]
    async fn test_execute_command_device_failure() {
        use super::super::command::DeviceValue;
        use eneros_device::DeviceManager;
        use super::super::executor::DeviceCommandExecutor;

        let dm = Arc::new(DeviceManager::new());
        let executor = Arc::new(DeviceCommandExecutor::new(dm));
        let gateway = SafetyGateway::with_executor(100, executor);

        // Command targets a device that doesn't exist
        let cmd = Command::new(
            super::super::command::CommandType::SwitchToggle,
            42,
            super::super::command::CommandPriority::Normal,
            "test",
        ).with_device("nonexistent", "coil:1", DeviceValue::Bool(true));

        let result = gateway.execute_command(cmd).await;
        assert!(result.is_err(), "Expected error for nonexistent device");
    }

    /// Mock executor that sleeps for a fixed duration to simulate slow device
    /// I/O (e.g., Modbus RTU retries). Used to verify per-device lock
    /// concurrency behavior.
    struct SlowExecutor {
        delay: Duration,
    }

    impl SlowExecutor {
        fn new(delay: Duration) -> Self {
            Self { delay }
        }
    }

    #[async_trait::async_trait]
    impl CommandExecutor for SlowExecutor {
        async fn execute(&self, _command: &Command) -> Result<ExecutionResult> {
            tokio::time::sleep(self.delay).await;
            Ok(ExecutionResult::ok(
                "slow-executor-completed".to_string(),
                self.delay,
            ))
        }

        async fn read_back(&self, _command: &Command) -> Option<eneros_device::adapter::DataValue> {
            None
        }
    }

    /// Build a command optionally routed to a specific device_id.
    fn command_for_device(device_id: Option<&str>) -> Command {
        let mut command = Command::new(
            super::super::command::CommandType::SwitchOperation,
            1,
            super::super::command::CommandPriority::Normal,
            "test",
        );
        if let Some(id) = device_id {
            command.device_id = Some(id.to_string());
        }
        command
    }

    #[tokio::test]
    async fn test_per_device_lock_concurrent_different_devices() {
        // Two commands targeting different devices should execute concurrently:
        // a slow device-A must not block device-B.
        let executor = Arc::new(SlowExecutor::new(Duration::from_millis(100)));
        let gateway = SafetyGateway::with_executor(100, executor);

        let cmd_a = command_for_device(Some("device-A"));
        let cmd_b = command_for_device(Some("device-B"));

        let start = std::time::Instant::now();
        let (res_a, res_b) = tokio::join!(
            gateway.execute_command(cmd_a),
            gateway.execute_command(cmd_b),
        );
        let elapsed = start.elapsed();

        res_a.expect("cmd_a should succeed");
        res_b.expect("cmd_b should succeed");

        // Concurrent execution: ~100ms, well under the 200ms serial baseline.
        assert!(
            elapsed < Duration::from_millis(200),
            "Different-device commands should run concurrently, but took {:?}",
            elapsed,
        );

        // Both commands should be recorded in history.
        assert_eq!(gateway.command_history().len(), 2);
    }

    #[tokio::test]
    async fn test_per_device_lock_serial_same_device() {
        // Two commands targeting the same device must serialize on the
        // per-device lock, so total time is at least 2 × delay.
        let executor = Arc::new(SlowExecutor::new(Duration::from_millis(100)));
        let gateway = SafetyGateway::with_executor(100, executor);

        let cmd_a = command_for_device(Some("device-A"));
        let cmd_b = command_for_device(Some("device-A"));

        let start = std::time::Instant::now();
        let (res_a, res_b) = tokio::join!(
            gateway.execute_command(cmd_a),
            gateway.execute_command(cmd_b),
        );
        let elapsed = start.elapsed();

        res_a.expect("cmd_a should succeed");
        res_b.expect("cmd_b should succeed");

        // Serial execution: >= 200ms (two 100ms sleeps back-to-back).
        assert!(
            elapsed >= Duration::from_millis(200),
            "Same-device commands should serialize, but took {:?}",
            elapsed,
        );

        assert_eq!(gateway.command_history().len(), 2);
    }

    #[tokio::test]
    async fn test_per_device_lock_no_device_id_fallback() {
        // Commands without a device_id fall back to the shared global_lock
        // and should still execute normally.
        let executor = Arc::new(SlowExecutor::new(Duration::from_millis(10)));
        let gateway = SafetyGateway::with_executor(100, executor);

        let cmd = command_for_device(None);
        let result = gateway.execute_command(cmd).await;
        assert!(
            result.is_ok(),
            "Command without device_id should execute via global_lock, got {:?}",
            result,
        );

        let history = gateway.command_history();
        assert_eq!(history.len(), 1);
    }
}

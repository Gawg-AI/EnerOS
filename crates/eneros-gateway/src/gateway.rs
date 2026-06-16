use std::collections::VecDeque;
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
    /// Serializes the full validate → execute → record pipeline so that
    /// history ordering matches the logical commit order.
    execution_lock: Mutex<()>,
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
            execution_lock: Mutex::new(()),
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
            execution_lock: Mutex::new(()),
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
            execution_lock: Mutex::new(()),
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
            execution_lock: Mutex::new(()),
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

    /// Execute a command after safety validation (synchronous direct execution).
    /// For priority-queue-based execution, use `submit_command()` instead.
    ///
    /// The entire validate → execute → record pipeline is serialized under
    /// `execution_lock` (a tokio async mutex) so that history ordering
    /// matches the logical commit order even under concurrent execution.
    ///
    /// If the command has device routing information (`device_id`, `device_address`,
    /// `device_value`), it will be dispatched to the real device via the configured
    /// `CommandExecutor`. Otherwise, it falls back to the logging executor.
    pub async fn execute_command(&self, command: Command) -> Result<()> {
        // Serialize the full pipeline under the async mutex so that
        // concurrent commands commit in the order they were validated.
        let _guard = self.execution_lock.lock().await;

        // Validate command first (rejects never enter history).
        self.validate_command(&command)?;

        // Execute the command through the execution backend (async, may take time).
        let exec_result = self.command_executor.execute(&command).await?;

        // Store the execution result for ACK verification by rt_executor.
        *self.last_execution_result.write() = Some(exec_result.clone());

        // Record command under a single write lock; O(1) ring-buffer eviction.
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
        use eneros_device::adapter::{ConnectionConfig, ProtocolConfig, DeviceInfo, DataValue};
        use eneros_device::mock_adapter::mock::MockAdapter;
        use eneros_device::DeviceManager;
        use eneros_device::protocol::ProtocolType;
        use super::super::executor::DeviceCommandExecutor;

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
        ).with_device("rtu-1", "coil:1", DataValue::Bool(true));

        let result = gateway.execute_command(cmd).await;
        assert!(result.is_ok(), "Expected Ok, got {:?}", result);

        let history = gateway.command_history();
        assert_eq!(history.len(), 1);

        let exec_result = gateway.last_execution_result().unwrap();
        assert!(exec_result.success);
    }

    #[tokio::test]
    async fn test_execute_command_device_failure() {
        use eneros_device::adapter::{ConnectionConfig, ProtocolConfig, DeviceInfo, DataValue};
        use eneros_device::mock_adapter::mock::MockAdapter;
        use eneros_device::DeviceManager;
        use eneros_device::protocol::ProtocolType;
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
        ).with_device("nonexistent", "coil:1", DataValue::Bool(true));

        let result = gateway.execute_command(cmd).await;
        assert!(result.is_err(), "Expected error for nonexistent device");
    }
}

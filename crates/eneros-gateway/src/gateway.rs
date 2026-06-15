use std::collections::VecDeque;
use std::sync::Arc;

use eneros_core::Result;
use parking_lot::{Mutex, RwLock};

use super::command::Command;
use super::priority_queue::SharedPriorityCommandQueue;
use super::rt_executor::RealtimeExecutor;
use super::safety::SafetyCheck;

/// Real-time safety gateway for cross-domain communication
pub struct SafetyGateway {
    safety_checks: RwLock<Vec<Box<dyn SafetyCheck>>>,
    // VecDeque gives O(1) eviction of the oldest command; the previous Vec
    // used `remove(0)` which is O(n) on every accepted realtime command.
    command_history: RwLock<VecDeque<Command>>,
    execution_lock: Mutex<()>,
    max_history: usize,
    /// Optional priority command queue for realtime execution
    queue: Option<Arc<SharedPriorityCommandQueue>>,
    /// Optional realtime executor
    executor: RwLock<Option<Arc<RealtimeExecutor>>>,
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
    /// Validation and history-recording are intentionally sequenced so that a
    /// rejected command is never recorded as executed, and accepted commands
    /// are recorded in commit order under the history write lock.
    pub fn execute_command(&self, command: Command) -> Result<()> {
        let _commit_guard = self.execution_lock.lock();

        // Validate command first (rejects never enter history).
        self.validate_command(&command)?;

        // Record command under a single write lock; O(1) ring-buffer eviction.
        {
            let mut history = self.command_history.write();
            history.push_back(command.clone());
            while history.len() > self.max_history {
                history.pop_front();
            }
        }

        // Execute command (placeholder - would interface with real systems)
        tracing::info!("Executing command: {:?}", command);

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

    #[test]
    fn test_execute_command_records_history_in_validation_commit_order() {
        let gateway = Arc::new(SafetyGateway::new(10));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();

        gateway.register_safety_check(Box::new(BlockingCheck {
            entered: StdMutex::new(Some(entered_tx)),
            release: StdMutex::new(release_rx),
        }));

        let slow_gateway = gateway.clone();
        let slow = std::thread::spawn(move || {
            slow_gateway
                .execute_command(command_with_id("slow"))
                .unwrap();
        });

        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        let fast_gateway = gateway.clone();
        let fast = std::thread::spawn(move || {
            fast_gateway
                .execute_command(command_with_id("fast"))
                .unwrap();
        });

        std::thread::sleep(Duration::from_millis(20));
        release_tx.send(()).unwrap();
        slow.join().unwrap();
        fast.join().unwrap();

        let ids: Vec<String> = gateway
            .command_history()
            .into_iter()
            .map(|command| command.id)
            .collect();
        assert_eq!(ids, vec!["slow".to_string(), "fast".to_string()]);
    }
}

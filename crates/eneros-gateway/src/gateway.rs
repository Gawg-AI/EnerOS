use std::sync::Arc;

use parking_lot::RwLock;
use eneros_core::Result;

use super::safety::SafetyCheck;
use super::command::Command;
use super::priority_queue::SharedPriorityCommandQueue;
use super::rt_executor::RealtimeExecutor;

/// Real-time safety gateway for cross-domain communication
pub struct SafetyGateway {
    safety_checks: RwLock<Vec<Box<dyn SafetyCheck>>>,
    command_history: RwLock<Vec<Command>>,
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
            command_history: RwLock::new(Vec::new()),
            max_history,
            queue: None,
            executor: RwLock::new(None),
        }
    }

    /// Create a safety gateway with a priority command queue.
    pub fn with_queue(max_history: usize, queue: Arc<SharedPriorityCommandQueue>) -> Self {
        Self {
            safety_checks: RwLock::new(Vec::new()),
            command_history: RwLock::new(Vec::new()),
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
    pub fn execute_command(&self, command: Command) -> Result<()> {
        // Validate command
        self.validate_command(&command)?;

        // Record command
        let mut history = self.command_history.write();
        history.push(command.clone());
        if history.len() > self.max_history {
            history.remove(0);
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
                    command.command_type, command.priority
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
        self.command_history.read().clone()
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

use parking_lot::RwLock;
use eneros_core::Result;

use super::safety::SafetyCheck;
use super::command::Command;

/// Real-time safety gateway for cross-domain communication
pub struct SafetyGateway {
    safety_checks: RwLock<Vec<Box<dyn SafetyCheck>>>,
    command_history: RwLock<Vec<Command>>,
    max_history: usize,
}

impl SafetyGateway {
    /// Create a new safety gateway
    pub fn new(max_history: usize) -> Self {
        Self {
            safety_checks: RwLock::new(Vec::new()),
            command_history: RwLock::new(Vec::new()),
            max_history,
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

    /// Execute a command after safety validation
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

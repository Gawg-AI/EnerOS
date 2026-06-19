//! Command execution backend.
//!
//! This module provides the `CommandExecutor` trait and its implementations
//! that bridge the SafetyGateway's command pipeline to real device adapters.
//!
//! # Architecture
//!
//! ```text
//! Command (with device_id + device_address + device_value)
//!     │
//!     ▼
//! CommandExecutor::execute()
//!     │
//!     ├── DeviceCommandExecutor → DeviceManager::write() → ProtocolAdapter::write()
//!     │                                                    ├── Iec104Adapter (C_SC_NA_1 / C_SE_NC_1)
//!     │                                                    ├── Iec61850Adapter (MMS Write)
//!     │                                                    ├── ModbusTcpAdapter (Write Single/Multiple Register)
//!     │                                                    └── MqttAdapter (Publish)
//!     │
//!     └── LoggingExecutor → tracing::info! (fallback when no device is configured)
//! ```
//!
//! # ACK Verification
//!
//! After writing a value to the device, `DeviceCommandExecutor` reads back
//! the value to confirm the device accepted the command (ACK). If the
//! read-back value matches within tolerance, the execution is considered
//! successful. Otherwise, it retries up to `max_retries` times.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use eneros_core::Result;
use eneros_device::adapter::DataValue;
use eneros_device::DeviceManager;
use tracing::{debug, info, warn};

use crate::command::Command;

pub use eneros_core::execution::ExecutionResult;

/// Trait for command execution backends.
///
/// Implementations translate a `Command` into real device operations.
/// The trait is async to support network I/O to physical devices.
#[async_trait]
pub trait CommandExecutor: Send + Sync {
    /// Execute a command on the target device.
    ///
    /// Returns `Ok(ExecutionResult)` if the command was dispatched (even if
    /// the device NACK'd it — check `ExecutionResult::success`), or `Err`
    /// if the execution infrastructure itself failed (e.g., device not found).
    async fn execute(&self, command: &Command) -> Result<ExecutionResult>;

    /// Read back a value from the device for ACK verification.
    ///
    /// Returns `None` if the device or address is not available for read-back.
    async fn read_back(&self, command: &Command) -> Option<DataValue>;
}

/// Real device command executor that bridges to `DeviceManager`.
///
/// This is the production executor: it takes a `Command` with `device_id`,
/// `device_address`, and `device_value` set, and calls `DeviceManager::write()`
/// to send the value to the physical device via the appropriate protocol adapter.
///
/// After writing, it reads back the value to verify the device accepted the
/// command (ACK verification). If the read-back doesn't match, it retries
/// up to `max_retries` times with `retry_delay` between attempts.
pub struct DeviceCommandExecutor {
    device_manager: Arc<DeviceManager>,
    /// Maximum number of retries when ACK verification fails
    max_retries: u32,
    /// Delay between retries
    retry_delay: Duration,
    /// Tolerance for float value comparison in ACK verification
    float_tolerance: f64,
}

impl DeviceCommandExecutor {
    pub fn new(device_manager: Arc<DeviceManager>) -> Self {
        Self {
            device_manager,
            max_retries: 3,
            retry_delay: Duration::from_millis(100),
            float_tolerance: 0.01,
        }
    }

    pub fn with_retries(mut self, max_retries: u32, retry_delay: Duration) -> Self {
        self.max_retries = max_retries;
        self.retry_delay = retry_delay;
        self
    }

    pub fn with_float_tolerance(mut self, tolerance: f64) -> Self {
        self.float_tolerance = tolerance;
        self
    }

    /// Check if a read-back value matches the expected value.
    fn values_match(&self, expected: &DataValue, actual: &DataValue) -> bool {
        match (expected, actual) {
            (DataValue::Bool(a), DataValue::Bool(b)) => a == b,
            (DataValue::Int16(a), DataValue::Int16(b)) => a == b,
            (DataValue::Int32(a), DataValue::Int32(b)) => a == b,
            (DataValue::Int64(a), DataValue::Int64(b)) => a == b,
            (DataValue::Float32(a), DataValue::Float32(b)) => (a - b).abs() < self.float_tolerance as f32,
            (DataValue::Float64(a), DataValue::Float64(b)) => (a - b).abs() < self.float_tolerance,
            (DataValue::String(a), DataValue::String(b)) => a == b,
            (DataValue::Bytes(a), DataValue::Bytes(b)) => a == b,
            // Cross-type comparison: Int → Float (common in SCADA where setpoint is f64 but device reports i32)
            (DataValue::Float64(a), DataValue::Int32(b)) => (a - *b as f64).abs() < self.float_tolerance,
            (DataValue::Float64(a), DataValue::Int16(b)) => (a - *b as f64).abs() < self.float_tolerance,
            (DataValue::Int32(a), DataValue::Float64(b)) => (*a as f64 - b).abs() < self.float_tolerance,
            _ => false,
        }
    }
}

#[async_trait]
impl CommandExecutor for DeviceCommandExecutor {
    async fn execute(&self, command: &Command) -> Result<ExecutionResult> {
        let start = std::time::Instant::now();

        let device_id = match &command.device_id {
            Some(id) => id,
            None => {
                return Ok(ExecutionResult::failed(
                    format!("Command {} has no device_id", command.id),
                    start.elapsed(),
                ));
            }
        };

        let address = match &command.device_address {
            Some(addr) => addr,
            None => {
                return Ok(ExecutionResult::failed(
                    format!("Command {} has no device_address", command.id),
                    start.elapsed(),
                ));
            }
        };

        let value: DataValue = match &command.device_value {
            Some(v) => v.clone().into(),
            None => {
                return Ok(ExecutionResult::failed(
                    format!("Command {} has no device_value", command.id),
                    start.elapsed(),
                ));
            }
        };

        // Write the value to the device
        info!(
            "Executing command {} on device '{}' address '{}' value {:?}",
            command.id, device_id, address, value
        );

        if let Err(e) = self.device_manager.write(device_id, address, &value).await {
            warn!("Command {} write failed: {}", command.id, e);
            return Ok(ExecutionResult::failed(
                format!("Write to device '{}' failed: {}", device_id, e),
                start.elapsed(),
            ));
        }

        debug!("Command {} written to device '{}', verifying ACK", command.id, device_id);

        // ACK verification: read back and compare
        let mut retries = 0u32;
        loop {
            if let Some(read_value) = self.read_back(command).await {
                if self.values_match(&value, &read_value) {
                    info!(
                        "Command {} ACK verified on device '{}' (retries: {})",
                        command.id, device_id, retries
                    );
                    return Ok(ExecutionResult::ok_with_retries(
                        format!("Executed on device '{}', ACK verified", device_id),
                        start.elapsed(),
                        retries,
                    ));
                } else {
                    debug!(
                        "Command {} ACK mismatch: expected {:?}, got {:?} (retry {}/{})",
                        command.id, value, read_value, retries, self.max_retries
                    );
                }
            } else {
                debug!(
                    "Command {} ACK read-back unavailable (retry {}/{})",
                    command.id, retries, self.max_retries
                );
            }

            if retries >= self.max_retries {
                warn!(
                    "Command {} ACK verification failed after {} retries on device '{}'",
                    command.id, retries, device_id
                );
                return Ok(ExecutionResult::failed(
                    format!("ACK verification failed after {} retries on device '{}'", retries, device_id),
                    start.elapsed(),
                ));
            }

            retries += 1;
            tokio::time::sleep(self.retry_delay).await;
        }
    }

    async fn read_back(&self, command: &Command) -> Option<DataValue> {
        let device_id = command.device_id.as_ref()?;
        let address = command.device_address.as_ref()?;

        match self.device_manager.read(device_id, address).await {
            Ok(data_point) => Some(data_point.value),
            Err(e) => {
                debug!("Read-back failed for device '{}' address '{}': {}", device_id, address, e);
                None
            }
        }
    }
}

/// Logging-only executor (fallback when no device manager is configured).
///
/// This executor simply logs the command without performing any real
/// device operation. It always returns success, preserving backward
/// compatibility with the old placeholder behavior while making it
/// explicit that no real execution occurred.
pub struct LoggingExecutor;

#[async_trait]
impl CommandExecutor for LoggingExecutor {
    async fn execute(&self, command: &Command) -> Result<ExecutionResult> {
        let start = std::time::Instant::now();
        info!(
            "LoggingExecutor: command {} type {:?} target {} (no device backend)",
            command.id, command.command_type, command.target_id
        );
        Ok(ExecutionResult::ok(
            format!("Logged (no device backend configured) for command {}", command.id),
            start.elapsed(),
        ))
    }

    async fn read_back(&self, _command: &Command) -> Option<DataValue> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{CommandType, CommandPriority, DeviceValue};
    use eneros_device::adapter::{ConnectionConfig, ProtocolConfig, DeviceInfo};
    use eneros_device::mock_adapter::mock::MockAdapter;
    use eneros_device::protocol::ProtocolType;

    fn test_config() -> ConnectionConfig {
        ConnectionConfig {
            host: "127.0.0.1".into(),
            port: 502,
            timeout_ms: 3000,
            credentials: None,
            protocol_config: ProtocolConfig::Modbus { slave_id: 1, baud_rate: None },
        }
    }

    fn test_info(name: &str) -> DeviceInfo {
        DeviceInfo {
            device_id: name.to_string(),
            name: name.to_string(),
            protocol: ProtocolType::Modbus,
            manufacturer: "Test".into(),
            model: "Mock-100".into(),
            firmware_version: "1.0.0".into(),
            ip_address: "127.0.0.1".into(),
            port: 502,
            capabilities: vec!["read".into(), "write".into()],
        }
    }

    async fn setup_device_manager() -> Arc<DeviceManager> {
        let manager = Arc::new(DeviceManager::new());
        let adapter = Box::new(MockAdapter::new("mock-rtu"));
        manager.register_device("rtu-1", adapter, test_config(), test_info("rtu-1")).await;
        manager.connect("rtu-1").await.unwrap();
        manager
    }

    #[tokio::test]
    async fn test_device_executor_write_and_ack() {
        let manager = setup_device_manager().await;
        let executor = DeviceCommandExecutor::new(manager);

        let cmd = Command::new(CommandType::SwitchToggle, 42, CommandPriority::High, "test")
            .with_parameter("closed", 1.0)
            .with_device("rtu-1", "coil:1", DeviceValue::Bool(true));

        let result = executor.execute(&cmd).await.unwrap();
        assert!(result.success, "Expected success, got: {}", result.description);
        assert_eq!(result.retries, 0);
    }

    #[tokio::test]
    async fn test_device_executor_float_setpoint() {
        let manager = setup_device_manager().await;
        let executor = DeviceCommandExecutor::new(manager)
            .with_float_tolerance(0.1);

        let cmd = Command::new(CommandType::GeneratorSetpoint, 1, CommandPriority::Normal, "test")
            .with_parameter("target_mw", 150.0)
            .with_device("rtu-1", "holding:40001", DeviceValue::Float64(150.0));

        let result = executor.execute(&cmd).await.unwrap();
        assert!(result.success, "Expected success, got: {}", result.description);
    }

    #[tokio::test]
    async fn test_device_executor_no_device_id() {
        let manager = setup_device_manager().await;
        let executor = DeviceCommandExecutor::new(manager);

        let cmd = Command::new(CommandType::SwitchToggle, 42, CommandPriority::Normal, "test");
        let result = executor.execute(&cmd).await.unwrap();
        assert!(!result.success);
        assert!(result.description.contains("no device_id"));
    }

    #[tokio::test]
    async fn test_device_executor_device_not_found() {
        let manager = setup_device_manager().await;
        let executor = DeviceCommandExecutor::new(manager);

        let cmd = Command::new(CommandType::SwitchToggle, 42, CommandPriority::Normal, "test")
            .with_device("nonexistent", "coil:1", DeviceValue::Bool(true));

        let result = executor.execute(&cmd).await.unwrap();
        assert!(!result.success);
        assert!(result.description.contains("not found") || result.description.contains("failed"));
    }

    #[tokio::test]
    async fn test_logging_executor() {
        let executor = LoggingExecutor;
        let cmd = Command::new(CommandType::SwitchToggle, 42, CommandPriority::Normal, "test");
        let result = executor.execute(&cmd).await.unwrap();
        assert!(result.success);
        assert!(result.description.contains("no device backend"));
    }

    #[tokio::test]
    async fn test_values_match() {
        let manager = setup_device_manager().await;
        let executor = DeviceCommandExecutor::new(manager)
            .with_float_tolerance(0.1);

        assert!(executor.values_match(&DataValue::Bool(true), &DataValue::Bool(true)));
        assert!(!executor.values_match(&DataValue::Bool(true), &DataValue::Bool(false)));
        assert!(executor.values_match(&DataValue::Int32(42), &DataValue::Int32(42)));
        assert!(executor.values_match(&DataValue::Float64(150.0), &DataValue::Float64(150.05)));
        assert!(!executor.values_match(&DataValue::Float64(150.0), &DataValue::Float64(151.0)));
        // Cross-type: Float64 vs Int32
        assert!(executor.values_match(&DataValue::Float64(42.0), &DataValue::Int32(42)));
        assert!(executor.values_match(&DataValue::Int32(42), &DataValue::Float64(42.0)));
    }

    #[tokio::test]
    async fn test_read_back() {
        let manager = setup_device_manager().await;
        let executor = DeviceCommandExecutor::new(manager);

        // Write first
        let cmd = Command::new(CommandType::SwitchToggle, 42, CommandPriority::Normal, "test")
            .with_device("rtu-1", "holding:40001", DeviceValue::Int16(99));

        executor.execute(&cmd).await.unwrap();

        // Read back
        let read_value = executor.read_back(&cmd).await;
        assert!(read_value.is_some());
        assert_eq!(read_value.unwrap(), DataValue::Int16(99));
    }
}

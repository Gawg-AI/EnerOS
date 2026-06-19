use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Result of executing a command through the execution backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    /// Whether the command was successfully executed on the device
    pub success: bool,
    /// Human-readable description of what happened
    pub description: String,
    /// Time taken for execution (including ACK verification)
    pub latency: Duration,
    /// Number of retries attempted (0 = first try success)
    pub retries: u32,
}

impl ExecutionResult {
    pub fn ok(description: String, latency: Duration) -> Self {
        Self { success: true, description, latency, retries: 0 }
    }

    pub fn ok_with_retries(description: String, latency: Duration, retries: u32) -> Self {
        Self { success: true, description, latency, retries }
    }

    pub fn failed(description: String, latency: Duration) -> Self {
        Self { success: false, description, latency, retries: 0 }
    }
}

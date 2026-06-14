use std::collections::HashMap;
use std::process::{Child, Command as StdCommand, Stdio};
use std::time::Duration;
use serde::Deserialize;
use tracing::{debug, info};

use crate::python_bridge::BridgeError;

pub type BridgeResult<T> = Result<T, BridgeError>;

/// HTTP-based client for the EnerOS Python Bridge
pub struct BridgeClient {
    base_url: String,
    python_path: String,
    script_path: String,
    child: Option<Child>,
    client: reqwest::blocking::Client,
}

impl BridgeClient {
    /// Create a new BridgeClient (does not start the server yet)
    pub fn new() -> Self {
        Self {
            base_url: "http://127.0.0.1:8321".to_string(),
            python_path: "python".to_string(),
            script_path: Self::find_bridge_script(),
            child: None,
            client: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| reqwest::blocking::Client::new()),
        }
    }

    /// Set custom port
    pub fn with_port(mut self, port: u16) -> Self {
        self.base_url = format!("http://127.0.0.1:{}", port);
        self
    }

    /// Set custom Python path
    pub fn with_python(mut self, python_path: impl Into<String>) -> Self {
        self.python_path = python_path.into();
        self
    }

    fn find_bridge_script() -> String {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let path = std::path::Path::new(manifest_dir).join("python/bridge_http_server.py");
        path.to_string_lossy().to_string()
    }

    /// Start the Python HTTP bridge server as a background process
    pub fn start(&mut self) -> BridgeResult<()> {
        if self.child.is_some() {
            return Ok(()); // Already started
        }

        info!("Starting Python bridge server: {} {}", self.python_path, self.script_path);

        let child = StdCommand::new(&self.python_path)
            .arg(&self.script_path)
            .arg("--port")
            .arg(self.base_url.split(':').next_back().unwrap_or("8321"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| BridgeError::ProcessError(format!("Failed to start Python bridge: {}", e)))?;

        self.child = Some(child);

        // Wait for the server to be ready (poll /api/health)
        let max_retries = 30;
        for i in 0..max_retries {
            std::thread::sleep(Duration::from_millis(500));
            match self.health_check() {
                Ok(_) => {
                    info!("Python bridge server ready after {} attempts", i + 1);
                    return Ok(());
                }
                Err(_) => {
                    debug!("Waiting for Python bridge server... attempt {}", i + 1);
                }
            }
        }

        Err(BridgeError::ProcessError("Python bridge server failed to start within timeout".to_string()))
    }

    /// Check if the Python bridge server is healthy
    pub fn health_check(&self) -> BridgeResult<serde_json::Value> {
        let url = format!("{}/api/health", self.base_url);
        let resp = self.client.get(&url)
            .send()
            .map_err(|e| BridgeError::ProcessError(format!("Health check failed: {}", e)))?;

        let data: serde_json::Value = resp.json()
            .map_err(|e| BridgeError::ProcessError(format!("Failed to parse health check response: {}", e)))?;

        Ok(data)
    }

    /// Call a command on the Python bridge server
    pub fn call<T: serde::de::DeserializeOwned>(
        &self,
        command: &str,
        params: HashMap<String, serde_json::Value>,
    ) -> BridgeResult<T> {
        let url = format!("{}/api/{}", self.base_url, command);
        debug!("Calling Python bridge: {} with {:?}", command, params);

        let resp = self.client.post(&url)
            .json(&params)
            .send()
            .map_err(|e| BridgeError::ProcessError(format!("HTTP request failed: {}", e)))?;

        let response: BridgeHttpResponse = resp.json()
            .map_err(|e| BridgeError::ProcessError(format!("Failed to parse bridge response: {}", e)))?;

        if response.ok {
            response.data
                .and_then(|v| serde_json::from_value(v).ok())
                .ok_or_else(|| BridgeError::CommandFailed("Failed to deserialize response data".to_string()))
        } else {
            Err(BridgeError::CommandFailed(
                response.error.unwrap_or_else(|| "Unknown error".to_string()),
            ))
        }
    }

    /// Call a command and return raw JSON
    pub fn call_raw(
        &self,
        command: &str,
        params: HashMap<String, serde_json::Value>,
    ) -> BridgeResult<serde_json::Value> {
        self.call::<serde_json::Value>(command, params)
    }

    /// Stop the Python bridge server
    pub fn stop(&mut self) -> BridgeResult<()> {
        if let Some(mut child) = self.child.take() {
            child.kill().map_err(|e| BridgeError::ProcessError(format!("Failed to kill Python process: {}", e)))?;
            info!("Python bridge server stopped");
        }
        Ok(())
    }

    /// Check if the server is running
    pub fn is_running(&self) -> bool {
        self.child.is_some()
    }
}

impl Default for BridgeClient {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for BridgeClient {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
        }
    }
}

#[derive(Debug, Deserialize)]
struct BridgeHttpResponse {
    ok: bool,
    data: Option<serde_json::Value>,
    error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bridge_script_path_exists() {
        let client = BridgeClient::new();
        assert!(
            std::path::Path::new(&client.script_path).exists(),
            "Bridge script not found at: {}",
            client.script_path
        );
    }
}

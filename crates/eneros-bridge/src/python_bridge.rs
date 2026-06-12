use std::io::Write;
use std::process::{Command, Stdio};
use serde::{Deserialize, Serialize};
use tracing::{debug, error};

#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error("Python process error: {0}")]
    ProcessError(String),
    #[error("JSON parse error: {0}")]
    JsonError(#[from] serde_json::Error),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Command failed: {0}")]
    CommandFailed(String),
}

pub type BridgeResult<T> = Result<T, BridgeError>;

#[derive(Debug, Serialize)]
pub struct BridgeRequest {
    pub command: String,
    #[serde(skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub params: std::collections::HashMap<String, serde_json::Value>,
}

#[derive(Debug)]
pub struct BridgeResponse {
    pub ok: bool,
    pub data: Option<serde_json::Value>,
    pub error: Option<String>,
}

impl<'de> Deserialize<'de> for BridgeResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de;

        struct BridgeResponseVisitor;

        impl<'de> de::Visitor<'de> for BridgeResponseVisitor {
            type Value = BridgeResponse;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a BridgeResponse object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: de::MapAccess<'de>,
            {
                let mut ok = None;
                let mut data = None;
                let mut error = None;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "ok" => ok = Some(map.next_value()?),
                        "data" => data = Some(map.next_value()?),
                        "error" => error = Some(map.next_value()?),
                        _ => { let _ = map.next_value::<serde_json::Value>()?; }
                    }
                }

                Ok(BridgeResponse {
                    ok: ok.ok_or_else(|| de::Error::missing_field("ok"))?,
                    data,
                    error,
                })
            }
        }

        deserializer.deserialize_map(BridgeResponseVisitor)
    }
}

pub struct PythonBridge {
    script_path: String,
    python_path: String,
}

impl PythonBridge {
    pub fn new() -> Self {
        Self {
            script_path: Self::find_bridge_script(),
            python_path: "python".to_string(),
        }
    }

    pub fn with_python(python_path: impl Into<String>) -> Self {
        Self {
            script_path: Self::find_bridge_script(),
            python_path: python_path.into(),
        }
    }

    pub fn with_script(script_path: impl Into<String>) -> Self {
        Self {
            script_path: script_path.into(),
            python_path: "python".to_string(),
        }
    }

    fn find_bridge_script() -> String {
        let candidates = [
            "python/bridge_server.py",
            "../python/bridge_server.py",
            "../../python/bridge_server.py",
        ];
        for candidate in &candidates {
            if std::path::Path::new(candidate).exists() {
                return candidate.to_string();
            }
        }
        "python/bridge_server.py".to_string()
    }

    pub fn call<T: serde::de::DeserializeOwned>(
        &self,
        command: &str,
        params: std::collections::HashMap<String, serde_json::Value>,
    ) -> BridgeResult<T> {
        let request = BridgeRequest {
            command: command.to_string(),
            params,
        };

        let request_json = serde_json::to_string(&request)?;
        debug!("Calling Python: {} with {}", command, request_json);

        let mut child = Command::new(&self.python_path)
            .arg(&self.script_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| BridgeError::ProcessError(format!("Failed to spawn Python: {}", e)))?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(request_json.as_bytes())
                .map_err(BridgeError::IoError)?;
            drop(stdin);
        }

        let output = child
            .wait_with_output()
            .map_err(|e| BridgeError::ProcessError(format!("Failed to wait for output: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("Python process failed: {}", stderr);
            return Err(BridgeError::CommandFailed(format!(
                "Exit code: {}, stderr: {}",
                output.status.code().unwrap_or(-1),
                stderr
            )));
        }

        let response_str = String::from_utf8(output.stdout)
            .map_err(|e| BridgeError::ProcessError(format!("Invalid UTF-8: {}", e)))?;

        let response: BridgeResponse = serde_json::from_str(&response_str)?;

        if response.ok {
            response
                .data
                .and_then(|v| serde_json::from_value(v).ok())
                .ok_or_else(|| BridgeError::CommandFailed("Failed to deserialize response data".to_string()))
        } else {
            Err(BridgeError::CommandFailed(
                response.error.unwrap_or_else(|| "Unknown error".to_string()),
            ))
        }
    }

    pub fn call_raw(
        &self,
        command: &str,
        params: std::collections::HashMap<String, serde_json::Value>,
    ) -> BridgeResult<serde_json::Value> {
        self.call::<serde_json::Value>(command, params)
    }
}

impl Default for PythonBridge {
    fn default() -> Self {
        Self::new()
    }
}

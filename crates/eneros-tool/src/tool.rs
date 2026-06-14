use async_trait::async_trait;
use eneros_core::Result;
use serde::{Deserialize, Serialize};

/// Tool trait — unified interface for agent tools
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name
    fn name(&self) -> &str;

    /// Tool description
    fn description(&self) -> &str;

    /// JSON Schema for parameters
    fn parameters_schema(&self) -> serde_json::Value;

    /// Execute the tool with given parameters
    async fn execute(&self, params: serde_json::Value) -> Result<ToolOutput>;
}

/// Tool execution output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    /// Whether execution succeeded
    pub success: bool,
    /// Output data
    pub data: serde_json::Value,
    /// Human-readable message
    pub message: String,
}

impl ToolOutput {
    /// Create a successful output
    pub fn ok(data: serde_json::Value, message: &str) -> Self {
        Self {
            success: true,
            data,
            message: message.to_string(),
        }
    }

    /// Create a failed output
    pub fn err(message: &str) -> Self {
        Self {
            success: false,
            data: serde_json::Value::Null,
            message: message.to_string(),
        }
    }
}

/// Tool metadata for listing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInfo {
    /// Tool name
    pub name: String,
    /// Tool description
    pub description: String,
    /// Parameters JSON Schema
    pub parameters_schema: serde_json::Value,
}

/// Tool execution engine
pub struct ToolEngine {
    tools: std::collections::HashMap<String, Box<dyn Tool>>,
}

impl ToolEngine {
    /// Create a new tool engine
    pub fn new() -> Self {
        Self {
            tools: std::collections::HashMap::new(),
        }
    }

    /// Register a tool
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Execute a tool by name
    pub async fn execute(&self, name: &str, params: serde_json::Value) -> Result<ToolOutput> {
        match self.tools.get(name) {
            Some(tool) => tool.execute(params).await,
            None => Ok(ToolOutput::err(&format!("Unknown tool: {}", name))),
        }
    }

    /// List all registered tools
    pub fn list_tools(&self) -> Vec<ToolInfo> {
        self.tools
            .values()
            .map(|t| ToolInfo {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters_schema: t.parameters_schema(),
            })
            .collect()
    }

    /// Check if a tool is registered
    pub fn has_tool(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Get tool count
    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }
}

impl Default for ToolEngine {
    fn default() -> Self {
        Self::new()
    }
}

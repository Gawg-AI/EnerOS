//! Rig-based reasoning engine adapter.
//!
//! This module wraps the `rig` framework behind EnerOS's `ReasoningEngine` trait,
//! providing a clean upgrade path: rig is an implementation detail, not a public API.
//!
//! **Architecture**:
//! - `RigReasoningEngine` implements `ReasoningEngine` by delegating to rig's Agent
//! - rig types are never exposed in EnerOS's public API
//! - Feature flag `rig` controls availability; project compiles without it
//! - Version pinning in Cargo.toml ensures rig upgrades don't break EnerOS
//!
//! **Upgrade strategy**:
//! - rig version is pinned to `0.38.x` in Cargo.toml
//! - To upgrade: change version in Cargo.toml, run tests, fix any breaking changes
//!   in this adapter module only — the rest of EnerOS is unaffected

use std::sync::Arc;

use async_trait::async_trait;
use eneros_core::Result;
use tracing::{info, warn};

use crate::engine::{ReasoningEngine, ReasoningInput, ReasoningOutput};
use crate::rig_tools::PowerSystemToolSet;

/// Configuration for the rig-based reasoning engine.
///
/// This is an EnerOS-native config type that maps to rig's provider clients internally.
/// When rig's API changes, only the `build_agent()` method needs updating.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RigConfig {
    /// LLM provider name: "openai" | "ollama" | "deepseek" | "anthropic" | "groq"
    pub provider: String,
    /// Model name (provider-specific)
    pub model: String,
    /// API base URL (optional, uses provider default if not set)
    pub api_url: Option<String>,
    /// API key (optional for local providers like Ollama)
    pub api_key: Option<String>,
    /// Sampling temperature
    pub temperature: f64,
    /// Maximum tokens in response
    pub max_tokens: u32,
}

impl Default for RigConfig {
    fn default() -> Self {
        Self {
            provider: "ollama".to_string(),
            model: "qwen2.5:14b".to_string(),
            api_url: None,
            api_key: None,
            temperature: 0.7,
            max_tokens: 1024,
        }
    }
}

impl RigConfig {
    /// Create config from environment variables.
    ///
    /// Environment variables:
    /// - `ENEROS_RIG_PROVIDER`: provider name (default: "ollama")
    /// - `ENEROS_RIG_MODEL`: model name
    /// - `ENEROS_RIG_API_URL`: API base URL override
    /// - `ENEROS_RIG_API_KEY`: API key
    /// - `ENEROS_RIG_TEMPERATURE`: temperature
    /// - `ENEROS_RIG_MAX_TOKENS`: max tokens
    pub fn from_env() -> Self {
        Self {
            provider: std::env::var("ENEROS_RIG_PROVIDER")
                .unwrap_or_else(|_| "ollama".to_string()),
            model: std::env::var("ENEROS_RIG_MODEL")
                .unwrap_or_else(|_| "qwen2.5:14b".to_string()),
            api_url: std::env::var("ENEROS_RIG_API_URL").ok(),
            api_key: std::env::var("ENEROS_RIG_API_KEY").ok(),
            temperature: std::env::var("ENEROS_RIG_TEMPERATURE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.7),
            max_tokens: std::env::var("ENEROS_RIG_MAX_TOKENS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1024),
        }
    }
}

/// Macro to build a rig Agent (with or without tools) and prompt it.
///
/// This macro handles the typestate divergence:
/// - Without tools: `AgentBuilder<NoToolConfig>.build()` → `Agent<M, P>`
/// - With tools: `AgentBuilder<NoToolConfig>.tool(T).build()` → `Agent<M, P>`
///
/// Both paths produce the same `Agent<M, P>` type which implements `Prompt`.
macro_rules! build_and_prompt {
    ($client:expr, $config:expr, $system_prompt:expr, $user_prompt:expr, $tool_set:expr) => {{
        use rig_core::client::CompletionClient;
        use rig_core::completion::Prompt;
        use rig_core::tool::ToolDyn;

        if let Some(network) = $tool_set.network() {
            // Collect enabled tools into a Vec<Box<dyn ToolDyn>> for a single
            // .tools() call. This avoids the typestate mismatch that would
            // occur with conditional .tool() calls (NoToolConfig → WithBuilderTools).
            let mut tools: Vec<Box<dyn ToolDyn>> = Vec::new();
            if $tool_set.include_power_flow {
                tools.push(Box::new(crate::rig_tools::PowerFlowTool {
                    network: Arc::clone(network),
                }));
            }
            if $tool_set.include_constraint_check {
                tools.push(Box::new(crate::rig_tools::ConstraintCheckTool {
                    network: Arc::clone(network),
                }));
            }
            if $tool_set.include_n1_analysis {
                tools.push(Box::new(crate::rig_tools::N1AnalysisTool {
                    network: Arc::clone(network),
                }));
            }
            if $tool_set.include_voltage_stability {
                tools.push(Box::new(crate::rig_tools::VoltageStabilityTool {
                    network: Arc::clone(network),
                }));
            }

            let agent = $client
                .agent(&$config.model)
                .preamble($system_prompt)
                .temperature($config.temperature)
                .tools(tools)
                .build();
            agent.prompt($user_prompt).await.map_err(|e| {
                eneros_core::EnerOSError::Internal(format!("rig agent prompt failed: {}", e))
            })?
        } else {
            // Build agent without tools — NoToolConfig state
            let agent = $client
                .agent(&$config.model)
                .preamble($system_prompt)
                .temperature($config.temperature)
                .build();
            agent.prompt($user_prompt).await.map_err(|e| {
                eneros_core::EnerOSError::Internal(format!("rig agent prompt failed: {}", e))
            })?
        }
    }};
}

/// Rig-based reasoning engine — adapts rig's Agent to EnerOS's ReasoningEngine trait.
///
/// This is the **sole integration point** between EnerOS and rig. All rig-specific
/// code lives here and in `rig_tools.rs`. When rig releases a new version with
/// breaking changes, only these two files need updating.
pub struct RigReasoningEngine {
    config: RigConfig,
    fallback_engine: Option<Arc<dyn ReasoningEngine>>,
    tool_set: PowerSystemToolSet,
}

impl RigReasoningEngine {
    /// Create a new rig reasoning engine with the given configuration and network.
    ///
    /// The network enables power system tools (power flow, constraint check,
    /// N-1 analysis, voltage stability) that the LLM agent can call during
    /// reasoning.
    pub fn new(
        config: RigConfig,
        network: Arc<parking_lot::RwLock<eneros_network::PowerNetwork>>,
    ) -> Self {
        Self {
            config,
            fallback_engine: None,
            tool_set: PowerSystemToolSet::all(network),
        }
    }

    /// Create a new rig reasoning engine without a network (no tools).
    ///
    /// Use this for backward compatibility or when tool-calling is not needed.
    pub fn new_without_network(config: RigConfig) -> Self {
        Self {
            config,
            fallback_engine: None,
            tool_set: PowerSystemToolSet::default(),
        }
    }

    /// Set a fallback reasoning engine for degradation when rig/LLM is unavailable.
    pub fn with_fallback(mut self, engine: Arc<dyn ReasoningEngine>) -> Self {
        self.fallback_engine = Some(engine);
        self
    }

    /// Build the system prompt from a ReasoningInput.
    /// This is EnerOS-specific and independent of rig's prompt construction.
    fn build_system_prompt(&self) -> String {
        let mut prompt = "You are an expert power system operator AI assistant embedded in EnerOS. \
                 Analyze the power system situation using the provided observations and tools. \
                 Provide structured analysis with: conclusion, confidence (0.0-1.0), \
                 recommended actions, and step-by-step reasoning chain. \
                 Always prioritize safety and grid stability."
            .to_string();

        // Append tool descriptions when tools are available
        let tool_descs = self.tool_set.tool_descriptions();
        if !tool_descs.is_empty() {
            prompt.push_str("\n\n## Available Tools\nYou can call these tools to analyze the power system:\n");
            for (name, desc) in &tool_descs {
                prompt.push_str(&format!("- **{}**: {}\n", name, desc));
            }
            prompt.push_str("\nUse these tools to gather data before forming your conclusion.");
        }

        prompt
    }

    /// Build the user prompt from a ReasoningInput.
    fn build_user_prompt(&self, input: &ReasoningInput) -> String {
        let mut prompt = String::new();

        prompt.push_str(&format!("## Goal\n{}\n\n", input.goal));

        if !input.observations.is_empty() {
            prompt.push_str("## Observations\n");
            for obs in &input.observations {
                prompt.push_str(&format!("- {}\n", obs));
            }
            prompt.push('\n');
        }

        if !input.constraints.is_empty() {
            prompt.push_str("## Constraints\n");
            for c in &input.constraints {
                prompt.push_str(&format!("- {}\n", c));
            }
            prompt.push('\n');
        }

        if let Some(ref obs) = input.power_observation {
            prompt.push_str("## Power System Data\n");
            prompt.push_str(&obs.summary());
            prompt.push_str("\n\n");
        }

        prompt.push_str(
            "## Output Format\n\
             Respond with a JSON object:\n\
             {\"conclusion\": \"...\", \"confidence\": 0.0-1.0, \"actions\": [...], \"reasoning_chain\": [...]}\n",
        );

        prompt
    }

    /// Execute reasoning via rig's Agent with tool calling.
    ///
    /// This method encapsulates all rig API calls. When rig changes its API,
    /// only this method needs updating.
    async fn reason_via_rig(&self, input: &ReasoningInput) -> Result<ReasoningOutput> {
        let user_prompt = self.build_user_prompt(input);
        let system_prompt = self.build_system_prompt();

        let provider_lower = self.config.provider.to_lowercase();

        // Each provider branch builds its own typed client + agent and prompts it.
        // We cannot unify the Agent type across providers, so we dispatch here.
        // When tools are available, we attach them to the agent builder.
        let response = match provider_lower.as_str() {
            "openai" => {
                let client = self.build_openai_client()?;
                build_and_prompt!(
                    client, &self.config, &system_prompt, &user_prompt, self.tool_set
                )
            }
            "ollama" => {
                let client = self.build_ollama_client()?;
                build_and_prompt!(
                    client, &self.config, &system_prompt, &user_prompt, self.tool_set
                )
            }
            "deepseek" => {
                let client = self.build_deepseek_client()?;
                build_and_prompt!(
                    client, &self.config, &system_prompt, &user_prompt, self.tool_set
                )
            }
            "anthropic" => {
                let client = self.build_anthropic_client()?;
                build_and_prompt!(
                    client, &self.config, &system_prompt, &user_prompt, self.tool_set
                )
            }
            "groq" => {
                let client = self.build_groq_client()?;
                build_and_prompt!(
                    client, &self.config, &system_prompt, &user_prompt, self.tool_set
                )
            }
            _ => {
                // Fallback: use OpenAI-compatible client with custom base URL
                let client = self.build_openai_compatible_client()?;
                build_and_prompt!(
                    client, &self.config, &system_prompt, &user_prompt, self.tool_set
                )
            }
        };

        // Parse the response into ReasoningOutput
        crate::llm_prompt::parse_llm_response(&response)
    }

    // --- Provider client builders ---
    // Each method maps RigConfig to a rig provider client.
    // When rig changes its provider API, only these methods need updating.

    fn build_openai_client(
        &self,
    ) -> Result<rig_core::providers::openai::Client> {
        let key = self.config.api_key.as_deref().unwrap_or("");
        rig_core::providers::openai::Client::new(key).map_err(|e| {
            eneros_core::EnerOSError::Internal(format!("Failed to build OpenAI client: {}", e))
        })
    }

    fn build_ollama_client(
        &self,
    ) -> Result<rig_core::providers::ollama::Client> {
        use rig_core::client::Nothing;

        // Use Client::new(Nothing) for the default no-auth case,
        // then rebuild with custom base URL if needed.
        if let Some(ref url) = self.config.api_url {
            let api_key = if let Some(ref key) = self.config.api_key {
                rig_core::providers::ollama::OllamaApiKey::from(key.as_str())
            } else {
                rig_core::providers::ollama::OllamaApiKey::from(Nothing)
            };
            rig_core::providers::ollama::Client::builder()
                .api_key(api_key)
                .base_url(url)
                .build()
                .map_err(|e| {
                    eneros_core::EnerOSError::Internal(format!(
                        "Failed to build Ollama client: {}",
                        e
                    ))
                })
        } else if let Some(ref key) = self.config.api_key {
            rig_core::providers::ollama::Client::builder()
                .api_key(rig_core::providers::ollama::OllamaApiKey::from(key.as_str()))
                .build()
                .map_err(|e| {
                    eneros_core::EnerOSError::Internal(format!(
                        "Failed to build Ollama client: {}",
                        e
                    ))
                })
        } else {
            rig_core::providers::ollama::Client::new(Nothing).map_err(|e| {
                eneros_core::EnerOSError::Internal(format!(
                    "Failed to build Ollama client: {}",
                    e
                ))
            })
        }
    }

    fn build_deepseek_client(
        &self,
    ) -> Result<rig_core::providers::deepseek::Client> {
        let key = self.config.api_key.as_deref().unwrap_or("");
        rig_core::providers::deepseek::Client::new(key).map_err(|e| {
            eneros_core::EnerOSError::Internal(format!("Failed to build DeepSeek client: {}", e))
        })
    }

    fn build_anthropic_client(
        &self,
    ) -> Result<rig_core::providers::anthropic::Client> {
        let key = self.config.api_key.as_deref().unwrap_or("");
        rig_core::providers::anthropic::Client::new(key).map_err(|e| {
            eneros_core::EnerOSError::Internal(format!("Failed to build Anthropic client: {}", e))
        })
    }

    fn build_groq_client(
        &self,
    ) -> Result<rig_core::providers::groq::Client> {
        let key = self.config.api_key.as_deref().unwrap_or("");
        rig_core::providers::groq::Client::new(key).map_err(|e| {
            eneros_core::EnerOSError::Internal(format!("Failed to build Groq client: {}", e))
        })
    }

    fn build_openai_compatible_client(
        &self,
    ) -> Result<rig_core::providers::openai::CompletionsClient> {
        let key = self.config.api_key.as_deref().unwrap_or("");
        let mut builder = rig_core::providers::openai::CompletionsClient::builder().api_key(key);
        if let Some(ref url) = self.config.api_url {
            builder = builder.base_url(url);
        }
        builder.build().map_err(|e| {
            eneros_core::EnerOSError::Internal(format!(
                "Failed to build OpenAI-compatible client: {}",
                e
            ))
        })
    }
}

#[async_trait]
impl ReasoningEngine for RigReasoningEngine {
    fn name(&self) -> &str {
        "rig-reasoning"
    }

    async fn reason(&self, input: ReasoningInput) -> Result<ReasoningOutput> {
        info!(provider = %self.config.provider, model = %self.config.model, "Rig reasoning engine called");

        match self.reason_via_rig(&input).await {
            Ok(output) => Ok(output),
            Err(e) => {
                warn!(error = %e, "Rig reasoning failed");
                if let Some(ref fallback) = self.fallback_engine {
                    info!("Falling back to fallback reasoning engine");
                    fallback.reason(input).await
                } else {
                    Err(e)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rig_config_default() {
        let config = RigConfig::default();
        assert_eq!(config.provider, "ollama");
        assert_eq!(config.model, "qwen2.5:14b");
        assert!((config.temperature - 0.7).abs() < f64::EPSILON);
        assert_eq!(config.max_tokens, 1024);
    }

    #[test]
    fn test_rig_config_from_env() {
        std::env::set_var("ENEROS_RIG_PROVIDER", "openai");
        std::env::set_var("ENEROS_RIG_MODEL", "gpt-4o");
        std::env::set_var("ENEROS_RIG_API_KEY", "test-key");
        std::env::set_var("ENEROS_RIG_TEMPERATURE", "0.5");
        std::env::set_var("ENEROS_RIG_MAX_TOKENS", "2048");

        let config = RigConfig::from_env();
        assert_eq!(config.provider, "openai");
        assert_eq!(config.model, "gpt-4o");
        assert_eq!(config.api_key.as_deref(), Some("test-key"));
        assert!((config.temperature - 0.5).abs() < f64::EPSILON);
        assert_eq!(config.max_tokens, 2048);

        std::env::remove_var("ENEROS_RIG_PROVIDER");
        std::env::remove_var("ENEROS_RIG_MODEL");
        std::env::remove_var("ENEROS_RIG_API_KEY");
        std::env::remove_var("ENEROS_RIG_TEMPERATURE");
        std::env::remove_var("ENEROS_RIG_MAX_TOKENS");
    }

    #[test]
    fn test_rig_engine_creation_without_network() {
        let config = RigConfig::default();
        let engine = RigReasoningEngine::new_without_network(config);
        assert_eq!(engine.name(), "rig-reasoning");
    }

    #[test]
    fn test_rig_engine_creation_with_network() {
        let config = RigConfig::default();
        let network = Arc::new(parking_lot::RwLock::new(
            eneros_network::PowerNetwork::from_ieee14(),
        ));
        let engine = RigReasoningEngine::new(config, network);
        assert_eq!(engine.name(), "rig-reasoning");
        assert!(engine.tool_set.has_tools());
    }

    #[test]
    fn test_rig_engine_with_fallback() {
        use crate::engine::RuleBasedEngine;
        let config = RigConfig::default();
        let fallback = Arc::new(RuleBasedEngine::new());
        let engine = RigReasoningEngine::new_without_network(config).with_fallback(fallback);
        assert_eq!(engine.name(), "rig-reasoning");
        assert!(engine.fallback_engine.is_some());
    }

    #[test]
    fn test_build_system_prompt() {
        let engine = RigReasoningEngine::new_without_network(RigConfig::default());
        let prompt = engine.build_system_prompt();
        assert!(prompt.contains("EnerOS"));
        assert!(prompt.contains("power system"));
    }

    #[test]
    fn test_build_system_prompt_with_tools() {
        let network = Arc::new(parking_lot::RwLock::new(
            eneros_network::PowerNetwork::from_ieee14(),
        ));
        let engine = RigReasoningEngine::new(RigConfig::default(), network);
        let prompt = engine.build_system_prompt();
        assert!(prompt.contains("Available Tools"));
        assert!(prompt.contains("power_flow"));
        assert!(prompt.contains("constraint_check"));
    }

    #[test]
    fn test_build_user_prompt() {
        let engine = RigReasoningEngine::new_without_network(RigConfig::default());
        let input = ReasoningInput::new("Test goal")
            .with_observation("Bus 3 voltage low")
            .with_constraint("Voltage must be 0.95-1.05 pu");
        let prompt = engine.build_user_prompt(&input);
        assert!(prompt.contains("Test goal"));
        assert!(prompt.contains("Bus 3 voltage low"));
        assert!(prompt.contains("0.95-1.05"));
        assert!(prompt.contains("conclusion"));
    }
}

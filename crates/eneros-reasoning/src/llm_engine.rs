use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use eneros_core::Result;
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use crate::engine::{ReasoningEngine, ReasoningInput, ReasoningOutput};
use crate::llm_prompt::{build_power_system_prompt, parse_llm_response};

/// Supported LLM providers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[deprecated(since = "0.2.0", note = "Use RigConfig with RigReasoningEngine instead")]
pub enum LlmProvider {
    OpenAI,
    Ollama,
    Custom(String),
}

#[allow(deprecated)]
impl LlmProvider {
    /// Default API URL for this provider.
    pub fn default_api_url(&self) -> &str {
        match self {
            LlmProvider::OpenAI => "https://api.openai.com",
            LlmProvider::Ollama => "http://localhost:11434",
            LlmProvider::Custom(url) => url,
        }
    }

    /// Default model for this provider.
    pub fn default_model(&self) -> &str {
        match self {
            LlmProvider::OpenAI => "gpt-4",
            LlmProvider::Ollama => "llama3",
            LlmProvider::Custom(_) => "default",
        }
    }
}

/// Configuration for the LLM reasoning engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[deprecated(since = "0.2.0", note = "Use RigConfig with RigReasoningEngine instead")]
pub struct LlmConfig {
    /// LLM provider
    pub provider: LlmProvider,
    /// API base URL
    pub api_url: String,
    /// Model name
    pub model: String,
    /// API key (optional, read from ENEROS_LLM_API_KEY)
    pub api_key: Option<String>,
    /// Maximum tokens in the response
    pub max_tokens: u32,
    /// Sampling temperature
    pub temperature: f64,
    /// Request timeout in seconds
    pub timeout_secs: u64,
}

#[allow(deprecated)]
impl Default for LlmConfig {
    fn default() -> Self {
        let provider = LlmProvider::Ollama;
        Self {
            api_url: provider.default_api_url().to_string(),
            model: provider.default_model().to_string(),
            provider,
            api_key: None,
            max_tokens: 1024,
            temperature: 0.7,
            timeout_secs: 30,
        }
    }
}

#[allow(deprecated)]
impl LlmConfig {
    /// Create config from environment variables.
    ///
    /// Environment variables:
    /// - `ENEROS_LLM_PROVIDER`: "openai" | "ollama" | "custom:<url>" (default: "ollama")
    /// - `ENEROS_LLM_API_URL`: override the default API URL
    /// - `ENEROS_LLM_MODEL`: model name
    /// - `ENEROS_LLM_API_KEY`: API key
    /// - `ENEROS_LLM_MAX_TOKENS`: max tokens (u32)
    /// - `ENEROS_LLM_TEMPERATURE`: temperature (f64)
    pub fn from_env() -> Self {
        let provider = match std::env::var("ENEROS_LLM_PROVIDER")
            .unwrap_or_else(|_| "ollama".to_string())
            .to_lowercase()
            .as_str()
        {
            "openai" => LlmProvider::OpenAI,
            v if v.starts_with("custom:") => {
                let url = v.strip_prefix("custom:").unwrap().to_string();
                LlmProvider::Custom(url)
            }
            _ => LlmProvider::Ollama,
        };

        let api_url = std::env::var("ENEROS_LLM_API_URL")
            .unwrap_or_else(|_| provider.default_api_url().to_string());

        let model = std::env::var("ENEROS_LLM_MODEL")
            .unwrap_or_else(|_| provider.default_model().to_string());

        let api_key = std::env::var("ENEROS_LLM_API_KEY").ok();

        let max_tokens = std::env::var("ENEROS_LLM_MAX_TOKENS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1024);

        let temperature = std::env::var("ENEROS_LLM_TEMPERATURE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.7);

        let timeout_secs = 30;

        Self {
            provider,
            api_url,
            model,
            api_key,
            max_tokens,
            temperature,
            timeout_secs,
        }
    }
}

/// LLM-powered reasoning engine.
#[deprecated(since = "0.2.0", note = "Use RigReasoningEngine instead — it supports tool-calling and more providers")]
pub struct LlmReasoningEngine {
    config: LlmConfig,
    client: reqwest::Client,
    fallback_engine: Option<Arc<dyn ReasoningEngine>>,
}

#[allow(deprecated)]
impl LlmReasoningEngine {
    /// Create a new LLM reasoning engine with the given configuration.
    pub fn new(config: LlmConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .expect("Failed to build HTTP client");

        Self {
            config,
            client,
            fallback_engine: None,
        }
    }

    /// Set a fallback reasoning engine for degradation when the LLM is unavailable.
    pub fn with_fallback(mut self, engine: Arc<dyn ReasoningEngine>) -> Self {
        self.fallback_engine = Some(engine);
        self
    }

    /// Call the LLM API (OpenAI-compatible /v1/chat/completions endpoint).
    async fn call_llm_api(&self, prompt: &str) -> Result<String> {
        let url = format!("{}/v1/chat/completions", self.config.api_url.trim_end_matches('/'));

        let body = serde_json::json!({
            "model": self.config.model,
            "messages": [
                {
                    "role": "system",
                    "content": "You are an expert power system operator AI assistant. Analyze the following power system situation and provide recommendations."
                },
                {
                    "role": "user",
                    "content": prompt
                }
            ],
            "max_tokens": self.config.max_tokens,
            "temperature": self.config.temperature
        });

        let mut request = self.client.post(&url).json(&body);

        if let Some(ref api_key) = self.config.api_key {
            request = request.bearer_auth(api_key);
        }

        info!(provider = ?self.config.provider, model = %self.config.model, "Calling LLM API");

        let response = request.send().await.map_err(|e| {
            eneros_core::EnerOSError::Internal(format!("LLM API request failed: {}", e))
        })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let msg = format!("LLM API returned status {}: {}", status, body);
            error!(%msg);
            return Err(eneros_core::EnerOSError::Internal(msg));
        }

        let resp_json: serde_json::Value = response.json().await.map_err(|e| {
            eneros_core::EnerOSError::Internal(format!("Failed to parse LLM API response: {}", e))
        })?;

        // Extract choices[0].message.content
        let content = resp_json
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .ok_or_else(|| {
                eneros_core::EnerOSError::Internal(
                    "Invalid LLM API response: missing choices[0].message.content".to_string(),
                )
            })?;

        Ok(content.to_string())
    }
}

#[async_trait]
#[allow(deprecated)]
impl ReasoningEngine for LlmReasoningEngine {
    fn name(&self) -> &str {
        "llm-reasoning"
    }

    async fn reason(&self, input: ReasoningInput) -> Result<ReasoningOutput> {
        let prompt = build_power_system_prompt(&input);

        match self.call_llm_api(&prompt).await {
            Ok(response_text) => {
                info!("LLM response received, parsing...");
                parse_llm_response(&response_text)
            }
            Err(e) => {
                warn!(error = %e, "LLM API call failed");
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
#[allow(deprecated)]
mod tests {
    use super::*;

    /// Mutex to serialize env-var tests (they use std::env::set_var which is not thread-safe)
    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_llm_config_default() {
        let config = LlmConfig::default();
        assert_eq!(config.provider, LlmProvider::Ollama);
        assert_eq!(config.api_url, "http://localhost:11434");
        assert_eq!(config.model, "llama3");
        assert!(config.api_key.is_none());
        assert_eq!(config.max_tokens, 1024);
        assert!((config.temperature - 0.7).abs() < f64::EPSILON);
        assert_eq!(config.timeout_secs, 30);
    }

    #[test]
    fn test_llm_config_from_env() {
        let _lock = ENV_MUTEX.lock().unwrap();
        // Set temp env vars
        std::env::set_var("ENEROS_LLM_PROVIDER", "openai");
        std::env::set_var("ENEROS_LLM_MODEL", "gpt-3.5-turbo");
        std::env::set_var("ENEROS_LLM_API_KEY", "test-key-123");
        std::env::set_var("ENEROS_LLM_MAX_TOKENS", "2048");
        std::env::set_var("ENEROS_LLM_TEMPERATURE", "0.5");

        let config = LlmConfig::from_env();
        assert_eq!(config.provider, LlmProvider::OpenAI);
        assert_eq!(config.model, "gpt-3.5-turbo");
        assert_eq!(config.api_key.as_deref(), Some("test-key-123"));
        assert_eq!(config.max_tokens, 2048);
        assert!((config.temperature - 0.5).abs() < f64::EPSILON);

        // Clean up
        std::env::remove_var("ENEROS_LLM_PROVIDER");
        std::env::remove_var("ENEROS_LLM_MODEL");
        std::env::remove_var("ENEROS_LLM_API_KEY");
        std::env::remove_var("ENEROS_LLM_MAX_TOKENS");
        std::env::remove_var("ENEROS_LLM_TEMPERATURE");
    }

    #[test]
    fn test_llm_config_from_env_ollama() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("ENEROS_LLM_PROVIDER", "ollama");
        std::env::remove_var("ENEROS_LLM_API_URL");
        std::env::remove_var("ENEROS_LLM_MODEL");
        std::env::remove_var("ENEROS_LLM_API_KEY");

        let config = LlmConfig::from_env();
        assert_eq!(config.provider, LlmProvider::Ollama);
        assert_eq!(config.api_url, "http://localhost:11434");
        assert_eq!(config.model, "llama3");

        std::env::remove_var("ENEROS_LLM_PROVIDER");
    }

    #[test]
    fn test_llm_config_from_env_custom() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("ENEROS_LLM_PROVIDER", "custom:http://my-llm:8080");
        std::env::remove_var("ENEROS_LLM_API_URL");
        std::env::remove_var("ENEROS_LLM_MODEL");

        let config = LlmConfig::from_env();
        assert_eq!(config.provider, LlmProvider::Custom("http://my-llm:8080".to_string()));

        std::env::remove_var("ENEROS_LLM_PROVIDER");
    }

    #[test]
    fn test_llm_engine_creation() {
        let config = LlmConfig::default();
        let engine = LlmReasoningEngine::new(config);
        assert_eq!(engine.name(), "llm-reasoning");
    }

    #[test]
    fn test_llm_engine_with_fallback() {
        use crate::engine::RuleBasedEngine;

        let config = LlmConfig::default();
        let fallback = Arc::new(RuleBasedEngine::new());
        let engine = LlmReasoningEngine::new(config).with_fallback(fallback);

        assert_eq!(engine.name(), "llm-reasoning");
        assert!(engine.fallback_engine.is_some());
    }

    #[test]
    fn test_llm_provider_default_api_url() {
        assert_eq!(LlmProvider::OpenAI.default_api_url(), "https://api.openai.com");
        assert_eq!(LlmProvider::Ollama.default_api_url(), "http://localhost:11434");
        assert_eq!(LlmProvider::Custom("http://custom:9090".to_string()).default_api_url(), "http://custom:9090");
    }

    #[test]
    fn test_llm_provider_default_model() {
        assert_eq!(LlmProvider::OpenAI.default_model(), "gpt-4");
        assert_eq!(LlmProvider::Ollama.default_model(), "llama3");
        assert_eq!(LlmProvider::Custom("http://x".to_string()).default_model(), "default");
    }
}

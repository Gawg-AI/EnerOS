//! Mock 部署后端（D12：默认后端，纯 Rust，无外部依赖）.
//!
//! [`MockDeployBackend`] 用于无 llama.cpp 环境下的接口验证。硬件检查始终返回
//! 满足要求（RAM 16GB / VRAM 8GB），模型加载委托给 [`LlmEngine`]，加载耗时
//! 默认 1ms（可覆盖）。

use eneros_llm_engine::LlmEngine;

use crate::backend::{DeployBackend, HardwareCheck};
use crate::config::QuantConfig7B;
use crate::error::DeployError;

/// Mock 部署后端（D12 默认）.
pub struct MockDeployBackend {
    /// 加载耗时覆盖值（None 时使用默认 1ms）.
    load_time_override: Option<u64>,
}

impl MockDeployBackend {
    /// 创建 Mock 后端（加载耗时默认 1ms）.
    pub fn new() -> Self {
        Self {
            load_time_override: None,
        }
    }

    /// 创建指定加载耗时的 Mock 后端.
    pub fn with_load_time(ns: u64) -> Self {
        Self {
            load_time_override: Some(ns),
        }
    }
}

impl Default for MockDeployBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl DeployBackend for MockDeployBackend {
    fn check_hardware(&self, _config: &QuantConfig7B) -> Result<HardwareCheck, DeployError> {
        Ok(HardwareCheck {
            ram_gb: 16.0,
            vram_gb: 8.0,
            meets_requirements: true,
        })
    }

    fn load_model(
        &self,
        engine: &mut dyn LlmEngine,
        config: &QuantConfig7B,
    ) -> Result<u64, DeployError> {
        engine
            .load_model(&config.model_path)
            .map_err(DeployError::from)?;
        Ok(self.load_time_override.unwrap_or(1_000_000))
    }
}

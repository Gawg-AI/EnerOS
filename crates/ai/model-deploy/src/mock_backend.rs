//! Mock 部署后端（D12：默认后端，纯 Rust，无外部依赖）.
//!
//! [`MockDeployBackend`] 用于无 llama.cpp 环境下的接口验证。硬件检查始终返回
//! 满足要求（RAM 16GB / VRAM 8GB），模型加载委托给 [`LlmEngine`]，加载耗时
//! 默认 1ms（可覆盖）。

#[cfg(any(test, feature = "mock"))]
use eneros_llm_engine::LlmEngine;

#[cfg(any(test, feature = "mock"))]
use crate::backend::{DeployBackend, HardwareCheck};
#[cfg(any(test, feature = "mock"))]
use crate::config::QuantConfig7B;
#[cfg(any(test, feature = "mock"))]
use crate::error::DeployError;

/// Mock 部署后端（D12 默认）.
#[cfg(any(test, feature = "mock"))]
pub struct MockDeployBackend {
    /// 加载耗时覆盖值（None 时使用默认 1ms）.
    load_time_override: Option<u64>,
}

#[cfg(any(test, feature = "mock"))]
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

#[cfg(any(test, feature = "mock"))]
impl Default for MockDeployBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(test, feature = "mock"))]
impl DeployBackend for MockDeployBackend {
    fn check_hardware(&self, config: &QuantConfig7B) -> Result<HardwareCheck, DeployError> {
        // ARC-006：Mock 检查不再恒真——需求超能力 → HardwareInsufficient
        // （出口测试可对 Mock 失败）。
        let ram_gb = 16.0;
        let vram_gb = 8.0;
        Ok(HardwareCheck {
            ram_gb,
            vram_gb,
            meets_requirements: ram_gb >= config.min_ram_gb && vram_gb >= config.min_vram_gb,
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

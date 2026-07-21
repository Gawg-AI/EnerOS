//! llama.cpp 部署后端（D3：feature-gated，启用 `llama-cpp` feature）.
//!
//! [`LlamaDeployBackend`] 为生产环境后端，硬件检查与模型加载通过 v0.59.0
//! `LlamaCppEngine`（FFI 由 v0.59.0 封装）完成。本 crate 不直接声明 FFI（D10），
//! 仅通过 [`eneros_llm_engine::LlmEngine`] trait 间接调用。

#![cfg(feature = "llama-cpp")]

use eneros_llm_engine::LlmEngine;

use crate::backend::{DeployBackend, HardwareCheck};
use crate::config::QuantConfig7B;
use crate::error::DeployError;

/// llama.cpp 部署后端（D3 feature-gated）.
///
/// 启用 `llama-cpp` feature 后可用。硬件检查简化为返回配置最小值
/// （实际实现应通过 FFI 查询设备显存），模型加载委托给 `LlmEngine`。
pub struct LlamaDeployBackend;

impl LlamaDeployBackend {
    /// 创建 llama.cpp 部署后端.
    pub fn new() -> Self {
        Self
    }
}

impl Default for LlamaDeployBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl DeployBackend for LlamaDeployBackend {
    fn check_hardware(&self, config: &QuantConfig7B) -> Result<HardwareCheck, DeployError> {
        // 简化实现：假设硬件满足（实际实现应通过 FFI 查询 GPU 显存）.
        Ok(HardwareCheck {
            ram_gb: config.min_ram_gb,
            vram_gb: config.min_vram_gb,
            meets_requirements: true,
        })
    }

    fn load_model(
        &self,
        engine: &mut dyn LlmEngine,
        config: &QuantConfig7B,
    ) -> Result<u64, DeployError> {
        // 模型加载委托给 LlmEngine（FFI 由 v0.59.0 LlamaCppEngine 处理，D10）.
        // 返回 0，实际加载耗时由调用方通过 engine.stats() 获取.
        engine
            .load_model(&config.model_path)
            .map_err(DeployError::from)?;
        Ok(0)
    }
}

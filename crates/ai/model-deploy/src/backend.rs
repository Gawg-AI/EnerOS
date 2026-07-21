//! 部署后端抽象（D2：抽象 trait + D12 默认 Mock 实现）.
//!
//! [`DeployBackend`] 抽象硬件检查与模型加载两个步骤，由
//! [`crate::mock_backend::MockDeployBackend`]（默认）与
//! [`crate::llama_backend::LlamaDeployBackend`]（feature-gated，D3）实现。

use eneros_llm_engine::LlmEngine;

use crate::config::QuantConfig7B;
use crate::error::DeployError;

/// 硬件检查结果.
#[derive(Debug, Clone)]
pub struct HardwareCheck {
    /// 可用系统内存（GB）.
    pub ram_gb: f64,
    /// 可用显存（GB）.
    pub vram_gb: f64,
    /// 是否满足配置要求.
    pub meets_requirements: bool,
}

/// 部署后端 trait.
///
/// 抽象硬件检查与模型加载，实现方包括 [`crate::mock_backend::MockDeployBackend`]
/// 与 [`crate::llama_backend::LlamaDeployBackend`]（D3 feature-gated）。
pub trait DeployBackend {
    /// 检查硬件是否满足配置要求.
    fn check_hardware(&self, config: &QuantConfig7B) -> Result<HardwareCheck, DeployError>;

    /// 加载模型到引擎，返回加载耗时（纳秒）.
    fn load_model(
        &self,
        engine: &mut dyn LlmEngine,
        config: &QuantConfig7B,
    ) -> Result<u64, DeployError>;
}

//! 部署错误类型（D7：7 变体 + Display + From<LlmError>）.
//!
//! 覆盖硬件不足、模型加载失败、推理失败、结果非法、超时、后端错误、未部署
//! 等场景。通过 `From<LlmError>` 将 v0.59.0 引擎错误转换为部署错误。

use core::fmt;

use eneros_llm_engine::LlmError;

/// 模型部署错误.
///
/// 7 变体覆盖部署验证全流程失败场景（D7）。
#[derive(Debug, Clone, PartialEq)]
pub enum DeployError {
    /// 硬件不满足要求（RAM/VRAM 不足 / GPU 不可用）.
    HardwareInsufficient,
    /// 模型加载失败（文件不存在 / 格式错误）.
    ModelLoadFailed,
    /// 推理失败（引擎内部错误 / 提示词非法）.
    InferenceFailed,
    /// 推理结果非法（为空 / 不含期望关键词）.
    InvalidResult,
    /// 推理超时.
    Timeout,
    /// 后端错误（部署后端内部异常）.
    BackendError,
    /// 模型未部署（未调用 deploy 或已 undeploy）.
    NotDeployed,
}

impl fmt::Display for DeployError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeployError::HardwareInsufficient => f.write_str("hardware insufficient"),
            DeployError::ModelLoadFailed => f.write_str("model load failed"),
            DeployError::InferenceFailed => f.write_str("inference failed"),
            DeployError::InvalidResult => f.write_str("invalid inference result"),
            DeployError::Timeout => f.write_str("deployment timeout"),
            DeployError::BackendError => f.write_str("backend error"),
            DeployError::NotDeployed => f.write_str("model not deployed"),
        }
    }
}

impl From<LlmError> for DeployError {
    /// 将 v0.59.0 [`LlmError`] 转换为 [`DeployError`]（D11 类型复用）.
    fn from(e: LlmError) -> Self {
        match e {
            LlmError::LoadFailed | LlmError::InvalidPath => DeployError::ModelLoadFailed,
            LlmError::InferFailed | LlmError::InvalidPrompt => DeployError::InferenceFailed,
            LlmError::Utf8Error => DeployError::InvalidResult,
            LlmError::GpuUnavailable => DeployError::HardwareInsufficient,
            LlmError::ModelNotLoaded => DeployError::NotDeployed,
            LlmError::OutOfMemory => DeployError::HardwareInsufficient,
        }
    }
}

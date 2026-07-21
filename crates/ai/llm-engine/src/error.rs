//! LLM 推理引擎错误类型（D7）.

use core::fmt;

/// LLM 推理错误.
///
/// 覆盖模型加载、推理、UTF-8 解码、GPU 可用性、内存等失败场景。
#[derive(Debug, Clone)]
pub enum LlmError {
    /// 模型加载失败（文件不存在 / 格式错误 / 版本不兼容）.
    LoadFailed,
    /// 推理失败（C 库内部错误 / 上下文无效）.
    InferFailed,
    /// 路径非法（空字符串 / 包含非 UTF-8 字节）.
    InvalidPath,
    /// 提示词非法（含内部 NUL 字节，无法转 C 字符串）.
    InvalidPrompt,
    /// UTF-8 解码失败（C 库返回非 UTF-8 字符串）.
    Utf8Error,
    /// GPU 不可用（请求 Cuda/Metal/Npu 但运行时无可用设备）.
    GpuUnavailable,
    /// 模型未加载（推理 / 流式推理前未调用 `load_model`）.
    ModelNotLoaded,
    /// 内存不足（无法分配推理上下文 / KV cache）.
    OutOfMemory,
}

impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LlmError::LoadFailed => f.write_str("model load failed"),
            LlmError::InferFailed => f.write_str("inference failed"),
            LlmError::InvalidPath => f.write_str("invalid model path"),
            LlmError::InvalidPrompt => f.write_str("invalid prompt"),
            LlmError::Utf8Error => f.write_str("utf-8 decode error"),
            LlmError::GpuUnavailable => f.write_str("gpu unavailable"),
            LlmError::ModelNotLoaded => f.write_str("model not loaded"),
            LlmError::OutOfMemory => f.write_str("out of memory"),
        }
    }
}

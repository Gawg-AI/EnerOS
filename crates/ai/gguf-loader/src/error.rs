//! GGUF 解析错误类型（D7）.
//!
//! `GgufError` 覆盖 GGUF 文件解析、后端加载、GPU 操作、生命周期管理错误。
//! 派生 `Debug` / `Clone`，实现 `core::fmt::Display`。

use alloc::string::String;

/// GGUF 解析/加载错误.
#[derive(Debug, Clone)]
pub enum GgufError {
    /// 魔数不匹配（非 0x46554747 "GGUF"）.
    InvalidMagic,
    /// 不支持的 GGUF 版本（包含实际版本号）.
    InvalidVersion(u32),
    /// 文件被截断（读取超出缓冲区范围）.
    TruncatedFile,
    /// 无效的值类型（包含原始 u32）.
    InvalidValueType(u32),
    /// 无效的张量数据类型（包含原始 u32）.
    InvalidDtype(u32),
    /// 后端错误（无法映射/读取数据）.
    BackendError,
    /// GPU 不可用（分配失败）.
    GpuUnavailable,
    /// 已有模型加载（同一 Loader 同时只允许一个模型）.
    AlreadyLoaded,
    /// 无已加载模型（卸载/访问时）.
    NotLoaded,
    /// UTF-8 解码错误.
    Utf8Error,
}

impl core::fmt::Display for GgufError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidMagic => write!(f, "invalid GGUF magic number"),
            Self::InvalidVersion(v) => write!(f, "unsupported GGUF version: {}", v),
            Self::TruncatedFile => write!(f, "truncated GGUF file"),
            Self::InvalidValueType(v) => write!(f, "invalid value type: {}", v),
            Self::InvalidDtype(v) => write!(f, "invalid tensor dtype: {}", v),
            Self::BackendError => write!(f, "backend error"),
            Self::GpuUnavailable => write!(f, "GPU unavailable"),
            Self::AlreadyLoaded => write!(f, "a model is already loaded"),
            Self::NotLoaded => write!(f, "no model loaded"),
            Self::Utf8Error => write!(f, "UTF-8 decode error"),
        }
    }
}

impl From<core::str::Utf8Error> for GgufError {
    fn from(_: core::str::Utf8Error) -> Self {
        GgufError::Utf8Error
    }
}

// 保留对 alloc::string::String 的引用以满足未来扩展（如错误携带上下文字符串）。
#[allow(dead_code)]
type _UnusedString = String;

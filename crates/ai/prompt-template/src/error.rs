//! Prompt 模板错误类型（D2/D3）.
//!
//! `TemplateError` 包装 JSON 提取/解析失败、Schema 校验失败、重试耗尽、引擎错误 5 类失败。
//! `Engine` 变体内嵌 v0.59.0 `LlmError`，通过 `From<LlmError>` 自动转换（D3）。

use alloc::string::String;
use core::fmt;
use core::mem::discriminant;

use eneros_llm_engine::error::LlmError;

/// Prompt 模板错误.
///
/// 覆盖未找到 JSON、JSON 解析失败、Schema 校验失败、重试次数耗尽、引擎错误 5 类失败场景。
#[derive(Debug, Clone)]
pub enum TemplateError {
    /// 未找到 JSON（输出中无 `{...}` 块或 markdown 代码块为空）.
    NoJson,
    /// JSON 解析失败（`serde_json::from_str` 错误）.
    ParseError,
    /// Schema 校验失败（含字段名等上下文信息）.
    SchemaValidation(String),
    /// 重试次数耗尽（所有尝试均未通过 Schema 校验）.
    MaxRetriesExceeded,
    /// 推理引擎错误（包装 v0.59.0 `LlmError`，D2/D3）.
    Engine(LlmError),
}

// 手动实现 PartialEq：LlmError 未派生 PartialEq，使用 `core::mem::discriminant` 比较（D2）.
impl PartialEq for TemplateError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::NoJson, Self::NoJson) => true,
            (Self::ParseError, Self::ParseError) => true,
            (Self::SchemaValidation(a), Self::SchemaValidation(b)) => a == b,
            (Self::MaxRetriesExceeded, Self::MaxRetriesExceeded) => true,
            (Self::Engine(a), Self::Engine(b)) => discriminant(a) == discriminant(b),
            _ => false,
        }
    }
}

impl fmt::Display for TemplateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TemplateError::NoJson => f.write_str("no json found in output"),
            TemplateError::ParseError => f.write_str("json parse error"),
            TemplateError::SchemaValidation(s) => write!(f, "schema validation failed: {}", s),
            TemplateError::MaxRetriesExceeded => f.write_str("max retries exceeded"),
            TemplateError::Engine(e) => write!(f, "engine error: {}", e),
        }
    }
}

impl From<LlmError> for TemplateError {
    fn from(e: LlmError) -> Self {
        Self::Engine(e)
    }
}

//! JSON 提取（从 LLM 输出中提取 JSON 字符串）.

use alloc::string::String;

use crate::error::TemplateError;

/// 从 LLM 输出中提取 JSON 字符串.
///
/// 处理两种情形：
/// 1. markdown 代码块（` ```json ... ``` `）— 提取首行换行后到最后 ` ``` ` 之间的内容.
/// 2. 纯文本 — 提取第一个 `{` 到最后一个 `}` 之间的内容（含两端）.
///
/// 返回 `Err(TemplateError::NoJson)` 表示未找到 JSON。
pub fn extract_json(output: &str) -> Result<String, TemplateError> {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return Err(TemplateError::NoJson);
    }
    if trimmed.starts_with("```") {
        let start = trimmed.find('\n').ok_or(TemplateError::NoJson)?;
        let end = trimmed.rfind("```").ok_or(TemplateError::NoJson)?;
        if end <= start {
            return Err(TemplateError::NoJson);
        }
        let content = trimmed[start + 1..end].trim();
        if content.is_empty() {
            return Err(TemplateError::NoJson);
        }
        return Ok(String::from(content));
    }
    let start = trimmed.find('{').ok_or(TemplateError::NoJson)?;
    let end = trimmed.rfind('}').ok_or(TemplateError::NoJson)?;
    if end < start {
        return Err(TemplateError::NoJson);
    }
    Ok(String::from(&trimmed[start..=end]))
}

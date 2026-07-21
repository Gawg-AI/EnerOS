//! Prompt 模板 trait（D1：无 Send + Sync bound）.

use crate::context::TemplateContext;
use crate::error::TemplateError;
use crate::extract::extract_json;
use crate::schema::SchemaSpec;

/// Prompt 模板统一接口.
///
/// 不要求 `Send + Sync`（D1：与 v0.59.0 `LlmEngine` 一致；单线程 no_std 无需）。
/// 提供 `validate` 默认实现：`extract_json` → `serde_json::from_str` → `schema.validate`。
pub trait PromptTemplate {
    /// 模板名称.
    fn name(&self) -> &'static str;

    /// 构建 prompt 文本.
    fn build(&self, context: &TemplateContext) -> alloc::string::String;

    /// 输出 JSON Schema（`&'static` 静态常量，D4）.
    fn output_schema(&self) -> &'static SchemaSpec;

    /// 验证 LLM 输出.
    ///
    /// 默认实现：提取 JSON → 解析为 `serde_json::Value` → Schema 校验。
    /// 返回解析后的 `Value`。
    fn validate(&self, output: &str) -> Result<serde_json::Value, TemplateError> {
        let json_str = extract_json(output)?;
        let value: serde_json::Value =
            serde_json::from_str(&json_str).map_err(|_| TemplateError::ParseError)?;
        self.output_schema().validate(&value)?;
        Ok(value)
    }
}

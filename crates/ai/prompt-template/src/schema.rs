//! 最小 JSON Schema 验证器（D5/D9）.
//!
//! 仅支持 required / type / enum / minimum / maximum 五项校验，
//! 满足电力场景 Prompt 输出约束需求（D5：完整 JSON Schema 为过度工程）。

use alloc::format;
use alloc::string::String;

use crate::error::TemplateError;

/// JSON Schema 字段类型.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaType {
    /// 字符串.
    String,
    /// 数字（整数或浮点）.
    Number,
    /// 布尔.
    Boolean,
    /// 对象.
    Object,
    /// 数组.
    Array,
}

/// Schema 字段定义（编译期静态常量，D4）.
///
/// 注意：因含 `Option<f64>`（minimum/maximum），无法派生 `Eq`（f64 无 `Eq`）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SchemaField {
    /// 字段名.
    pub name: &'static str,
    /// 字段类型.
    pub field_type: SchemaType,
    /// 是否必填.
    pub required: bool,
    /// 枚举允许值（空表示无枚举约束）.
    pub enum_values: &'static [&'static str],
    /// 数值下界（含）.
    pub minimum: Option<f64>,
    /// 数值上界（含）.
    pub maximum: Option<f64>,
}

/// Schema 验证规范（`&'static` 静态字段集合，D4/D9）.
#[derive(Debug, Clone, Copy)]
pub struct SchemaSpec {
    /// 字段集合.
    pub fields: &'static [SchemaField],
}

impl SchemaSpec {
    /// 构造 SchemaSpec（const fn，编译期可用）.
    pub const fn new(fields: &'static [SchemaField]) -> Self {
        Self { fields }
    }

    /// 校验 JSON 值.
    ///
    /// 1. `value` 必须是 Object.
    /// 2. 遍历 `fields`：required 字段必须存在；存在字段校验类型/枚举/范围.
    pub fn validate(&self, value: &serde_json::Value) -> Result<(), TemplateError> {
        let obj = value.as_object().ok_or_else(|| {
            TemplateError::SchemaValidation(String::from("expected json object at root"))
        })?;
        for field in self.fields {
            if field.required && !obj.contains_key(field.name) {
                return Err(TemplateError::SchemaValidation(format!(
                    "missing required field: {}",
                    field.name
                )));
            }
            if let Some(v) = obj.get(field.name) {
                self.validate_field(field, v)?;
            }
        }
        Ok(())
    }

    /// 校验单个字段（类型/枚举/范围）.
    fn validate_field(
        &self,
        field: &SchemaField,
        value: &serde_json::Value,
    ) -> Result<(), TemplateError> {
        let type_ok = match field.field_type {
            SchemaType::String => value.is_string(),
            SchemaType::Number => value.is_number(),
            SchemaType::Boolean => value.is_boolean(),
            SchemaType::Object => value.is_object(),
            SchemaType::Array => value.is_array(),
        };
        if !type_ok {
            return Err(TemplateError::SchemaValidation(format!(
                "field {} type mismatch",
                field.name
            )));
        }
        if !field.enum_values.is_empty() {
            if let Some(s) = value.as_str() {
                // clippy 建议 `contains(&s)`，但 `enum_values: &[&'static str]`
                // 要求参数类型 `&&'static str`，而 `s: &str`（源自 `value.as_str()`，
                // 非 'static）无法满足该生命周期约束，故保留 `iter().any()`。
                #[allow(clippy::manual_contains)]
                if !field.enum_values.iter().any(|&ev| ev == s) {
                    return Err(TemplateError::SchemaValidation(format!(
                        "field {} enum value {} not allowed",
                        field.name, s
                    )));
                }
            }
        }
        if let Some(n) = value.as_f64() {
            if let Some(min) = field.minimum {
                if n < min {
                    return Err(TemplateError::SchemaValidation(format!(
                        "field {} value {} < minimum {}",
                        field.name, n, min
                    )));
                }
            }
            if let Some(max) = field.maximum {
                if n > max {
                    return Err(TemplateError::SchemaValidation(format!(
                        "field {} value {} > maximum {}",
                        field.name, n, max
                    )));
                }
            }
        }
        Ok(())
    }
}

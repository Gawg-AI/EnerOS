//! 严格输出 schema（[11] §3.5 第 2/3 步）：在 prompt-template SchemaSpec
//! 的 required/type/enum/range 之上增加**未知字段拒绝**与 non-finite
//! 守卫——模型输出不能被偷偷塞进额外字段（tool 注入/JSON smuggling 面）。

use alloc::format;
use alloc::string::String;

/// 字段类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaType {
    String,
    Number,
    Boolean,
    Object,
    Array,
}

/// schema 字段（编译期静态常量）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SchemaField {
    pub name: &'static str,
    pub ty: SchemaType,
    pub required: bool,
    /// 枚举允许值（空 = 无枚举约束）。
    pub enum_values: &'static [&'static str],
    pub minimum: Option<f64>,
    pub maximum: Option<f64>,
}

/// 严格 JSON schema。
#[derive(Debug, Clone, Copy)]
pub struct StrictSchema {
    pub fields: &'static [SchemaField],
}

impl StrictSchema {
    pub const fn new(fields: &'static [SchemaField]) -> Self {
        Self { fields }
    }

    /// 校验（fail-closed）：根必须 object；required 必须存在；存在字段
    /// 类型/枚举/范围校验；**未知字段拒绝**；一切数值 finite。
    pub fn validate(&self, value: &serde_json::Value) -> Result<(), SchemaError> {
        let obj = value.as_object().ok_or(SchemaError::NotObject)?;
        let mut seen = 0usize;
        for field in self.fields {
            if field.required && !obj.contains_key(field.name) {
                return Err(SchemaError::MissingField(field.name));
            }
            if let Some(v) = obj.get(field.name) {
                self.validate_field(field, v)?;
                seen += 1;
            }
        }
        for key in obj.keys() {
            if !self.fields.iter().any(|f| f.name == key) {
                return Err(SchemaError::UnknownField(key.clone()));
            }
        }
        let _ = seen;
        Ok(())
    }

    fn validate_field(
        &self,
        field: &SchemaField,
        v: &serde_json::Value,
    ) -> Result<(), SchemaError> {
        let type_ok = match field.ty {
            SchemaType::String => v.is_string(),
            SchemaType::Number => v.is_number(),
            SchemaType::Boolean => v.is_boolean(),
            SchemaType::Object => v.is_object(),
            SchemaType::Array => v.is_array(),
        };
        if !type_ok {
            return Err(SchemaError::WrongType {
                field: field.name,
                expected: format!("{:?}", field.ty),
            });
        }
        if let SchemaType::Number = field.ty {
            let n = v.as_f64().ok_or(SchemaError::NonFinite(field.name))?;
            if !n.is_finite() {
                return Err(SchemaError::NonFinite(field.name));
            }
            if let Some(lo) = field.minimum {
                if n < lo {
                    return Err(SchemaError::RangeViolation(field.name));
                }
            }
            if let Some(hi) = field.maximum {
                if n > hi {
                    return Err(SchemaError::RangeViolation(field.name));
                }
            }
        }
        if let (SchemaType::String, Some(v)) = (field.ty, v.as_str()) {
            if !field.enum_values.is_empty() && !field.enum_values.contains(&v) {
                return Err(SchemaError::EnumViolation(field.name));
            }
        }
        Ok(())
    }
}

/// schema 校验错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaError {
    NotObject,
    MissingField(&'static str),
    UnknownField(String),
    WrongType {
        field: &'static str,
        expected: String,
    },
    EnumViolation(&'static str),
    RangeViolation(&'static str),
    NonFinite(&'static str),
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]
    use serde_json::json;

    use super::*;

    static FIELDS: &[SchemaField] = &[
        SchemaField {
            name: "kind",
            ty: SchemaType::String,
            required: true,
            enum_values: &["PeakShave", "MaxRevenue"],
            minimum: None,
            maximum: None,
        },
        SchemaField {
            name: "confidence_bp",
            ty: SchemaType::Number,
            required: true,
            enum_values: &[],
            minimum: Some(0.0),
            maximum: Some(10_000.0),
        },
        SchemaField {
            name: "note",
            ty: SchemaType::String,
            required: false,
            enum_values: &[],
            minimum: None,
            maximum: None,
        },
    ];

    static SCHEMA: StrictSchema = StrictSchema::new(FIELDS);

    #[test]
    fn valid_output_passes() {
        let v = json!({"kind": "PeakShave", "confidence_bp": 8000.0, "note": "ok"});
        assert!(SCHEMA.validate(&v).is_ok());
        // 可选字段可缺。
        let v = json!({"kind": "MaxRevenue", "confidence_bp": 100.0});
        assert!(SCHEMA.validate(&v).is_ok());
    }

    #[test]
    fn unknown_field_rejected() {
        // JSON smuggling：偷偷加字段 → 拒绝。
        let v = json!({"kind": "PeakShave", "confidence_bp": 1.0, "dispatch": {"device": 7}});
        assert!(matches!(
            SCHEMA.validate(&v),
            Err(SchemaError::UnknownField(_))
        ));
        let v = json!({"kind": "PeakShave", "confidence_bp": 1.0, "x": 1});
        assert!(matches!(
            SCHEMA.validate(&v),
            Err(SchemaError::UnknownField(f)) if f == "x"
        ));
    }

    #[test]
    fn enum_and_range_rejected() {
        let v = json!({"kind": "FlyToDevice", "confidence_bp": 1.0});
        assert!(matches!(
            SCHEMA.validate(&v),
            Err(SchemaError::EnumViolation("kind"))
        ));
        let v = json!({"kind": "PeakShave", "confidence_bp": 20_000.0});
        assert!(matches!(
            SCHEMA.validate(&v),
            Err(SchemaError::RangeViolation("confidence_bp"))
        ));
    }

    #[test]
    fn non_finite_rejected() {
        // 1e999 超出 f64 表示：serde_json 拒绝（解析错）或解析成 inf
        // （门 Finite/Range 拦截）——任一路径都必须拒绝。
        let result = serde_json::from_str::<serde_json::Value>(
            r#"{"kind":"PeakShave","confidence_bp":1e999}"#,
        );
        match result {
            Err(_) => {}
            Ok(v) => match SCHEMA.validate(&v) {
                Err(SchemaError::NonFinite(_)) | Err(SchemaError::RangeViolation(_)) => {}
                other => panic!("expected finite guard, got {other:?}"),
            },
        }
    }

    #[test]
    fn missing_required_rejected() {
        let v = json!({"confidence_bp": 1.0});
        assert_eq!(SCHEMA.validate(&v), Err(SchemaError::MissingField("kind")));
        assert_eq!(
            SCHEMA.validate(&json!({"kind": "PeakShave"})),
            Err(SchemaError::MissingField("confidence_bp"))
        );
    }

    #[test]
    fn wrong_type_rejected() {
        let v = json!({"kind": "PeakShave", "confidence_bp": "high"});
        assert!(matches!(
            SCHEMA.validate(&v),
            Err(SchemaError::WrongType {
                field: "confidence_bp",
                ..
            })
        ));
    }
}

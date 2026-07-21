//! Configuration value types and schema definitions.
//!
//! [`ConfigValue`] is the runtime representation of a configuration value,
//! supporting Bool/Int/Float/String/Array/Table. [`ConfigSchema`] defines
//! the expected structure for validation.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::error::ConfigError;

/// Supported configuration value types (for schema declaration).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigType {
    Bool,
    Int,
    Float,
    String,
    Array,
    Table,
}

impl ConfigType {
    /// Returns the string name of this config type.
    pub fn as_str(self) -> &'static str {
        match self {
            ConfigType::Bool => "bool",
            ConfigType::Int => "int",
            ConfigType::Float => "float",
            ConfigType::String => "string",
            ConfigType::Array => "array",
            ConfigType::Table => "table",
        }
    }
}

impl core::fmt::Display for ConfigType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A runtime configuration value.
///
/// `Table` uses [`BTreeMap`] for no_std compatibility and deterministic
/// iteration order.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Array(Vec<ConfigValue>),
    Table(BTreeMap<String, ConfigValue>),
}

impl ConfigValue {
    /// Returns `Some(b)` if the value is `Bool(b)`, otherwise `None`.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            ConfigValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Returns `Some(n)` if the value is `Int(n)`, otherwise `None`.
    pub fn as_int(&self) -> Option<i64> {
        match self {
            ConfigValue::Int(n) => Some(*n),
            _ => None,
        }
    }

    /// Returns `Some(f)` if the value is `Float(f)`, otherwise `None`.
    pub fn as_float(&self) -> Option<f64> {
        match self {
            ConfigValue::Float(f) => Some(*f),
            _ => None,
        }
    }

    /// Returns `Some(s)` if the value is `String(s)`, otherwise `None`.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            ConfigValue::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Returns `Some(arr)` if the value is `Array(arr)`, otherwise `None`.
    pub fn as_array(&self) -> Option<&Vec<ConfigValue>> {
        match self {
            ConfigValue::Array(arr) => Some(arr),
            _ => None,
        }
    }

    /// Returns `Some(table)` if the value is `Table(table)`, otherwise `None`.
    pub fn as_table(&self) -> Option<&BTreeMap<String, ConfigValue>> {
        match self {
            ConfigValue::Table(t) => Some(t),
            _ => None,
        }
    }

    /// Returns the [`ConfigType`] corresponding to this value.
    pub fn config_type(&self) -> ConfigType {
        match self {
            ConfigValue::Bool(_) => ConfigType::Bool,
            ConfigValue::Int(_) => ConfigType::Int,
            ConfigValue::Float(_) => ConfigType::Float,
            ConfigValue::String(_) => ConfigType::String,
            ConfigValue::Array(_) => ConfigType::Array,
            ConfigValue::Table(_) => ConfigType::Table,
        }
    }
}

impl From<bool> for ConfigValue {
    fn from(b: bool) -> Self {
        ConfigValue::Bool(b)
    }
}

impl From<i64> for ConfigValue {
    fn from(i: i64) -> Self {
        ConfigValue::Int(i)
    }
}

impl From<f64> for ConfigValue {
    fn from(f: f64) -> Self {
        ConfigValue::Float(f)
    }
}

impl From<String> for ConfigValue {
    fn from(s: String) -> Self {
        ConfigValue::String(s)
    }
}

impl From<&str> for ConfigValue {
    fn from(s: &str) -> Self {
        ConfigValue::String(String::from(s))
    }
}

/// A single field declaration in a configuration schema.
#[derive(Debug, Clone)]
pub struct ConfigField {
    /// Dotted path, e.g. `"device.port"`.
    pub path: String,
    /// Expected value type.
    pub config_type: ConfigType,
    /// Whether the field must be present.
    pub required: bool,
    /// Default value used when the field is absent.
    pub default: Option<ConfigValue>,
}

impl ConfigField {
    /// Creates a new required field with the given path and type.
    pub fn new(path: &str, config_type: ConfigType) -> Self {
        Self {
            path: String::from(path),
            config_type,
            required: true,
            default: None,
        }
    }

    /// Marks this field as optional (not required).
    pub fn optional(mut self) -> Self {
        self.required = false;
        self
    }

    /// Sets the default value for this optional field.
    pub fn with_default(mut self, value: ConfigValue) -> Self {
        self.default = Some(value);
        self.required = false;
        self
    }
}

/// A configuration schema defining the expected structure.
///
/// Used by [`ConfigManager`](crate::ConfigManager) to validate loaded
/// configurations before applying them.
#[derive(Debug, Clone)]
pub struct ConfigSchema {
    /// Schema name (matches the config file name without extension).
    pub name: String,
    /// Declared fields.
    pub fields: Vec<ConfigField>,
}

impl ConfigSchema {
    /// Creates a new empty schema with the given name.
    pub fn new(name: String) -> Self {
        Self {
            name,
            fields: Vec::new(),
        }
    }

    /// Adds a field declaration to the schema.
    pub fn add_field(&mut self, field: ConfigField) {
        self.fields.push(field);
    }

    /// Validates a config value against this schema.
    ///
    /// The value must be a `Table`. Each declared field is checked:
    /// - If `required` and missing → `SchemaViolation`.
    /// - If present and the type doesn't match → `SchemaViolation`.
    /// - Optional fields with a default that are absent are OK (no error).
    pub fn validate(&self, value: &ConfigValue) -> Result<(), ConfigError> {
        let table = value
            .as_table()
            .ok_or_else(|| ConfigError::SchemaViolation {
                field: self.name.clone(),
                reason: String::from("root value must be a Table"),
            })?;

        for field in &self.fields {
            let found = get_nested(table, &field.path);
            match (found, field.required, &field.default) {
                (Some(v), _, _) => {
                    // Type check.
                    if v.config_type() != field.config_type {
                        return Err(ConfigError::SchemaViolation {
                            field: field.path.clone(),
                            reason: format!(
                                "expected type {}, got {}",
                                field.config_type,
                                v.config_type()
                            ),
                        });
                    }
                }
                (None, true, _) => {
                    return Err(ConfigError::SchemaViolation {
                        field: field.path.clone(),
                        reason: String::from("required field is missing"),
                    });
                }
                (None, false, _) => {
                    // Optional or has default — OK.
                }
            }
        }
        Ok(())
    }
}

/// Navigates a dotted path through nested tables.
///
/// Returns `None` if any intermediate key is missing or not a table.
fn get_nested<'a>(
    table: &'a BTreeMap<String, ConfigValue>,
    path: &'a str,
) -> Option<&'a ConfigValue> {
    let (first, rest) = match path.find('.') {
        Some(idx) => (&path[..idx], Some(&path[idx + 1..])),
        None => (path, None),
    };
    let value = table.get(first)?;
    match rest {
        None => Some(value),
        Some(rest_path) => {
            let inner = value.as_table()?;
            get_nested(inner, rest_path)
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    // ---- ConfigType ----

    #[test]
    fn test_config_type_as_str() {
        assert_eq!(ConfigType::Bool.as_str(), "bool");
        assert_eq!(ConfigType::Int.as_str(), "int");
        assert_eq!(ConfigType::Float.as_str(), "float");
        assert_eq!(ConfigType::String.as_str(), "string");
        assert_eq!(ConfigType::Array.as_str(), "array");
        assert_eq!(ConfigType::Table.as_str(), "table");
    }

    #[test]
    fn test_config_type_display() {
        assert_eq!(format!("{}", ConfigType::Bool), "bool");
        assert_eq!(format!("{}", ConfigType::Int), "int");
        assert_eq!(format!("{}", ConfigType::Float), "float");
        assert_eq!(format!("{}", ConfigType::String), "string");
        assert_eq!(format!("{}", ConfigType::Array), "array");
        assert_eq!(format!("{}", ConfigType::Table), "table");
    }

    #[test]
    fn test_config_type_eq() {
        assert_eq!(ConfigType::Bool, ConfigType::Bool);
        assert_ne!(ConfigType::Bool, ConfigType::Int);
        assert_ne!(ConfigType::Array, ConfigType::Table);
    }

    // ---- ConfigValue constructors and accessors ----

    #[test]
    fn test_config_value_bool() {
        let v = ConfigValue::Bool(true);
        assert_eq!(v.as_bool(), Some(true));
        assert_eq!(v.config_type(), ConfigType::Bool);
        assert!(v.as_int().is_none());
        assert!(v.as_float().is_none());
        assert!(v.as_str().is_none());
        assert!(v.as_array().is_none());
        assert!(v.as_table().is_none());
    }

    #[test]
    fn test_config_value_int() {
        let v = ConfigValue::Int(-42);
        assert_eq!(v.as_int(), Some(-42));
        assert_eq!(v.config_type(), ConfigType::Int);
        assert!(v.as_bool().is_none());
    }

    #[test]
    fn test_config_value_float() {
        let v = ConfigValue::Float(2.5);
        assert_eq!(v.as_float(), Some(2.5));
        assert_eq!(v.config_type(), ConfigType::Float);
        assert!(v.as_int().is_none());
    }

    #[test]
    fn test_config_value_string() {
        let v = ConfigValue::String(String::from("hello"));
        assert_eq!(v.as_str(), Some("hello"));
        assert_eq!(v.config_type(), ConfigType::String);
        assert!(v.as_int().is_none());
    }

    #[test]
    fn test_config_value_array() {
        let v = ConfigValue::Array(vec![
            ConfigValue::Int(1),
            ConfigValue::Int(2),
            ConfigValue::Int(3),
        ]);
        let arr = v.as_array().expect("should be array");
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0], ConfigValue::Int(1));
        assert_eq!(v.config_type(), ConfigType::Array);
    }

    #[test]
    fn test_config_value_table() {
        let mut t = BTreeMap::new();
        t.insert(String::from("port"), ConfigValue::Int(8080));
        let v = ConfigValue::Table(t);
        let table = v.as_table().expect("should be table");
        assert_eq!(table.get("port"), Some(&ConfigValue::Int(8080)));
        assert_eq!(v.config_type(), ConfigType::Table);
    }

    #[test]
    fn test_config_value_empty_array() {
        let v = ConfigValue::Array(Vec::new());
        assert_eq!(v.as_array().map(|a| a.len()), Some(0));
        assert_eq!(v.config_type(), ConfigType::Array);
    }

    #[test]
    fn test_config_value_empty_table() {
        let v = ConfigValue::Table(BTreeMap::new());
        assert_eq!(v.as_table().map(|t| t.len()), Some(0));
        assert_eq!(v.config_type(), ConfigType::Table);
    }

    // ---- From impls ----

    #[test]
    fn test_from_bool() {
        let v: ConfigValue = true.into();
        assert_eq!(v, ConfigValue::Bool(true));
    }

    #[test]
    fn test_from_i64() {
        let v: ConfigValue = 42i64.into();
        assert_eq!(v, ConfigValue::Int(42));
    }

    #[test]
    fn test_from_f64() {
        let v: ConfigValue = 2.5f64.into();
        assert_eq!(v, ConfigValue::Float(2.5));
    }

    #[test]
    fn test_from_string() {
        let v: ConfigValue = String::from("hello").into();
        assert_eq!(v, ConfigValue::String(String::from("hello")));
    }

    #[test]
    fn test_from_str() {
        let v: ConfigValue = "hello".into();
        assert_eq!(v, ConfigValue::String(String::from("hello")));
    }

    // ---- ConfigField ----

    #[test]
    fn test_config_field_new_required() {
        let f = ConfigField::new("port", ConfigType::Int);
        assert_eq!(f.path, "port");
        assert_eq!(f.config_type, ConfigType::Int);
        assert!(f.required);
        assert!(f.default.is_none());
    }

    #[test]
    fn test_config_field_optional() {
        let f = ConfigField::new("port", ConfigType::Int).optional();
        assert!(!f.required);
        assert!(f.default.is_none());
    }

    #[test]
    fn test_config_field_with_default() {
        let f = ConfigField::new("port", ConfigType::Int).with_default(ConfigValue::Int(8080));
        assert!(!f.required);
        assert_eq!(f.default, Some(ConfigValue::Int(8080)));
    }

    #[test]
    fn test_config_field_nested_path() {
        let f = ConfigField::new("device.network.port", ConfigType::Int);
        assert_eq!(f.path, "device.network.port");
    }

    // ---- ConfigSchema ----

    #[test]
    fn test_schema_new() {
        let s = ConfigSchema::new(String::from("device"));
        assert_eq!(s.name, "device");
        assert!(s.fields.is_empty());
    }

    #[test]
    fn test_schema_add_field() {
        let mut s = ConfigSchema::new(String::from("device"));
        s.add_field(ConfigField::new("port", ConfigType::Int));
        s.add_field(ConfigField::new("host", ConfigType::String));
        assert_eq!(s.fields.len(), 2);
        assert_eq!(s.fields[0].path, "port");
        assert_eq!(s.fields[1].path, "host");
    }

    #[test]
    fn test_schema_clone() {
        let mut s = ConfigSchema::new(String::from("device"));
        s.add_field(ConfigField::new("port", ConfigType::Int));
        let s2 = s.clone();
        assert_eq!(s2.name, s.name);
        assert_eq!(s2.fields.len(), s.fields.len());
    }

    // ---- ConfigSchema::validate ----

    #[test]
    fn test_validate_empty_schema_accepts_any_table() {
        let schema = ConfigSchema::new(String::from("empty"));
        let mut t = BTreeMap::new();
        t.insert(String::from("x"), ConfigValue::Int(1));
        let value = ConfigValue::Table(t);
        assert!(schema.validate(&value).is_ok());
    }

    #[test]
    fn test_validate_empty_schema_accepts_empty_table() {
        let schema = ConfigSchema::new(String::from("empty"));
        let value = ConfigValue::Table(BTreeMap::new());
        assert!(schema.validate(&value).is_ok());
    }

    #[test]
    fn test_validate_rejects_non_table_root() {
        let schema = ConfigSchema::new(String::from("device"));
        let value = ConfigValue::Int(42);
        let result = schema.validate(&value);
        assert!(matches!(result, Err(ConfigError::SchemaViolation { .. })));
    }

    #[test]
    fn test_validate_required_field_present() {
        let mut schema = ConfigSchema::new(String::from("device"));
        schema.add_field(ConfigField::new("port", ConfigType::Int));

        let mut t = BTreeMap::new();
        t.insert(String::from("port"), ConfigValue::Int(8080));
        let value = ConfigValue::Table(t);

        assert!(schema.validate(&value).is_ok());
    }

    #[test]
    fn test_validate_required_field_missing() {
        let mut schema = ConfigSchema::new(String::from("device"));
        schema.add_field(ConfigField::new("port", ConfigType::Int));

        let value = ConfigValue::Table(BTreeMap::new());
        let result = schema.validate(&value);
        assert!(
            matches!(result, Err(ConfigError::SchemaViolation { field, .. }) if field == "port")
        );
    }

    #[test]
    fn test_validate_optional_field_missing_ok() {
        let mut schema = ConfigSchema::new(String::from("device"));
        schema.add_field(ConfigField::new("port", ConfigType::Int).optional());

        let value = ConfigValue::Table(BTreeMap::new());
        assert!(schema.validate(&value).is_ok());
    }

    #[test]
    fn test_validate_field_with_default_missing_ok() {
        let mut schema = ConfigSchema::new(String::from("device"));
        schema.add_field(
            ConfigField::new("port", ConfigType::Int).with_default(ConfigValue::Int(8080)),
        );

        let value = ConfigValue::Table(BTreeMap::new());
        assert!(schema.validate(&value).is_ok());
    }

    #[test]
    fn test_validate_type_mismatch() {
        let mut schema = ConfigSchema::new(String::from("device"));
        schema.add_field(ConfigField::new("port", ConfigType::Int));

        let mut t = BTreeMap::new();
        t.insert(String::from("port"), ConfigValue::String(String::from("x")));
        let value = ConfigValue::Table(t);

        let result = schema.validate(&value);
        assert!(
            matches!(result, Err(ConfigError::SchemaViolation { field, .. }) if field == "port")
        );
    }

    #[test]
    fn test_validate_multiple_fields_all_ok() {
        let mut schema = ConfigSchema::new(String::from("device"));
        schema.add_field(ConfigField::new("port", ConfigType::Int));
        schema.add_field(ConfigField::new("host", ConfigType::String));
        schema.add_field(ConfigField::new("enabled", ConfigType::Bool));

        let mut t = BTreeMap::new();
        t.insert(String::from("port"), ConfigValue::Int(8080));
        t.insert(
            String::from("host"),
            ConfigValue::String(String::from("0.0.0.0")),
        );
        t.insert(String::from("enabled"), ConfigValue::Bool(true));
        let value = ConfigValue::Table(t);

        assert!(schema.validate(&value).is_ok());
    }

    #[test]
    fn test_validate_mixed_required_optional() {
        let mut schema = ConfigSchema::new(String::from("device"));
        schema.add_field(ConfigField::new("port", ConfigType::Int));
        schema.add_field(ConfigField::new("timeout", ConfigType::Int).optional());
        schema.add_field(
            ConfigField::new("retries", ConfigType::Int).with_default(ConfigValue::Int(3)),
        );

        // Only the required field is present.
        let mut t = BTreeMap::new();
        t.insert(String::from("port"), ConfigValue::Int(8080));
        let value = ConfigValue::Table(t);

        assert!(schema.validate(&value).is_ok());
    }

    #[test]
    fn test_validate_nested_path_present() {
        let mut schema = ConfigSchema::new(String::from("device"));
        schema.add_field(ConfigField::new("network.port", ConfigType::Int));

        let mut inner = BTreeMap::new();
        inner.insert(String::from("port"), ConfigValue::Int(9090));
        let mut outer = BTreeMap::new();
        outer.insert(String::from("network"), ConfigValue::Table(inner));
        let value = ConfigValue::Table(outer);

        assert!(schema.validate(&value).is_ok());
    }

    #[test]
    fn test_validate_nested_path_missing() {
        let mut schema = ConfigSchema::new(String::from("device"));
        schema.add_field(ConfigField::new("network.port", ConfigType::Int));

        let value = ConfigValue::Table(BTreeMap::new());
        let result = schema.validate(&value);
        assert!(
            matches!(result, Err(ConfigError::SchemaViolation { field, .. }) if field == "network.port")
        );
    }

    #[test]
    fn test_validate_nested_path_intermediate_not_table() {
        let mut schema = ConfigSchema::new(String::from("device"));
        schema.add_field(ConfigField::new("network.port", ConfigType::Int));

        let mut t = BTreeMap::new();
        // "network" is an Int, not a Table — so "network.port" can't be navigated.
        t.insert(String::from("network"), ConfigValue::Int(42));
        let value = ConfigValue::Table(t);

        let result = schema.validate(&value);
        assert!(
            matches!(result, Err(ConfigError::SchemaViolation { field, .. }) if field == "network.port")
        );
    }

    #[test]
    fn test_validate_array_type_ok() {
        let mut schema = ConfigSchema::new(String::from("device"));
        schema.add_field(ConfigField::new("ports", ConfigType::Array));

        let mut t = BTreeMap::new();
        t.insert(
            String::from("ports"),
            ConfigValue::Array(vec![ConfigValue::Int(8080), ConfigValue::Int(9090)]),
        );
        let value = ConfigValue::Table(t);

        assert!(schema.validate(&value).is_ok());
    }

    #[test]
    fn test_validate_table_type_ok() {
        let mut schema = ConfigSchema::new(String::from("device"));
        schema.add_field(ConfigField::new("network", ConfigType::Table));

        let mut inner = BTreeMap::new();
        inner.insert(String::from("port"), ConfigValue::Int(8080));
        let mut outer = BTreeMap::new();
        outer.insert(String::from("network"), ConfigValue::Table(inner));
        let value = ConfigValue::Table(outer);

        assert!(schema.validate(&value).is_ok());
    }

    #[test]
    fn test_validate_float_type_ok() {
        let mut schema = ConfigSchema::new(String::from("device"));
        schema.add_field(ConfigField::new("pi", ConfigType::Float));

        let mut t = BTreeMap::new();
        t.insert(String::from("pi"), ConfigValue::Float(2.5));
        let value = ConfigValue::Table(t);

        assert!(schema.validate(&value).is_ok());
    }

    #[test]
    fn test_validate_bool_type_ok() {
        let mut schema = ConfigSchema::new(String::from("device"));
        schema.add_field(ConfigField::new("enabled", ConfigType::Bool));

        let mut t = BTreeMap::new();
        t.insert(String::from("enabled"), ConfigValue::Bool(true));
        let value = ConfigValue::Table(t);

        assert!(schema.validate(&value).is_ok());
    }

    #[test]
    fn test_validate_error_message_contains_field() {
        let mut schema = ConfigSchema::new(String::from("device"));
        schema.add_field(ConfigField::new("port", ConfigType::Int));

        let value = ConfigValue::Table(BTreeMap::new());
        let err = schema.validate(&value).unwrap_err();
        let msg = format!("{}", err);
        assert!(msg.contains("port"));
    }

    // ---- get_nested helper ----

    #[test]
    fn test_get_nested_simple() {
        let mut t = BTreeMap::new();
        t.insert(String::from("port"), ConfigValue::Int(8080));
        let result = get_nested(&t, "port");
        assert_eq!(result, Some(&ConfigValue::Int(8080)));
    }

    #[test]
    fn test_get_nested_missing() {
        let t = BTreeMap::new();
        let result = get_nested(&t, "port");
        assert!(result.is_none());
    }

    #[test]
    fn test_get_nested_nested() {
        let mut inner = BTreeMap::new();
        inner.insert(String::from("port"), ConfigValue::Int(9090));
        let mut outer = BTreeMap::new();
        outer.insert(String::from("network"), ConfigValue::Table(inner));
        let result = get_nested(&outer, "network.port");
        assert_eq!(result, Some(&ConfigValue::Int(9090)));
    }

    #[test]
    fn test_get_nested_deep() {
        let mut leaf = BTreeMap::new();
        leaf.insert(String::from("value"), ConfigValue::Int(42));
        let mut mid = BTreeMap::new();
        mid.insert(String::from("leaf"), ConfigValue::Table(leaf));
        let mut top = BTreeMap::new();
        top.insert(String::from("mid"), ConfigValue::Table(mid));
        let result = get_nested(&top, "mid.leaf.value");
        assert_eq!(result, Some(&ConfigValue::Int(42)));
    }

    #[test]
    fn test_get_nested_intermediate_not_table() {
        let mut t = BTreeMap::new();
        t.insert(String::from("network"), ConfigValue::Int(42));
        let result = get_nested(&t, "network.port");
        assert!(result.is_none());
    }
}

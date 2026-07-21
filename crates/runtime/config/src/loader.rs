//! Configuration loaders for TOML and JSON formats.
//!
//! The [`ConfigLoader`] trait abstracts over file formats so that
//! [`ConfigManager`](crate::ConfigManager) can load `.toml` and `.json` files
//! uniformly. [`TomlLoader`] and [`JsonLoader`] are the two built-in
//! implementations.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use eneros_fs::{FileSystem, Lfs, OpenFlags};

use crate::error::ConfigError;
use crate::schema::ConfigValue;

/// Supported configuration file formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFormat {
    Toml,
    Json,
}

impl ConfigFormat {
    /// Infers the format from a file extension.
    ///
    /// Returns `None` for unrecognized extensions.
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext {
            "toml" => Some(ConfigFormat::Toml),
            "json" => Some(ConfigFormat::Json),
            _ => None,
        }
    }

    /// Returns the file extension for this format.
    pub fn extension(self) -> &'static str {
        match self {
            ConfigFormat::Toml => "toml",
            ConfigFormat::Json => "json",
        }
    }
}

/// Trait for loading and saving configuration values in a specific format.
pub trait ConfigLoader {
    /// Parses raw bytes into a [`ConfigValue::Table`].
    fn parse(&self, data: &[u8]) -> Result<ConfigValue, ConfigError>;

    /// Serializes a [`ConfigValue`] into raw bytes.
    fn serialize(&self, value: &ConfigValue) -> Result<Vec<u8>, ConfigError>;

    /// Loads a configuration file from the filesystem.
    fn load_from_file(&self, fs: &mut Lfs, path: &str) -> Result<ConfigValue, ConfigError> {
        let mut file = fs.open(path, OpenFlags::READ)?;
        let stat = fs.stat(path)?;
        let mut buf = vec![0u8; stat.size as usize];
        file.read(fs, &mut buf)?;
        // Trim trailing NUL bytes that littlefs may append.
        let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        buf.truncate(len);
        self.parse(&buf)
    }

    /// Saves a configuration value to the filesystem.
    fn save_to_file(
        &self,
        fs: &mut Lfs,
        path: &str,
        value: &ConfigValue,
    ) -> Result<(), ConfigError> {
        let data = self.serialize(value)?;
        let mut file = fs.open(
            path,
            OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE,
        )?;
        file.write(fs, &data)?;
        Ok(())
    }
}

// ============================================================================
// toml::Value ↔ ConfigValue conversion
// ============================================================================

/// Converts a `toml::Value` into a [`ConfigValue`].
///
/// `Datetime` values are serialized to their string representation since
/// [`ConfigValue`] has no datetime variant.
fn toml_to_config(value: toml::Value) -> ConfigValue {
    match value {
        toml::Value::Boolean(b) => ConfigValue::Bool(b),
        toml::Value::Integer(i) => ConfigValue::Int(i),
        toml::Value::Float(f) => ConfigValue::Float(f),
        toml::Value::String(s) => ConfigValue::String(s),
        toml::Value::Array(arr) => {
            ConfigValue::Array(arr.into_iter().map(toml_to_config).collect())
        }
        toml::Value::Table(t) => {
            let mut map = BTreeMap::new();
            for (k, v) in t.into_iter() {
                map.insert(k, toml_to_config(v));
            }
            ConfigValue::Table(map)
        }
        toml::Value::Datetime(d) => ConfigValue::String(d.to_string()),
    }
}

/// Converts a [`ConfigValue`] into a `toml::Value`.
fn config_to_toml(value: &ConfigValue) -> toml::Value {
    match value {
        ConfigValue::Bool(b) => toml::Value::Boolean(*b),
        ConfigValue::Int(i) => toml::Value::Integer(*i),
        ConfigValue::Float(f) => toml::Value::Float(*f),
        ConfigValue::String(s) => toml::Value::String(s.clone()),
        ConfigValue::Array(arr) => toml::Value::Array(arr.iter().map(config_to_toml).collect()),
        ConfigValue::Table(t) => {
            let mut table = toml::Table::new();
            for (k, v) in t.iter() {
                table.insert(k.clone(), config_to_toml(v));
            }
            toml::Value::Table(table)
        }
    }
}

// ============================================================================
// serde_json::Value ↔ ConfigValue conversion
// ============================================================================

/// Converts a `serde_json::Value` into a [`ConfigValue`].
///
/// `Null` values produce a parse error (config values must be concrete).
/// Numbers are stored as `Int` when they fit in `i64`, otherwise as `Float`.
fn json_to_config(value: serde_json::Value) -> Result<ConfigValue, ConfigError> {
    match value {
        serde_json::Value::Null => Err(ConfigError::JsonParse(String::from(
            "null is not a valid config value",
        ))),
        serde_json::Value::Bool(b) => Ok(ConfigValue::Bool(b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(ConfigValue::Int(i))
            } else if let Some(f) = n.as_f64() {
                Ok(ConfigValue::Float(f))
            } else {
                Err(ConfigError::JsonParse(format!(
                    "unsupported number value: {}",
                    n
                )))
            }
        }
        serde_json::Value::String(s) => Ok(ConfigValue::String(s)),
        serde_json::Value::Array(arr) => {
            let mut out = Vec::with_capacity(arr.len());
            for v in arr {
                out.push(json_to_config(v)?);
            }
            Ok(ConfigValue::Array(out))
        }
        serde_json::Value::Object(obj) => {
            let mut map = BTreeMap::new();
            for (k, v) in obj.into_iter() {
                map.insert(k, json_to_config(v)?);
            }
            Ok(ConfigValue::Table(map))
        }
    }
}

/// Converts a [`ConfigValue`] into a `serde_json::Value`.
///
/// `Float` values that are NaN/Infinity become `Null` (JSON has no NaN).
fn config_to_json(value: &ConfigValue) -> serde_json::Value {
    match value {
        ConfigValue::Bool(b) => serde_json::Value::Bool(*b),
        ConfigValue::Int(i) => serde_json::Value::Number((*i).into()),
        ConfigValue::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        ConfigValue::String(s) => serde_json::Value::String(s.clone()),
        ConfigValue::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(config_to_json).collect())
        }
        ConfigValue::Table(t) => {
            let mut map = serde_json::Map::new();
            for (k, v) in t.iter() {
                map.insert(k.clone(), config_to_json(v));
            }
            serde_json::Value::Object(map)
        }
    }
}

// ============================================================================
// TomlLoader
// ============================================================================

/// TOML format loader (backed by the `toml` crate).
pub struct TomlLoader;

impl TomlLoader {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TomlLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigLoader for TomlLoader {
    fn parse(&self, data: &[u8]) -> Result<ConfigValue, ConfigError> {
        let s = core::str::from_utf8(data)
            .map_err(|e| ConfigError::TomlParse(format!("invalid UTF-8: {}", e)))?;
        let table: toml::Table = s
            .parse()
            .map_err(|e| ConfigError::TomlParse(format!("{}", e)))?;
        Ok(toml_to_config(toml::Value::Table(table)))
    }

    fn serialize(&self, value: &ConfigValue) -> Result<Vec<u8>, ConfigError> {
        let toml_val = config_to_toml(value);
        let table = match toml_val {
            toml::Value::Table(t) => t,
            _ => {
                return Err(ConfigError::Internal(String::from(
                    "TOML root value must be a Table",
                )));
            }
        };
        let s = table.to_string();
        Ok(s.into_bytes())
    }
}

// ============================================================================
// JsonLoader
// ============================================================================

/// JSON format loader (backed by the `serde_json` crate).
pub struct JsonLoader;

impl JsonLoader {
    pub fn new() -> Self {
        Self
    }
}

impl Default for JsonLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigLoader for JsonLoader {
    fn parse(&self, data: &[u8]) -> Result<ConfigValue, ConfigError> {
        let s = core::str::from_utf8(data)
            .map_err(|e| ConfigError::JsonParse(format!("invalid UTF-8: {}", e)))?;
        let value: serde_json::Value =
            serde_json::from_str(s).map_err(|e| ConfigError::JsonParse(format!("{}", e)))?;
        json_to_config(value)
    }

    fn serialize(&self, value: &ConfigValue) -> Result<Vec<u8>, ConfigError> {
        let json_val = config_to_json(value);
        let s = serde_json::to_string(&json_val)
            .map_err(|e| ConfigError::Internal(format!("JSON serialization: {}", e)))?;
        Ok(s.into_bytes())
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    // ---- ConfigFormat ----

    #[test]
    fn test_format_from_extension() {
        assert_eq!(
            ConfigFormat::from_extension("toml"),
            Some(ConfigFormat::Toml)
        );
        assert_eq!(
            ConfigFormat::from_extension("json"),
            Some(ConfigFormat::Json)
        );
        assert_eq!(ConfigFormat::from_extension("yaml"), None);
        assert_eq!(ConfigFormat::from_extension(""), None);
    }

    #[test]
    fn test_format_extension() {
        assert_eq!(ConfigFormat::Toml.extension(), "toml");
        assert_eq!(ConfigFormat::Json.extension(), "json");
    }

    // ---- toml::Value ↔ ConfigValue ----

    #[test]
    fn test_toml_to_config_bool() {
        let result = toml_to_config(toml::Value::Boolean(true));
        assert_eq!(result, ConfigValue::Bool(true));
    }

    #[test]
    fn test_toml_to_config_int() {
        let result = toml_to_config(toml::Value::Integer(-42));
        assert_eq!(result, ConfigValue::Int(-42));
    }

    #[test]
    fn test_toml_to_config_float() {
        let result = toml_to_config(toml::Value::Float(2.5));
        assert_eq!(result, ConfigValue::Float(2.5));
    }

    #[test]
    fn test_toml_to_config_string() {
        let result = toml_to_config(toml::Value::String(String::from("hello")));
        assert_eq!(result, ConfigValue::String(String::from("hello")));
    }

    #[test]
    fn test_toml_to_config_array() {
        let arr = vec![
            toml::Value::Integer(1),
            toml::Value::Integer(2),
            toml::Value::Integer(3),
        ];
        let result = toml_to_config(toml::Value::Array(arr));
        assert_eq!(
            result,
            ConfigValue::Array(vec![
                ConfigValue::Int(1),
                ConfigValue::Int(2),
                ConfigValue::Int(3),
            ])
        );
    }

    #[test]
    fn test_toml_to_config_table() {
        let mut table = toml::Table::new();
        table.insert(String::from("port"), toml::Value::Integer(8080));
        table.insert(
            String::from("host"),
            toml::Value::String(String::from("0.0.0.0")),
        );
        let result = toml_to_config(toml::Value::Table(table));
        let mut expected = BTreeMap::new();
        expected.insert(
            String::from("host"),
            ConfigValue::String(String::from("0.0.0.0")),
        );
        expected.insert(String::from("port"), ConfigValue::Int(8080));
        assert_eq!(result, ConfigValue::Table(expected));
    }

    #[test]
    fn test_toml_to_config_nested() {
        let mut inner = toml::Table::new();
        inner.insert(String::from("port"), toml::Value::Integer(9090));
        let mut outer = toml::Table::new();
        outer.insert(String::from("device"), toml::Value::Table(inner));
        let result = toml_to_config(toml::Value::Table(outer));
        let mut expected_inner = BTreeMap::new();
        expected_inner.insert(String::from("port"), ConfigValue::Int(9090));
        let mut expected_outer = BTreeMap::new();
        expected_outer.insert(String::from("device"), ConfigValue::Table(expected_inner));
        assert_eq!(result, ConfigValue::Table(expected_outer));
    }

    #[test]
    fn test_config_to_toml_roundtrip() {
        let mut table = BTreeMap::new();
        table.insert(String::from("enabled"), ConfigValue::Bool(true));
        table.insert(String::from("port"), ConfigValue::Int(8080));
        table.insert(String::from("pi"), ConfigValue::Float(2.5));
        table.insert(
            String::from("name"),
            ConfigValue::String(String::from("test")),
        );
        let original = ConfigValue::Table(table);

        let toml_val = config_to_toml(&original);
        let back = toml_to_config(toml_val);
        assert_eq!(original, back);
    }

    // ---- serde_json::Value ↔ ConfigValue ----

    #[test]
    fn test_json_to_config_bool() {
        let result = json_to_config(serde_json::Value::Bool(false)).unwrap();
        assert_eq!(result, ConfigValue::Bool(false));
    }

    #[test]
    fn test_json_to_config_int() {
        let result = json_to_config(serde_json::json!(42)).unwrap();
        assert_eq!(result, ConfigValue::Int(42));
    }

    #[test]
    fn test_json_to_config_float() {
        let result = json_to_config(serde_json::json!(2.5)).unwrap();
        assert_eq!(result, ConfigValue::Float(2.5));
    }

    #[test]
    fn test_json_to_config_string() {
        let result = json_to_config(serde_json::Value::String(String::from("hello"))).unwrap();
        assert_eq!(result, ConfigValue::String(String::from("hello")));
    }

    #[test]
    fn test_json_to_config_null_errors() {
        let result = json_to_config(serde_json::Value::Null);
        assert!(matches!(result, Err(ConfigError::JsonParse(_))));
    }

    #[test]
    fn test_json_to_config_array() {
        let result = json_to_config(serde_json::json!([1, 2, 3])).unwrap();
        assert_eq!(
            result,
            ConfigValue::Array(vec![
                ConfigValue::Int(1),
                ConfigValue::Int(2),
                ConfigValue::Int(3),
            ])
        );
    }

    #[test]
    fn test_json_to_config_object() {
        let result = json_to_config(serde_json::json!({"port": 8080, "host": "0.0.0.0"})).unwrap();
        let mut expected = BTreeMap::new();
        expected.insert(
            String::from("host"),
            ConfigValue::String(String::from("0.0.0.0")),
        );
        expected.insert(String::from("port"), ConfigValue::Int(8080));
        assert_eq!(result, ConfigValue::Table(expected));
    }

    #[test]
    fn test_json_to_config_nested() {
        let result = json_to_config(serde_json::json!({"device": {"port": 9090}})).unwrap();
        let mut inner = BTreeMap::new();
        inner.insert(String::from("port"), ConfigValue::Int(9090));
        let mut outer = BTreeMap::new();
        outer.insert(String::from("device"), ConfigValue::Table(inner));
        assert_eq!(result, ConfigValue::Table(outer));
    }

    #[test]
    fn test_json_to_config_array_with_mixed_types() {
        let result = json_to_config(serde_json::json!([true, 42, "hello"])).unwrap();
        assert_eq!(
            result,
            ConfigValue::Array(vec![
                ConfigValue::Bool(true),
                ConfigValue::Int(42),
                ConfigValue::String(String::from("hello")),
            ])
        );
    }

    #[test]
    fn test_config_to_json_roundtrip() {
        let mut table = BTreeMap::new();
        table.insert(String::from("enabled"), ConfigValue::Bool(true));
        table.insert(String::from("port"), ConfigValue::Int(8080));
        table.insert(String::from("pi"), ConfigValue::Float(2.5));
        table.insert(
            String::from("name"),
            ConfigValue::String(String::from("test")),
        );
        let original = ConfigValue::Table(table);

        let json_val = config_to_json(&original);
        let back = json_to_config(json_val).unwrap();
        assert_eq!(original, back);
    }

    #[test]
    fn test_config_to_json_array() {
        let arr = ConfigValue::Array(vec![
            ConfigValue::Int(1),
            ConfigValue::Bool(false),
            ConfigValue::String(String::from("x")),
        ]);
        let json_val = config_to_json(&arr);
        let back = json_to_config(json_val).unwrap();
        assert_eq!(arr, back);
    }

    // ---- TomlLoader parse/serialize ----

    #[test]
    fn test_toml_parse_simple() {
        let loader = TomlLoader::new();
        let data = b"port = 8080\nhost = \"0.0.0.0\"\nenabled = true\n";
        let result = loader.parse(data).unwrap();
        let table = result.as_table().expect("should be table");
        assert_eq!(table.get("port"), Some(&ConfigValue::Int(8080)));
        assert_eq!(
            table.get("host"),
            Some(&ConfigValue::String(String::from("0.0.0.0")))
        );
        assert_eq!(table.get("enabled"), Some(&ConfigValue::Bool(true)));
    }

    #[test]
    fn test_toml_parse_nested() {
        let loader = TomlLoader::new();
        let data = b"[device]\nport = 9090\nname = \"sensor\"\n";
        let result = loader.parse(data).unwrap();
        let table = result.as_table().expect("should be table");
        let device = table.get("device").expect("device key");
        let device_table = device.as_table().expect("device should be table");
        assert_eq!(device_table.get("port"), Some(&ConfigValue::Int(9090)));
        assert_eq!(
            device_table.get("name"),
            Some(&ConfigValue::String(String::from("sensor")))
        );
    }

    #[test]
    fn test_toml_parse_array() {
        let loader = TomlLoader::new();
        let data = b"ports = [8080, 9090, 10000]\n";
        let result = loader.parse(data).unwrap();
        let table = result.as_table().expect("should be table");
        let ports = table.get("ports").expect("ports key");
        let arr = ports.as_array().expect("should be array");
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0], ConfigValue::Int(8080));
        assert_eq!(arr[1], ConfigValue::Int(9090));
        assert_eq!(arr[2], ConfigValue::Int(10000));
    }

    #[test]
    fn test_toml_parse_float() {
        let loader = TomlLoader::new();
        let data = b"pi = 2.5\n";
        let result = loader.parse(data).unwrap();
        let table = result.as_table().expect("should be table");
        assert_eq!(table.get("pi"), Some(&ConfigValue::Float(2.5)));
    }

    #[test]
    fn test_toml_parse_invalid() {
        let loader = TomlLoader::new();
        let data = b"this is not valid toml = = =";
        let result = loader.parse(data);
        assert!(matches!(result, Err(ConfigError::TomlParse(_))));
    }

    #[test]
    fn test_toml_parse_invalid_utf8() {
        let loader = TomlLoader::new();
        let data = &[0xFF, 0xFE, 0xFD];
        let result = loader.parse(data);
        assert!(matches!(result, Err(ConfigError::TomlParse(_))));
    }

    #[test]
    fn test_toml_serialize_simple() {
        let loader = TomlLoader::new();
        let mut table = BTreeMap::new();
        table.insert(String::from("port"), ConfigValue::Int(8080));
        table.insert(String::from("enabled"), ConfigValue::Bool(true));
        let value = ConfigValue::Table(table);

        let data = loader.serialize(&value).unwrap();
        let s = core::str::from_utf8(&data).unwrap();
        assert!(s.contains("port = 8080"));
        assert!(s.contains("enabled = true"));
    }

    #[test]
    fn test_toml_serialize_nested() {
        let loader = TomlLoader::new();
        let mut inner = BTreeMap::new();
        inner.insert(String::from("port"), ConfigValue::Int(9090));
        let mut outer = BTreeMap::new();
        outer.insert(String::from("device"), ConfigValue::Table(inner));
        let value = ConfigValue::Table(outer);

        let data = loader.serialize(&value).unwrap();
        let s = core::str::from_utf8(&data).unwrap();
        assert!(s.contains("port = 9090"));
        assert!(s.contains("[device]"));
    }

    #[test]
    fn test_toml_serialize_non_table_errors() {
        let loader = TomlLoader::new();
        let value = ConfigValue::Int(42);
        let result = loader.serialize(&value);
        assert!(matches!(result, Err(ConfigError::Internal(_))));
    }

    #[test]
    fn test_toml_roundtrip() {
        let loader = TomlLoader::new();
        let mut table = BTreeMap::new();
        table.insert(String::from("port"), ConfigValue::Int(8080));
        table.insert(
            String::from("host"),
            ConfigValue::String(String::from("0.0.0.0")),
        );
        table.insert(String::from("enabled"), ConfigValue::Bool(true));
        table.insert(String::from("pi"), ConfigValue::Float(2.5));
        let original = ConfigValue::Table(table);

        let data = loader.serialize(&original).unwrap();
        let parsed = loader.parse(&data).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn test_toml_roundtrip_nested() {
        let loader = TomlLoader::new();
        let mut inner = BTreeMap::new();
        inner.insert(String::from("port"), ConfigValue::Int(9090));
        inner.insert(
            String::from("name"),
            ConfigValue::String(String::from("sensor")),
        );
        let mut outer = BTreeMap::new();
        outer.insert(String::from("device"), ConfigValue::Table(inner));
        outer.insert(String::from("count"), ConfigValue::Int(3));
        let original = ConfigValue::Table(outer);

        let data = loader.serialize(&original).unwrap();
        let parsed = loader.parse(&data).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn test_toml_roundtrip_with_array() {
        let loader = TomlLoader::new();
        let mut table = BTreeMap::new();
        table.insert(
            String::from("ports"),
            ConfigValue::Array(vec![
                ConfigValue::Int(8080),
                ConfigValue::Int(9090),
                ConfigValue::Int(10000),
            ]),
        );
        let original = ConfigValue::Table(table);

        let data = loader.serialize(&original).unwrap();
        let parsed = loader.parse(&data).unwrap();
        assert_eq!(original, parsed);
    }

    // ---- JsonLoader parse/serialize ----

    #[test]
    fn test_json_parse_simple() {
        let loader = JsonLoader::new();
        let data = br#"{"port": 8080, "host": "0.0.0.0", "enabled": true}"#;
        let result = loader.parse(data).unwrap();
        let table = result.as_table().expect("should be table");
        assert_eq!(table.get("port"), Some(&ConfigValue::Int(8080)));
        assert_eq!(
            table.get("host"),
            Some(&ConfigValue::String(String::from("0.0.0.0")))
        );
        assert_eq!(table.get("enabled"), Some(&ConfigValue::Bool(true)));
    }

    #[test]
    fn test_json_parse_nested() {
        let loader = JsonLoader::new();
        let data = br#"{"device": {"port": 9090, "name": "sensor"}}"#;
        let result = loader.parse(data).unwrap();
        let table = result.as_table().expect("should be table");
        let device = table.get("device").expect("device key");
        let device_table = device.as_table().expect("device should be table");
        assert_eq!(device_table.get("port"), Some(&ConfigValue::Int(9090)));
        assert_eq!(
            device_table.get("name"),
            Some(&ConfigValue::String(String::from("sensor")))
        );
    }

    #[test]
    fn test_json_parse_array() {
        let loader = JsonLoader::new();
        let data = br#"{"ports": [8080, 9090, 10000]}"#;
        let result = loader.parse(data).unwrap();
        let table = result.as_table().expect("should be table");
        let ports = table.get("ports").expect("ports key");
        let arr = ports.as_array().expect("should be array");
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0], ConfigValue::Int(8080));
    }

    #[test]
    fn test_json_parse_invalid() {
        let loader = JsonLoader::new();
        let data = b"{this is not json}";
        let result = loader.parse(data);
        assert!(matches!(result, Err(ConfigError::JsonParse(_))));
    }

    #[test]
    fn test_json_parse_invalid_utf8() {
        let loader = JsonLoader::new();
        let data = &[0xFF, 0xFE];
        let result = loader.parse(data);
        assert!(matches!(result, Err(ConfigError::JsonParse(_))));
    }

    #[test]
    fn test_json_parse_null_in_array_errors() {
        let loader = JsonLoader::new();
        let data = br#"[1, null, 3]"#;
        let result = loader.parse(data);
        assert!(matches!(result, Err(ConfigError::JsonParse(_))));
    }

    #[test]
    fn test_json_serialize_simple() {
        let loader = JsonLoader::new();
        let mut table = BTreeMap::new();
        table.insert(String::from("port"), ConfigValue::Int(8080));
        table.insert(String::from("enabled"), ConfigValue::Bool(true));
        let value = ConfigValue::Table(table);

        let data = loader.serialize(&value).unwrap();
        let s = core::str::from_utf8(&data).unwrap();
        assert!(s.contains("\"port\":8080"));
        assert!(s.contains("\"enabled\":true"));
    }

    #[test]
    fn test_json_serialize_array() {
        let loader = JsonLoader::new();
        let value = ConfigValue::Array(vec![
            ConfigValue::Int(1),
            ConfigValue::Int(2),
            ConfigValue::Int(3),
        ]);
        let data = loader.serialize(&value).unwrap();
        let s = core::str::from_utf8(&data).unwrap();
        assert_eq!(s, "[1,2,3]");
    }

    #[test]
    fn test_json_roundtrip() {
        let loader = JsonLoader::new();
        let mut table = BTreeMap::new();
        table.insert(String::from("port"), ConfigValue::Int(8080));
        table.insert(
            String::from("host"),
            ConfigValue::String(String::from("0.0.0.0")),
        );
        table.insert(String::from("enabled"), ConfigValue::Bool(true));
        let original = ConfigValue::Table(table);

        let data = loader.serialize(&original).unwrap();
        let parsed = loader.parse(&data).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn test_json_roundtrip_nested() {
        let loader = JsonLoader::new();
        let mut inner = BTreeMap::new();
        inner.insert(String::from("port"), ConfigValue::Int(9090));
        inner.insert(
            String::from("name"),
            ConfigValue::String(String::from("sensor")),
        );
        let mut outer = BTreeMap::new();
        outer.insert(String::from("device"), ConfigValue::Table(inner));
        outer.insert(String::from("count"), ConfigValue::Int(3));
        let original = ConfigValue::Table(outer);

        let data = loader.serialize(&original).unwrap();
        let parsed = loader.parse(&data).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn test_json_roundtrip_with_array() {
        let loader = JsonLoader::new();
        let mut table = BTreeMap::new();
        table.insert(
            String::from("ports"),
            ConfigValue::Array(vec![
                ConfigValue::Int(8080),
                ConfigValue::Int(9090),
                ConfigValue::Int(10000),
            ]),
        );
        let original = ConfigValue::Table(table);

        let data = loader.serialize(&original).unwrap();
        let parsed = loader.parse(&data).unwrap();
        assert_eq!(original, parsed);
    }

    // ---- Default impls ----

    #[test]
    fn test_toml_loader_default() {
        let loader = TomlLoader::new();
        let data = b"x = 1\n";
        let result = loader.parse(data).unwrap();
        assert!(result.as_table().is_some());
    }

    #[test]
    fn test_json_loader_default() {
        let loader = JsonLoader::new();
        let data = br#"{"x": 1}"#;
        let result = loader.parse(data).unwrap();
        assert!(result.as_table().is_some());
    }
}

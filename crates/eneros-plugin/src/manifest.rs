//! 插件清单（manifest）定义与解析
//!
//! 插件清单采用 TOML 格式，描述插件的元数据、依赖与安全信息。
//!
//! 示例：
//! ```toml
//! [plugin]
//! name = "iec104-driver"
//! version = "1.2.0"
//! api_version = "0.27.0"
//! plugin_type = "Protocol"
//! description = "IEC 104 protocol driver"
//! author = "EnerOS"
//!
//! [dependencies]
//! plugins = ["core-mbus"]
//!
//! [security]
//! signer = "eneros-trusted"
//! ```

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::PluginError;

/// 插件类型
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PluginType {
    /// 协议插件（IEC 104/61850/Modbus 等）
    Protocol,
    /// Agent 插件
    Agent,
    /// 分析插件（潮流、状态估计等）
    Analysis,
}

/// 插件清单
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// 插件基本信息
    pub plugin: PluginSection,
    /// 依赖配置
    #[serde(default)]
    pub dependencies: DependenciesSection,
    /// 安全配置
    #[serde(default)]
    pub security: SecuritySection,
}

/// 插件基本信息段
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginSection {
    /// 插件名称（唯一标识）
    pub name: String,
    /// 插件版本
    pub version: String,
    /// 插件 API 版本（与 EnerOS API 版本兼容性检查）
    pub api_version: String,
    /// 插件类型
    pub plugin_type: PluginType,
    /// 插件描述
    #[serde(default)]
    pub description: String,
    /// 插件作者
    #[serde(default)]
    pub author: String,
}

/// 依赖配置段
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DependenciesSection {
    /// 依赖的其他插件名列表
    #[serde(default)]
    pub plugins: Vec<String>,
}

/// 安全配置段
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecuritySection {
    /// 签名者标识
    #[serde(default)]
    pub signer: String,
}

/// 插件元数据（从清单提取的精简信息）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginMetadata {
    /// 插件名称
    pub name: String,
    /// 插件版本
    pub version: String,
    /// 插件 API 版本
    pub api_version: String,
    /// 插件类型
    pub plugin_type: PluginType,
    /// 插件描述
    pub description: String,
}

impl From<&PluginManifest> for PluginMetadata {
    fn from(m: &PluginManifest) -> Self {
        Self {
            name: m.plugin.name.clone(),
            version: m.plugin.version.clone(),
            api_version: m.plugin.api_version.clone(),
            plugin_type: m.plugin.plugin_type.clone(),
            description: m.plugin.description.clone(),
        }
    }
}

impl PluginManifest {
    /// 从 TOML 字符串加载清单
    ///
    /// 解析错误统一映射为 `PluginError::InvalidManifest`。
    pub fn load_from_str(s: &str) -> Result<Self, PluginError> {
        toml::from_str(s)
            .map_err(|e| PluginError::InvalidManifest(format!("parse failed: {}", e)))
    }

    /// 从文件加载清单
    ///
    /// 读取文件内容并解析为 TOML，IO 错误与解析错误统一映射为 `PluginError::InvalidManifest`。
    pub fn load_from_file(path: &Path) -> Result<Self, PluginError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| PluginError::InvalidManifest(format!("read file failed: {}", e)))?;
        Self::load_from_str(&content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_from_str_minimal() {
        let toml_str = r#"
[plugin]
name = "test-plugin"
version = "0.1.0"
api_version = "0.27.0"
plugin_type = "Protocol"
"#;
        let manifest = PluginManifest::load_from_str(toml_str).unwrap();
        assert_eq!(manifest.plugin.name, "test-plugin");
        assert_eq!(manifest.plugin.version, "0.1.0");
        assert_eq!(manifest.plugin.api_version, "0.27.0");
        assert_eq!(manifest.plugin.plugin_type, PluginType::Protocol);
        assert_eq!(manifest.plugin.description, "");
        assert_eq!(manifest.plugin.author, "");
        assert!(manifest.dependencies.plugins.is_empty());
        assert_eq!(manifest.security.signer, "");
    }

    #[test]
    fn test_load_from_str_full() {
        let toml_str = r#"
[plugin]
name = "iec104-driver"
version = "1.2.0"
api_version = "0.27.0"
plugin_type = "Protocol"
description = "IEC 104 protocol driver"
author = "EnerOS"

[dependencies]
plugins = ["core-mbus", "scada-core"]

[security]
signer = "eneros-trusted"
"#;
        let manifest = PluginManifest::load_from_str(toml_str).unwrap();
        assert_eq!(manifest.plugin.name, "iec104-driver");
        assert_eq!(manifest.plugin.description, "IEC 104 protocol driver");
        assert_eq!(manifest.plugin.author, "EnerOS");
        assert_eq!(
            manifest.dependencies.plugins,
            vec!["core-mbus", "scada-core"]
        );
        assert_eq!(manifest.security.signer, "eneros-trusted");
    }

    #[test]
    fn test_metadata_from_manifest() {
        let toml_str = r#"
[plugin]
name = "forecast-agent"
version = "0.3.0"
api_version = "0.27.0"
plugin_type = "Agent"
description = "Load forecast agent"
"#;
        let manifest = PluginManifest::load_from_str(toml_str).unwrap();
        let metadata = PluginMetadata::from(&manifest);
        assert_eq!(metadata.name, "forecast-agent");
        assert_eq!(metadata.version, "0.3.0");
        assert_eq!(metadata.api_version, "0.27.0");
        assert_eq!(metadata.plugin_type, PluginType::Agent);
        assert_eq!(metadata.description, "Load forecast agent");
    }

    #[test]
    fn test_serialize_roundtrip() {
        let toml_str = r#"
[plugin]
name = "opf-analysis"
version = "0.5.0"
api_version = "0.27.0"
plugin_type = "Analysis"
description = "OPF analysis plugin"

[dependencies]
plugins = ["powerflow-core"]
"#;
        let manifest = PluginManifest::load_from_str(toml_str).unwrap();
        let serialized = toml::to_string(&manifest).unwrap();
        let deserialized = PluginManifest::load_from_str(&serialized).unwrap();
        assert_eq!(manifest.plugin.name, deserialized.plugin.name);
        assert_eq!(manifest.plugin.version, deserialized.plugin.version);
        assert_eq!(
            manifest.plugin.plugin_type,
            deserialized.plugin.plugin_type
        );
        assert_eq!(
            manifest.dependencies.plugins,
            deserialized.dependencies.plugins
        );
    }

    #[test]
    fn test_invalid_toml() {
        let result = PluginManifest::load_from_str("not valid toml = = ");
        assert!(result.is_err());
        assert!(matches!(result, Err(PluginError::InvalidManifest(_))));
    }

    #[test]
    fn test_load_from_file_missing() {
        let path = Path::new("/nonexistent/path/manifest.toml");
        let result = PluginManifest::load_from_file(path);
        assert!(result.is_err());
        assert!(matches!(result, Err(PluginError::InvalidManifest(_))));
    }

    #[test]
    fn test_load_from_file_temp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plugin.toml");
        let toml_str = r#"
[plugin]
name = "file-plugin"
version = "1.0.0"
api_version = "0.27.0"
plugin_type = "Agent"
"#;
        std::fs::write(&path, toml_str).unwrap();
        let manifest = PluginManifest::load_from_file(&path).unwrap();
        assert_eq!(manifest.plugin.name, "file-plugin");
        assert_eq!(manifest.plugin.plugin_type, PluginType::Agent);
    }

    #[test]
    fn test_plugin_type_serde() {
        let json = serde_json::to_string(&PluginType::Protocol).unwrap();
        assert_eq!(json, "\"Protocol\"");
        let t: PluginType = serde_json::from_str("\"Agent\"").unwrap();
        assert_eq!(t, PluginType::Agent);
        let t: PluginType = serde_json::from_str("\"Analysis\"").unwrap();
        assert_eq!(t, PluginType::Analysis);
    }

    #[test]
    fn test_metadata_serde_roundtrip() {
        let metadata = PluginMetadata {
            name: "p".to_string(),
            version: "1.0.0".to_string(),
            api_version: "0.27.0".to_string(),
            plugin_type: PluginType::Protocol,
            description: "desc".to_string(),
        };
        let json = serde_json::to_string(&metadata).unwrap();
        let deserialized: PluginMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(metadata.name, deserialized.name);
        assert_eq!(metadata.plugin_type, deserialized.plugin_type);
    }
}

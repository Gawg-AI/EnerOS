//! EnerOS SDK 插件开发模块
//!
//! 提供插件开发的便捷 API，包括：
//! - `PluginBuilder`：链式构造插件清单（PluginManifest）
//! - `PluginSdk`：插件签名与验证封装
//! - `generate_keypair`：生成 Ed25519 密钥对
//! - `#[eneros_plugin]` 宏的再导出
//!
//! # 示例
//!
//! ```
//! use eneros_plugin::PluginType;
//! use eneros_sdk::plugin::PluginBuilder;
//!
//! let manifest = PluginBuilder::new("my-plugin", "1.0.0", PluginType::Protocol)
//!     .description("My custom plugin")
//!     .author("EnerOS")
//!     .dependency("core-mbus")
//!     .build()
//!     .unwrap();
//! ```

use crate::common::{SdkError, SdkResult};
use eneros_plugin::{PluginManifest, PluginType};
use std::path::Path;

// Re-export #[eneros_plugin] 宏
pub use eneros_plugin_macros::eneros_plugin;

/// 插件构造器
///
/// 使用链式 API 构造 `PluginManifest`，简化插件清单的创建过程。
/// 必填字段通过 `new` 提供，可选字段通过链式方法设置。
pub struct PluginBuilder {
    /// 插件名称（唯一标识）
    name: String,
    /// 插件版本
    version: String,
    /// 插件 API 版本（默认为当前 SDK 版本，从 Cargo.toml 动态读取）
    api_version: String,
    /// 插件类型
    plugin_type: PluginType,
    /// 插件描述
    description: String,
    /// 插件作者
    author: String,
    /// 依赖的其他插件名列表
    dependencies: Vec<String>,
}

impl PluginBuilder {
    /// 创建插件构造器
    ///
    /// # 参数
    /// - `name`：插件名称（唯一标识）
    /// - `version`：插件版本
    /// - `plugin_type`：插件类型（Protocol/Agent/Analysis）
    pub fn new(name: impl Into<String>, version: impl Into<String>, plugin_type: PluginType) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            // 从 Cargo.toml 动态读取版本号，避免硬编码导致版本不一致
            api_version: env!("CARGO_PKG_VERSION").to_string(),
            plugin_type,
            description: String::new(),
            author: String::new(),
            dependencies: Vec::new(),
        }
    }

    /// 设置插件 API 版本（默认为当前 SDK 版本，从 Cargo.toml 动态读取）
    pub fn api_version(mut self, v: impl Into<String>) -> Self {
        self.api_version = v.into();
        self
    }

    /// 设置插件描述
    pub fn description(mut self, d: impl Into<String>) -> Self {
        self.description = d.into();
        self
    }

    /// 设置插件作者
    pub fn author(mut self, a: impl Into<String>) -> Self {
        self.author = a.into();
        self
    }

    /// 添加单个依赖插件
    pub fn dependency(mut self, dep: impl Into<String>) -> Self {
        self.dependencies.push(dep.into());
        self
    }

    /// 设置依赖插件列表（替换原有列表）
    pub fn dependencies(mut self, deps: Vec<String>) -> Self {
        self.dependencies = deps;
        self
    }

    /// 构建 PluginManifest
    ///
    /// 将构造器中的字段组装为 `PluginManifest`，安全配置段使用默认值。
    pub fn build(self) -> SdkResult<PluginManifest> {
        Ok(PluginManifest {
            plugin: eneros_plugin::manifest::PluginSection {
                name: self.name,
                version: self.version,
                api_version: self.api_version,
                plugin_type: self.plugin_type,
                description: self.description,
                author: self.author,
            },
            dependencies: eneros_plugin::manifest::DependenciesSection {
                plugins: self.dependencies,
            },
            security: eneros_plugin::manifest::SecuritySection::default(),
        })
    }

    /// 生成 manifest.toml 字符串
    ///
    /// 将构造器内容序列化为 TOML 格式字符串。
    pub fn build_toml(self) -> SdkResult<String> {
        let manifest = self.build()?;
        toml::to_string_pretty(&manifest)
            .map_err(|e| SdkError::Config(format!("manifest serialize failed: {}", e)))
    }
}

/// 插件 SDK 封装
///
/// 封装插件签名与验证操作，提供统一的 SDK 入口。
pub struct PluginSdk {
    /// 插件清单
    pub manifest: PluginManifest,
}

impl PluginSdk {
    /// 创建插件 SDK 实例
    pub fn new(manifest: PluginManifest) -> Self {
        Self { manifest }
    }

    /// 签名插件文件
    ///
    /// 使用指定私钥对插件文件生成 Ed25519 签名，签名文件写入 `<plugin_path>.sig`。
    ///
    /// # 参数
    /// - `plugin_path`：插件文件路径
    /// - `key_path`：私钥文件路径（32 字节原始 Ed25519 私钥）
    pub fn sign_plugin(&self, plugin_path: &str, key_path: &str) -> SdkResult<()> {
        eneros_plugin::signature::sign_plugin(Path::new(plugin_path), Path::new(key_path))
            .map_err(|e| SdkError::Plugin(format!("sign failed: {}", e)))?;
        Ok(())
    }

    /// 验证插件签名
    ///
    /// 使用空验证器（不强制要求签名）验证插件签名格式。
    ///
    /// 注意：此方法为简化实现，使用无可信公钥的验证器，无法验证签名者可信性。
    /// 如需完整验证，请直接使用 `eneros_plugin::signature::PluginSignatureVerifier`。
    ///
    /// # 参数
    /// - `plugin_path`：插件文件路径
    ///
    /// # 返回
    /// - `true`：签名有效或插件未签名（不强制要求签名时视为有效）
    /// - `false`：签名无效或签名者不可信
    pub fn verify_plugin(&self, plugin_path: &str) -> SdkResult<bool> {
        let verifier = eneros_plugin::signature::PluginSignatureVerifier::empty(false);
        let result = verifier
            .verify_plugin(Path::new(plugin_path))
            .map_err(|e| SdkError::Plugin(format!("verify failed: {}", e)))?;
        Ok(matches!(
            result,
            eneros_plugin::signature::VerificationResult::Valid { .. }
        ))
    }
}

/// 生成 Ed25519 密钥对
///
/// 使用 OS 随机源生成 Ed25519 密钥对，写入 `output_dir`：
/// - `signing.key`：32 字节原始私钥
/// - `signing.pub`：32 字节原始公钥
///
/// # 参数
/// - `output_dir`：输出目录（不存在则自动创建）
pub fn generate_keypair(output_dir: &str) -> SdkResult<()> {
    eneros_plugin::signature::generate_keypair(Path::new(output_dir))
        .map_err(|e| SdkError::Plugin(format!("keypair generation failed: {}", e)))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_builder_new() {
        let builder = PluginBuilder::new("test-plugin", "1.0.0", PluginType::Protocol);
        assert_eq!(builder.name, "test-plugin");
        assert_eq!(builder.version, "1.0.0");
        // api_version 默认值应与 Cargo.toml 声明的版本号一致
        assert_eq!(builder.api_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(builder.plugin_type, PluginType::Protocol);
        assert_eq!(builder.description, "");
        assert_eq!(builder.author, "");
        assert!(builder.dependencies.is_empty());
    }

    #[test]
    fn test_plugin_builder_with_description() {
        let builder = PluginBuilder::new("test-plugin", "1.0.0", PluginType::Agent)
            .description("A test agent plugin");
        assert_eq!(builder.description, "A test agent plugin");
    }

    #[test]
    fn test_plugin_builder_with_author() {
        let builder = PluginBuilder::new("test-plugin", "1.0.0", PluginType::Analysis)
            .author("EnerOS Team");
        assert_eq!(builder.author, "EnerOS Team");
    }

    #[test]
    fn test_plugin_builder_with_dependencies() {
        let builder = PluginBuilder::new("test-plugin", "1.0.0", PluginType::Protocol)
            .dependency("core-mbus")
            .dependency("scada-core");
        assert_eq!(builder.dependencies, vec!["core-mbus", "scada-core"]);

        // 测试 dependencies 方法替换列表
        let builder = builder.dependencies(vec!["dep-a".to_string(), "dep-b".to_string()]);
        assert_eq!(builder.dependencies, vec!["dep-a", "dep-b"]);
    }

    #[test]
    fn test_plugin_builder_build() {
        let manifest = PluginBuilder::new("iec104-driver", "1.2.0", PluginType::Protocol)
            .api_version("0.27.0")
            .description("IEC 104 protocol driver")
            .author("EnerOS")
            .dependency("core-mbus")
            .build()
            .unwrap();

        assert_eq!(manifest.plugin.name, "iec104-driver");
        assert_eq!(manifest.plugin.version, "1.2.0");
        assert_eq!(manifest.plugin.api_version, "0.27.0");
        assert_eq!(manifest.plugin.plugin_type, PluginType::Protocol);
        assert_eq!(manifest.plugin.description, "IEC 104 protocol driver");
        assert_eq!(manifest.plugin.author, "EnerOS");
        assert_eq!(manifest.dependencies.plugins, vec!["core-mbus"]);
        assert_eq!(manifest.security.signer, "");
    }

    #[test]
    fn test_plugin_builder_build_toml() {
        let toml_str = PluginBuilder::new("opcua-driver", "0.5.0", PluginType::Protocol)
            .description("OPC UA driver")
            .author("EnerOS")
            .dependency("core-mbus")
            .build_toml()
            .unwrap();

        // 验证 TOML 字符串包含关键字段
        assert!(
            toml_str.contains("opcua-driver"),
            "TOML should contain plugin name"
        );
        assert!(
            toml_str.contains("0.5.0"),
            "TOML should contain plugin version"
        );
        assert!(
            toml_str.contains("OPC UA driver"),
            "TOML should contain description"
        );
        assert!(toml_str.contains("EnerOS"), "TOML should contain author");
        assert!(
            toml_str.contains("core-mbus"),
            "TOML should contain dependency"
        );
        assert!(
            toml_str.contains("Protocol"),
            "TOML should contain plugin_type"
        );
    }
}

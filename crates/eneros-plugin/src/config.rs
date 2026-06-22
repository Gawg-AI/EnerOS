//! 插件系统配置定义与解析
//!
//! 对应 `/etc/eneros/plugin.toml` 配置文件，描述插件目录、签名策略、
//! 沙箱规则与资源配额等系统级配置。
//!
//! 示例：
//! ```toml
//! [plugin]
//! plugin_dir = "/var/lib/eneros/plugins"
//! trusted_keys_dir = "/etc/eneros/keys"
//! require_signature = true
//! enable_seccomp = true
//! enable_quota = true
//!
//! [quota]
//! default_cpu_percent = 50
//! default_memory_mb = 256
//!
//! [sandbox]
//! allowed_paths = ["/etc/eneros/plugins"]
//! denied_paths = ["/etc/shadow"]
//! allowed_network = []
//! ```

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{PluginError, PluginResult};
use crate::loader::LoadMode;

/// 插件系统配置（对应 /etc/eneros/plugin.toml）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginConfig {
    /// 插件加载模式（Inline 同进程 / Daemon 独立进程，默认 Daemon）
    #[serde(default)]
    pub load_mode: LoadMode,
    /// 插件基础配置段
    #[serde(default)]
    pub plugin: PluginSection,
    /// 资源配额配置段
    #[serde(default)]
    pub quota: QuotaSection,
    /// 沙箱配置段
    #[serde(default)]
    pub sandbox: SandboxSection,
}

/// 插件基础配置段
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginSection {
    /// 插件目录（动态库与 manifest.toml 存放位置）
    #[serde(default = "default_plugin_dir")]
    pub plugin_dir: PathBuf,
    /// 可信公钥目录（Ed25519 公钥 .pub 文件存放位置）
    #[serde(default = "default_trusted_keys_dir")]
    pub trusted_keys_dir: PathBuf,
    /// 是否强制要求插件签名（生产环境必须为 true）
    #[serde(default = "default_true")]
    pub require_signature: bool,
    /// 是否启用 seccomp 沙箱（仅 Linux 生效）
    #[serde(default = "default_true")]
    pub enable_seccomp: bool,
    /// 是否启用资源配额（cgroups v2，仅 Linux 生效）
    #[serde(default = "default_true")]
    pub enable_quota: bool,
    /// 是否允许 IPC 调用方跳过签名验证（默认 false，生产环境必须为 false）
    ///
    /// v0.28.0 Task 11 修复 H3：原实现中 `skip_signature` 由请求方控制，
    /// daemon 端无配置项限制。恶意 IPC 客户端可发送 `skip_signature=true`
    /// 绕过签名验证加载未签名/被篡改的插件。新增此字段后，daemon 仅在
    /// `allow_skip_signature=true` 时才接受 `skip_signature=true` 请求。
    #[serde(default = "default_false")]
    pub allow_skip_signature: bool,
}

/// 资源配额配置段
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaSection {
    /// 默认 CPU 配额百分比（相对单核，100 = 1 个完整 CPU 核）
    #[serde(default = "default_cpu_percent")]
    pub default_cpu_percent: u32,
    /// 默认内存上限（MB）
    #[serde(default = "default_memory_mb")]
    pub default_memory_mb: u64,
}

/// 沙箱配置段
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxSection {
    /// 允许插件访问的路径白名单（只读访问，写入需显式声明）
    #[serde(default = "default_allowed_paths")]
    pub allowed_paths: Vec<PathBuf>,
    /// 禁止插件访问的路径黑名单（即使白名单中包含也会被拒绝）
    #[serde(default = "default_denied_paths")]
    pub denied_paths: Vec<PathBuf>,
    /// 允许插件访问的网络目标（CIDR 或 host:port，留空表示禁止所有网络）
    #[serde(default)]
    pub allowed_network: Vec<String>,
}

// ---- serde 默认值函数 ----

fn default_plugin_dir() -> PathBuf {
    PathBuf::from("/var/lib/eneros/plugins")
}

fn default_trusted_keys_dir() -> PathBuf {
    PathBuf::from("/etc/eneros/keys")
}

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

fn default_cpu_percent() -> u32 {
    50
}

fn default_memory_mb() -> u64 {
    256
}

fn default_allowed_paths() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/etc/eneros/plugins"),
        PathBuf::from("/var/lib/eneros/plugins"),
    ]
}

fn default_denied_paths() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/etc/shadow"),
        PathBuf::from("/etc/eneros/keys"),
        PathBuf::from("/root"),
    ]
}

impl Default for PluginSection {
    fn default() -> Self {
        Self {
            plugin_dir: default_plugin_dir(),
            trusted_keys_dir: default_trusted_keys_dir(),
            require_signature: default_true(),
            enable_seccomp: default_true(),
            enable_quota: default_true(),
            allow_skip_signature: default_false(),
        }
    }
}

impl Default for QuotaSection {
    fn default() -> Self {
        Self {
            default_cpu_percent: default_cpu_percent(),
            default_memory_mb: default_memory_mb(),
        }
    }
}

impl Default for SandboxSection {
    fn default() -> Self {
        Self {
            allowed_paths: default_allowed_paths(),
            denied_paths: default_denied_paths(),
            allowed_network: Vec::new(),
        }
    }
}

impl PluginConfig {
    /// 从 TOML 字符串加载配置
    ///
    /// 解析错误统一映射为 `PluginError::InvalidManifest`。
    pub fn load_from_str(s: &str) -> PluginResult<Self> {
        let config: Self = toml::from_str(s)
            .map_err(|e| PluginError::InvalidManifest(format!("config parse failed: {}", e)))?;
        config.validate()?;
        Ok(config)
    }

    /// 验证配置字段范围
    ///
    /// - `default_cpu_percent` 必须在 1-100 范围内
    /// - `default_memory_mb` 必须大于 0
    fn validate(&self) -> PluginResult<()> {
        if self.quota.default_cpu_percent == 0 || self.quota.default_cpu_percent > 100 {
            return Err(PluginError::InvalidManifest(format!(
                "default_cpu_percent must be in 1..=100, got {}",
                self.quota.default_cpu_percent
            )));
        }
        if self.quota.default_memory_mb == 0 {
            return Err(PluginError::InvalidManifest(
                "default_memory_mb must be greater than 0".to_string(),
            ));
        }
        Ok(())
    }

    /// 从文件加载配置
    ///
    /// 读取文件内容并解析为 TOML，IO 错误通过 `PluginError::Io` 返回，
    /// 解析错误通过 `PluginError::InvalidManifest` 返回。
    pub fn load_from_file(path: &Path) -> PluginResult<Self> {
        let content = std::fs::read_to_string(path)?;
        Self::load_from_str(&content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 默认值应与 plugin.toml 中的默认配置一致
    #[test]
    fn test_default_values() {
        let cfg = PluginConfig::default();
        assert_eq!(cfg.load_mode, LoadMode::Daemon);
        assert_eq!(cfg.plugin.plugin_dir, PathBuf::from("/var/lib/eneros/plugins"));
        assert_eq!(cfg.plugin.trusted_keys_dir, PathBuf::from("/etc/eneros/keys"));
        assert!(cfg.plugin.require_signature);
        assert!(cfg.plugin.enable_seccomp);
        assert!(cfg.plugin.enable_quota);
        // v0.28.0 Task 11：默认不允许跳过签名验证
        assert!(!cfg.plugin.allow_skip_signature);

        assert_eq!(cfg.quota.default_cpu_percent, 50);
        assert_eq!(cfg.quota.default_memory_mb, 256);

        assert_eq!(
            cfg.sandbox.allowed_paths,
            vec![
                PathBuf::from("/etc/eneros/plugins"),
                PathBuf::from("/var/lib/eneros/plugins"),
            ]
        );
        assert_eq!(
            cfg.sandbox.denied_paths,
            vec![
                PathBuf::from("/etc/shadow"),
                PathBuf::from("/etc/eneros/keys"),
                PathBuf::from("/root"),
            ]
        );
        assert!(cfg.sandbox.allowed_network.is_empty());
    }

    /// 完整配置加载后字段应与输入一致
    #[test]
    fn test_load_full() {
        let toml_str = r#"
[plugin]
plugin_dir = "/opt/plugins"
trusted_keys_dir = "/opt/keys"
require_signature = false
enable_seccomp = false
enable_quota = false

[quota]
default_cpu_percent = 75
default_memory_mb = 512

[sandbox]
allowed_paths = ["/data"]
denied_paths = ["/secret"]
allowed_network = ["127.0.0.1/32", "10.0.0.0/8:502"]
"#;
        let cfg = PluginConfig::load_from_str(toml_str).unwrap();
        assert_eq!(cfg.load_mode, LoadMode::Daemon);
        assert_eq!(cfg.plugin.plugin_dir, PathBuf::from("/opt/plugins"));
        assert_eq!(cfg.plugin.trusted_keys_dir, PathBuf::from("/opt/keys"));
        assert!(!cfg.plugin.require_signature);
        assert!(!cfg.plugin.enable_seccomp);
        assert!(!cfg.plugin.enable_quota);

        assert_eq!(cfg.quota.default_cpu_percent, 75);
        assert_eq!(cfg.quota.default_memory_mb, 512);

        assert_eq!(cfg.sandbox.allowed_paths, vec![PathBuf::from("/data")]);
        assert_eq!(cfg.sandbox.denied_paths, vec![PathBuf::from("/secret")]);
        assert_eq!(
            cfg.sandbox.allowed_network,
            vec!["127.0.0.1/32".to_string(), "10.0.0.0/8:502".to_string()]
        );
    }

    /// 部分加载：仅指定部分字段，其余使用 serde 默认值
    #[test]
    fn test_load_partial() {
        let toml_str = r#"
[plugin]
plugin_dir = "/custom/plugins"
require_signature = false
"#;
        let cfg = PluginConfig::load_from_str(toml_str).unwrap();
        // 已指定的字段
        assert_eq!(cfg.plugin.plugin_dir, PathBuf::from("/custom/plugins"));
        assert!(!cfg.plugin.require_signature);
        // 未指定的字段使用默认值
        assert_eq!(cfg.plugin.trusted_keys_dir, PathBuf::from("/etc/eneros/keys"));
        assert!(cfg.plugin.enable_seccomp);
        assert!(cfg.plugin.enable_quota);
        // quota 与 sandbox 段未出现，使用段级默认
        assert_eq!(cfg.quota.default_cpu_percent, 50);
        assert_eq!(cfg.quota.default_memory_mb, 256);
        assert_eq!(cfg.sandbox.allowed_paths.len(), 2);
        assert_eq!(cfg.sandbox.denied_paths.len(), 3);
    }

    /// TOML 中 `load_mode = "inline"` 应正确反序列化为 `LoadMode::Inline`
    #[test]
    fn test_load_mode_inline() {
        let toml_str = r#"
load_mode = "inline"
"#;
        let cfg = PluginConfig::load_from_str(toml_str).unwrap();
        assert_eq!(cfg.load_mode, LoadMode::Inline);
    }

    /// 空配置应解析成功并返回全部默认值
    #[test]
    fn test_load_empty() {
        let cfg = PluginConfig::load_from_str("").unwrap();
        assert_eq!(cfg.load_mode, LoadMode::Daemon);
        assert_eq!(cfg.plugin.plugin_dir, PathBuf::from("/var/lib/eneros/plugins"));
        assert!(cfg.plugin.require_signature);
        assert_eq!(cfg.quota.default_cpu_percent, 50);
        assert_eq!(cfg.quota.default_memory_mb, 256);
        assert_eq!(cfg.sandbox.denied_paths.len(), 3);
        assert!(cfg.sandbox.allowed_network.is_empty());
    }

    /// 从临时文件加载配置
    #[test]
    fn test_load_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plugin.toml");
        let toml_str = r#"
[plugin]
plugin_dir = "/file/plugins"
require_signature = false

[quota]
default_memory_mb = 1024
"#;
        std::fs::write(&path, toml_str).unwrap();
        let cfg = PluginConfig::load_from_file(&path).unwrap();
        assert_eq!(cfg.plugin.plugin_dir, PathBuf::from("/file/plugins"));
        assert!(!cfg.plugin.require_signature);
        assert_eq!(cfg.quota.default_memory_mb, 1024);
        // 未指定字段使用默认值
        assert_eq!(cfg.quota.default_cpu_percent, 50);
    }

    /// 从不存在的文件加载应返回 IO 错误
    #[test]
    fn test_load_from_file_missing() {
        let path = Path::new("/nonexistent/path/plugin.toml");
        let result = PluginConfig::load_from_file(path);
        assert!(result.is_err());
        assert!(matches!(result, Err(PluginError::Io(_))));
    }

    /// 无效 TOML 应返回 InvalidManifest 错误
    #[test]
    fn test_invalid_toml() {
        let result = PluginConfig::load_from_str("not valid toml = = ");
        assert!(result.is_err());
        assert!(matches!(result, Err(PluginError::InvalidManifest(_))));
    }

    /// 类型不匹配应返回 InvalidManifest 错误
    #[test]
    fn test_invalid_field_type() {
        let toml_str = r#"
[plugin]
require_signature = "not_a_bool"
"#;
        let result = PluginConfig::load_from_str(toml_str);
        assert!(result.is_err());
        assert!(matches!(result, Err(PluginError::InvalidManifest(_))));
    }

    /// 序列化-反序列化往返应保持一致
    #[test]
    fn test_serialize_roundtrip() {
        let cfg = PluginConfig::default();
        let serialized = toml::to_string(&cfg).unwrap();
        let deserialized = PluginConfig::load_from_str(&serialized).unwrap();
        assert_eq!(cfg.load_mode, LoadMode::Daemon);
        assert_eq!(cfg.load_mode, deserialized.load_mode);
        assert_eq!(cfg.plugin.plugin_dir, deserialized.plugin.plugin_dir);
        assert_eq!(
            cfg.plugin.trusted_keys_dir,
            deserialized.plugin.trusted_keys_dir
        );
        assert_eq!(
            cfg.plugin.require_signature,
            deserialized.plugin.require_signature
        );
        assert_eq!(
            cfg.quota.default_cpu_percent,
            deserialized.quota.default_cpu_percent
        );
        assert_eq!(
            cfg.quota.default_memory_mb,
            deserialized.quota.default_memory_mb
        );
        assert_eq!(
            cfg.sandbox.allowed_paths,
            deserialized.sandbox.allowed_paths
        );
        assert_eq!(
            cfg.sandbox.denied_paths,
            deserialized.sandbox.denied_paths
        );
    }

    /// cpu_percent=0 应返回错误
    #[test]
    fn test_validate_cpu_percent_zero() {
        let toml_str = r#"
[quota]
default_cpu_percent = 0
"#;
        let result = PluginConfig::load_from_str(toml_str);
        assert!(result.is_err());
        assert!(matches!(result, Err(PluginError::InvalidManifest(_))));
        assert!(result.unwrap_err().to_string().contains("default_cpu_percent"));
    }

    /// cpu_percent=200 应返回错误
    #[test]
    fn test_validate_cpu_percent_too_high() {
        let toml_str = r#"
[quota]
default_cpu_percent = 200
"#;
        let result = PluginConfig::load_from_str(toml_str);
        assert!(result.is_err());
        assert!(matches!(result, Err(PluginError::InvalidManifest(_))));
    }

    /// memory_mb=0 应返回错误
    #[test]
    fn test_validate_memory_mb_zero() {
        let toml_str = r#"
[quota]
default_memory_mb = 0
"#;
        let result = PluginConfig::load_from_str(toml_str);
        assert!(result.is_err());
        assert!(matches!(result, Err(PluginError::InvalidManifest(_))));
        assert!(result.unwrap_err().to_string().contains("default_memory_mb"));
    }

    /// 合法边界值 cpu_percent=1 和 cpu_percent=100 应通过
    #[test]
    fn test_validate_cpu_percent_boundaries() {
        let toml_str = r#"
[quota]
default_cpu_percent = 1
default_memory_mb = 1
"#;
        let cfg = PluginConfig::load_from_str(toml_str).unwrap();
        assert_eq!(cfg.quota.default_cpu_percent, 1);

        let toml_str = r#"
[quota]
default_cpu_percent = 100
default_memory_mb = 1
"#;
        let cfg = PluginConfig::load_from_str(toml_str).unwrap();
        assert_eq!(cfg.quota.default_cpu_percent, 100);
    }
}

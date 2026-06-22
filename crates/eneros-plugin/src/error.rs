//! 插件错误类型定义

use thiserror::Error;

/// 插件操作错误
#[derive(Error, Debug)]
pub enum PluginError {
    /// 动态库加载失败
    #[error("plugin load failed: {0}")]
    LoadFailed(String),

    /// 签名文件缺失
    #[error("plugin signature missing")]
    SignatureMissing,

    /// 签名验证失败
    #[error("plugin signature invalid: {0}")]
    SignatureInvalid(String),

    /// 不可信签名者
    #[error("untrusted signer: {0}")]
    UntrustedSigner(String),

    /// 版本不兼容
    #[error("plugin {plugin} incompatible with current API version {current}")]
    IncompatibleVersion {
        /// 插件声明的 API 版本
        plugin: String,
        /// 当前系统 API 版本
        current: String,
    },

    /// 依赖插件缺失
    #[error("dependency missing: {0}")]
    DependencyMissing(String),

    /// 插件已加载
    #[error("plugin already loaded: {0}")]
    AlreadyLoaded(String),

    /// 插件未加载
    #[error("plugin not loaded: {0}")]
    NotLoaded(String),

    /// 初始化失败
    #[error("plugin init failed: {0}")]
    InitFailed(String),

    /// 启动失败
    #[error("plugin start failed: {0}")]
    StartFailed(String),

    /// 停止失败
    #[error("plugin stop failed: {0}")]
    StopFailed(String),

    /// 沙箱应用失败
    #[error("sandbox failed: {0}")]
    SandboxFailed(String),

    /// 插件崩溃
    #[error("plugin crashed: {0}")]
    Crashed(String),

    /// 平台不支持
    #[error("unsupported: {0}")]
    Unsupported(String),

    /// IO 错误
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// 序列化错误
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// 清单无效
    #[error("invalid manifest: {0}")]
    InvalidManifest(String),

    /// 非法状态转换
    #[error("invalid state transition: {0}")]
    InvalidStateTransition(String),
}

/// 插件操作结果
pub type PluginResult<T> = std::result::Result<T, PluginError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_failed_display() {
        let e = PluginError::LoadFailed("lib not found".to_string());
        assert_eq!(e.to_string(), "plugin load failed: lib not found");
    }

    #[test]
    fn test_signature_missing_display() {
        let e = PluginError::SignatureMissing;
        assert_eq!(e.to_string(), "plugin signature missing");
    }

    #[test]
    fn test_signature_invalid_display() {
        let e = PluginError::SignatureInvalid("bad sig".to_string());
        assert_eq!(e.to_string(), "plugin signature invalid: bad sig");
    }

    #[test]
    fn test_untrusted_signer_display() {
        let e = PluginError::UntrustedSigner("unknown-key".to_string());
        assert_eq!(e.to_string(), "untrusted signer: unknown-key");
    }

    #[test]
    fn test_incompatible_version_display() {
        let e = PluginError::IncompatibleVersion {
            plugin: "0.26.0".to_string(),
            current: "0.27.0".to_string(),
        };
        assert_eq!(
            e.to_string(),
            "plugin 0.26.0 incompatible with current API version 0.27.0"
        );
    }

    #[test]
    fn test_dependency_missing_display() {
        let e = PluginError::DependencyMissing("core-mbus".to_string());
        assert_eq!(e.to_string(), "dependency missing: core-mbus");
    }

    #[test]
    fn test_already_loaded_display() {
        let e = PluginError::AlreadyLoaded("plugin-a".to_string());
        assert_eq!(e.to_string(), "plugin already loaded: plugin-a");
    }

    #[test]
    fn test_not_loaded_display() {
        let e = PluginError::NotLoaded("plugin-a".to_string());
        assert_eq!(e.to_string(), "plugin not loaded: plugin-a");
    }

    #[test]
    fn test_from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let e: PluginError = io_err.into();
        assert!(matches!(e, PluginError::Io(_)));
        assert!(e.to_string().contains("IO error"));
    }

    #[test]
    fn test_from_serde_error() {
        let serde_err = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
        let e: PluginError = serde_err.into();
        assert!(matches!(e, PluginError::Serialization(_)));
    }

    #[test]
    fn test_invalid_manifest_display() {
        let e = PluginError::InvalidManifest("missing name".to_string());
        assert_eq!(e.to_string(), "invalid manifest: missing name");
    }

    #[test]
    fn test_unsupported_display() {
        let e = PluginError::Unsupported("tpm on windows".to_string());
        assert_eq!(e.to_string(), "unsupported: tpm on windows");
    }

    #[test]
    fn test_invalid_state_transition_display() {
        let e = PluginError::InvalidStateTransition("Loaded -> Running".to_string());
        assert_eq!(e.to_string(), "invalid state transition: Loaded -> Running");
    }
}

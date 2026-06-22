//! EnerOS SDK 通用类型与错误处理
//!
//! 提供 SDK 层面的错误类型、结果别名与版本信息，供所有模块共享。

use std::fmt;

/// SDK 错误类型，封装开发者在 Agent/协议/插件开发中可能遇到的错误
#[derive(Debug, thiserror::Error)]
pub enum SdkError {
    /// IO 错误
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    /// 配置错误
    #[error("配置错误: {0}")]
    Config(String),
    /// IPC 通信错误
    #[error("IPC 错误: {0}")]
    Ipc(String),
    /// 插件错误
    #[error("插件错误: {0}")]
    Plugin(String),
    /// 其他错误
    #[error("其他错误: {0}")]
    Other(String),
}

/// SDK 结果类型别名
pub type SdkResult<T> = Result<T, SdkError>;

/// SDK 版本号
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SdkVersion {
    /// 主版本号
    pub major: u32,
    /// 次版本号
    pub minor: u32,
    /// 修订号
    pub patch: u32,
}

impl SdkVersion {
    /// 返回当前 SDK 版本（从 Cargo.toml 动态读取）
    ///
    /// 版本号通过 `env!("CARGO_PKG_VERSION")` 在编译时从 Cargo.toml 读取，
    /// 避免硬编码导致的版本号不一致问题。
    pub fn current() -> Self {
        // env! 宏在编译时将 Cargo.toml 的 version 字段内联为字符串字面量
        let v = env!("CARGO_PKG_VERSION");
        let mut parts = v.split('.');
        Self {
            major: parts.next().and_then(|s| s.parse().ok()).unwrap_or(0),
            minor: parts.next().and_then(|s| s.parse().ok()).unwrap_or(0),
            patch: parts.next().and_then(|s| s.parse().ok()).unwrap_or(0),
        }
    }
}

impl fmt::Display for SdkVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sdk_error_display() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "文件不存在");
        let err = SdkError::Io(io_err);
        assert!(err.to_string().contains("IO 错误"));

        let err = SdkError::Config("无效配置".to_string());
        assert_eq!(err.to_string(), "配置错误: 无效配置");

        let err = SdkError::Ipc("连接失败".to_string());
        assert_eq!(err.to_string(), "IPC 错误: 连接失败");

        let err = SdkError::Plugin("加载失败".to_string());
        assert_eq!(err.to_string(), "插件错误: 加载失败");

        let err = SdkError::Other("未知".to_string());
        assert_eq!(err.to_string(), "其他错误: 未知");
    }

    #[test]
    fn test_sdk_version_current() {
        let v = SdkVersion::current();
        // 从 Cargo.toml 动态解析版本号，避免硬编码
        let pkg_ver = env!("CARGO_PKG_VERSION");
        let mut parts = pkg_ver.split('.');
        let major: u32 = parts.next().unwrap().parse().unwrap();
        let minor: u32 = parts.next().unwrap().parse().unwrap();
        let patch: u32 = parts.next().unwrap().parse().unwrap();
        assert_eq!(v.major, major);
        assert_eq!(v.minor, minor);
        assert_eq!(v.patch, patch);
    }

    #[test]
    fn test_sdk_version_to_string() {
        let v = SdkVersion::current();
        // Display 输出应与 Cargo.toml 声明的版本号一致
        assert_eq!(v.to_string(), env!("CARGO_PKG_VERSION"));
    }
}

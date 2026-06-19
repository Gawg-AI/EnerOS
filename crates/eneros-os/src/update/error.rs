//! OTA 更新错误类型（v0.22.0）
//!
//! 所有 update 模块的函数统一返回 `Result<T, UpdateError>`。

use thiserror::Error;

/// OTA 更新错误
#[derive(Debug, Error)]
pub enum UpdateError {
    /// IO 错误（文件读写、网络下载）
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// 配置错误（无效参数、缺失字段）
    #[error("Config error: {0}")]
    Config(String),

    /// 签名验证失败（Ed25519 签名无效、密钥不匹配）
    #[error("Signature verification failed: {0}")]
    SignatureFailed(String),

    /// 哈希不匹配（文件被篡改或损坏）
    #[error("Hash mismatch for {name}: expected {expected}, got {actual}")]
    HashMismatch {
        name: String,
        expected: String,
        actual: String,
    },

    /// 不支持的平台（非 Linux 调用了 Linux 特定功能）
    #[error("Unsupported platform: this operation requires Linux")]
    UnsupportedPlatform,

    /// 更新包格式无效（tar.gz 解压失败、manifest 缺失）
    #[error("Invalid update bundle: {0}")]
    BundleInvalid(String),

    /// 槽位错误（无可用槽位、槽位状态异常）
    #[error("Slot error: {0}")]
    SlotError(String),

    /// 序列化/反序列化错误
    #[error("Serialization error: {0}")]
    Serialize(String),

    /// HTTP 下载错误
    #[error("HTTP download error: {0}")]
    HttpDownload(String),

    /// 密钥错误（密钥文件缺失、格式无效）
    #[error("Key error: {0}")]
    Key(String),
}

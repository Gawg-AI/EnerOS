//! TPM 2.0 错误类型 (workspace v0.192.0).
//!
//! 定义 TPM 操作相关的所有错误变体。

use alloc::string::String;

/// TPM 错误类型.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TpmError {
    /// TPM 芯片不存在或无法访问
    NotPresent,
    /// TPM 初始化失败
    InitFailed(String),
    /// TPM 命令响应超时
    Timeout,
    /// TPM Locality 请求超时
    LocalityTimeout,
    /// TPM 命令执行失败（含 TPM 返回码）
    CommandFailed(u32),
    /// PCR 索引无效（0~23 有效）
    InvalidPcrIndex(u32),
    /// PCR 度量值不匹配（平台完整性验证失败）
    PcrMismatch {
        /// PCR 索引
        index: u32,
        /// 期望值
        expected: [u8; 32],
        /// 实际值
        actual: [u8; 32],
    },
    /// 密钥句柄无效
    InvalidKeyHandle(u32),
    /// 密钥创建失败
    KeyCreationFailed(String),
    /// 签名操作失败
    SignFailed(String),
    /// Seal/Unseal 操作失败
    SealFailed(String),
    /// 认证失败（授权值错误）
    AuthFailed,
    /// SPI/MMIO 总线通信错误
    BusError(String),
    /// 命令缓冲区溢出
    BufferOverflow { max: usize, actual: usize },
    /// 响应解析错误
    ResponseParseError(String),
    /// TPM 内部错误
    InternalError,
}

impl TpmError {
    /// 判断是否为可重试错误.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            TpmError::Timeout | TpmError::LocalityTimeout | TpmError::BusError(_)
        )
    }

    /// 判断是否为安全关键错误.
    pub fn is_security_critical(&self) -> bool {
        matches!(
            self,
            TpmError::PcrMismatch { .. } | TpmError::AuthFailed | TpmError::NotPresent
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_retryable() {
        assert!(TpmError::Timeout.is_retryable());
        assert!(TpmError::LocalityTimeout.is_retryable());
        assert!(!TpmError::NotPresent.is_retryable());
        assert!(!TpmError::AuthFailed.is_retryable());
    }

    #[test]
    fn test_error_security_critical() {
        assert!(TpmError::AuthFailed.is_security_critical());
        assert!(TpmError::NotPresent.is_security_critical());
        assert!(!TpmError::Timeout.is_security_critical());
    }

    #[test]
    fn test_pcr_mismatch() {
        let err = TpmError::PcrMismatch {
            index: 0,
            expected: [0xAA; 32],
            actual: [0xBB; 32],
        };
        assert!(err.is_security_critical());
        assert!(!err.is_retryable());
    }
}

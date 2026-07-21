//! EnerOS Crypto 错误类型.
//!
//! 所有密码学操作返回 `Result<T, CryptoError>`，错误变体涵盖：
//! - 输入长度/格式错误
//! - 数学运算错误（模逆不存在、点不在曲线等）
//! - 验证失败（签名/标签/填充）
//! - 随机数生成错误

use alloc::string::String;

/// 密码学错误类型（13 变体）.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CryptoError {
    /// 输入长度无效（期望长度，实际长度）
    InvalidLength { expected: usize, actual: usize },
    /// 输入数据格式无效（含描述）
    InvalidInput(String),
    /// 密钥长度无效（期望长度，实际长度）
    InvalidKeyLength { expected: usize, actual: usize },
    /// IV/Nonce 长度无效
    InvalidNonceLength { expected: usize, actual: usize },
    /// 模逆运算失败（输入与模数不互素）
    ModInverseFailed,
    /// 点不在椭圆曲线上
    PointNotOnCurve,
    /// 点编码/解码错误
    InvalidPointEncoding,
    /// 标量超出范围 [1, n-1]
    ScalarOutOfRange,
    /// 签名验证失败
    SignatureInvalid,
    /// GCM 认证标签验证失败
    TagMismatch,
    /// PKCS#7 填充无效
    InvalidPadding,
    /// 随机数生成失败（熵源不可用等）
    RngFailed,
    /// 内部状态错误（不应发生）
    InternalError,
}

impl CryptoError {
    /// 判断是否为安全关键错误（涉及密钥/签名/标签验证失败）.
    ///
    /// 安全关键错误通常意味着潜在的攻击或数据篡改，调用方应记录并采取防御措施。
    pub fn is_security_critical(&self) -> bool {
        matches!(
            self,
            CryptoError::SignatureInvalid
                | CryptoError::TagMismatch
                | CryptoError::InvalidPadding
                | CryptoError::PointNotOnCurve
                | CryptoError::InvalidPointEncoding
                | CryptoError::ScalarOutOfRange
                | CryptoError::ModInverseFailed
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_variants() {
        let e1 = CryptoError::InvalidLength {
            expected: 32,
            actual: 16,
        };
        assert_eq!(
            format!("{:?}", e1),
            "InvalidLength { expected: 32, actual: 16 }"
        );
        assert!(!e1.is_security_critical());

        let e2 = CryptoError::SignatureInvalid;
        assert!(e2.is_security_critical());

        let e3 = CryptoError::TagMismatch;
        assert!(e3.is_security_critical());

        let e4 = CryptoError::InvalidPadding;
        assert!(e4.is_security_critical());

        let e5 = CryptoError::PointNotOnCurve;
        assert!(e5.is_security_critical());

        let e6 = CryptoError::InvalidPointEncoding;
        assert!(e6.is_security_critical());

        let e7 = CryptoError::ScalarOutOfRange;
        assert!(e7.is_security_critical());

        let e8 = CryptoError::ModInverseFailed;
        assert!(e8.is_security_critical());

        // 非安全关键
        assert!(!CryptoError::InvalidInput("test".into()).is_security_critical());
        assert!(!CryptoError::InvalidKeyLength {
            expected: 16,
            actual: 32
        }
        .is_security_critical());
        assert!(!CryptoError::InvalidNonceLength {
            expected: 12,
            actual: 16
        }
        .is_security_critical());
        assert!(!CryptoError::RngFailed.is_security_critical());
        assert!(!CryptoError::InternalError.is_security_critical());
    }

    #[test]
    fn test_error_clone_eq() {
        let e1 = CryptoError::InvalidLength {
            expected: 32,
            actual: 16,
        };
        let e2 = e1.clone();
        assert_eq!(e1, e2);
    }
}

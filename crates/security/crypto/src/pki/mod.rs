//! EnerOS PKI (公钥基础设施) 证书基础模块 (v0.32.0).
//!
//! 提供 X.509 证书解析、验证与链式信任校验的基础错误类型定义。
//! 基于 v0.31.0 国密 SM2/SM3 算法栈，支持国密证书体系。
//!
//! # 模块概述
//! - [`PkiError`]：PKI 操作错误类型（13 变体），涵盖 DER/PEM 解析、
//!   签名验证、证书有效期/吊销/信任链/密钥用法等错误场景。
//! - 后续 Task 将逐步添加 ASN.1 编解码、X.509 证书结构与解析器。
//!
//! # no_std 合规
//! no_std 由 crate 根 (`lib.rs` 的 `#![cfg_attr(not(test), no_std)]`) 继承，
//! 本模块通过 `extern crate alloc` 引入堆分配支持，使用 `alloc::string::String`，不使用 `std::*`。
//!
//! # 参考
//! - GB/T 32918 信息安全技术 SM2 椭圆曲线公钥密码算法
//! - GB/T 35275 信息安全技术 SM2 密码算法加密签名消息语法规范
//! - RFC 5280 Internet X.509 Public Key Infrastructure Certificate and CRL Profile

extern crate alloc;

pub mod asn1;
pub mod builder;
pub mod ca;
pub mod crl;
pub mod parser;
pub mod verify;
pub mod x509;

/// PKI 错误类型（13 变体）.
///
/// 涵盖证书解析、签名验证、信任链校验、密钥用法检查等 PKI 操作中
/// 可能出现的所有错误场景。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PkiError {
    /// DER 编码格式无效
    InvalidDerFormat,
    /// PEM 编码格式无效
    InvalidPemFormat,
    /// 不支持的算法
    UnsupportedAlgorithm,
    /// 签名验证失败
    SignatureInvalid,
    /// 证书已过期（not_after 为证书有效期截止时间，Unix 时间戳，秒）
    CertExpired { not_after: u64 },
    /// 证书尚未生效（not_before 为证书有效期起始时间，Unix 时间戳，秒）
    CertNotYetValid { not_before: u64 },
    /// 证书已被吊销（serial 为证书序列号的十六进制字符串）
    CertRevoked { serial: alloc::string::String },
    /// 不可信的根证书
    UntrustedRoot,
    /// 证书链过长（超出最大深度限制）
    ChainTooLong,
    /// 未找到颁发者证书
    NoIssuerFound,
    /// 密钥用法无效（证书密钥用法扩展不符合预期）
    InvalidKeyUsage,
    /// CRL（证书吊销列表）错误（含描述）
    CrlError(alloc::string::String),
    /// ASN.1 编解码错误（含描述）
    Asn1Error(alloc::string::String),
}

#[cfg(test)]
mod tests {
    use alloc::string::String;

    use super::*;

    #[test]
    fn test_pki_error_unit_variants() {
        // 无字段变体（8 个）
        let e1 = PkiError::InvalidDerFormat;
        assert_eq!(format!("{:?}", e1), "InvalidDerFormat");

        let e2 = PkiError::InvalidPemFormat;
        assert_eq!(format!("{:?}", e2), "InvalidPemFormat");

        let e3 = PkiError::UnsupportedAlgorithm;
        assert_eq!(format!("{:?}", e3), "UnsupportedAlgorithm");

        let e4 = PkiError::SignatureInvalid;
        assert_eq!(format!("{:?}", e4), "SignatureInvalid");

        let e5 = PkiError::UntrustedRoot;
        assert_eq!(format!("{:?}", e5), "UntrustedRoot");

        let e6 = PkiError::ChainTooLong;
        assert_eq!(format!("{:?}", e6), "ChainTooLong");

        let e7 = PkiError::NoIssuerFound;
        assert_eq!(format!("{:?}", e7), "NoIssuerFound");

        let e8 = PkiError::InvalidKeyUsage;
        assert_eq!(format!("{:?}", e8), "InvalidKeyUsage");
    }

    #[test]
    fn test_pki_error_struct_variants() {
        // 带命名字段变体
        let e1 = PkiError::CertExpired {
            not_after: 1700000000,
        };
        assert_eq!(format!("{:?}", e1), "CertExpired { not_after: 1700000000 }");

        let e2 = PkiError::CertNotYetValid {
            not_before: 1600000000,
        };
        assert_eq!(
            format!("{:?}", e2),
            "CertNotYetValid { not_before: 1600000000 }"
        );

        let e3 = PkiError::CertRevoked {
            serial: String::from("0A1B2C"),
        };
        assert_eq!(format!("{:?}", e3), "CertRevoked { serial: \"0A1B2C\" }");
    }

    #[test]
    fn test_pki_error_tuple_variants() {
        // 元组变体
        let e1 = PkiError::CrlError(String::from("crl fetch failed"));
        assert_eq!(format!("{:?}", e1), "CrlError(\"crl fetch failed\")");

        let e2 = PkiError::Asn1Error(String::from("bad tag"));
        assert_eq!(format!("{:?}", e2), "Asn1Error(\"bad tag\")");
    }

    #[test]
    fn test_pki_error_clone_eq() {
        let e1 = PkiError::CertExpired { not_after: 100 };
        let e2 = e1.clone();
        assert_eq!(e1, e2);

        let e3 = PkiError::Asn1Error(String::from("err"));
        let e4 = e3.clone();
        assert_eq!(e3, e4);

        // 不同变体不相等
        assert_ne!(e1, e3);
        assert_ne!(PkiError::InvalidDerFormat, PkiError::InvalidPemFormat);
    }

    #[test]
    fn test_pki_error_variant_count() {
        // 确保 13 个变体全部存在且可构造
        let errors: [PkiError; 13] = [
            PkiError::InvalidDerFormat,
            PkiError::InvalidPemFormat,
            PkiError::UnsupportedAlgorithm,
            PkiError::SignatureInvalid,
            PkiError::CertExpired { not_after: 0 },
            PkiError::CertNotYetValid { not_before: 0 },
            PkiError::CertRevoked {
                serial: String::new(),
            },
            PkiError::UntrustedRoot,
            PkiError::ChainTooLong,
            PkiError::NoIssuerFound,
            PkiError::InvalidKeyUsage,
            PkiError::CrlError(String::new()),
            PkiError::Asn1Error(String::new()),
        ];

        // 验证各无字段变体互不相等
        assert_eq!(errors[0], PkiError::InvalidDerFormat);
        assert_ne!(errors[0], errors[1]);
        assert_ne!(errors[1], errors[2]);
        assert_ne!(errors[2], errors[3]);
        assert_ne!(errors[3], errors[7]);
    }
}

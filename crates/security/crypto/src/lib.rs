//! EnerOS 国密算法库 + PKI 证书基础 (v0.32.0).
//!
//! 提供 GB/T 32905-2016 (SM3)、GB/T 32907-2016 (SM4)、GB/T 32918.1~5-2017 (SM2)
//! 的纯 Rust 实现，以及基于 SM3 的 CSRNG (NIST SP 800-90A 风格)。
//!
//! # 国标引用
//! - GB/T 32905-2016 信息安全技术 SM3 密码杂凑算法
//! - GB/T 32907-2016 信息安全技术 SM4 分组密码算法
//! - GB/T 32918.1-2017 信息安全技术 SM2 椭圆曲线公钥密码算法 第1部分：总则
//! - GB/T 32918.2-2017 信息安全技术 SM2 椭圆曲线公钥密码算法 第2部分：数字签名算法
//! - GB/T 32918.4-2016 信息安全技术 SM2 椭圆曲线公钥密码算法 第4部分：公钥加密算法
//! - GB/T 32918.5-2017 信息安全技术 SM2 椭圆曲线公钥密码算法 第5部分：参数定义
//! - GB/T 35275 信息安全技术 SM2 密码算法加密签名消息语法规范
//! - RFC 5280 Internet X.509 Public Key Infrastructure Certificate and CRL Profile
//!
//! # 架构
//! ```text
//! ┌──────────────────────────────────────────────────────┐
//! │  eneros-crypto (v0.32.0)                             │
//! │  ┌──────────┐  ┌──────────┐  ┌────────────────────┐  │
//! │  │ bigint   │  │ sm3      │  │ sm4                │  │
//! │  │ (U256)   │  │ (Hash)   │  │ ┌────┐ ┌─────┐     │  │
//! │  └────┬─────┘  └────┬─────┘  │ │cbc │ │gcm  │     │  │
//! │       │             │        │ └────┘ └─────┘     │  │
//! │       │       ┌─────┴─────┐  └────────────────────┘  │
//! │       │       │ rng       │                          │
//! │       │       │ (CsRng)   │                          │
//! │       │       └─────┬─────┘                          │
//! │       └──────┐      │                                │
//! │       ┌──────▼──────▼──────┐                         │
//! │       │ sm2                │                         │
//! │       │ ┌──────┐ ┌──────┐  │                         │
//! │       │ │sign  │ │encrypt│  │                        │
//! │       │ └──────┘ └──────┘  │                         │
//! │       │ │keypair│          │                         │
//! │       │ └──────┘           │                         │
//! │       └────────────────────┘                         │
//! │  ┌─────────────┐  ┌──────────────┐                   │
//! │  │error        │  │constant_time │                   │
//! │  │(CryptoError)│  │(ct_eq/zero)  │                   │
//! │  └─────────────┘  └──────────────┘                   │
//! │  ┌──────────────────────────────────────────────────┐  │
//! │  │ pki (v0.32.0)                                     │  │
//! │  │ ┌────┐ ┌─────┐ ┌──────┐ ┌──────┐ ┌────┐ ┌────┐  │  │
//! │  │ │asn1│ │x509 │ │parser│ │builder│ │verify│ │ca│ │  │
//! │  │ └────┘ └─────┘ └──────┘ └──────┘ └──────┘ └───┘  │  │
//! │  │ ┌────┐                                         │  │
//! │  │ │crl │                                         │  │
//! │  │ └────┘                                         │  │
//! │  └──────────────────────────────────────────────────┘  │
//! └──────────────────────────────────────────────────────┘
//! ```
//!
//! # 使用示例
//! ```
//! use eneros_crypto::{sm3, sm4::Sm4, sm4::cbc::Sm4Cbc};
//!
//! // SM3 哈希
//! let hash = sm3::hash(b"hello");
//! assert_eq!(hash.len(), 32);
//!
//! // SM4-CBC 加解密
//! let key = [0x42u8; 16];
//! let iv = [0x00u8; 16];
//! let cipher = Sm4Cbc::new(&key, &iv);
//! let plaintext = b"Hello, SM4-CBC!";
//! let ciphertext = cipher.encrypt(plaintext);
//! let decrypted = cipher.decrypt(&ciphertext).unwrap();
//! assert_eq!(decrypted, plaintext);
//! ```
//!
//! ## PKI 自签名证书示例
//! ```
//! use eneros_crypto::{
//!     pki::builder::build_self_signed,
//!     pki::x509::{CertRequest, DistinguishedName, SubjectPublicKey},
//!     pki::parser::to_pem,
//!     rng::CsRng,
//!     sm2::Sm2KeyPair,
//! };
//!
//! // 1. 生成 SM2 密钥对（生产环境应接入硬件 TRNG）
//! let mut rng = CsRng::new();
//! let kp = Sm2KeyPair::generate(&mut rng).expect("keypair gen");
//!
//! // 2. 构造 CSR（subject + 公钥）
//! let subject = DistinguishedName::new("EnerOS Test Root CA")
//!     .with_o("EnerOS")
//!     .with_c("CN");
//! let req = CertRequest::new(subject, SubjectPublicKey::Sm2(kp.public_key.clone()));
//!
//! // 3. 自签名颁发根证书（now 为 Unix 时间戳）
//! let now: u64 = 1_700_000_000;
//! let cert = build_self_signed(
//!     &req,
//!     &kp.private_key,
//!     &kp.public_key,
//!     now,
//!     &mut rng,
//! ).expect("build self-signed");
//!
//! // 4. 序列化为 PEM
//! let pem = to_pem(&cert).expect("to pem");
//! assert!(pem.starts_with("-----BEGIN CERTIFICATE-----"));
//! ```
//!
//! # no_std 合规
//! 所有代码 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 使用 `alloc::vec::Vec`，不使用 `std::*`。
//!
//! # 偏差声明
//! 1. **CSRNG 熵源**：当前实现使用固定种子（仅用于测试），生产环境需接入硬件 TRNG。
//! 2. **性能基准**：no_std 环境无系统时钟，性能基准用循环计数占位，实机验证延后。
//! 3. **NIST 测试**：未集成 NIST CAVP 测试向量，仅使用国标 KAT。
//! 4. **SM4 工作模式**：仅实现 ECB/CBC/GCM 三种模式，CTR/CFB/OFB 后续按需添加。
//! 5. **SM2 用户 ID**：默认 `"1234567812345678"`（国标默认），可通过 `Sm2Signer::with_user_id` 配置。
//! 6. **SM2 压缩点格式**：v0.31.0 仅支持未压缩格式 (04 前缀)，压缩格式 (02/03) 后续按需添加。
//! 7. **PKI 时间**：no_std 无系统时钟，证书验证接收外部 now: u64（Unix 时间戳）。
//! 8. **PKI RSA**：仅保留 RSA 枚举变体，编解码返回 UnsupportedAlgorithm（国密优先）。
//! 9. **PKI CRL**：简化版 CRL 不含签名（签名验证由 verify 模块负责）。
//! 10. **PKI ASN.1**：自研 ASN.1 DER 编解码，不引入外部 crate（零依赖）。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod bigint;
pub mod constant_time;
pub mod error;
pub mod pki;
pub mod rng;
pub mod sm2;
pub mod sm3;
pub mod sm4;

// Re-export key types for convenience.
pub use bigint::U256;
pub use constant_time::{ct_eq, ct_zeroize};
pub use error::CryptoError;
// PKI (v0.32.0) re-exports
pub use pki::asn1::{Asn1Error, DerReader, DerWriter};
pub use pki::builder::{build_certificate, build_self_signed, build_tbs, sign_tbs};
pub use pki::ca::CaIssuer;
pub use pki::crl::{Crl, RevocationReason, RevokedCert};
pub use pki::parser::{base64_decode, base64_encode, parse_der, parse_pem, to_der, to_pem};
pub use pki::verify::{verify_signature, CertVerifier};
pub use pki::x509::{
    CertRequest, DistinguishedName, ExtKeyUsage, Extension, KeyUsage, SignatureAlgorithm,
    SubjectPublicKey, X509Certificate,
};
pub use pki::PkiError;
pub use rng::CsRng;
pub use sm2::{
    sm2_sign, sm2_verify, EcPoint, Sm2KeyPair, Sm2PrivateKey, Sm2PublicKey, Sm2Signature, Sm2Signer,
};
pub use sm3::{
    hash as sm3_hash,
    hmac::{hmac_sm3, Sm3Hmac},
    Sm3Hasher,
};
pub use sm4::Sm4;

/// Crate version string.
pub const VERSION: &str = "0.32.0";

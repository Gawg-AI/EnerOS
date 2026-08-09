//! eneros-crypto-gmssl — 国密 vetted binding（ADR-022）。
//!
//! OpenSSL 3 libcrypto 的 SM2/SM3/SM4 封装，供 **std 面**（host 工具链、
//! 富 OS 服务、互操作验证对端）使用；`eneros-crypto`（no_std 手卷）是
//! seL4 EL0 目标侧唯一可链接实现，其正确性由本 crate 的 KAT 交叉验证。
//!
//! # 算法来源
//! - SM3：`EVP_Digest(EVP_sm3)`（GB/T 32905-2016）
//! - SM2：`EVP_PKEY_sign/verify`（EVP_PKEY_SM2，distid 显式 = 国标默认
//!   `"1234567812345678"`，与手卷 `Sm2Signer::new()` 一致）
//! - SM4：`EVP_Cipher`（SM4-ECB，单块无填充，与手卷 `Sm4` 对齐）
//!
//! # no_std / 平台约束
//! 本 crate 依赖系统 libcrypto（`libssl-dev >= 3.0`），仅编译用于 std 平台。
//! no_std 目标（aarch64-unknown-none）不可链接——见 ADR-022 双实现裁决。

pub mod der;
pub mod ffi;

use core::fmt;

/// Binding 层错误（FFI 失败 / 参数非法）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GmsslError(String);

impl fmt::Display for GmsslError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "crypto-gmssl: {}", self.0)
    }
}

impl std::error::Error for GmsslError {}

/// SM3 哈希（GB/T 32905-2016）。
pub mod sm3 {
    /// 计算 32 字节 SM3 摘要。
    pub fn hash(data: &[u8]) -> [u8; 32] {
        crate::ffi::sm3_digest(data)
    }
}

/// SM2 数字签名（GB/T 32918.2-2017，distid = 国标默认）。
pub mod sm2 {
    /// SM2 签名结果：`r||s`（各 32 字节，大端）。
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Signature {
        /// 签名分量 r（大端 32 字节）。
        pub r: [u8; 32],
        /// 签名分量 s（大端 32 字节）。
        pub s: [u8; 32],
    }

    impl Signature {
        /// DER 编码（ECDSA-Sig-Value）。
        pub fn to_der(&self) -> Result<Vec<u8>, crate::GmsslError> {
            crate::der::rs_to_der(&self.r, &self.s).map_err(|e| crate::GmsslError(format!("{e:?}")))
        }

        /// 从 DER 解析。
        pub fn from_der(der: &[u8]) -> Result<Self, crate::GmsslError> {
            let (r, s) =
                crate::der::der_to_rs(der).map_err(|e| crate::GmsslError(format!("{e:?}")))?;
            Ok(Self { r, s })
        }

        /// 64 字节 `r||s`。
        pub fn to_bytes(&self) -> [u8; 64] {
            let mut out = [0u8; 64];
            out[..32].copy_from_slice(&self.r);
            out[32..].copy_from_slice(&self.s);
            out
        }
    }

    /// SM2 公钥：65 字节未压缩点（`04||X||Y`）。
    pub type PublicKey = [u8; 65];

    /// 从 32 字节私钥标量 `d` + 未压缩公钥点签名消息 → DER 签名。
    ///
    /// 公钥点必须与 `d` 匹配（SM2 keymgmt 不从 priv 自动派生 pub，
    /// Z 值计算需要公钥）。
    pub fn sign_der(
        message: &[u8],
        d: &[u8; 32],
        public_key: &PublicKey,
    ) -> Result<Vec<u8>, crate::GmsslError> {
        crate::ffi::sm2_sign(message, d, public_key).map_err(crate::GmsslError)
    }

    /// 从 32 字节私钥标量 `d` + 未压缩公钥点签名消息 → `Signature`（r||s）。
    pub fn sign(
        message: &[u8],
        d: &[u8; 32],
        public_key: &PublicKey,
    ) -> Result<Signature, crate::GmsslError> {
        let der = sign_der(message, d, public_key)?;
        Signature::from_der(&der)
    }

    /// 验签：消息 + 未压缩公钥点 + DER 签名 → 是否有效。
    pub fn verify_der(
        message: &[u8],
        public_key: &PublicKey,
        sig_der: &[u8],
    ) -> Result<bool, crate::GmsslError> {
        crate::ffi::sm2_verify(message, public_key, sig_der).map_err(crate::GmsslError)
    }

    /// 验签：消息 + 未压缩公钥点 + `Signature`（r||s）。
    pub fn verify(
        message: &[u8],
        public_key: &PublicKey,
        signature: &Signature,
    ) -> Result<bool, crate::GmsslError> {
        let der = signature.to_der()?;
        verify_der(message, public_key, &der)
    }
}

/// SM4（GB/T 32907-2016），ECB 单块（16 字节，无填充）。
pub mod sm4 {
    /// SM4-ECB 加密一个 16 字节块。
    pub fn encrypt_block(key: &[u8; 16], block: &[u8; 16]) -> Result<[u8; 16], crate::GmsslError> {
        crate::ffi::sm4_ecb_block(key, block, true).map_err(crate::GmsslError)
    }

    /// SM4-ECB 解密一个 16 字节块。
    pub fn decrypt_block(key: &[u8; 16], block: &[u8; 16]) -> Result<[u8; 16], crate::GmsslError> {
        crate::ffi::sm4_ecb_block(key, block, false).map_err(crate::GmsslError)
    }
}

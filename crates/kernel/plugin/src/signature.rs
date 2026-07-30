//! 插件签名验证模块 (v0.166.0).
//!
//! 基于国密 SM2/SM3 实现插件包签名验证，确保插件来源可信、内容未被篡改。
//! 复用 `eneros-crypto` 的 SM2 验签与 SM3 哈希能力（记忆 §5.5 禁止重复造轮子）。
//!
//! SM2 验签内部自动计算 Z 值 + SM3 哈希，外部直接传入原始消息即可。

use alloc::vec::Vec;

use eneros_crypto::{sm2_verify, Sm2PublicKey, Sm2Signature};

use crate::types::{Certificate, PluginPackage};
use crate::PluginError;

/// 签名验证器 — 验证插件包签名的合法性.
pub struct SignatureVerifier {
    /// 受信任的根公钥列表（CA 公钥）.
    trusted_roots: Vec<Sm2PublicKey>,
}

impl SignatureVerifier {
    /// 创建签名验证器.
    ///
    /// `trusted_roots` 为受信任的 CA 根公钥列表，插件签名必须由其中之一的私钥签发。
    pub fn new(trusted_roots: Vec<Sm2PublicKey>) -> Self {
        Self { trusted_roots }
    }

    /// 验证插件包签名.
    ///
    /// 签名覆盖范围：manifest 规范化字节 ‖ binary（确保权限声明不可篡改）。
    ///
    /// 流程：
    /// 1. 构造签名负载（manifest + binary）
    /// 2. 解析 SM2 签名（64 字节 r‖s）
    /// 3. 从签名者证书提取公钥
    /// 4. 验证证书是否在受信任根列表中
    /// 5. SM2 验签（内部自动 SM3 哈希 + Z 值计算）
    pub fn verify_package(&self, package: &PluginPackage) -> Result<(), PluginError> {
        // 1. 构造签名负载：manifest 规范化 ‖ binary
        let signed_data = build_signed_payload(package);

        // 2. 解析签名（64 字节 = r(32) ‖ s(32)）
        let signature = self.parse_signature(&package.signature)?;

        // 3. 从证书提取公钥
        let signer_key = self.extract_public_key(&package.signer_cert)?;

        // 4. 验证签名者是否受信任
        self.verify_trust(&signer_key)?;

        // 5. SM2 验签（传入组合负载，内部自动 SM3）
        let valid = sm2_verify(&signed_data, &signature, &signer_key)
            .map_err(|_| PluginError::SignatureInvalid)?;
        if !valid {
            return Err(PluginError::SignatureInvalid);
        }

        Ok(())
    }

    /// 验证原始数据的签名（通用接口）.
    ///
    /// - `data`: 待验证数据
    /// - `sig_bytes`: 64 字节签名（r‖s）
    /// - `public_key`: 签名者公钥
    pub fn verify_data(
        &self,
        data: &[u8],
        sig_bytes: &[u8],
        public_key: &Sm2PublicKey,
    ) -> Result<bool, PluginError> {
        let signature = self.parse_signature(sig_bytes)?;
        sm2_verify(data, &signature, public_key).map_err(|_| PluginError::SignatureInvalid)
    }

    /// 解析 64 字节签名为 Sm2Signature.
    fn parse_signature(&self, sig_bytes: &[u8]) -> Result<Sm2Signature, PluginError> {
        if sig_bytes.len() != 64 {
            return Err(PluginError::InvalidSignatureFormat);
        }
        let arr: &[u8; 64] = sig_bytes
            .try_into()
            .map_err(|_| PluginError::InvalidSignatureFormat)?;
        Ok(Sm2Signature::from_bytes(arr))
    }

    /// 从证书中提取 SM2 公钥.
    fn extract_public_key(&self, cert: &Certificate) -> Result<Sm2PublicKey, PluginError> {
        if cert.public_key.len() != 65 {
            return Err(PluginError::InvalidCertificate);
        }
        // 未压缩格式：0x04 ‖ x(32) ‖ y(32)
        if cert.public_key[0] != 0x04 {
            return Err(PluginError::InvalidCertificate);
        }
        Sm2PublicKey::from_bytes(&cert.public_key).map_err(|_| PluginError::InvalidCertificate)
    }

    /// 验证公钥是否在受信任根列表中.
    fn verify_trust(&self, key: &Sm2PublicKey) -> Result<(), PluginError> {
        let key_bytes = key.point.to_bytes_uncompressed();
        for root in &self.trusted_roots {
            if root.point.to_bytes_uncompressed() == key_bytes {
                return Ok(());
            }
        }
        Err(PluginError::UntrustedSigner)
    }
}

/// 检查 SDK 版本兼容性.
///
/// 插件声明的 `min_sdk_version` 必须 ≤ 当前 SDK 版本。
/// 简化版语义化版本比较（major.minor.patch）。
pub fn is_sdk_compatible(min_sdk_version: &str) -> bool {
    let current = parse_semver(crate::types::SDK_VERSION);
    let required = parse_semver(min_sdk_version);
    match (current, required) {
        (Some(cur), Some(req)) => cur >= req,
        _ => false,
    }
}

/// 构造签名负载：manifest 规范化字节 ‖ binary.
///
/// 签名覆盖 manifest + binary，确保权限声明不可篡改。
pub fn build_signed_payload(package: &PluginPackage) -> Vec<u8> {
    let mut payload = package.manifest.canonical_bytes();
    payload.extend_from_slice(&package.binary);
    payload
}

/// 解析语义化版本号为 (major, minor, patch) 元组.
fn parse_semver(version: &str) -> Option<(u32, u32, u32)> {
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let major = parts[0].parse().ok()?;
    let minor = parts[1].parse().ok()?;
    let patch = parts[2].parse().ok()?;
    Some((major, minor, patch))
}

#[cfg(test)]
mod tests {
    use eneros_crypto::rng::CsRng;
    use eneros_crypto::sm2::{sm2_sign, Sm2KeyPair};

    use super::*;
    use crate::types::{PluginManifest, PluginType};

    fn make_test_package(keypair: &Sm2KeyPair) -> PluginPackage {
        let binary = alloc::vec![0x7f, 0x45, 0x4c, 0x46, 0x01, 0x02, 0x03, 0x04];

        let manifest = PluginManifest::new(
            alloc::string::String::from("test-plugin"),
            alloc::string::String::from("1.0.0"),
            alloc::string::String::from("tester"),
            PluginType::Generic,
            alloc::vec![],
            alloc::string::String::from("0.165.0"),
        );

        // 签名覆盖 manifest + binary
        let mut payload = manifest.canonical_bytes();
        payload.extend_from_slice(&binary);

        let mut rng = CsRng::new();
        let sig = sm2_sign(
            &payload,
            &keypair.private_key,
            &keypair.public_key,
            &mut rng,
        )
        .unwrap();
        let sig_bytes = sig.to_bytes();

        let cert = Certificate {
            subject: alloc::string::String::from("Test Signer"),
            public_key: keypair.public_key.point.to_bytes_uncompressed().to_vec(),
            not_before: 0,
            not_after: u64::MAX,
        };

        PluginPackage {
            manifest,
            binary,
            signature: sig_bytes.to_vec(),
            signer_cert: cert,
        }
    }

    #[test]
    fn test_valid_signature_verification() {
        let mut rng = CsRng::new();
        let keypair = Sm2KeyPair::generate(&mut rng).unwrap();
        let verifier = SignatureVerifier::new(alloc::vec![keypair.public_key]);
        let package = make_test_package(&keypair);
        assert!(verifier.verify_package(&package).is_ok());
    }

    #[test]
    fn test_invalid_signature_rejected() {
        let mut rng = CsRng::new();
        let keypair = Sm2KeyPair::generate(&mut rng).unwrap();
        let other_kp = Sm2KeyPair::generate(&mut rng).unwrap();

        // 用 other_kp 签名，但信任 keypair
        let verifier = SignatureVerifier::new(alloc::vec![keypair.public_key]);
        let package = make_test_package(&other_kp);
        // 签名者不在信任列表中
        assert_eq!(
            verifier.verify_package(&package).unwrap_err(),
            PluginError::UntrustedSigner
        );
    }

    #[test]
    fn test_tampered_binary_rejected() {
        let mut rng = CsRng::new();
        let keypair = Sm2KeyPair::generate(&mut rng).unwrap();
        let verifier = SignatureVerifier::new(alloc::vec![keypair.public_key]);
        let mut package = make_test_package(&keypair);
        // 篡改二进制
        package.binary[0] = 0xFF;
        assert!(verifier.verify_package(&package).is_err());
    }

    #[test]
    fn test_invalid_signature_length() {
        let mut rng = CsRng::new();
        let keypair = Sm2KeyPair::generate(&mut rng).unwrap();
        let verifier = SignatureVerifier::new(alloc::vec![keypair.public_key]);
        let mut package = make_test_package(&keypair);
        package.signature = alloc::vec![0u8; 32]; // 错误长度
        assert_eq!(
            verifier.verify_package(&package).unwrap_err(),
            PluginError::InvalidSignatureFormat
        );
    }

    #[test]
    fn test_sdk_compatibility() {
        assert!(is_sdk_compatible("0.165.0"));
        assert!(is_sdk_compatible("0.166.0"));
        assert!(!is_sdk_compatible("0.167.0"));
        assert!(!is_sdk_compatible("1.0.0"));
        assert!(!is_sdk_compatible("invalid"));
    }

    #[test]
    fn test_semver_parsing() {
        assert_eq!(parse_semver("0.166.0"), Some((0, 166, 0)));
        assert_eq!(parse_semver("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_semver("bad"), None);
        assert_eq!(parse_semver("1.2"), None);
    }
}

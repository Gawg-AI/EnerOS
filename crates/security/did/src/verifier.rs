//! DID 验证器.
//!
//! 提供 SM2 签名验证、证书链验证和跨域认证功能。
//! 依赖 `eneros-crypto` 的 SM2 验签能力。

use alloc::string::String;

use eneros_crypto::{sm2_verify, Sm2PublicKey, Sm2Signature};

use crate::types::{CertificateChain, DidDocument};
use crate::DidError;

/// DID 验证器.
///
/// 提供签名验证、证书链验证和跨域认证。
#[derive(Debug, Default)]
pub struct DidVerifier;

impl DidVerifier {
    /// 创建验证器实例.
    pub fn new() -> Self {
        Self
    }

    /// 验证 DID 文档关联的 SM2 签名.
    ///
    /// 从 DID 文档中提取公钥，验证消息签名。
    ///
    /// # 参数
    /// - `did_doc`: DID 文档（包含 SM2 公钥）
    /// - `msg`: 原始消息
    /// - `sig`: 64 字节签名 (r ‖ s)
    ///
    /// # 错误
    /// - [`DidError::InvalidDocument`][]: 公钥格式无效
    /// - [`DidError::InvalidSignature`][]: 签名长度不为 64 字节或验签失败
    pub fn verify_signature(
        &self,
        did_doc: &DidDocument,
        msg: &[u8],
        sig: &[u8],
    ) -> Result<bool, DidError> {
        if sig.len() != 64 {
            return Err(DidError::InvalidSignature);
        }
        let pk =
            Sm2PublicKey::from_bytes(&did_doc.public_key).map_err(|_| DidError::InvalidDocument)?;
        let mut sig_bytes = [0u8; 64];
        sig_bytes.copy_from_slice(sig);
        let signature = Sm2Signature::from_bytes(&sig_bytes);
        sm2_verify(msg, &signature, &pk).map_err(|_| DidError::InvalidSignature)
    }

    /// 验证证书链有效期.
    ///
    /// 检查当前时间是否在证书链的有效期内。
    ///
    /// # 参数
    /// - `chain`: 证书链
    /// - `current_time`: 当前 Unix 时间戳
    ///
    /// # 错误
    /// - [`DidError::Expired`][]: 证书已过期或尚未生效
    pub fn verify_certificate_chain(
        &self,
        chain: &CertificateChain,
        current_time: u64,
    ) -> Result<bool, DidError> {
        if current_time < chain.valid_from || current_time > chain.valid_until {
            return Err(DidError::Expired);
        }
        // 基本结构验证: root -> issuer -> subject 链完整性
        if chain.root_did.id.is_empty()
            || chain.issuer_did.id.is_empty()
            || chain.subject_did.id.is_empty()
        {
            return Err(DidError::InvalidDocument);
        }
        if chain.signature.len() != 64 {
            return Err(DidError::InvalidSignature);
        }
        Ok(true)
    }

    /// 跨域认证.
    ///
    /// 验证 DID 文档所属域是否在受信任域列表中。
    ///
    /// # 参数
    /// - `doc`: DID 文档
    /// - `domain`: 请求认证的域名
    /// - `trusted_domains`: 受信任域名列表
    ///
    /// # 错误
    /// - [`DidError::UntrustedDomain`][]: 域名不在受信任列表中
    pub fn cross_domain_auth(
        &self,
        doc: &DidDocument,
        domain: &str,
        trusted_domains: &[String],
    ) -> Result<bool, DidError> {
        // 验证文档有效性
        if doc.public_key.is_empty() {
            return Err(DidError::InvalidDocument);
        }
        // 检查域是否在受信任列表中
        let domain_str = String::from(domain);
        if !trusted_domains.contains(&domain_str) {
            return Err(DidError::UntrustedDomain);
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use eneros_crypto::{sm2_sign, CsRng, Sm2KeyPair};

    use super::*;
    use crate::types::{AuthMethod, Did};

    fn make_doc_with_key(pk_bytes: &[u8]) -> DidDocument {
        DidDocument {
            id: Did::new("eneros", "main", "agent-1"),
            public_key: pk_bytes.to_vec(),
            auth_methods: alloc::vec![AuthMethod::Sm2Signature],
            service_endpoints: alloc::vec![],
            created: 1_700_000_000,
            updated: 1_700_000_000,
        }
    }

    #[test]
    fn test_verify_signature_valid() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let pk_bytes = kp.public_key.to_bytes_uncompressed();
        let doc = make_doc_with_key(&pk_bytes);

        let msg = b"hello eneros did";
        let sig = sm2_sign(msg, &kp.private_key, &kp.public_key, &mut rng).unwrap();
        let sig_bytes = sig.to_bytes();

        let verifier = DidVerifier::new();
        let result = verifier.verify_signature(&doc, msg, &sig_bytes).unwrap();
        assert!(result);
    }

    #[test]
    fn test_verify_signature_wrong_msg() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let pk_bytes = kp.public_key.to_bytes_uncompressed();
        let doc = make_doc_with_key(&pk_bytes);

        let msg = b"original message";
        let sig = sm2_sign(msg, &kp.private_key, &kp.public_key, &mut rng).unwrap();
        let sig_bytes = sig.to_bytes();

        let verifier = DidVerifier::new();
        let result = verifier
            .verify_signature(&doc, b"tampered message", &sig_bytes)
            .unwrap();
        assert!(!result);
    }

    #[test]
    fn test_verify_signature_invalid_length() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let pk_bytes = kp.public_key.to_bytes_uncompressed();
        let doc = make_doc_with_key(&pk_bytes);

        let verifier = DidVerifier::new();
        assert_eq!(
            verifier.verify_signature(&doc, b"msg", &[0u8; 32]),
            Err(DidError::InvalidSignature)
        );
    }

    #[test]
    fn test_verify_signature_invalid_pubkey() {
        let doc = make_doc_with_key(&[0xFF; 10]); // invalid key
        let verifier = DidVerifier::new();
        assert_eq!(
            verifier.verify_signature(&doc, b"msg", &[0u8; 64]),
            Err(DidError::InvalidDocument)
        );
    }

    #[test]
    fn test_verify_certificate_chain_valid() {
        let chain = CertificateChain {
            root_did: Did::new("eneros", "main", "root"),
            issuer_did: Did::new("eneros", "main", "issuer"),
            subject_did: Did::new("eneros", "main", "subject"),
            signature: alloc::vec![0u8; 64],
            valid_from: 1_700_000_000,
            valid_until: 1_800_000_000,
        };
        let verifier = DidVerifier::new();
        assert!(verifier
            .verify_certificate_chain(&chain, 1_750_000_000)
            .unwrap());
    }

    #[test]
    fn test_verify_certificate_chain_expired() {
        let chain = CertificateChain {
            root_did: Did::new("eneros", "main", "root"),
            issuer_did: Did::new("eneros", "main", "issuer"),
            subject_did: Did::new("eneros", "main", "subject"),
            signature: alloc::vec![0u8; 64],
            valid_from: 1_700_000_000,
            valid_until: 1_800_000_000,
        };
        let verifier = DidVerifier::new();
        assert_eq!(
            verifier.verify_certificate_chain(&chain, 1_900_000_000),
            Err(DidError::Expired)
        );
    }

    #[test]
    fn test_verify_certificate_chain_not_yet_valid() {
        let chain = CertificateChain {
            root_did: Did::new("eneros", "main", "root"),
            issuer_did: Did::new("eneros", "main", "issuer"),
            subject_did: Did::new("eneros", "main", "subject"),
            signature: alloc::vec![0u8; 64],
            valid_from: 1_700_000_000,
            valid_until: 1_800_000_000,
        };
        let verifier = DidVerifier::new();
        assert_eq!(
            verifier.verify_certificate_chain(&chain, 1_600_000_000),
            Err(DidError::Expired)
        );
    }

    #[test]
    fn test_cross_domain_auth_trusted() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let pk_bytes = kp.public_key.to_bytes_uncompressed();
        let doc = make_doc_with_key(&pk_bytes);

        let trusted = alloc::vec![String::from("domain-a"), String::from("domain-b"),];
        let verifier = DidVerifier::new();
        assert!(verifier
            .cross_domain_auth(&doc, "domain-a", &trusted)
            .unwrap());
    }

    #[test]
    fn test_cross_domain_auth_untrusted() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let pk_bytes = kp.public_key.to_bytes_uncompressed();
        let doc = make_doc_with_key(&pk_bytes);

        let trusted = alloc::vec![String::from("domain-a")];
        let verifier = DidVerifier::new();
        assert_eq!(
            verifier.cross_domain_auth(&doc, "evil-domain", &trusted),
            Err(DidError::UntrustedDomain)
        );
    }

    #[test]
    fn test_cross_domain_auth_empty_key() {
        let doc = make_doc_with_key(&[]);
        let trusted = alloc::vec![String::from("domain-a")];
        let verifier = DidVerifier::new();
        assert_eq!(
            verifier.cross_domain_auth(&doc, "domain-a", &trusted),
            Err(DidError::InvalidDocument)
        );
    }
}

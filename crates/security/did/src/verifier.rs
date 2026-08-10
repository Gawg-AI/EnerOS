//! DID 验证器.
//!
//! 提供 SM2 签名验证、证书链验证和跨域认证功能。
//! 依赖 `eneros-crypto` 的 SM2 验签能力。

use alloc::string::String;

use eneros_crypto::{sm2_verify, Sm2PublicKey, Sm2Signature};

use crate::types::{CertificateChain, Did, DidDocument};
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

    /// 验证证书链（29 SE-08：补链签名验证——原实现只查有效期+长度，
    /// 全零签名通过验证（verifier.rs 测试实证），签名验证名义缺失）。
    ///
    /// # 模型声明（S8 诚实纪律）
    /// 单锚模型：链整体（root/issuer/subject 三 DID + 有效期）由 root 私钥
    /// 签发，`root_public_key` 验证 `chain.signature`；签名材料 = 链字段
    /// 定序编码 SM3。两级级联签发（issuer 独立密钥分别签发 subject）需
    /// 链内嵌 issuer 公钥与双签名结构，当前 CertificateChain 类型不支持
    /// ——不冒充两级模型。
    ///
    /// # 参数
    /// - `chain`: 证书链
    /// - `root_public_key`: 信任锚（root DID 对应）公钥
    /// - `current_time`: 当前 Unix 时间戳
    ///
    /// # 错误
    /// - [`DidError::Expired`][]: 证书已过期或尚未生效
    /// - [`DidError::InvalidDocument`][]: 链结构非法
    /// - [`DidError::InvalidSignature`][]: 签名长度非法或验签失败
    pub fn verify_certificate_chain(
        &self,
        chain: &CertificateChain,
        root_public_key: &Sm2PublicKey,
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
        // 29 SE-08：root 公钥验链签名（签名材料 = 链字段定序编码 SM3）。
        let mut digest = [0u8; 32];
        {
            let mut hasher = eneros_crypto::sm3::Sm3Hasher::new();
            let mut update = |d: &Did| {
                hasher.update(d.method.as_bytes());
                hasher.update(&[0xFF]);
                hasher.update(d.network.as_bytes());
                hasher.update(&[0xFF]);
                hasher.update(d.id.as_bytes());
                hasher.update(&[0xFF]);
            };
            update(&chain.root_did);
            update(&chain.issuer_did);
            update(&chain.subject_did);
            hasher.update(&chain.valid_from.to_le_bytes());
            hasher.update(&chain.valid_until.to_le_bytes());
            digest = hasher.finalize();
        }
        let mut sig_bytes = [0u8; 64];
        sig_bytes.copy_from_slice(&chain.signature);
        let ok = sm2_verify(&digest, &Sm2Signature::from_bytes(&sig_bytes), root_public_key)
            .map_err(|_| DidError::InvalidSignature)?;
        if !ok {
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
    use eneros_crypto::{sm2_sign, sm3_hash, CsRng, Sm2KeyPair};

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

    /// 构造 root 签名的合法证书链（29 SE-08 测试夹具）。
    fn signed_chain(root: &Sm2KeyPair, rng: &mut CsRng) -> CertificateChain {
        let chain = CertificateChain {
            root_did: Did::new("eneros", "main", "root"),
            issuer_did: Did::new("eneros", "main", "issuer"),
            subject_did: Did::new("eneros", "main", "subject"),
            signature: alloc::vec![0u8; 64],
            valid_from: 1_700_000_000,
            valid_until: 1_800_000_000,
        };
        let digest = chain_fields_digest(&chain);
        let sig = sm2_sign(&digest, &root.private_key, &root.public_key, rng).unwrap();
        CertificateChain {
            signature: sig.to_bytes().to_vec(),
            ..chain
        }
    }

    /// 与 verifier.rs 验签路径一致的链字段定序编码摘要（测试侧重算）。
    fn chain_fields_digest(chain: &CertificateChain) -> [u8; 32] {
        let mut canonical = alloc::vec::Vec::new();
        for d in [&chain.root_did, &chain.issuer_did, &chain.subject_did] {
            canonical.extend_from_slice(d.method.as_bytes());
            canonical.push(0xFF);
            canonical.extend_from_slice(d.network.as_bytes());
            canonical.push(0xFF);
            canonical.extend_from_slice(d.id.as_bytes());
            canonical.push(0xFF);
        }
        canonical.extend_from_slice(&chain.valid_from.to_le_bytes());
        canonical.extend_from_slice(&chain.valid_until.to_le_bytes());
        sm3_hash(&canonical)
    }

    /// 29 SE-08 判据正例：root 真实签名链 → Ok（修复前全零签名也通过）。
    #[test]
    fn test_verify_certificate_chain_valid() {
        let mut rng = CsRng::new();
        let root = Sm2KeyPair::generate(&mut rng).unwrap();
        let chain = signed_chain(&root, &mut rng);
        let verifier = DidVerifier::new();
        assert!(verifier
            .verify_certificate_chain(&chain, &root.public_key, 1_750_000_000)
            .unwrap());
    }

    /// 29 SE-08 判据负例：篡改 signature 任一字节 → 必 Err(InvalidSignature)
    /// （修复前全零签名通过测试——签名验证名义缺失）。
    #[test]
    fn test_verify_certificate_chain_tampered_signature() {
        let mut rng = CsRng::new();
        let root = Sm2KeyPair::generate(&mut rng).unwrap();
        let mut chain = signed_chain(&root, &mut rng);
        chain.signature[0] ^= 0xFF;
        let verifier = DidVerifier::new();
        assert_eq!(
            verifier.verify_certificate_chain(&chain, &root.public_key, 1_750_000_000),
            Err(DidError::InvalidSignature)
        );
    }

    /// 29 SE-08 负例：全零签名（修复前的'通过'输入）→ 必拒。
    #[test]
    fn test_verify_certificate_chain_all_zero_signature_rejected() {
        let mut rng = CsRng::new();
        let root = Sm2KeyPair::generate(&mut rng).unwrap();
        let mut chain = signed_chain(&root, &mut rng);
        chain.signature = alloc::vec![0u8; 64];
        let verifier = DidVerifier::new();
        assert_eq!(
            verifier.verify_certificate_chain(&chain, &root.public_key, 1_750_000_000),
            Err(DidError::InvalidSignature)
        );
    }

    /// 29 SE-08 负例：root 公钥与签发方不符（张冠李戴）→ 必拒。
    #[test]
    fn test_verify_certificate_chain_wrong_root_key() {
        let mut rng = CsRng::new();
        let root = Sm2KeyPair::generate(&mut rng).unwrap();
        let other = Sm2KeyPair::generate(&mut rng).unwrap();
        let chain = signed_chain(&root, &mut rng);
        let verifier = DidVerifier::new();
        assert_eq!(
            verifier.verify_certificate_chain(&chain, &other.public_key, 1_750_000_000),
            Err(DidError::InvalidSignature)
        );
    }

    #[test]
    fn test_verify_certificate_chain_expired() {
        let mut rng = CsRng::new();
        let root = Sm2KeyPair::generate(&mut rng).unwrap();
        let chain = signed_chain(&root, &mut rng);
        let verifier = DidVerifier::new();
        assert_eq!(
            verifier.verify_certificate_chain(&chain, &root.public_key, 1_900_000_000),
            Err(DidError::Expired)
        );
    }

    #[test]
    fn test_verify_certificate_chain_not_yet_valid() {
        let mut rng = CsRng::new();
        let root = Sm2KeyPair::generate(&mut rng).unwrap();
        let chain = signed_chain(&root, &mut rng);
        let verifier = DidVerifier::new();
        assert_eq!(
            verifier.verify_certificate_chain(&chain, &root.public_key, 1_600_000_000),
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

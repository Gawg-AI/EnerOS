//! PKI 证书链验证器 (v0.32.0 Task 7).
//!
//! 提供 X.509 证书链验证功能，包括有效期检查、CRL 吊销检查、SM2 签名验证
//! 与信任链校验。基于 v0.31.0 国密 SM2 签名算法与 v0.32.0 Task 3/5/6 的
//! X.509 证书结构、CRL 吊销列表与证书签发器。
//!
//! # 核心组件
//! - [`CertVerifier`]：证书链验证器（信任根 + CRL + 最大链深度）
//! - [`verify_signature`]：用颁发者公钥验证证书签名（自由函数）
//!
//! # 验证流程
//! 1. **单证书验证**（[`CertVerifier::verify`]）：
//!    有效期 → CRL 吊销 → SM2 签名
//! 2. **链验证**（[`CertVerifier::verify_chain`]）：
//!    链长度 → 逐级验证 → 末端信任根
//!
//! # no_std 合规
//! no_std 由 crate 根继承，本模块通过 `extern crate alloc` 引入堆分配。
//! 使用 `alloc::string::String` / `alloc::vec::Vec`，不使用 `std::*`。
//!
//! # 参考
//! - RFC 5280 Internet X.509 Public Key Infrastructure Certificate and CRL Profile
//! - GB/T 35275 信息安全技术 SM2 密码算法加密签名消息语法规范

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use crate::pki::crl::Crl;
use crate::pki::x509::{SubjectPublicKey, X509Certificate};
use crate::pki::PkiError;
use crate::sm2::{Sm2Signature, Sm2Signer};

// ============================================================================
// 辅助函数
// ============================================================================

/// 将字节转为十六进制字符串（用于错误信息中的 serial 显示）.
fn hex_string(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let hex = alloc::format!("{:02X}", b);
        s.push_str(&hex);
    }
    s
}

// ============================================================================
// SubTask 7.5: verify_signature（自由函数）
// ============================================================================

/// 验证证书签名.
///
/// 用颁发者（issuer）的公钥验证证书（cert）的签名。
///
/// # 流程
/// 1. 获取 issuer 公钥（仅支持 SM2，RSA 返回 `UnsupportedAlgorithm`）
/// 2. 检查 cert.signature 长度 == 64（SM2 签名 r‖s）
/// 3. 编码 cert 的 TBS 字节
/// 4. 构造 `Sm2Signature` 并调用 `Sm2Signer::verify`
///
/// # 参数
/// - `cert`：待验证的证书（提供 TBS 与签名值）
/// - `issuer`：颁发者证书（提供验证公钥）
///
/// # 返回
/// - `Ok(())`：签名有效
/// - `Err(PkiError::UnsupportedAlgorithm)`：issuer 公钥非 SM2
/// - `Err(PkiError::SignatureInvalid)`：签名长度错误或验证失败
/// - `Err(PkiError::Asn1Error)`：TBS 编码失败
pub fn verify_signature(cert: &X509Certificate, issuer: &X509Certificate) -> Result<(), PkiError> {
    // 1. 获取 issuer 公钥（仅支持 SM2）
    let issuer_pk = match issuer.public_key() {
        SubjectPublicKey::Sm2(pk) => pk,
        SubjectPublicKey::Rsa(_) => return Err(PkiError::UnsupportedAlgorithm),
    };

    // 2. 检查签名长度（SM2 签名为 64 字节 r‖s）
    if cert.signature.len() != 64 {
        return Err(PkiError::SignatureInvalid);
    }

    // 3. 编码 TBS 字节
    let tbs = cert.encode_tbs()?;

    // 4. 构造 Sm2Signature 并验证（避免 panic，已确保长度为 64）
    let mut sig_bytes = [0u8; 64];
    sig_bytes.copy_from_slice(&cert.signature);
    let sig = Sm2Signature::from_bytes(&sig_bytes);

    let signer = Sm2Signer::new();
    match signer.verify(&tbs, &sig, issuer_pk) {
        Ok(true) => Ok(()),
        Ok(false) => Err(PkiError::SignatureInvalid),
        Err(_) => Err(PkiError::SignatureInvalid),
    }
}

// ============================================================================
// SubTask 7.1 ~ 7.4, 7.6: CertVerifier
// ============================================================================

/// 证书链验证器.
///
/// 维护信任的根证书列表、可选的 CRL（证书吊销列表）与最大链深度，
/// 提供单证书验证与证书链验证功能。
///
/// # 默认值
/// - `crl`：`None`（不检查吊销）
/// - `max_chain_length`：`10`
#[derive(Debug, Clone)]
pub struct CertVerifier {
    /// 信任的根证书列表
    trusted_roots: Vec<X509Certificate>,
    /// 可选的 CRL（证书吊销列表）
    crl: Option<Crl>,
    /// 最大链深度（默认 10）
    max_chain_length: usize,
}

impl CertVerifier {
    /// 创建验证器（至少一个信任根）.
    ///
    /// # 参数
    /// - `roots`：初始信任根证书列表
    pub fn new(roots: Vec<X509Certificate>) -> Self {
        Self {
            trusted_roots: roots,
            crl: None,
            max_chain_length: 10,
        }
    }

    /// 添加信任根.
    pub fn add_trusted_root(&mut self, root: X509Certificate) {
        self.trusted_roots.push(root);
    }

    /// 设置 CRL.
    pub fn set_crl(&mut self, crl: Crl) {
        self.crl = Some(crl);
    }

    /// 设置最大链深度.
    pub fn set_max_chain_length(&mut self, max: usize) {
        self.max_chain_length = max;
    }

    /// 验证单个证书（由指定颁发者签发）.
    ///
    /// # 检查项
    /// 1. 有效期：`not_before <= now <= not_after`
    /// 2. CRL 吊销检查（如果设置了 CRL）
    /// 3. 签名验证（用 issuer 的公钥验证 cert 的签名）
    ///
    /// # 参数
    /// - `cert`：待验证的证书
    /// - `issuer`：颁发者证书（提供验证公钥）
    /// - `now`：当前 Unix 时间戳（秒）
    ///
    /// # 返回
    /// - `Ok(())`：证书有效
    /// - `Err(PkiError::CertNotYetValid)`：证书尚未生效
    /// - `Err(PkiError::CertExpired)`：证书已过期
    /// - `Err(PkiError::CertRevoked)`：证书已被吊销
    /// - `Err(PkiError::SignatureInvalid)`：签名验证失败
    /// - `Err(PkiError::UnsupportedAlgorithm)`：不支持的公钥算法
    pub fn verify(
        &self,
        cert: &X509Certificate,
        issuer: &X509Certificate,
        now: u64,
    ) -> Result<(), PkiError> {
        // 1. 有效期检查
        if now < cert.not_before {
            return Err(PkiError::CertNotYetValid {
                not_before: cert.not_before,
            });
        }
        if now > cert.not_after {
            return Err(PkiError::CertExpired {
                not_after: cert.not_after,
            });
        }

        // 2. CRL 吊销检查（如果设置了 CRL）
        if let Some(ref crl) = self.crl {
            if crl.is_revoked(&cert.serial_number) {
                return Err(PkiError::CertRevoked {
                    serial: hex_string(&cert.serial_number),
                });
            }
        }

        // 3. 签名验证
        verify_signature(cert, issuer)
    }

    /// 验证证书链.
    ///
    /// `chain[0]` = 叶子证书，`chain[last]` = 根证书（或最接近根的中间证书）。
    ///
    /// # 验证流程
    /// 1. 空链检查：空链返回 `NoIssuerFound`
    /// 2. 链长度检查：`chain.len() <= max_chain_length`
    /// 3. 从叶子到根逐级验证：`verify(chain[i], chain[i+1], now)`
    /// 4. 末端证书必须是信任根：`is_trusted_root(chain[last])`
    ///
    /// # 参数
    /// - `chain`：证书链（叶子 → ... → 根）
    /// - `now`：当前 Unix 时间戳（秒）
    ///
    /// # 返回
    /// - `Ok(())`：链验证通过
    /// - `Err(PkiError::NoIssuerFound)`：空链
    /// - `Err(PkiError::ChainTooLong)`：链过长
    /// - `Err(PkiError::UntrustedRoot)`：末端不是信任根
    /// - 其他错误继承自 [`CertVerifier::verify`]
    pub fn verify_chain(&self, chain: &[X509Certificate], now: u64) -> Result<(), PkiError> {
        // 1. 空链检查
        if chain.is_empty() {
            return Err(PkiError::NoIssuerFound);
        }

        // 2. 链长度检查
        if chain.len() > self.max_chain_length {
            return Err(PkiError::ChainTooLong);
        }

        // 3. 从叶子到根逐级验证
        for window in chain.windows(2) {
            self.verify(&window[0], &window[1], now)?;
        }

        // 4. 末端证书必须是信任根
        let last = &chain[chain.len() - 1];
        if !self.is_trusted_root(last) {
            return Err(PkiError::UntrustedRoot);
        }

        Ok(())
    }

    /// 检查证书是否为信任根.
    ///
    /// 匹配条件：`subject.cn` 与 `serial_number` 相同。
    ///
    /// # 参数
    /// - `cert`：待检查的证书
    ///
    /// # 返回
    /// - `true`：证书在信任根列表中
    /// - `false`：证书不在信任根列表中
    pub fn is_trusted_root(&self, cert: &X509Certificate) -> bool {
        self.trusted_roots.iter().any(|root| {
            root.subject.cn == cert.subject.cn && root.serial_number == cert.serial_number
        })
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pki::builder::{build_certificate, build_self_signed};
    use crate::pki::crl::{Crl, RevocationReason, RevokedCert};
    use crate::pki::x509::{CertRequest, DistinguishedName, ExtKeyUsage, SubjectPublicKey};
    use crate::rng::CsRng;
    use crate::sm2::Sm2KeyPair;

    /// 生成测试用 SM2 密钥对.
    fn gen_keypair() -> Sm2KeyPair {
        let mut rng = CsRng::new();
        Sm2KeyPair::generate(&mut rng).expect("密钥对生成失败")
    }

    /// 创建确定性 RNG（种子全 1）.
    fn fixed_rng() -> CsRng {
        CsRng::from_seed(&[1u8; 32])
    }

    /// 固定测试时间戳（2023-11-14 22:13:20 UTC）.
    const NOW: u64 = 1_700_000_000;

    /// 构建自签名根证书.
    fn build_root(kp: &Sm2KeyPair, cn: &str, now: u64) -> X509Certificate {
        let req = CertRequest::new(
            DistinguishedName::new(cn),
            SubjectPublicKey::Sm2(kp.public_key),
        );
        let mut rng = fixed_rng();
        build_self_signed(&req, &kp.private_key, &kp.public_key, now, &mut rng).unwrap()
    }

    /// 用 CA 密钥签发叶子证书.
    fn build_leaf(
        leaf_kp: &Sm2KeyPair,
        ca_kp: &Sm2KeyPair,
        ca_cn: &str,
        leaf_cn: &str,
        serial: &[u8],
        now: u64,
    ) -> X509Certificate {
        let req = CertRequest::new(
            DistinguishedName::new(leaf_cn),
            SubjectPublicKey::Sm2(leaf_kp.public_key),
        )
        .add_ext_key_usage(ExtKeyUsage::ServerAuth);
        let mut rng = fixed_rng();
        build_certificate(
            &req,
            &DistinguishedName::new(ca_cn),
            &ca_kp.private_key,
            &ca_kp.public_key,
            serial,
            now,
            &mut rng,
        )
        .unwrap()
    }

    // ===== SubTask 7.3: verify 测试 =====

    #[test]
    fn test_verify_valid_cert_passes() {
        // 1. verify 有效证书通过（not_before < now < not_after，签名有效）
        let kp = gen_keypair();
        let cert = build_root(&kp, "Root CA", NOW);

        let verifier = CertVerifier::new(vec![cert.clone()]);
        let result = verifier.verify(&cert, &cert, NOW);
        assert!(result.is_ok(), "有效证书应验证通过: {:?}", result);
    }

    #[test]
    fn test_verify_expired_cert() {
        // 2. verify 过期证书 → Err(CertExpired)
        let kp = gen_keypair();
        // 构造 not_after = 1 + 86400 = 86401 的证书
        let cert = build_root(&kp, "Root CA", 1);

        let verifier = CertVerifier::new(vec![cert.clone()]);
        let result = verifier.verify(&cert, &cert, 2_000_000_000);
        assert_eq!(
            result,
            Err(PkiError::CertExpired {
                not_after: 1 + 365 * 86_400
            })
        );
    }

    #[test]
    fn test_verify_not_yet_valid_cert() {
        // 3. verify 未到期证书 → Err(CertNotYetValid)
        let kp = gen_keypair();
        // 构造 not_before = 2_000_000_000 的证书
        let cert = build_root(&kp, "Root CA", 2_000_000_000);

        let verifier = CertVerifier::new(vec![cert.clone()]);
        let result = verifier.verify(&cert, &cert, NOW);
        assert_eq!(
            result,
            Err(PkiError::CertNotYetValid {
                not_before: 2_000_000_000
            })
        );
    }

    #[test]
    fn test_verify_revoked_cert() {
        // 4. verify 被吊销证书 → Err(CertRevoked)
        let kp = gen_keypair();
        let cert = build_root(&kp, "Root CA", NOW);

        let mut crl = Crl::new(cert.issuer.clone(), NOW + 86_400);
        crl.add_revoked(RevokedCert::new(
            &cert.serial_number,
            NOW,
            RevocationReason::KeyCompromise,
        ));

        let mut verifier = CertVerifier::new(vec![cert.clone()]);
        verifier.set_crl(crl);

        let result = verifier.verify(&cert, &cert, NOW);
        assert_eq!(
            result,
            Err(PkiError::CertRevoked {
                serial: hex_string(&cert.serial_number)
            })
        );
    }

    #[test]
    fn test_verify_tampered_signature() {
        // 5. verify 签名篡改 → Err(SignatureInvalid)
        let kp = gen_keypair();
        let cert = build_root(&kp, "Root CA", NOW);

        let mut tampered = cert.clone();
        tampered.signature[0] ^= 0xFF;

        let verifier = CertVerifier::new(vec![cert.clone()]);
        let result = verifier.verify(&tampered, &cert, NOW);
        assert_eq!(result, Err(PkiError::SignatureInvalid));
    }

    // ===== SubTask 7.4: verify_chain 测试 =====

    #[test]
    fn test_verify_chain_single_cert() {
        // 6. verify_chain 单证书（叶子=根=自签名）通过
        let kp = gen_keypair();
        let cert = build_root(&kp, "Root CA", NOW);

        let verifier = CertVerifier::new(vec![cert.clone()]);
        let chain = vec![cert];
        let result = verifier.verify_chain(&chain, NOW);
        assert!(result.is_ok(), "单证书链应验证通过: {:?}", result);
    }

    #[test]
    fn test_verify_chain_two_level() {
        // 7. verify_chain 两级链（叶子 → 自签名根）通过
        let ca_kp = gen_keypair();
        let leaf_kp = gen_keypair();
        let root = build_root(&ca_kp, "Root CA", NOW);
        let leaf = build_leaf(&leaf_kp, &ca_kp, "Root CA", "Leaf", &[2], NOW);

        let verifier = CertVerifier::new(vec![root.clone()]);
        let chain = vec![leaf, root];
        let result = verifier.verify_chain(&chain, NOW);
        assert!(result.is_ok(), "两级链应验证通过: {:?}", result);
    }

    #[test]
    fn test_verify_chain_too_long() {
        // 8. verify_chain 链过长 → Err(ChainTooLong)
        let kp = gen_keypair();
        let cert = build_root(&kp, "Root CA", NOW);

        let mut verifier = CertVerifier::new(vec![cert.clone()]);
        verifier.set_max_chain_length(1);

        let chain = vec![cert.clone(), cert.clone(), cert];
        let result = verifier.verify_chain(&chain, NOW);
        assert_eq!(result, Err(PkiError::ChainTooLong));
    }

    #[test]
    fn test_verify_chain_untrusted_root() {
        // 9. verify_chain 末端不可信根 → Err(UntrustedRoot)
        let kp = gen_keypair();
        let cert = build_root(&kp, "Root CA", NOW);

        // 不将 cert 添加到 trusted_roots
        let other_kp = gen_keypair();
        let other_cert = build_root(&other_kp, "Other CA", NOW);
        let verifier = CertVerifier::new(vec![other_cert]);

        let chain = vec![cert];
        let result = verifier.verify_chain(&chain, NOW);
        assert_eq!(result, Err(PkiError::UntrustedRoot));
    }

    #[test]
    fn test_verify_chain_empty() {
        // 10. verify_chain 空链 → Err
        let kp = gen_keypair();
        let cert = build_root(&kp, "Root CA", NOW);
        let verifier = CertVerifier::new(vec![cert]);

        let chain: Vec<X509Certificate> = Vec::new();
        let result = verifier.verify_chain(&chain, NOW);
        assert_eq!(result, Err(PkiError::NoIssuerFound));
    }

    // ===== SubTask 7.6: is_trusted_root 测试 =====

    #[test]
    fn test_is_trusted_root_match() {
        // 11. is_trusted_root 匹配的证书返回 true
        let kp = gen_keypair();
        let cert = build_root(&kp, "Root CA", NOW);

        let verifier = CertVerifier::new(vec![cert.clone()]);
        assert!(verifier.is_trusted_root(&cert));
    }

    #[test]
    fn test_is_trusted_root_no_match() {
        // 12. is_trusted_root 不匹配的证书返回 false
        let kp1 = gen_keypair();
        let cert1 = build_root(&kp1, "Root CA", NOW);

        let kp2 = gen_keypair();
        let cert2 = build_root(&kp2, "Other CA", NOW);

        let verifier = CertVerifier::new(vec![cert1]);
        assert!(!verifier.is_trusted_root(&cert2));
    }

    // ===== SubTask 7.5: verify_signature 测试 =====

    #[test]
    fn test_verify_signature_valid() {
        // 13. verify_signature 有效签名通过
        let kp = gen_keypair();
        let cert = build_root(&kp, "Root CA", NOW);

        let result = verify_signature(&cert, &cert);
        assert!(result.is_ok(), "有效签名应验证通过: {:?}", result);
    }

    #[test]
    fn test_verify_signature_wrong_length() {
        // 14. verify_signature 签名长度错误 → Err(SignatureInvalid)
        let kp = gen_keypair();
        let cert = build_root(&kp, "Root CA", NOW);

        let mut bad_cert = cert.clone();
        bad_cert.signature = vec![0u8; 63]; // 63 字节，非 64

        let result = verify_signature(&bad_cert, &cert);
        assert_eq!(result, Err(PkiError::SignatureInvalid));
    }

    // ===== CRL 相关测试 =====

    #[test]
    fn test_verify_crl_not_revoked_passes() {
        // 15. verify CRL 未吊销的证书通过（设置 CRL 但证书不在列表中）
        let kp = gen_keypair();
        let cert = build_root(&kp, "Root CA", NOW);

        // CRL 中吊销的是 serial [2]，而 cert 的 serial 是 [1]
        let mut crl = Crl::new(cert.issuer.clone(), NOW + 86_400);
        crl.add_revoked(RevokedCert::new(&[2], NOW, RevocationReason::KeyCompromise));

        let mut verifier = CertVerifier::new(vec![cert.clone()]);
        verifier.set_crl(crl);

        let result = verifier.verify(&cert, &cert, NOW);
        assert!(result.is_ok(), "未吊销的证书应验证通过: {:?}", result);
    }

    #[test]
    fn test_set_crl_enables_revocation_check() {
        // 16. set_crl 后 verify 能检查吊销
        let kp = gen_keypair();
        let cert = build_root(&kp, "Root CA", NOW);

        let mut verifier = CertVerifier::new(vec![cert.clone()]);

        // 无 CRL 时验证通过
        let result_before = verifier.verify(&cert, &cert, NOW);
        assert!(result_before.is_ok(), "无 CRL 时应验证通过");

        // 设置 CRL（吊销该证书）
        let mut crl = Crl::new(cert.issuer.clone(), NOW + 86_400);
        crl.add_revoked(RevokedCert::new(
            &cert.serial_number,
            NOW,
            RevocationReason::KeyCompromise,
        ));
        verifier.set_crl(crl);

        // 有 CRL 后验证失败
        let result_after = verifier.verify(&cert, &cert, NOW);
        assert_eq!(
            result_after,
            Err(PkiError::CertRevoked {
                serial: hex_string(&cert.serial_number)
            })
        );
    }

    // ===== 附加测试 =====

    #[test]
    fn test_add_trusted_root() {
        // 验证 add_trusted_root 能将证书添加到信任根列表
        let kp1 = gen_keypair();
        let cert1 = build_root(&kp1, "CA One", NOW);

        let kp2 = gen_keypair();
        let cert2 = build_root(&kp2, "CA Two", NOW);

        let mut verifier = CertVerifier::new(vec![cert1]);
        assert!(!verifier.is_trusted_root(&cert2));

        verifier.add_trusted_root(cert2.clone());
        assert!(verifier.is_trusted_root(&cert2));
    }

    #[test]
    fn test_verify_chain_three_level() {
        // 三级链验证：叶子 → 中间 CA → 根 CA
        let root_kp = gen_keypair();
        let inter_kp = gen_keypair();
        let leaf_kp = gen_keypair();

        let root = build_root(&root_kp, "Root CA", NOW);
        let intermediate = build_leaf(&inter_kp, &root_kp, "Root CA", "Intermediate CA", &[2], NOW);
        let leaf = build_leaf(&leaf_kp, &inter_kp, "Intermediate CA", "Leaf", &[3], NOW);

        let verifier = CertVerifier::new(vec![root.clone()]);
        let chain = vec![leaf, intermediate, root];
        let result = verifier.verify_chain(&chain, NOW);
        assert!(result.is_ok(), "三级链应验证通过: {:?}", result);
    }

    #[test]
    fn test_verify_signature_rsa_unsupported() {
        // verify_signature 对 RSA 颁发者返回 UnsupportedAlgorithm
        let kp = gen_keypair();
        let cert = build_root(&kp, "Root CA", NOW);

        // 构造一个使用 RSA 公钥的颁发者
        let rsa_issuer = X509Certificate::new(
            2,
            vec![1],
            crate::pki::x509::SignatureAlgorithm::Sm2WithSm3,
            DistinguishedName::new("RSA CA"),
            DistinguishedName::new("RSA CA"),
            NOW,
            NOW + 365 * 86_400,
            SubjectPublicKey::Rsa(vec![0x01, 0x02, 0x03]),
            Vec::new(),
            vec![0u8; 64],
        );

        let result = verify_signature(&cert, &rsa_issuer);
        assert_eq!(result, Err(PkiError::UnsupportedAlgorithm));
    }
}

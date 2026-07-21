//! PKI CA 管理器 (v0.32.0 Task 8).
//!
//! 提供 CA（证书颁发机构）管理器 [`CaIssuer`]，封装 CA 证书、CA 私钥、
//! 序列号计数器与吊销列表，提供证书签发、吊销与 CRL 生成的一站式管理。
//!
//! # 核心功能
//! - [`CaIssuer::new`]：从 CA 证书与私钥构造管理器
//! - [`CaIssuer::issue_certificate`]：签发叶子证书（序列号自增）
//! - [`CaIssuer::revoke_certificate`]：吊销证书
//! - [`CaIssuer::generate_crl`]：生成证书吊销列表（CRL）
//!
//! # no_std 合规
//! no_std 由 crate 根继承，本模块通过 `extern crate alloc` 引入堆分配。
//! 使用 `alloc::vec::Vec`，不使用 `std::*`。
//!
//! # 参考
//! - RFC 5280 Internet X.509 Public Key Infrastructure Certificate and CRL Profile
//! - GB/T 35275 信息安全技术 SM2 密码算法加密签名消息语法规范

extern crate alloc;

use alloc::vec::Vec;

use crate::pki::builder;
use crate::pki::crl::{Crl, RevocationReason, RevokedCert};
use crate::pki::x509::{CertRequest, SubjectPublicKey, X509Certificate};
use crate::pki::PkiError;
use crate::rng::CsRng;
use crate::sm2::{Sm2PrivateKey, Sm2PublicKey};

// ============================================================================
// SubTask 8.1: CaIssuer 结构体
// ============================================================================

/// CA 颁发者管理器.
///
/// 封装 CA 证书 + CA 私钥 + 序列号计数器 + 吊销列表，
/// 提供证书签发、吊销、CRL 生成的一站式管理。
///
/// # 生命周期
/// 1. 通过 [`CaIssuer::new`] 从自签名根证书构造
/// 2. 调用 [`CaIssuer::issue_certificate`] 签发下级证书
/// 3. 必要时调用 [`CaIssuer::revoke_certificate`] 吊销证书
/// 4. 调用 [`CaIssuer::generate_crl`] 生成 CRL 供验证器使用
pub struct CaIssuer {
    /// CA 自身的证书（自签名根证书）
    ca_cert: X509Certificate,
    /// CA 私钥
    ca_key: Sm2PrivateKey,
    /// CA 公钥（SM2 签名需要公钥计算 Z 值，从 ca_cert 提取）
    ca_pk: Sm2PublicKey,
    /// 序列号计数器（每次签发自增）
    serial_counter: u64,
    /// 已吊销证书列表
    revoked: Vec<RevokedCert>,
    /// 内部 RNG（用于签名时的随机数 k）
    rng: CsRng,
}

// ============================================================================
// SubTask 8.2 ~ 8.5: CaIssuer 方法
// ============================================================================

impl CaIssuer {
    /// 创建 CA 管理器.
    ///
    /// 从 CA 证书和私钥构造。公钥从 `ca_cert.public_key()` 提取（必须是 SM2 变体）。
    ///
    /// # 参数
    /// - `ca_cert`：CA 自身的证书（通常为自签名根证书）
    /// - `ca_key`：CA 私钥（用于签发下级证书）
    /// - `rng`：随机数生成器（用于签名时的随机数 k，生产环境应接入硬件 TRNG）
    ///
    /// # 返回
    /// - `Ok(Self)`：构造成功
    /// - `Err(PkiError::UnsupportedAlgorithm)`：CA 证书公钥非 SM2
    pub fn new(
        ca_cert: X509Certificate,
        ca_key: Sm2PrivateKey,
        rng: CsRng,
    ) -> Result<Self, PkiError> {
        // 从 ca_cert 提取公钥（必须是 SM2 变体）
        let ca_pk = match ca_cert.public_key() {
            SubjectPublicKey::Sm2(pk) => *pk,
            SubjectPublicKey::Rsa(_) => return Err(PkiError::UnsupportedAlgorithm),
        };
        Ok(Self {
            ca_cert,
            ca_key,
            ca_pk,
            serial_counter: 1, // 从 1 开始
            revoked: Vec::new(),
            rng,
        })
    }

    /// 签发证书.
    ///
    /// 序列号自增（serial_counter → serial_counter+1），用 CA 私钥签发。
    /// 叶子证书的 issuer 字段自动设置为 CA 证书的 subject。
    ///
    /// # 参数
    /// - `req`：证书请求（提供 subject / public_key / validity / key_usage）
    /// - `now`：当前 Unix 时间戳（秒），作为 not_before
    ///
    /// # 返回
    /// 已签名的 X.509 v3 证书。
    ///
    /// # 错误
    /// 继承 [`builder::build_certificate`] 的错误类型。
    pub fn issue_certificate(
        &mut self,
        req: &CertRequest,
        now: u64,
    ) -> Result<X509Certificate, PkiError> {
        // 1. 生成序列号（serial_counter 转 8 字节大端）
        let serial = self.serial_counter.to_be_bytes().to_vec();
        self.serial_counter += 1;

        // 2. 调用 builder::build_certificate
        // issuer_dn = self.ca_cert.subject（CA 的 subject 是叶子证书的 issuer）
        builder::build_certificate(
            req,
            &self.ca_cert.subject,
            &self.ca_key,
            &self.ca_pk,
            &serial,
            now,
            &mut self.rng,
        )
    }

    /// 吊销证书.
    ///
    /// 将证书序列号加入吊销列表。
    ///
    /// # 参数
    /// - `serial`：被吊销证书的序列号
    /// - `reason`：吊销原因
    /// - `now`：吊销时间（Unix 时间戳，秒）
    pub fn revoke_certificate(
        &mut self,
        serial: &[u8],
        reason: RevocationReason,
        now: u64,
    ) -> Result<(), PkiError> {
        self.revoked.push(RevokedCert::new(serial, now, reason));
        Ok(())
    }

    /// 生成 CRL（证书吊销列表）.
    ///
    /// 用 CA 当前已吊销的证书列表生成 CRL。CRL 的 issuer 字段
    /// 自动设置为 CA 证书的 subject。
    ///
    /// # 参数
    /// - `next_update`：CRL 下次更新时间（Unix 时间戳，秒）
    ///
    /// # 返回
    /// 包含所有已吊销证书的 CRL。
    pub fn generate_crl(&self, next_update: u64) -> Result<Crl, PkiError> {
        let mut crl = Crl::new(self.ca_cert.subject.clone(), next_update);
        for revoked in &self.revoked {
            crl.add_revoked(revoked.clone());
        }
        Ok(crl)
    }

    /// 获取 CA 证书引用.
    pub fn ca_cert(&self) -> &X509Certificate {
        &self.ca_cert
    }

    /// 获取当前序列号计数器值.
    pub fn serial_counter(&self) -> u64 {
        self.serial_counter
    }

    /// 获取已吊销证书数量.
    pub fn revoked_count(&self) -> usize {
        self.revoked.len()
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pki::builder::build_self_signed;
    use crate::pki::verify::CertVerifier;
    use crate::pki::x509::{
        CertRequest, DistinguishedName, ExtKeyUsage, SignatureAlgorithm, SubjectPublicKey,
    };
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

    // ===== SubTask 8.2: CaIssuer::new 测试 =====

    #[test]
    fn test_ca_issuer_new_from_sm2_cert() {
        // 1. CaIssuer::new 从 SM2 CA 证书构造成功
        let kp = gen_keypair();
        let ca_cert = build_root(&kp, "Test CA", NOW);

        let ca = CaIssuer::new(ca_cert.clone(), kp.private_key.clone(), CsRng::new());

        assert!(ca.is_ok(), "从 SM2 CA 证书构造应成功");
        let ca = ca.unwrap();
        assert_eq!(ca.serial_counter(), 1, "初始 serial_counter 应为 1");
        assert_eq!(ca.revoked_count(), 0, "初始吊销列表应为空");
        assert_eq!(ca.ca_cert().subject.cn, "Test CA");
    }

    #[test]
    fn test_ca_issuer_new_from_rsa_cert_unsupported() {
        // 2. CaIssuer::new 从 RSA CA 证书返回 UnsupportedAlgorithm
        let kp = gen_keypair();
        let rsa_cert = X509Certificate::new(
            2,
            vec![1],
            SignatureAlgorithm::Sm2WithSm3,
            DistinguishedName::new("RSA CA"),
            DistinguishedName::new("RSA CA"),
            NOW,
            NOW + 365 * 86_400,
            SubjectPublicKey::Rsa(vec![0x01, 0x02, 0x03]),
            Vec::new(),
            vec![0u8; 64],
        );

        let result = CaIssuer::new(rsa_cert, kp.private_key.clone(), CsRng::new());
        assert!(matches!(result, Err(PkiError::UnsupportedAlgorithm)));
    }

    // ===== SubTask 8.3: issue_certificate 测试 =====

    #[test]
    fn test_issue_certificate_returns_valid_cert() {
        // 3. issue_certificate 返回有效证书
        let ca_kp = gen_keypair();
        let ca_cert = build_root(&ca_kp, "Test CA", NOW);
        let mut ca = CaIssuer::new(ca_cert, ca_kp.private_key.clone(), CsRng::new()).unwrap();

        let leaf_kp = gen_keypair();
        let req = CertRequest::new(
            DistinguishedName::new("Leaf Subject"),
            SubjectPublicKey::Sm2(leaf_kp.public_key),
        );

        let cert = ca.issue_certificate(&req, NOW);

        assert!(cert.is_ok(), "签发证书应成功");
        let cert = cert.unwrap();
        assert_eq!(cert.version, 2, "应为 v3 证书");
        assert_eq!(cert.signature.len(), 64, "签名应为 64 字节");
        assert!(!cert.extensions.is_empty(), "应含扩展");
    }

    #[test]
    fn test_issue_certificate_issuer_matches_ca_subject() {
        // 4. issue_certificate 证书的 issuer == CA 的 subject
        let ca_kp = gen_keypair();
        let ca_cert = build_root(&ca_kp, "My CA", NOW);
        let ca_subject = ca_cert.subject.clone();
        let mut ca = CaIssuer::new(ca_cert, ca_kp.private_key.clone(), CsRng::new()).unwrap();

        let leaf_kp = gen_keypair();
        let req = CertRequest::new(
            DistinguishedName::new("Leaf"),
            SubjectPublicKey::Sm2(leaf_kp.public_key),
        );

        let cert = ca.issue_certificate(&req, NOW).unwrap();

        assert_eq!(
            cert.issuer, ca_subject,
            "叶子证书的 issuer 应等于 CA 的 subject"
        );
    }

    #[test]
    fn test_issue_certificate_subject_matches_req() {
        // 5. issue_certificate 证书的 subject == req.subject
        let ca_kp = gen_keypair();
        let ca_cert = build_root(&ca_kp, "Test CA", NOW);
        let mut ca = CaIssuer::new(ca_cert, ca_kp.private_key.clone(), CsRng::new()).unwrap();

        let leaf_kp = gen_keypair();
        let subject_dn = DistinguishedName::new("Leaf Subject").with_o("Leaf Org");
        let req = CertRequest::new(
            subject_dn.clone(),
            SubjectPublicKey::Sm2(leaf_kp.public_key),
        );

        let cert = ca.issue_certificate(&req, NOW).unwrap();

        assert_eq!(cert.subject, subject_dn, "证书 subject 应匹配 req.subject");
    }

    #[test]
    fn test_issue_certificate_serial_counter_increments() {
        // 6. 多次 issue_certificate 后 serial_counter 递增（1→2→3...）
        let ca_kp = gen_keypair();
        let ca_cert = build_root(&ca_kp, "Test CA", NOW);
        let mut ca = CaIssuer::new(ca_cert, ca_kp.private_key.clone(), CsRng::new()).unwrap();

        assert_eq!(ca.serial_counter(), 1, "初始 serial_counter 应为 1");

        let leaf_kp = gen_keypair();
        let req = CertRequest::new(
            DistinguishedName::new("Leaf"),
            SubjectPublicKey::Sm2(leaf_kp.public_key),
        );

        ca.issue_certificate(&req, NOW).unwrap();
        assert_eq!(ca.serial_counter(), 2, "第一次签发后 serial_counter 应为 2");

        ca.issue_certificate(&req, NOW).unwrap();
        assert_eq!(ca.serial_counter(), 3, "第二次签发后 serial_counter 应为 3");

        ca.issue_certificate(&req, NOW).unwrap();
        assert_eq!(ca.serial_counter(), 4, "第三次签发后 serial_counter 应为 4");
    }

    #[test]
    fn test_issue_certificate_serial_number_matches_counter() {
        // 7. issue_certificate 证书的 serial_number 与 serial_counter 匹配
        let ca_kp = gen_keypair();
        let ca_cert = build_root(&ca_kp, "Test CA", NOW);
        let mut ca = CaIssuer::new(ca_cert, ca_kp.private_key.clone(), CsRng::new()).unwrap();

        let leaf_kp = gen_keypair();
        let req = CertRequest::new(
            DistinguishedName::new("Leaf"),
            SubjectPublicKey::Sm2(leaf_kp.public_key),
        );

        // 第一次签发：serial_counter=1 → serial=[0,0,0,0,0,0,0,1]，之后 counter=2
        let cert1 = ca.issue_certificate(&req, NOW).unwrap();
        let expected_serial1 = 1u64.to_be_bytes().to_vec();
        assert_eq!(
            cert1.serial_number, expected_serial1,
            "第一张证书的 serial_number 应为 1 的大端表示"
        );

        // 第二次签发：serial_counter=2 → serial=[0,0,0,0,0,0,0,2]，之后 counter=3
        let cert2 = ca.issue_certificate(&req, NOW).unwrap();
        let expected_serial2 = 2u64.to_be_bytes().to_vec();
        assert_eq!(
            cert2.serial_number, expected_serial2,
            "第二张证书的 serial_number 应为 2 的大端表示"
        );
    }

    // ===== SubTask 8.4: revoke_certificate 测试 =====

    #[test]
    fn test_revoke_certificate_increases_count() {
        // 8. revoke_certificate 后 revoked_count 增加
        let ca_kp = gen_keypair();
        let ca_cert = build_root(&ca_kp, "Test CA", NOW);
        let mut ca = CaIssuer::new(ca_cert, ca_kp.private_key.clone(), CsRng::new()).unwrap();

        assert_eq!(ca.revoked_count(), 0, "初始吊销数应为 0");

        ca.revoke_certificate(&[1], RevocationReason::KeyCompromise, NOW)
            .unwrap();
        assert_eq!(ca.revoked_count(), 1, "吊销一张后应为 1");

        ca.revoke_certificate(&[2], RevocationReason::Superseded, NOW)
            .unwrap();
        assert_eq!(ca.revoked_count(), 2, "吊销两张后应为 2");
    }

    // ===== SubTask 8.5: generate_crl 测试 =====

    #[test]
    fn test_generate_crl_contains_revoked_cert() {
        // 9. revoke_certificate 后 generate_crl 包含被吊销的证书
        let ca_kp = gen_keypair();
        let ca_cert = build_root(&ca_kp, "Test CA", NOW);
        let mut ca = CaIssuer::new(ca_cert, ca_kp.private_key.clone(), CsRng::new()).unwrap();

        let serial = 5u64.to_be_bytes().to_vec();
        ca.revoke_certificate(&serial, RevocationReason::KeyCompromise, NOW)
            .unwrap();

        let crl = ca.generate_crl(NOW + 86_400).unwrap();

        assert_eq!(crl.revoked.len(), 1, "CRL 应含 1 条吊销记录");
        assert!(crl.is_revoked(&serial), "CRL 应包含被吊销的序列号");
        assert_eq!(crl.next_update, NOW + 86_400);
    }

    #[test]
    fn test_generate_crl_issuer_matches_ca_subject() {
        // 10. generate_crl 的 issuer == CA 的 subject
        let ca_kp = gen_keypair();
        let ca_cert = build_root(&ca_kp, "My CA", NOW);
        let ca_subject = ca_cert.subject.clone();
        let ca = CaIssuer::new(ca_cert, ca_kp.private_key.clone(), CsRng::new()).unwrap();

        let crl = ca.generate_crl(NOW + 86_400).unwrap();

        assert_eq!(crl.issuer, ca_subject, "CRL 的 issuer 应等于 CA 的 subject");
    }

    #[test]
    fn test_generate_crl_empty_when_no_revocations() {
        // 11. generate_crl 空 CRL（无吊销）revoked 为空
        let ca_kp = gen_keypair();
        let ca_cert = build_root(&ca_kp, "Test CA", NOW);
        let ca = CaIssuer::new(ca_cert, ca_kp.private_key.clone(), CsRng::new()).unwrap();

        let crl = ca.generate_crl(NOW + 86_400).unwrap();

        assert!(crl.revoked.is_empty(), "无吊销时 CRL 的 revoked 应为空");
        assert_eq!(crl.next_update, NOW + 86_400);
    }

    // ===== 端到端测试 =====

    #[test]
    fn test_end_to_end_issue_and_verify() {
        // 12. 端到端：CA 签发证书 → 用 verify::CertVerifier 验证通过
        let ca_kp = gen_keypair();
        let ca_cert = build_root(&ca_kp, "Root CA", NOW);
        let mut ca =
            CaIssuer::new(ca_cert.clone(), ca_kp.private_key.clone(), CsRng::new()).unwrap();

        let leaf_kp = gen_keypair();
        let req = CertRequest::new(
            DistinguishedName::new("Leaf Subject"),
            SubjectPublicKey::Sm2(leaf_kp.public_key),
        )
        .add_ext_key_usage(ExtKeyUsage::ServerAuth);

        let leaf_cert = ca.issue_certificate(&req, NOW).unwrap();

        // 用 CertVerifier 验证叶子证书（issuer = CA 证书）
        let verifier = CertVerifier::new(vec![ca_cert.clone()]);
        let result = verifier.verify(&leaf_cert, &ca_cert, NOW);
        assert!(
            result.is_ok(),
            "CA 签发的证书应通过 CertVerifier 验证: {:?}",
            result
        );
    }

    #[test]
    fn test_end_to_end_revoke_and_crl_verify() {
        // 13. 端到端：CA 签发证书 → 吊销 → generate_crl → 验证返回 CertRevoked
        let ca_kp = gen_keypair();
        let ca_cert = build_root(&ca_kp, "Root CA", NOW);
        let mut ca =
            CaIssuer::new(ca_cert.clone(), ca_kp.private_key.clone(), CsRng::new()).unwrap();

        let leaf_kp = gen_keypair();
        let req = CertRequest::new(
            DistinguishedName::new("Leaf Subject"),
            SubjectPublicKey::Sm2(leaf_kp.public_key),
        );

        let leaf_cert = ca.issue_certificate(&req, NOW).unwrap();

        // 验证未吊销时通过
        let verifier = CertVerifier::new(vec![ca_cert.clone()]);
        let result_before = verifier.verify(&leaf_cert, &ca_cert, NOW);
        assert!(result_before.is_ok(), "未吊销时应验证通过");

        // 吊销证书
        ca.revoke_certificate(
            &leaf_cert.serial_number,
            RevocationReason::KeyCompromise,
            NOW,
        )
        .unwrap();

        // 生成 CRL 并设置到验证器
        let crl = ca.generate_crl(NOW + 86_400).unwrap();
        let mut verifier = CertVerifier::new(vec![ca_cert.clone()]);
        verifier.set_crl(crl);

        // 验证已吊销证书 → CertRevoked
        let result_after = verifier.verify(&leaf_cert, &ca_cert, NOW);
        assert!(
            matches!(result_after, Err(PkiError::CertRevoked { .. })),
            "已吊销证书应返回 CertRevoked, 实际: {:?}",
            result_after
        );
    }
}

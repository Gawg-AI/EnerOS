//! PKI 证书签发器 (v0.32.0 Task 6).
//!
//! 提供 TBSCertificate 构造、SM2 签名与完整 X.509 证书签发功能，
//! 基于 v0.31.0 国密 SM2 签名算法与 v0.32.0 Task 3 的 X.509 证书结构。
//!
//! # 核心函数
//! - [`build_tbs`]：构造 TBSCertificate 的 DER 字节
//! - [`sign_tbs`]：用 SM2 对 TBS 签名，返回 r‖s 拼接（64 字节）
//! - [`build_certificate`]：CA 签发叶子证书（构造 TBS → 签名 → 组装）
//! - [`build_self_signed`]：签发自签名根证书（issuer = subject）
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

use crate::pki::asn1::encode_oid;
use crate::pki::x509::{
    CertRequest, DistinguishedName, ExtKeyUsage, Extension, KeyUsage, SignatureAlgorithm,
    X509Certificate,
};
use crate::pki::PkiError;
use crate::rng::CsRng;
use crate::sm2::{Sm2PrivateKey, Sm2PublicKey, Sm2Signer};

/// 一天的秒数。
const SECONDS_PER_DAY: u64 = 86_400;

/// KeyUsage 扩展 OID：2.5.29.15。
fn key_usage_oid() -> Vec<u8> {
    encode_oid(&[2, 5, 29, 15])
}

/// ExtKeyUsage 扩展 OID：2.5.29.37。
fn ext_key_usage_oid() -> Vec<u8> {
    encode_oid(&[2, 5, 29, 37])
}

/// 从证书请求构造 v3 扩展列表。
///
/// - KeyUsage（critical=true）：来自 `req.key_usage`
/// - ExtKeyUsage（critical=false）：来自 `req.ext_key_usage`，仅当非空时添加
fn build_extensions(req: &CertRequest) -> Vec<Extension> {
    let mut exts = Vec::new();

    // KeyUsage 扩展（RFC 5280 §4.2.1.3，应为 critical）
    exts.push(Extension {
        oid: key_usage_oid(),
        critical: true,
        value: req.key_usage.encode(),
    });

    // ExtKeyUsage 扩展（RFC 5280 §4.2.1.12，仅当非空时添加）
    if !req.ext_key_usage.is_empty() {
        exts.push(Extension {
            oid: ext_key_usage_oid(),
            critical: false,
            value: ExtKeyUsage::encode_sequence(&req.ext_key_usage),
        });
    }

    exts
}

// ============================================================================
// SubTask 6.1: build_tbs
// ============================================================================

/// 构造 TBSCertificate 的 DER 字节。
///
/// 组装顺序：version(v3) + serial + sigAlg + issuer + validity + subject
/// + subjectPKInfo + extensions。
///
/// 实现方式：构造临时 [`X509Certificate`] 对象（signature 填空 Vec），
/// 调用其 `encode_tbs()` 复用已有的 TBS 编码逻辑。
///
/// # 参数
/// - `req`：证书请求（提供 subject / public_key / validity_days / key_usage / ext_key_usage）
/// - `issuer`：颁发者可分辨名称
/// - `serial`：序列号（大端 INTEGER 字节）
/// - `now`：当前 Unix 时间戳（秒），作为 not_before
///
/// # 返回
/// TBS 的完整 DER 字节（SEQUENCE TLV）。
///
/// # 错误
/// - `PkiError::UnsupportedAlgorithm`：公钥为 RSA（不支持 SPKI 编码）
/// - `PkiError::Asn1Error`：DER 编码失败
pub fn build_tbs(
    req: &CertRequest,
    issuer: &DistinguishedName,
    serial: &[u8],
    now: u64,
) -> Result<Vec<u8>, PkiError> {
    let not_before = now;
    let not_after = now + (req.validity_days as u64) * SECONDS_PER_DAY;
    let extensions = build_extensions(req);

    // 构造临时证书对象（signature 填空 Vec，不影响 TBS 编码）
    let tmp_cert = X509Certificate::new(
        2, // v3（含 extensions）
        serial.to_vec(),
        SignatureAlgorithm::Sm2WithSm3,
        issuer.clone(),
        req.subject.clone(),
        not_before,
        not_after,
        req.public_key.clone(),
        extensions,
        Vec::new(),
    );

    tmp_cert.encode_tbs()
}

// ============================================================================
// SubTask 6.2: sign_tbs
// ============================================================================

/// 用 SM2 对 TBS 签名。
///
/// 返回签名值的 r‖s 拼接字节（64 字节）。
///
/// SM2 签名与 ECDSA/RSA 不同，需要公钥参与签名过程（计算 Z 值），
/// 因此本函数同时需要私钥和公钥。
///
/// # 参数
/// - `tbs`：待签名的 TBS DER 字节
/// - `sk`：签名私钥
/// - `pk`：对应公钥（SM2 签名需要公钥计算 Z 值）
/// - `rng`：随机数生成器（SM2 签名需要随机数 k）
///
/// # 错误
/// - `PkiError::SignatureInvalid`：SM2 签名失败
pub fn sign_tbs(
    tbs: &[u8],
    sk: &Sm2PrivateKey,
    pk: &Sm2PublicKey,
    rng: &mut CsRng,
) -> Result<Vec<u8>, PkiError> {
    let signer = Sm2Signer::new();
    match signer.sign(tbs, sk, pk, rng) {
        Ok(sig) => Ok(sig.to_bytes().to_vec()),
        Err(_) => Err(PkiError::SignatureInvalid),
    }
}

// ============================================================================
// SubTask 6.3: build_certificate
// ============================================================================

/// 签发证书（CA 用私钥签发叶子证书）。
///
/// 完整流程：构造 TBS → 签名 → 组装 [`X509Certificate`]。
///
/// # 参数
/// - `req`：证书请求
/// - `issuer_dn`：颁发者可分辨名称（CA 的 DN）
/// - `issuer_sk`：CA 私钥（用于签名）
/// - `issuer_pk`：CA 公钥（SM2 签名需要公钥计算 Z 值）
/// - `serial`：序列号（大端 INTEGER 字节）
/// - `now`：当前 Unix 时间戳（秒）
/// - `rng`：随机数生成器
///
/// # 返回
/// 已签名的 X.509 v3 证书。
///
/// # 错误
/// - `PkiError::UnsupportedAlgorithm`：公钥为 RSA
/// - `PkiError::SignatureInvalid`：签名失败
/// - `PkiError::Asn1Error`：DER 编码失败
pub fn build_certificate(
    req: &CertRequest,
    issuer_dn: &DistinguishedName,
    issuer_sk: &Sm2PrivateKey,
    issuer_pk: &Sm2PublicKey,
    serial: &[u8],
    now: u64,
    rng: &mut CsRng,
) -> Result<X509Certificate, PkiError> {
    // 1. 构造 TBS
    let tbs = build_tbs(req, issuer_dn, serial, now)?;

    // 2. 签名 TBS（用 CA 私钥 + CA 公钥 + rng）
    let signature = sign_tbs(&tbs, issuer_sk, issuer_pk, rng)?;

    // 3. 构造 extensions（与 build_tbs 中相同，确保签名验证通过）
    let extensions = build_extensions(req);

    // 4. 组装完整证书
    let not_before = now;
    let not_after = now + (req.validity_days as u64) * SECONDS_PER_DAY;

    Ok(X509Certificate::new(
        2, // v3
        serial.to_vec(),
        SignatureAlgorithm::Sm2WithSm3,
        issuer_dn.clone(),
        req.subject.clone(),
        not_before,
        not_after,
        req.public_key.clone(),
        extensions,
        signature,
    ))
}

// ============================================================================
// SubTask 6.4: build_self_signed
// ============================================================================

/// 签发自签名根证书（issuer = subject）。
///
/// 用于创建 CA 根证书。自动添加 KeyUsage 中的 `KEY_CERT_SIGN | CRL_SIGN`
/// 位，使根证书具备签发下级证书与 CRL 的权限。
///
/// # 参数
/// - `req`：证书请求（subject 同时作为 issuer）
/// - `sk`：根 CA 私钥
/// - `pk`：根 CA 公钥（SM2 签名需要公钥计算 Z 值）
/// - `now`：当前 Unix 时间戳（秒）
/// - `rng`：随机数生成器
///
/// # 返回
/// 已自签名的 X.509 v3 根证书（serial = `[1]`）。
///
/// # 错误
/// 继承 [`build_certificate`] 的错误类型。
pub fn build_self_signed(
    req: &CertRequest,
    sk: &Sm2PrivateKey,
    pk: &Sm2PublicKey,
    now: u64,
    rng: &mut CsRng,
) -> Result<X509Certificate, PkiError> {
    // 根 CA 需要 KEY_CERT_SIGN | CRL_SIGN 权限以签发下级证书与 CRL
    let mut ca_req = req.clone();
    ca_req.key_usage.add(KeyUsage::KEY_CERT_SIGN);
    ca_req.key_usage.add(KeyUsage::CRL_SIGN);

    // 自签名：issuer = subject，serial = [1]（根证书惯例）
    build_certificate(&ca_req, &req.subject, sk, pk, &[1], now, rng)
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pki::parser;
    use crate::pki::x509::SubjectPublicKey;
    use crate::rng::CsRng;
    use crate::sm2::{Sm2KeyPair, Sm2Signature};

    /// 生成测试用 SM2 密钥对。
    fn gen_keypair() -> Sm2KeyPair {
        let mut rng = CsRng::new();
        Sm2KeyPair::generate(&mut rng).expect("密钥对生成失败")
    }

    /// 创建确定性 RNG（种子全 1）。
    fn fixed_rng() -> CsRng {
        CsRng::from_seed(&[1u8; 32])
    }

    /// 创建确定性 RNG（自定义种子字节）。
    fn seeded_rng(seed_byte: u8) -> CsRng {
        CsRng::from_seed(&[seed_byte; 32])
    }

    /// 固定测试时间戳（2023-11-14 22:13:20 UTC）。
    const NOW: u64 = 1_700_000_000;

    // ===== SubTask 6.1: build_tbs 测试 =====

    #[test]
    fn test_build_tbs_non_empty_and_sequence_tag() {
        let kp = gen_keypair();
        let req = CertRequest::new(
            DistinguishedName::new("Test Subject"),
            SubjectPublicKey::Sm2(kp.public_key),
        );
        let issuer = DistinguishedName::new("Test CA");

        let tbs = build_tbs(&req, &issuer, &[1], NOW).unwrap();

        assert!(!tbs.is_empty(), "TBS 不应为空");
        assert_eq!(tbs[0], 0x30, "TBS 应以 SEQUENCE tag 0x30 开头");
    }

    #[test]
    fn test_build_tbs_consistent_with_cert() {
        // build_tbs 产生的 TBS 应与 build_certificate 产生的证书的 encode_tbs 一致
        let kp = gen_keypair();
        let req = CertRequest::new(
            DistinguishedName::new("Test Subject"),
            SubjectPublicKey::Sm2(kp.public_key),
        );
        let issuer = DistinguishedName::new("Test CA");

        // 1. 直接构造 TBS
        let tbs_direct = build_tbs(&req, &issuer, &[1], NOW).unwrap();

        // 2. 构造完整证书，再取其 TBS
        let mut rng = fixed_rng();
        let cert = build_certificate(
            &req,
            &issuer,
            &kp.private_key,
            &kp.public_key,
            &[1],
            NOW,
            &mut rng,
        )
        .unwrap();
        let tbs_from_cert = cert.encode_tbs().unwrap();

        assert_eq!(
            tbs_direct, tbs_from_cert,
            "build_tbs 输出应与证书内 TBS 一致"
        );
    }

    // ===== SubTask 6.2: sign_tbs 测试 =====

    #[test]
    fn test_sign_tbs_returns_64_bytes() {
        let kp = gen_keypair();
        let tbs = vec![0x30u8, 0x05, 0x00, 0x01, 0x02, 0x03, 0x04];
        let mut rng = fixed_rng();

        let sig = sign_tbs(&tbs, &kp.private_key, &kp.public_key, &mut rng).unwrap();

        assert_eq!(sig.len(), 64, "SM2 签名应为 64 字节（r‖s）");
    }

    #[test]
    fn test_sign_tbs_different_rng_different_sig() {
        let kp = gen_keypair();
        let tbs = vec![0x30u8, 0x05, 0x00, 0x01, 0x02, 0x03, 0x04];

        let mut rng1 = seeded_rng(1);
        let mut rng2 = seeded_rng(2);

        let sig1 = sign_tbs(&tbs, &kp.private_key, &kp.public_key, &mut rng1).unwrap();
        let sig2 = sign_tbs(&tbs, &kp.private_key, &kp.public_key, &mut rng2).unwrap();

        assert_ne!(sig1, sig2, "不同 RNG 种子应产生不同签名");
    }

    // ===== SubTask 6.4: build_self_signed 测试 =====

    #[test]
    fn test_build_self_signed_valid() {
        let kp = gen_keypair();
        let req = CertRequest::new(
            DistinguishedName::new("Root CA"),
            SubjectPublicKey::Sm2(kp.public_key),
        );
        let mut rng = fixed_rng();

        let cert = build_self_signed(&req, &kp.private_key, &kp.public_key, NOW, &mut rng).unwrap();

        assert_eq!(cert.version, 2, "应为 v3 证书（version=2）");
        assert_eq!(cert.serial_number, vec![1], "根证书 serial 应为 [1]");
        assert_eq!(cert.signature.len(), 64, "签名应为 64 字节");
        assert!(!cert.extensions.is_empty(), "应含扩展");
    }

    #[test]
    fn test_build_self_signed_issuer_eq_subject() {
        let kp = gen_keypair();
        let req = CertRequest::new(
            DistinguishedName::new("Root CA").with_o("Test Org"),
            SubjectPublicKey::Sm2(kp.public_key),
        );
        let mut rng = fixed_rng();

        let cert = build_self_signed(&req, &kp.private_key, &kp.public_key, NOW, &mut rng).unwrap();

        assert_eq!(cert.issuer, cert.subject, "自签名证书 issuer == subject");
        assert_eq!(cert.issuer.cn, "Root CA");
        assert_eq!(cert.issuer.o.as_deref(), Some("Test Org"));
    }

    #[test]
    fn test_build_self_signed_signature_verifies() {
        let kp = gen_keypair();
        let req = CertRequest::new(
            DistinguishedName::new("Root CA"),
            SubjectPublicKey::Sm2(kp.public_key),
        );
        let mut rng = fixed_rng();

        let cert = build_self_signed(&req, &kp.private_key, &kp.public_key, NOW, &mut rng).unwrap();

        // 提取 TBS 与签名，用 Sm2Signer::verify 验证
        let tbs = cert.encode_tbs().unwrap();
        let mut sig_bytes = [0u8; 64];
        sig_bytes.copy_from_slice(&cert.signature);
        let sig = Sm2Signature::from_bytes(&sig_bytes);

        let signer = Sm2Signer::new();
        let valid = signer.verify(&tbs, &sig, &kp.public_key).unwrap();
        assert!(valid, "自签名证书的签名应验证通过");
    }

    #[test]
    fn test_build_self_signed_has_ca_key_usage() {
        let kp = gen_keypair();
        let req = CertRequest::new(
            DistinguishedName::new("Root CA"),
            SubjectPublicKey::Sm2(kp.public_key),
        );
        let mut rng = fixed_rng();

        let cert = build_self_signed(&req, &kp.private_key, &kp.public_key, NOW, &mut rng).unwrap();

        // 查找 KeyUsage 扩展并验证包含 KEY_CERT_SIGN | CRL_SIGN
        let ku_ext = cert
            .extensions
            .iter()
            .find(|e| e.is_key_usage())
            .expect("根证书应含 KeyUsage 扩展");
        let ku = KeyUsage::decode(&ku_ext.value).unwrap();
        assert!(
            ku.contains(KeyUsage::KEY_CERT_SIGN),
            "根证书应含 KEY_CERT_SIGN"
        );
        assert!(ku.contains(KeyUsage::CRL_SIGN), "根证书应含 CRL_SIGN");
    }

    // ===== SubTask 6.3: build_certificate 测试 =====

    #[test]
    fn test_build_certificate_valid() {
        let ca_kp = gen_keypair();
        let leaf_kp = gen_keypair();

        let ca_req = CertRequest::new(
            DistinguishedName::new("Root CA"),
            SubjectPublicKey::Sm2(ca_kp.public_key),
        );
        let leaf_req = CertRequest::new(
            DistinguishedName::new("Leaf Subject"),
            SubjectPublicKey::Sm2(leaf_kp.public_key),
        )
        .add_ext_key_usage(ExtKeyUsage::ServerAuth);

        let mut rng = fixed_rng();
        // 先建 CA（自签名）
        let ca_cert = build_self_signed(
            &ca_req,
            &ca_kp.private_key,
            &ca_kp.public_key,
            NOW,
            &mut rng,
        )
        .unwrap();
        // 再用 CA 签发叶子
        let leaf_cert = build_certificate(
            &leaf_req,
            &ca_cert.subject,
            &ca_kp.private_key,
            &ca_kp.public_key,
            &[2],
            NOW,
            &mut rng,
        )
        .unwrap();

        assert_eq!(leaf_cert.version, 2, "应为 v3 证书");
        assert_eq!(leaf_cert.serial_number, vec![2]);
        assert_eq!(leaf_cert.signature.len(), 64, "签名应为 64 字节");
    }

    #[test]
    fn test_build_certificate_issuer_matches() {
        let ca_kp = gen_keypair();
        let leaf_kp = gen_keypair();
        let issuer_dn = DistinguishedName::new("Test CA")
            .with_o("CA Org")
            .with_c("CN");

        let leaf_req = CertRequest::new(
            DistinguishedName::new("Leaf"),
            SubjectPublicKey::Sm2(leaf_kp.public_key),
        );

        let mut rng = fixed_rng();
        let cert = build_certificate(
            &leaf_req,
            &issuer_dn,
            &ca_kp.private_key,
            &ca_kp.public_key,
            &[0x0A],
            NOW,
            &mut rng,
        )
        .unwrap();

        assert_eq!(cert.issuer, issuer_dn, "证书 issuer 应匹配传入的 issuer_dn");
    }

    #[test]
    fn test_build_certificate_subject_matches() {
        let ca_kp = gen_keypair();
        let leaf_kp = gen_keypair();
        let subject_dn = DistinguishedName::new("Leaf Subject").with_ou("Engineering");

        let leaf_req = CertRequest::new(
            subject_dn.clone(),
            SubjectPublicKey::Sm2(leaf_kp.public_key),
        );

        let mut rng = fixed_rng();
        let cert = build_certificate(
            &leaf_req,
            &DistinguishedName::new("CA"),
            &ca_kp.private_key,
            &ca_kp.public_key,
            &[1],
            NOW,
            &mut rng,
        )
        .unwrap();

        assert_eq!(cert.subject, subject_dn, "证书 subject 应匹配 req.subject");
    }

    #[test]
    fn test_build_certificate_roundtrip() {
        let ca_kp = gen_keypair();
        let leaf_kp = gen_keypair();

        let leaf_req = CertRequest::new(
            DistinguishedName::new("Roundtrip Subject").with_o("Org"),
            SubjectPublicKey::Sm2(leaf_kp.public_key),
        )
        .with_validity_days(730)
        .add_ext_key_usage(ExtKeyUsage::ClientAuth);

        let mut rng = fixed_rng();
        let cert = build_certificate(
            &leaf_req,
            &DistinguishedName::new("Roundtrip CA"),
            &ca_kp.private_key,
            &ca_kp.public_key,
            &[0x42],
            NOW,
            &mut rng,
        )
        .unwrap();

        // 编码后解析应得到相同结构
        let der = cert.encode().unwrap();
        let decoded = X509Certificate::decode(&der).unwrap();
        assert_eq!(decoded, cert, "X509Certificate::decode 往返应一致");

        // 也测试 parser::parse_der
        let parsed = parser::parse_der(&der).unwrap();
        assert_eq!(parsed, cert, "parser::parse_der 往返应一致");
    }

    #[test]
    fn test_build_self_signed_roundtrip() {
        let kp = gen_keypair();
        let req = CertRequest::new(
            DistinguishedName::new("Root CA").with_c("CN"),
            SubjectPublicKey::Sm2(kp.public_key),
        )
        .with_validity_days(3650);

        let mut rng = fixed_rng();
        let cert = build_self_signed(&req, &kp.private_key, &kp.public_key, NOW, &mut rng).unwrap();

        // 编码后解析应得到相同结构
        let der = cert.encode().unwrap();
        let decoded = X509Certificate::decode(&der).unwrap();
        assert_eq!(decoded, cert, "X509Certificate::decode 往返应一致");

        // 也测试 parser::parse_der
        let parsed = parser::parse_der(&der).unwrap();
        assert_eq!(parsed, cert, "parser::parse_der 往返应一致");
    }
}

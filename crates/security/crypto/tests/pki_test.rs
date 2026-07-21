//! PKI 端到端集成测试 (v0.32.0 Task 10).
//!
//! 覆盖 Task 1~9 实现的 PKI 模块全链路：自签名根证书、CA 签发叶子证书、
//! 证书链验证、CRL 吊销、DER/PEM 序列化、CA 管理器序列号自增等场景。
//!
//! 集成测试运行于 host std 环境（非 no_std），通过 `eneros_crypto` crate
//! 的公共 API 调用被测的 no_std 代码。
//!
//! # 子任务
//! - 10.1: 自签名根证书生成 + 验证通过
//! - 10.2: CA 签发叶子证书 → verify_chain 通过
//! - 10.3: CA 签发 → 吊销 → verify 返回 CertRevoked
//! - 10.4: 过期证书 → CertExpired
//! - 10.5: 未到期证书 → CertNotYetValid
//! - 10.6: 签名篡改 → SignatureInvalid
//! - 10.7: 不可信根 → UntrustedRoot
//! - 10.8: DER/PEM 往返
//! - 10.9: 证书链（leaf → intermediate → root）验证通过
//! - 10.10: serial_number 单调递增（CaIssuer）

use eneros_crypto::{
    base64_decode, base64_encode, build_certificate, build_self_signed, parse_der, parse_pem,
    to_der, to_pem, verify_signature, CaIssuer, CertRequest, CertVerifier, Crl, CsRng,
    DistinguishedName, Extension, KeyUsage, PkiError, RevocationReason, RevokedCert,
    SignatureAlgorithm, Sm2KeyPair, Sm2PrivateKey, SubjectPublicKey,
};

// ============================================================================
// 测试辅助函数
// ============================================================================

/// 生成测试用 SM2 密钥对.
fn gen_keypair() -> Sm2KeyPair {
    let mut rng = CsRng::new();
    Sm2KeyPair::generate(&mut rng).expect("密钥对生成失败")
}

/// 创建确定性 RNG（种子全 42）.
fn gen_rng() -> CsRng {
    CsRng::from_seed(&[42u8; 32])
}

/// 固定时间戳（2023-11-14 22:13:20 UTC）.
const NOW: u64 = 1_700_000_000;

/// 一年秒数.
const ONE_YEAR: u64 = 365 * 86_400;

// ============================================================================
// SubTask 10.1: 自签名根证书生成 + 验证通过
// ============================================================================

#[test]
fn test_self_signed_root_verify() {
    let ca_kp = gen_keypair();
    let req = CertRequest::new(
        DistinguishedName::new("Test Root CA"),
        SubjectPublicKey::Sm2(ca_kp.public_key),
    );
    let root_cert = build_self_signed(
        &req,
        &ca_kp.private_key,
        &ca_kp.public_key,
        NOW,
        &mut gen_rng(),
    )
    .unwrap();

    // 用自身公钥验证自身签名（自签名）
    verify_signature(&root_cert, &root_cert).unwrap();
}

// ============================================================================
// SubTask 10.2: CA 签发叶子证书 → verify_chain 通过
// ============================================================================

#[test]
fn test_ca_issue_leaf_chain_verify() {
    // 1. 生成 CA 密钥对 + 自签名根证书
    let ca_kp = gen_keypair();
    let ca_req = CertRequest::new(
        DistinguishedName::new("Test CA"),
        SubjectPublicKey::Sm2(ca_kp.public_key),
    );
    let root_cert = build_self_signed(
        &ca_req,
        &ca_kp.private_key,
        &ca_kp.public_key,
        NOW,
        &mut gen_rng(),
    )
    .unwrap();

    // 2. 生成叶子密钥对 + CA 签发叶子证书
    let leaf_kp = gen_keypair();
    let leaf_req = CertRequest::new(
        DistinguishedName::new("Leaf Cert"),
        SubjectPublicKey::Sm2(leaf_kp.public_key),
    );
    let leaf_cert = build_certificate(
        &leaf_req,
        &root_cert.subject, // issuer = CA subject
        &ca_kp.private_key, // CA 私钥
        &ca_kp.public_key,  // CA 公钥
        &[1],               // serial
        NOW,
        &mut gen_rng(),
    )
    .unwrap();

    // 3. 验证链
    let verifier = CertVerifier::new(vec![root_cert.clone()]);
    let chain = vec![leaf_cert, root_cert];
    verifier.verify_chain(&chain, NOW).unwrap();
}

// ============================================================================
// SubTask 10.3: CA 签发 → 吊销 → verify 返回 CertRevoked
// ============================================================================

#[test]
fn test_ca_issue_revoke_verify_fails() {
    // 1. 生成 CA + 根证书
    let ca_kp = gen_keypair();
    let ca_req = CertRequest::new(
        DistinguishedName::new("Test CA"),
        SubjectPublicKey::Sm2(ca_kp.public_key),
    );
    let root_cert = build_self_signed(
        &ca_req,
        &ca_kp.private_key,
        &ca_kp.public_key,
        NOW,
        &mut gen_rng(),
    )
    .unwrap();

    // 2. CA 签发叶子证书
    let leaf_kp = gen_keypair();
    let leaf_req = CertRequest::new(
        DistinguishedName::new("Leaf Cert"),
        SubjectPublicKey::Sm2(leaf_kp.public_key),
    );
    let leaf_cert = build_certificate(
        &leaf_req,
        &root_cert.subject,
        &ca_kp.private_key,
        &ca_kp.public_key,
        &[1],
        NOW,
        &mut gen_rng(),
    )
    .unwrap();

    // 3. 吊销叶子证书
    let mut crl = Crl::new(root_cert.subject.clone(), NOW + ONE_YEAR);
    crl.add_revoked(RevokedCert::new(
        &leaf_cert.serial_number,
        NOW,
        RevocationReason::KeyCompromise,
    ));

    // 4. 验证应返回 CertRevoked
    let mut verifier = CertVerifier::new(vec![root_cert.clone()]);
    verifier.set_crl(crl);
    let result = verifier.verify(&leaf_cert, &root_cert, NOW);
    assert!(matches!(result, Err(PkiError::CertRevoked { .. })));
}

// ============================================================================
// SubTask 10.4: 过期证书 → CertExpired
// ============================================================================

#[test]
fn test_expired_cert() {
    // 生成证书时 now = 1（很早），验证时 now = NOW（很晚）
    let ca_kp = gen_keypair();
    let req = CertRequest::new(
        DistinguishedName::new("Expired Cert"),
        SubjectPublicKey::Sm2(ca_kp.public_key),
    );
    let cert = build_self_signed(
        &req,
        &ca_kp.private_key,
        &ca_kp.public_key,
        1,
        &mut gen_rng(),
    )
    .unwrap();

    let verifier = CertVerifier::new(vec![cert.clone()]);
    let result = verifier.verify(&cert, &cert, NOW);
    assert!(matches!(result, Err(PkiError::CertExpired { .. })));
}

// ============================================================================
// SubTask 10.5: 未到期证书 → CertNotYetValid
// ============================================================================

#[test]
fn test_not_yet_valid_cert() {
    // 生成证书时 now = NOW + 10*ONE_YEAR（未来），验证时 now = NOW
    let ca_kp = gen_keypair();
    let req = CertRequest::new(
        DistinguishedName::new("Future Cert"),
        SubjectPublicKey::Sm2(ca_kp.public_key),
    );
    let cert = build_self_signed(
        &req,
        &ca_kp.private_key,
        &ca_kp.public_key,
        NOW + 10 * ONE_YEAR,
        &mut gen_rng(),
    )
    .unwrap();

    let verifier = CertVerifier::new(vec![cert.clone()]);
    let result = verifier.verify(&cert, &cert, NOW);
    assert!(matches!(result, Err(PkiError::CertNotYetValid { .. })));
}

// ============================================================================
// SubTask 10.6: 签名篡改 → SignatureInvalid
// ============================================================================

#[test]
fn test_tampered_signature() {
    let ca_kp = gen_keypair();
    let leaf_kp = gen_keypair();
    let ca_req = CertRequest::new(
        DistinguishedName::new("CA"),
        SubjectPublicKey::Sm2(ca_kp.public_key),
    );
    let root_cert = build_self_signed(
        &ca_req,
        &ca_kp.private_key,
        &ca_kp.public_key,
        NOW,
        &mut gen_rng(),
    )
    .unwrap();

    let leaf_req = CertRequest::new(
        DistinguishedName::new("Leaf"),
        SubjectPublicKey::Sm2(leaf_kp.public_key),
    );
    let mut leaf_cert = build_certificate(
        &leaf_req,
        &root_cert.subject,
        &ca_kp.private_key,
        &ca_kp.public_key,
        &[1],
        NOW,
        &mut gen_rng(),
    )
    .unwrap();

    // 篡改签名
    leaf_cert.signature[0] ^= 0xFF;

    let result = verify_signature(&leaf_cert, &root_cert);
    assert!(matches!(result, Err(PkiError::SignatureInvalid)));
}

// ============================================================================
// SubTask 10.7: 不可信根 → UntrustedRoot
// ============================================================================

#[test]
fn test_untrusted_root() {
    // 生成两个独立的 CA
    let ca1_kp = gen_keypair();
    let ca2_kp = gen_keypair();

    let ca1_req = CertRequest::new(
        DistinguishedName::new("CA1"),
        SubjectPublicKey::Sm2(ca1_kp.public_key),
    );
    let root1 = build_self_signed(
        &ca1_req,
        &ca1_kp.private_key,
        &ca1_kp.public_key,
        NOW,
        &mut gen_rng(),
    )
    .unwrap();

    let ca2_req = CertRequest::new(
        DistinguishedName::new("CA2"),
        SubjectPublicKey::Sm2(ca2_kp.public_key),
    );
    let root2 = build_self_signed(
        &ca2_req,
        &ca2_kp.private_key,
        &ca2_kp.public_key,
        NOW,
        &mut gen_rng(),
    )
    .unwrap();

    // verifier 只信任 root2，但链末端是 root1
    let verifier = CertVerifier::new(vec![root2]);
    let chain = vec![root1.clone()];
    let result = verifier.verify_chain(&chain, NOW);
    assert!(matches!(result, Err(PkiError::UntrustedRoot)));
}

// ============================================================================
// SubTask 10.8: DER/PEM 往返
// ============================================================================

#[test]
fn test_der_pem_roundtrip() {
    let ca_kp = gen_keypair();
    let req = CertRequest::new(
        DistinguishedName::new("PEM Test"),
        SubjectPublicKey::Sm2(ca_kp.public_key),
    );
    let cert = build_self_signed(
        &req,
        &ca_kp.private_key,
        &ca_kp.public_key,
        NOW,
        &mut gen_rng(),
    )
    .unwrap();

    // DER 往返
    let der = to_der(&cert).unwrap();
    let cert_from_der = parse_der(&der).unwrap();
    assert_eq!(cert_from_der.subject.cn, cert.subject.cn);
    assert_eq!(cert_from_der.serial_number, cert.serial_number);

    // PEM 往返
    let pem = to_pem(&cert).unwrap();
    assert!(pem.contains("-----BEGIN CERTIFICATE-----"));
    assert!(pem.contains("-----END CERTIFICATE-----"));
    let cert_from_pem = parse_pem(&pem).unwrap();
    assert_eq!(cert_from_pem.subject.cn, cert.subject.cn);
    assert_eq!(cert_from_pem.serial_number, cert.serial_number);

    // 顺带验证 base64 编解码一致性
    let b64 = base64_encode(&der);
    let decoded = base64_decode(b64.as_bytes()).unwrap();
    assert_eq!(decoded, der);
}

// ============================================================================
// SubTask 10.9: 证书链（leaf → intermediate → root）验证通过
// ============================================================================

#[test]
fn test_three_level_chain() {
    // Root CA
    let root_kp = gen_keypair();
    let root_req = CertRequest::new(
        DistinguishedName::new("Root CA"),
        SubjectPublicKey::Sm2(root_kp.public_key),
    );
    let root_cert = build_self_signed(
        &root_req,
        &root_kp.private_key,
        &root_kp.public_key,
        NOW,
        &mut gen_rng(),
    )
    .unwrap();

    // Intermediate CA（由 Root 签发）
    let inter_kp = gen_keypair();
    let inter_req = CertRequest::new(
        DistinguishedName::new("Intermediate CA"),
        SubjectPublicKey::Sm2(inter_kp.public_key),
    )
    .with_key_usage(KeyUsage::new(KeyUsage::KEY_CERT_SIGN | KeyUsage::CRL_SIGN));

    let inter_cert = build_certificate(
        &inter_req,
        &root_cert.subject,
        &root_kp.private_key,
        &root_kp.public_key,
        &[2],
        NOW,
        &mut gen_rng(),
    )
    .unwrap();

    // Leaf（由 Intermediate 签发）
    let leaf_kp = gen_keypair();
    let leaf_req = CertRequest::new(
        DistinguishedName::new("Leaf"),
        SubjectPublicKey::Sm2(leaf_kp.public_key),
    );
    let leaf_cert = build_certificate(
        &leaf_req,
        &inter_cert.subject,
        &inter_kp.private_key,
        &inter_kp.public_key,
        &[3],
        NOW,
        &mut gen_rng(),
    )
    .unwrap();

    // 验证三级链
    let verifier = CertVerifier::new(vec![root_cert.clone()]);
    let chain = vec![leaf_cert, inter_cert, root_cert];
    verifier.verify_chain(&chain, NOW).unwrap();
}

// ============================================================================
// SubTask 10.10: serial_number 单调递增（CaIssuer）
// ============================================================================

#[test]
fn test_ca_serial_increment() {
    let ca_kp = gen_keypair();
    let ca_req = CertRequest::new(
        DistinguishedName::new("Serial CA"),
        SubjectPublicKey::Sm2(ca_kp.public_key),
    );
    let root_cert = build_self_signed(
        &ca_req,
        &ca_kp.private_key,
        &ca_kp.public_key,
        NOW,
        &mut gen_rng(),
    )
    .unwrap();

    let mut ca = CaIssuer::new(root_cert, ca_kp.private_key.clone(), CsRng::new()).unwrap();

    let leaf_req = CertRequest::new(
        DistinguishedName::new("Leaf1"),
        SubjectPublicKey::Sm2(gen_keypair().public_key),
    );
    let cert1 = ca.issue_certificate(&leaf_req, NOW).unwrap();
    assert_eq!(cert1.serial_number, 1u64.to_be_bytes().to_vec());

    let leaf_req2 = CertRequest::new(
        DistinguishedName::new("Leaf2"),
        SubjectPublicKey::Sm2(gen_keypair().public_key),
    );
    let cert2 = ca.issue_certificate(&leaf_req2, NOW).unwrap();
    assert_eq!(cert2.serial_number, 2u64.to_be_bytes().to_vec());

    assert_eq!(ca.serial_counter(), 3);
}

// ============================================================================
// 附加测试：Extension / SignatureAlgorithm 公共 API 可用性
// ============================================================================

/// 验证公共 API 中的 Extension 与 SignatureAlgorithm 可正常使用.
///
/// 这是对 re-export 完整性的补充校验，确保 Task 9 的 lib.rs re-export
/// 覆盖了所有 PKI 公共类型。
#[test]
fn test_public_api_extension_and_sig_alg() {
    // SignatureAlgorithm 可构造与比较
    let alg = SignatureAlgorithm::Sm2WithSm3;
    assert_eq!(alg, SignatureAlgorithm::Sm2WithSm3);

    // Extension 可构造
    let ext = Extension {
        oid: vec![0x55, 0x1D, 0x0F], // KeyUsage OID content bytes (示意)
        critical: true,
        value: vec![0x03, 0x03, 0x00, 0x80, 0x00],
    };
    assert!(ext.critical);
    assert_eq!(ext.value.len(), 5);

    // Sm2PrivateKey 可从字节恢复（验证 from_bytes / to_bytes re-export 可用）
    let kp = gen_keypair();
    let sk_bytes = kp.private_key.to_bytes();
    let restored_sk = Sm2PrivateKey::from_bytes(&sk_bytes).unwrap();
    assert_eq!(restored_sk.to_bytes(), sk_bytes);
}

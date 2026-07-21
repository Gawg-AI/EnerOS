//! SM2 KAT (Known Answer Tests) — GB/T 32918.2/4-2017 集成测试.
//!
//! 本文件为 v0.31.0 Task 14 交付物，验证 `eneros_crypto::sm2` 公共 API 的正确性。
//!
//! # 测试说明
//! SM2 数字签名使用随机数 k，无法使用固定 KAT 向量进行确定性验证（除非使用
//! 确定性 k 生成器）。因此签名测试采用往返一致性（sign → verify）与差分测试
//! （篡改消息/签名/公钥后验签失败）策略。加密测试同理。
//!
//! # 测试覆盖
//! - SubTask 14.1: SM2 数字签名测试（往返、篡改检测、序列化、用户 ID）
//! - SubTask 14.2: SM2 公钥加密测试（往返、篡改检测、空明文、大明文）
//! - SubTask 14.3: SM2 密钥对派生与私钥范围校验测试
//! - 附加测试：多次签名差异性、错误公钥验签、自定义用户 ID、用户 ID 不匹配、
//!   大明文加密、错误私钥解密

use eneros_crypto::bigint::U256;
use eneros_crypto::rng::CsRng;
use eneros_crypto::sm2::{
    sm2_decrypt, sm2_encrypt, EcPoint, Sm2KeyPair, Sm2PrivateKey, Sm2Signature, Sm2Signer, SM2_N,
};

// ============================================================
// 辅助函数
// ============================================================

/// 生成测试用密钥对与 RNG.
fn setup_keypair() -> (Sm2KeyPair, CsRng) {
    let mut rng = CsRng::new();
    let kp = Sm2KeyPair::generate(&mut rng).expect("keypair generation should succeed");
    (kp, rng)
}

// ============================================================
// SubTask 14.1: SM2 数字签名测试
// ============================================================

#[test]
fn test_sm2_sign_verify_round_trip() {
    let (kp, mut rng) = setup_keypair();
    let signer = Sm2Signer::new();

    let messages: &[&[u8]] = &[
        b"",
        b"hello",
        b"Hello, SM2!",
        &[0x42u8; 32],
        &[0xAAu8; 1000],
    ];

    for msg in messages {
        let sig = signer
            .sign(msg, &kp.private_key, &kp.public_key, &mut rng)
            .expect("sign should succeed");
        let valid = signer
            .verify(msg, &sig, &kp.public_key)
            .expect("verify should succeed");
        assert!(
            valid,
            "Signature verification failed for message len {}",
            msg.len()
        );
    }
}

#[test]
fn test_sm2_sign_tampered_message_fails() {
    let (kp, mut rng) = setup_keypair();
    let signer = Sm2Signer::new();

    let msg = b"original message";
    let sig = signer
        .sign(msg, &kp.private_key, &kp.public_key, &mut rng)
        .unwrap();

    // Tamper with message
    let tampered = b"tampered message";
    let valid = signer.verify(tampered, &sig, &kp.public_key).unwrap();
    assert!(!valid, "Tampered message should fail verification");
}

#[test]
fn test_sm2_sign_tampered_signature_fails() {
    let (kp, mut rng) = setup_keypair();
    let signer = Sm2Signer::new();

    let msg = b"test message";
    let mut sig = signer
        .sign(msg, &kp.private_key, &kp.public_key, &mut rng)
        .unwrap();

    // Tamper with r
    sig.r[0] ^= 0xFF;
    let valid = signer.verify(msg, &sig, &kp.public_key).unwrap();
    assert!(!valid, "Tampered r should fail verification");
}

#[test]
fn test_sm2_signature_serialization() {
    let (kp, mut rng) = setup_keypair();
    let signer = Sm2Signer::new();

    let msg = b"serialization test";
    let sig = signer
        .sign(msg, &kp.private_key, &kp.public_key, &mut rng)
        .unwrap();

    let bytes = sig.to_bytes();
    let sig2 = Sm2Signature::from_bytes(&bytes);
    assert_eq!(sig.r, sig2.r, "r should match after serialization");
    assert_eq!(sig.s, sig2.s, "s should match after serialization");

    let valid = signer.verify(msg, &sig2, &kp.public_key).unwrap();
    assert!(valid, "Deserialized signature should verify");
}

// ============================================================
// SubTask 14.2: SM2 公钥加密测试
// ============================================================

#[test]
fn test_sm2_encrypt_decrypt_round_trip() {
    let (kp, mut rng) = setup_keypair();

    let plaintexts: &[&[u8]] = &[
        b"",
        b"a",
        b"hello",
        b"Hello, SM2 encryption!",
        &[0x42u8; 32],
        &[0xAAu8; 100],
        &[0x55u8; 1000],
    ];

    for pt in plaintexts {
        let ct = sm2_encrypt(pt, &kp.public_key, &mut rng).expect("encrypt should succeed");
        let decrypted = sm2_decrypt(&ct, &kp.private_key).expect("decrypt should succeed");
        assert_eq!(
            decrypted,
            *pt,
            "Encryption round-trip failed for len {}",
            pt.len()
        );
    }
}

#[test]
fn test_sm2_encrypt_tampered_ciphertext_fails() {
    let (kp, mut rng) = setup_keypair();
    let pt = b"sensitive data";
    let mut ct = sm2_encrypt(pt, &kp.public_key, &mut rng).unwrap();

    // Tamper with C2 (ciphertext data, after C1+C3 = 65+32 = 97 bytes)
    ct[100] ^= 0xFF;
    let result = sm2_decrypt(&ct, &kp.private_key);
    assert!(
        result.is_err(),
        "Tampered ciphertext should fail decryption"
    );
}

#[test]
fn test_sm2_encrypt_empty_plaintext() {
    let (kp, mut rng) = setup_keypair();
    let pt = b"";
    let ct = sm2_encrypt(pt, &kp.public_key, &mut rng).unwrap();
    // C1 (65) + C3 (32) + C2 (0) = 97 bytes
    assert_eq!(
        ct.len(),
        97,
        "Empty plaintext should produce 97-byte ciphertext"
    );
    let decrypted = sm2_decrypt(&ct, &kp.private_key).unwrap();
    assert_eq!(decrypted, pt);
}

// ============================================================
// SubTask 14.3: SM2 密钥对派生与私钥校验测试
// ============================================================

#[test]
fn test_sm2_keypair_derivation() {
    // Create a private key from a known value
    let d_bytes: [u8; 32] = [
        0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54, 0x32,
        0x10, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54,
        0x32, 0x10,
    ];
    let sk = Sm2PrivateKey::from_bytes(&d_bytes).expect("valid private key");
    let kp = Sm2KeyPair::from_private_key(&sk).expect("keypair derivation should succeed");

    // Verify P = d * G
    let d = U256::from_be_bytes(&d_bytes);
    let expected_point = EcPoint::scalar_base_mult(&d);
    assert_eq!(
        kp.public_key.point.x, expected_point.x,
        "Public key x should match d*G"
    );
    assert_eq!(
        kp.public_key.point.y, expected_point.y,
        "Public key y should match d*G"
    );
    assert!(
        kp.public_key.point.is_on_curve(),
        "Public key should be on curve"
    );
}

#[test]
fn test_sm2_private_key_validation() {
    // Zero private key → error
    let zero = [0u8; 32];
    assert!(
        Sm2PrivateKey::from_bytes(&zero).is_err(),
        "Zero private key should fail"
    );

    // n (curve order) → error (must be < n)
    let n_bytes = SM2_N.to_be_bytes();
    assert!(
        Sm2PrivateKey::from_bytes(&n_bytes).is_err(),
        "Private key = n should fail"
    );

    // n-1 → valid
    let n_copy = U256 { limbs: SM2_N.limbs };
    let one = U256 {
        limbs: [1, 0, 0, 0],
    };
    let n_minus_1 = n_copy.sub_mod(&one, &SM2_N);
    let n_minus_1_bytes = n_minus_1.to_be_bytes();
    assert!(
        Sm2PrivateKey::from_bytes(&n_minus_1_bytes).is_ok(),
        "Private key = n-1 should be valid"
    );
}

// ============================================================
// 附加测试：签名差分与用户 ID
// ============================================================

#[test]
fn test_sm2_multiple_signatures_different() {
    let (kp, mut rng) = setup_keypair();
    let signer = Sm2Signer::new();
    let msg = b"same message for both signatures";

    // Sign the same message twice — different k should produce different signatures
    let sig1 = signer
        .sign(msg, &kp.private_key, &kp.public_key, &mut rng)
        .expect("first sign should succeed");
    let sig2 = signer
        .sign(msg, &kp.private_key, &kp.public_key, &mut rng)
        .expect("second sign should succeed");

    assert_ne!(
        sig1, sig2,
        "Different k should produce different signatures"
    );

    // Both signatures should verify
    assert!(
        signer.verify(msg, &sig1, &kp.public_key).unwrap(),
        "First signature should verify"
    );
    assert!(
        signer.verify(msg, &sig2, &kp.public_key).unwrap(),
        "Second signature should verify"
    );
}

#[test]
fn test_sm2_verify_with_wrong_public_key() {
    let (kp1, mut rng) = setup_keypair();
    let kp2 = Sm2KeyPair::generate(&mut rng).expect("second keypair should generate");
    let signer = Sm2Signer::new();
    let msg = b"verify with wrong public key";

    let sig = signer
        .sign(msg, &kp1.private_key, &kp1.public_key, &mut rng)
        .expect("sign should succeed");

    // Verify with a different public key → should fail
    let valid = signer.verify(msg, &sig, &kp2.public_key).unwrap();
    assert!(!valid, "Verification with wrong public key should fail");
}

#[test]
fn test_sm2_custom_user_id() {
    let (kp, mut rng) = setup_keypair();
    let custom_id = b"my-custom-user-id-2026";
    let signer = Sm2Signer::with_user_id(custom_id);
    let msg = b"custom user id sign/verify test";

    let sig = signer
        .sign(msg, &kp.private_key, &kp.public_key, &mut rng)
        .expect("sign with custom user_id should succeed");

    let valid = signer
        .verify(msg, &sig, &kp.public_key)
        .expect("verify with custom user_id should succeed");
    assert!(valid, "Sign/verify with same custom user_id should pass");
}

#[test]
fn test_sm2_user_id_mismatch() {
    let (kp, mut rng) = setup_keypair();
    let msg = b"user id mismatch test";

    // Sign with user_id_1
    let signer1 = Sm2Signer::with_user_id(b"signer-user-id-one");
    let sig = signer1
        .sign(msg, &kp.private_key, &kp.public_key, &mut rng)
        .expect("sign should succeed");

    // Verify with user_id_2 → should fail
    let signer2 = Sm2Signer::with_user_id(b"signer-user-id-two");
    let valid = signer2
        .verify(msg, &sig, &kp.public_key)
        .expect("verify should not error");
    assert!(!valid, "Verification with mismatched user_id should fail");
}

// ============================================================
// 附加测试：加密边界与错误私钥
// ============================================================

#[test]
fn test_sm2_encrypt_large_plaintext() {
    let (kp, mut rng) = setup_keypair();
    let pt = vec![0x55u8; 1000];

    let ct = sm2_encrypt(&pt, &kp.public_key, &mut rng).expect("encrypt should succeed");
    // C1(65) + C3(32) + C2(1000) = 1097 bytes
    assert_eq!(ct.len(), 1097, "1000-byte plaintext ciphertext length");

    let decrypted = sm2_decrypt(&ct, &kp.private_key).expect("decrypt should succeed");
    assert_eq!(decrypted, pt, "Large plaintext round-trip failed");
}

#[test]
fn test_sm2_decrypt_wrong_private_key() {
    let (kp1, mut rng) = setup_keypair();
    let kp2 = Sm2KeyPair::generate(&mut rng).expect("second keypair should generate");
    let pt = b"secret message for wrong key test";

    let ct = sm2_encrypt(pt, &kp1.public_key, &mut rng).expect("encrypt should succeed");

    // Decrypt with wrong private key → should fail (C3 tag mismatch)
    let result = sm2_decrypt(&ct, &kp2.private_key);
    assert!(
        result.is_err(),
        "Decryption with wrong private key should fail"
    );
}

//! SM4 KAT (Known Answer Tests) — GB/T 32907-2016 集成测试.
//!
//! 本文件为 v0.31.0 Task 13 交付物，验证 `eneros_crypto::sm4` 公共 API 的正确性。
//!
//! # 测试向量来源
//! - **官方 KAT**：GB/T 32907-2016 信息安全技术 SM4 分组密码算法
//!   （标准示例：key = 0123456789abcdeffedcba9876543210）
//! - **独立参考**：通过加解密往返一致性、CBC/GCM 模式差分测试交叉验证
//!
//! # 测试覆盖
//! - SubTask 13.1: SM4-ECB KAT — GB/T 32907-2016 标准示例
//! - SubTask 13.2: SM4 加解密一致性（多分组往返）
//! - SubTask 13.3: SM4-CBC 测试（往返、PKCS#7 填充、IV 差分、填充校验）
//! - SubTask 13.4: SM4-GCM 测试（往返、标签篡改、Nonce 差分、AAD-only）
//! - 附加测试：CBC 不同 IV、CBC 无效填充、GCM 不同 Nonce、GCM 仅 AAD

use eneros_crypto::sm4::cbc::Sm4Cbc;
use eneros_crypto::sm4::gcm::Sm4Gcm;
use eneros_crypto::sm4::Sm4;
use eneros_crypto::CryptoError;

// ============================================================
// SubTask 13.1: SM4-ECB KAT (GB/T 32907-2016 标准示例)
// ============================================================

#[test]
fn test_sm4_ecb_kat() {
    // GB/T 32907-2016 示例:
    // key = 0123456789abcdeffedcba9876543210
    // plaintext = 0123456789abcdeffedcba9876543210
    // ciphertext = 681edf34d206965e86b3e94f536e4246
    let key: [u8; 16] = [
        0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54, 0x32,
        0x10,
    ];
    let plaintext: [u8; 16] = key; // same as key for this KAT
    let expected_ct: [u8; 16] = [
        0x68, 0x1e, 0xdf, 0x34, 0xd2, 0x06, 0x96, 0x5e, 0x86, 0xb3, 0xe9, 0x4f, 0x53, 0x6e, 0x42,
        0x46,
    ];

    let cipher = Sm4::new(&key);
    let ct = cipher.encrypt_block(&plaintext);
    assert_eq!(ct, expected_ct, "SM4-ECB encrypt KAT failed");

    let pt = cipher.decrypt_block(&ct);
    assert_eq!(pt, plaintext, "SM4-ECB decrypt KAT failed");
}

// ============================================================
// SubTask 13.2: SM4 encrypt/decrypt consistency
// ============================================================

#[test]
fn test_sm4_encrypt_decrypt_consistency() {
    let key: [u8; 16] = [
        0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54, 0x32,
        0x10,
    ];
    let cipher = Sm4::new(&key);

    // Test multiple plaintext blocks
    for i in 0..10u8 {
        let mut pt = [0u8; 16];
        for (j, pt_j) in pt.iter_mut().enumerate() {
            *pt_j = i.wrapping_add(j as u8);
        }
        let ct = cipher.encrypt_block(&pt);
        let decrypted = cipher.decrypt_block(&ct);
        assert_eq!(decrypted, pt, "Block {} round-trip failed", i);
    }
}

// ============================================================
// SubTask 13.3: SM4-CBC tests
// ============================================================

#[test]
fn test_sm4_cbc_round_trip() {
    let key: [u8; 16] = [0x42u8; 16];
    let iv: [u8; 16] = [0x00u8; 16];
    let cipher = Sm4Cbc::new(&key, &iv);

    let plaintexts: &[&[u8]] = &[
        b"",
        b"a",
        b"hello",
        b"123456789012345",   // 15 bytes
        b"1234567890123456",  // 16 bytes
        b"12345678901234567", // 17 bytes
        b"The quick brown fox jumps over the lazy dog",
    ];

    for pt in plaintexts {
        let ct = cipher.encrypt(pt);
        let decrypted = cipher.decrypt(&ct).expect("decrypt should succeed");
        assert_eq!(decrypted, *pt, "CBC round-trip failed for len {}", pt.len());
    }
}

#[test]
fn test_sm4_cbc_pkcs7_padding() {
    let key: [u8; 16] = [0x42u8; 16];
    let iv: [u8; 16] = [0x00u8; 16];
    let cipher = Sm4Cbc::new(&key, &iv);

    // 16 bytes → padding adds full block (pad_len = 16)
    let pt16 = [0xAAu8; 16];
    let ct = cipher.encrypt(&pt16);
    assert_eq!(
        ct.len(),
        32,
        "16-byte plaintext should produce 32-byte ciphertext"
    );

    // 15 bytes → padding adds 1 byte
    let pt15 = [0xBBu8; 15];
    let ct15 = cipher.encrypt(&pt15);
    assert_eq!(
        ct15.len(),
        16,
        "15-byte plaintext should produce 16-byte ciphertext"
    );
}

// ============================================================
// SubTask 13.4: SM4-GCM tests
// ============================================================

#[test]
fn test_sm4_gcm_round_trip() {
    let key: [u8; 16] = [0x42u8; 16];
    let nonce: [u8; 12] = [0x00u8; 12];
    let cipher = Sm4Gcm::new(&key);

    let test_cases: &[(&[u8], &[u8])] = &[
        (b"", b""),                   // empty PT, empty AAD
        (b"hello", b""),              // PT only
        (b"", b"aad"),                // AAD only
        (b"hello", b"aad"),           // both
        (&[0x42u8; 16], b""),         // block-aligned
        (&[0x42u8; 20], b"aad data"), // non-block-aligned
    ];

    for (pt, aad) in test_cases {
        let (ct, tag) = cipher.encrypt(&nonce, pt, aad);
        let decrypted = cipher
            .decrypt(&nonce, &ct, aad, &tag)
            .expect("decrypt should succeed");
        assert_eq!(
            decrypted,
            *pt,
            "GCM round-trip failed for pt_len={}, aad_len={}",
            pt.len(),
            aad.len()
        );
    }
}

#[test]
fn test_sm4_gcm_tag_tamper_detection() {
    let key: [u8; 16] = [0x42u8; 16];
    let nonce: [u8; 12] = [0x00u8; 12];
    let cipher = Sm4Gcm::new(&key);

    let pt = b"sensitive data";
    let aad = b"auth";
    let (ct, mut tag) = cipher.encrypt(&nonce, pt, aad);

    // Tamper with tag
    tag[0] ^= 0xFF;
    let result = cipher.decrypt(&nonce, &ct, aad, &tag);
    assert!(result.is_err(), "Tampered tag should fail decryption");
    assert_eq!(result, Err(CryptoError::TagMismatch));

    // Restore and tamper with ciphertext
    tag[0] ^= 0xFF;
    let mut ct_tampered = ct.clone();
    ct_tampered[0] ^= 0xFF;
    let result = cipher.decrypt(&nonce, &ct_tampered, aad, &tag);
    assert!(
        result.is_err(),
        "Tampered ciphertext should fail decryption"
    );
    assert_eq!(result, Err(CryptoError::TagMismatch));
}

// ============================================================
// Additional tests: CBC differential and padding validation
// ============================================================

#[test]
fn test_sm4_cbc_different_iv() {
    // 相同明文，不同 IV → 不同密文
    let key: [u8; 16] = [0x42u8; 16];
    let iv1: [u8; 16] = [0x00u8; 16];
    let iv2: [u8; 16] = [0xFFu8; 16];
    let plaintext = b"same plaintext for both ivs";

    let c1 = Sm4Cbc::new(&key, &iv1);
    let c2 = Sm4Cbc::new(&key, &iv2);
    let ct1 = c1.encrypt(plaintext);
    let ct2 = c2.encrypt(plaintext);
    assert_ne!(ct1, ct2, "different IVs should yield different ciphertexts");

    // 两者均能正确解密回原明文
    let pt1 = c1.decrypt(&ct1).expect("decrypt with iv1 should succeed");
    let pt2 = c2.decrypt(&ct2).expect("decrypt with iv2 should succeed");
    assert_eq!(pt1, plaintext);
    assert_eq!(pt2, plaintext);
}

#[test]
fn test_sm4_cbc_invalid_padding() {
    // 构造有效密文后破坏填充字节，解密应返回 InvalidPadding 错误
    let key: [u8; 16] = [0x42u8; 16];
    let iv: [u8; 16] = [0x00u8; 16];
    let cipher = Sm4Cbc::new(&key, &iv);

    let plaintext = b"padding corruption test";
    let mut ciphertext = cipher.encrypt(plaintext);
    // 翻转最后一字节（破坏 PKCS#7 填充字节值）
    let last = ciphertext.len() - 1;
    ciphertext[last] ^= 0xFF;
    let result = cipher.decrypt(&ciphertext);
    assert!(
        matches!(result, Err(CryptoError::InvalidPadding)),
        "corrupted padding should yield InvalidPadding"
    );
}

// ============================================================
// Additional tests: GCM differential and AAD-only
// ============================================================

#[test]
fn test_sm4_gcm_different_nonce() {
    // 相同明文，不同 Nonce → 不同密文与不同标签
    let key: [u8; 16] = [0x42u8; 16];
    let nonce1: [u8; 12] = [0x00u8; 12];
    let mut nonce2: [u8; 12] = [0x00u8; 12];
    nonce2[0] ^= 0xFF;
    let cipher = Sm4Gcm::new(&key);

    let plaintext = b"same plaintext for both nonces";
    let (ct1, tag1) = cipher.encrypt(&nonce1, plaintext, b"");
    let (ct2, tag2) = cipher.encrypt(&nonce2, plaintext, b"");
    assert_ne!(
        ct1, ct2,
        "different nonces should yield different ciphertexts"
    );
    assert_ne!(tag1, tag2, "different nonces should yield different tags");

    // 两者均能正确解密回原明文
    let pt1 = cipher
        .decrypt(&nonce1, &ct1, b"", &tag1)
        .expect("decrypt with nonce1 should succeed");
    let pt2 = cipher
        .decrypt(&nonce2, &ct2, b"", &tag2)
        .expect("decrypt with nonce2 should succeed");
    assert_eq!(pt1, plaintext);
    assert_eq!(pt2, plaintext);
}

#[test]
fn test_sm4_gcm_empty_plaintext_with_aad() {
    // AEAD 仅认证（空明文 + 非空 AAD）：密文为空，标签非零且可验证
    let key: [u8; 16] = [0x42u8; 16];
    let nonce: [u8; 12] = [0x00u8; 12];
    let cipher = Sm4Gcm::new(&key);

    let aad = b"authentication only, no payload";
    let (ct, tag) = cipher.encrypt(&nonce, b"", aad);
    assert!(
        ct.is_empty(),
        "empty plaintext should yield empty ciphertext"
    );
    // 标签不应全零（极大概率）
    assert!(
        tag.iter().any(|&b| b != 0),
        "tag should be non-zero for non-empty AAD"
    );

    // 正确 AAD 能解密成功（返回空明文）
    let decrypted = cipher
        .decrypt(&nonce, &ct, aad, &tag)
        .expect("decrypt should succeed with correct AAD");
    assert!(decrypted.is_empty());

    // 篡改 AAD 应导致标签验证失败
    let tampered_aad = b"authentication only, no payloaX";
    let result = cipher.decrypt(&nonce, &ct, tampered_aad, &tag);
    assert_eq!(
        result,
        Err(CryptoError::TagMismatch),
        "tampered AAD should fail tag verification"
    );
}

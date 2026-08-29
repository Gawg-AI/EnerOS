//! 国密硬件加速引擎集成测试 (workspace v0.192.0).
//!
//! 验证 CryptoEngine trait 实现、CryptoSelector fallback 逻辑、
//! 国标测试向量（GM/T 0003/0004/0002）。
//!
//! SPEC-SERIES-2026-08-27/A · WP-A.3 收编注：依赖 `new_simulated` /
//! `create_simulated_selector` 的用例已迁入 `src/hw/tests_sim.rs`
//! （构造器已门控为 `#[cfg(any(test, feature = "simulated"))]`，
//! 默认 feature 的集成测试构建不再可见）。本文件保留纯软件引擎用例。

use eneros_crypto::hw::{create_software_selector, CryptoEngine};
use eneros_crypto::rng::CsRng;
use eneros_crypto::sm2::Sm2KeyPair;

/// 确定性测试种子（与 `CsRng::new` 内部种子逐字节一致——WP-53.2：release
/// 构建下 `new()` 被门禁移除，集成测试改走 `from_seed` 保持序列不变）。
const TEST_FIXED_SEED: [u8; 32] = [
    0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36,
    0x37, 0x38, 0x39, 0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x30, 0x31, 0x32,
];

#[test]
fn test_sm3_kat_abc_software_engine() {
    // SM3("abc") 国标结果
    const SM3_KAT_ABC: [u8; 32] = [
        0x66, 0xC7, 0xF0, 0xF4, 0x62, 0xEE, 0xED, 0xD9, 0xD1, 0xF2, 0xD4, 0x6B, 0xDC, 0x10, 0xE4,
        0xE2, 0x41, 0x67, 0xC4, 0x87, 0x5C, 0xF2, 0xF7, 0xA2, 0x29, 0x7D, 0xA0, 0x2B, 0x8F, 0x4B,
        0xA8, 0xE0,
    ];
    let mut engine: Box<dyn CryptoEngine> =
        Box::new(eneros_crypto::hw::SoftwareCryptoEngine::new());
    let hash = engine.sm3_hash(b"abc").unwrap();
    assert_eq!(hash, SM3_KAT_ABC);
}

#[test]
fn test_sm4_ecb_kat_software_engine() {
    // 国标测试向量: SM4-ECB(key, key) = 681EDF34D206965E86B3E94F536E4246
    let sm4_key: [u8; 16] = [
        0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54, 0x32,
        0x10,
    ];
    const SM4_ECB_KAT: [u8; 16] = [
        0x68, 0x1E, 0xDF, 0x34, 0xD2, 0x06, 0x96, 0x5E, 0x86, 0xB3, 0xE9, 0x4F, 0x53, 0x6E, 0x42,
        0x46,
    ];
    let mut engine: Box<dyn CryptoEngine> =
        Box::new(eneros_crypto::hw::SoftwareCryptoEngine::new());
    let ct = engine
        .sm4_encrypt(&sm4_key, None, eneros_crypto::hw::Sm4Mode::Ecb, &sm4_key)
        .unwrap();
    assert_eq!(ct, SM4_ECB_KAT);
    let pt = engine
        .sm4_decrypt(&sm4_key, None, eneros_crypto::hw::Sm4Mode::Ecb, &ct)
        .unwrap();
    assert_eq!(pt, sm4_key);
}

#[test]
fn test_sm2_sign_verify_software_engine() {
    let mut rng = CsRng::from_seed(&TEST_FIXED_SEED);
    let kp = Sm2KeyPair::generate(&mut rng).unwrap();
    let sk_bytes = kp.private_key.to_bytes();
    let pk_bytes = kp.public_key.to_bytes_uncompressed();
    let msg = b"SM2 integration test message for v0.158.0";

    let mut engine: Box<dyn CryptoEngine> =
        Box::new(eneros_crypto::hw::SoftwareCryptoEngine::new());
    let sig = engine.sm2_sign(&sk_bytes, msg).unwrap();
    assert_ne!(sig, [0u8; 64], "signature should not be all zeros");

    let valid = engine.sm2_verify(&pk_bytes, msg, &sig).unwrap();
    assert!(valid, "SM2 verify failed");

    // 篡改消息后验签应失败
    let invalid = engine.sm2_verify(&pk_bytes, b"tampered", &sig).unwrap();
    assert!(!invalid, "SM2 tampered verify should fail");
}

#[test]
fn test_sm3_selector_consistency() {
    // CryptoSelector 与直接调用软件实现结果一致
    let mut selector = create_software_selector();
    let data = b"integration test data for SM3";
    let hash_selector = selector.sm3_hash(data).unwrap();
    let hash_direct = eneros_crypto::sm3::hash(data);
    assert_eq!(hash_selector, hash_direct);
}

// ============================================================
// CryptoSelector fallback 集成测试
// ============================================================

#[test]
fn test_selector_software_only() {
    let mut selector = create_software_selector();
    assert_eq!(selector.engine_count(), 1);
    assert!(!selector.is_active_hardware());
    assert_eq!(selector.active_engine_name(), "Software Crypto (Pure Rust)");

    let hash = selector.sm3_hash(b"software only").unwrap();
    assert_eq!(hash.len(), 32);
}

// ============================================================
// 引擎信息测试
// ============================================================

#[test]
fn test_engine_metadata_software() {
    let sw = eneros_crypto::hw::SoftwareCryptoEngine::new();
    assert_eq!(sw.name(), "Software Crypto (Pure Rust)");
    assert!(!sw.is_hardware());
    assert!(sw.is_available());
}

#[test]
fn test_engine_stats_accumulation() {
    let mut selector = create_software_selector();

    // 执行多次操作
    for i in 0..10 {
        let data = [i as u8; 32];
        let _ = selector.sm3_hash(&data);
    }

    let stats = selector.stats();
    assert_eq!(stats.total_ops, 10);
    assert_eq!(stats.success_ops, 10);
    assert_eq!(stats.failed_ops, 0);
    assert_eq!(stats.success_rate(), 100);
}

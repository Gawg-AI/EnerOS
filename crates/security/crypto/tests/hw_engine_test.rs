//! 国密硬件加速引擎集成测试 (workspace v0.192.0).
//!
//! 验证 CryptoEngine trait 实现、CryptoSelector fallback 逻辑、
//! 国标测试向量（GM/T 0003/0004/0002）。

use eneros_crypto::hw::{
    create_simulated_selector, create_software_selector, CryptoEngine, KunpengKaeEngine,
    PhytiumCryptoEngine, Sm4Mode, SoftwareCryptoEngine,
};
use eneros_crypto::rng::CsRng;
use eneros_crypto::sm2::Sm2KeyPair;

// ============================================================
// SM3 国标测试向量 (GB/T 32905-2016)
// ============================================================

/// GB/T 32905-2016 示例 1: SM3("abc")
const SM3_KAT_ABC: [u8; 32] = [
    0x66, 0xC7, 0xF0, 0xF4, 0x62, 0xEE, 0xED, 0xD9, 0xD1, 0xF2, 0xD4, 0x6B, 0xDC, 0x10, 0xE4, 0xE2,
    0x41, 0x67, 0xC4, 0x87, 0x5C, 0xF2, 0xF7, 0xA2, 0x29, 0x7D, 0xA0, 0x2B, 0x8F, 0x4B, 0xA8, 0xE0,
];

/// GB/T 32905-2016 示例 2: SM3("abcd" × 16)
const SM3_KAT_64B: [u8; 32] = [
    0xDE, 0xBE, 0x9F, 0xF9, 0x22, 0x75, 0xB8, 0xA1, 0x38, 0x60, 0x48, 0x89, 0xC1, 0x8E, 0x5A, 0x4D,
    0x6F, 0xDB, 0x70, 0xE5, 0x38, 0x7E, 0x57, 0x65, 0x29, 0x3D, 0xCB, 0xA3, 0x9C, 0x0C, 0x57, 0x32,
];

// ============================================================
// SM4 国标测试向量 (GB/T 32907-2016)
// ============================================================

/// GB/T 32907-2016 示例: SM4-ECB(key, key)
const SM4_KEY: [u8; 16] = [
    0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54, 0x32, 0x10,
];

const SM4_ECB_KAT: [u8; 16] = [
    0x68, 0x1E, 0xDF, 0x34, 0xD2, 0x06, 0x96, 0x5E, 0x86, 0xB3, 0xE9, 0x4F, 0x53, 0x6E, 0x42, 0x46,
];

// ============================================================
// SM3 集成测试
// ============================================================

#[test]
fn test_sm3_kat_abc_all_engines() {
    // 所有引擎的 SM3("abc") 应产生相同的国标结果
    let mut engines: Vec<Box<dyn eneros_crypto::hw::CryptoEngine>> = vec![
        Box::new(SoftwareCryptoEngine::new()),
        Box::new(PhytiumCryptoEngine::new_simulated()),
        Box::new(KunpengKaeEngine::new_simulated()),
    ];

    for engine in engines.iter_mut() {
        let hash = engine.sm3_hash(b"abc").unwrap();
        assert_eq!(
            hash,
            SM3_KAT_ABC,
            "SM3 KAT failed for engine: {}",
            engine.name()
        );
    }
}

#[test]
fn test_sm3_kat_64b_all_engines() {
    let msg = b"abcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcd";
    let mut engines: Vec<Box<dyn eneros_crypto::hw::CryptoEngine>> = vec![
        Box::new(SoftwareCryptoEngine::new()),
        Box::new(PhytiumCryptoEngine::new_simulated()),
        Box::new(KunpengKaeEngine::new_simulated()),
    ];

    for engine in engines.iter_mut() {
        let hash = engine.sm3_hash(msg).unwrap();
        assert_eq!(
            hash,
            SM3_KAT_64B,
            "SM3 64B KAT failed for engine: {}",
            engine.name()
        );
    }
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
// SM4 集成测试
// ============================================================

#[test]
fn test_sm4_ecb_kat_all_engines() {
    // 国标测试向量: SM4-ECB(key, key) = 681EDF34D206965E86B3E94F536E4246
    let mut engines: Vec<Box<dyn eneros_crypto::hw::CryptoEngine>> = vec![
        Box::new(SoftwareCryptoEngine::new()),
        Box::new(PhytiumCryptoEngine::new_simulated()),
        Box::new(KunpengKaeEngine::new_simulated()),
    ];

    for engine in engines.iter_mut() {
        let ct = engine
            .sm4_encrypt(&SM4_KEY, None, Sm4Mode::Ecb, &SM4_KEY)
            .unwrap();
        assert_eq!(
            ct,
            SM4_ECB_KAT,
            "SM4-ECB KAT failed for engine: {}",
            engine.name()
        );

        // 解密验证
        let pt = engine
            .sm4_decrypt(&SM4_KEY, None, Sm4Mode::Ecb, &ct)
            .unwrap();
        assert_eq!(pt, SM4_KEY);
    }
}

#[test]
fn test_sm4_cbc_roundtrip_all_engines() {
    let key = [0xAAu8; 16];
    let iv = [0xBBu8; 16];
    let plaintext = b"SM4-CBC integration test data!"; // 30 bytes

    let mut engines: Vec<Box<dyn eneros_crypto::hw::CryptoEngine>> = vec![
        Box::new(SoftwareCryptoEngine::new()),
        Box::new(PhytiumCryptoEngine::new_simulated()),
        Box::new(KunpengKaeEngine::new_simulated()),
    ];

    for engine in engines.iter_mut() {
        let ct = engine
            .sm4_encrypt(&key, Some(&iv), Sm4Mode::Cbc, plaintext)
            .unwrap();
        assert_ne!(&ct[..], &plaintext[..], "ciphertext should differ");

        let pt = engine
            .sm4_decrypt(&key, Some(&iv), Sm4Mode::Cbc, &ct)
            .unwrap();
        assert_eq!(
            pt,
            plaintext,
            "SM4-CBC roundtrip failed for engine: {}",
            engine.name()
        );
    }
}

// ============================================================
// SM2 集成测试
// ============================================================

#[test]
fn test_sm2_sign_verify_all_engines() {
    let mut rng = CsRng::new();
    let kp = Sm2KeyPair::generate(&mut rng).unwrap();
    let sk_bytes = kp.private_key.to_bytes();
    let pk_bytes = kp.public_key.to_bytes_uncompressed();
    let msg = b"SM2 integration test message for v0.158.0";

    let mut engines: Vec<Box<dyn eneros_crypto::hw::CryptoEngine>> = vec![
        Box::new(SoftwareCryptoEngine::new()),
        Box::new(PhytiumCryptoEngine::new_simulated()),
        Box::new(KunpengKaeEngine::new_simulated()),
    ];

    for engine in engines.iter_mut() {
        let sig = engine.sm2_sign(&sk_bytes, msg).unwrap();
        assert_ne!(sig, [0u8; 64], "signature should not be all zeros");

        let valid = engine.sm2_verify(&pk_bytes, msg, &sig).unwrap();
        assert!(valid, "SM2 verify failed for engine: {}", engine.name());

        // 篡改消息后验签应失败
        let invalid = engine.sm2_verify(&pk_bytes, b"tampered", &sig).unwrap();
        assert!(
            !invalid,
            "SM2 tampered verify should fail for engine: {}",
            engine.name()
        );
    }
}

// ============================================================
// CryptoSelector fallback 集成测试
// ============================================================

#[test]
fn test_selector_hardware_first_strategy() {
    // 模拟选择器：硬件引擎优先
    let mut selector = create_simulated_selector();
    assert_eq!(selector.engine_count(), 2);
    assert!(selector.is_active_hardware());
    assert_eq!(selector.active_engine_name(), "Phytium D2000 Crypto Engine");

    // 功能正常
    let hash = selector.sm3_hash(b"test").unwrap();
    assert_eq!(hash, eneros_crypto::sm3::hash(b"test"));
}

#[test]
fn test_selector_software_only() {
    let mut selector = create_software_selector();
    assert_eq!(selector.engine_count(), 1);
    assert!(!selector.is_active_hardware());
    assert_eq!(selector.active_engine_name(), "Software Crypto (Pure Rust)");

    let hash = selector.sm3_hash(b"software only").unwrap();
    assert_eq!(hash.len(), 32);
}

#[test]
fn test_selector_full_crypto_workflow() {
    // 完整密码学工作流：SM3 + SM4 + SM2
    let mut selector = create_simulated_selector();

    // 1. SM3 哈希
    let data = b"workflow integration data";
    let hash = selector.sm3_hash(data).unwrap();
    assert_eq!(hash.len(), 32);

    // 2. SM4-CBC 加密
    let key = [0x55u8; 16];
    let iv = [0x66u8; 16];
    let ct = selector
        .sm4_encrypt(&key, Some(&iv), Sm4Mode::Cbc, data)
        .unwrap();
    let pt = selector
        .sm4_decrypt(&key, Some(&iv), Sm4Mode::Cbc, &ct)
        .unwrap();
    assert_eq!(pt, data);

    // 3. SM2 签名/验签
    let mut rng = CsRng::new();
    let kp = Sm2KeyPair::generate(&mut rng).unwrap();
    let sk = kp.private_key.to_bytes();
    let pk = kp.public_key.to_bytes_uncompressed();

    let sig = selector.sm2_sign(&sk, data).unwrap();
    let valid = selector.sm2_verify(&pk, data, &sig).unwrap();
    assert!(valid);
}

// ============================================================
// 引擎信息测试
// ============================================================

#[test]
fn test_engine_metadata() {
    let sw = SoftwareCryptoEngine::new();
    assert_eq!(sw.name(), "Software Crypto (Pure Rust)");
    assert!(!sw.is_hardware());
    assert!(sw.is_available());

    let phy = PhytiumCryptoEngine::new_simulated();
    assert_eq!(phy.name(), "Phytium D2000 Crypto Engine");
    assert!(phy.is_hardware());
    assert!(phy.is_available());

    let kae = KunpengKaeEngine::new_simulated();
    assert_eq!(kae.name(), "Kunpeng 920 KAE");
    assert!(kae.is_hardware());
    assert!(kae.is_available());
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

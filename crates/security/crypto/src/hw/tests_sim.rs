//! 模拟硬件引擎行为测试（SPEC-SERIES-2026-08-27/A · WP-A.3）.
//!
//! 自 `tests/hw_engine_test.rs` 迁入：`PhytiumCryptoEngine::new_simulated`
//! 与 `create_simulated_selector` 已门控为 `#[cfg(any(test, feature = "simulated"))]`，
//! 集成测试的默认 feature 构建不再可见，而单测域（cfg(test)）是合法消费面。
//! 断言同步转为诚实语义（鲲鹏模拟态自报非硬件）。

use alloc::boxed::Box;
use alloc::vec::Vec;

use super::engine::{CryptoEngine, Sm4Mode};
use super::kunpeng::KunpengKaeEngine;
use super::phytium::PhytiumCryptoEngine;
use super::{create_simulated_selector, create_software_selector, software::SoftwareCryptoEngine};
use crate::rng::CsRng;
use crate::sm2::Sm2KeyPair;

/// 确定性测试种子（与迁出前集成测试一致；WP-53.2 先例：from_seed 保持序列不变）。
const TEST_FIXED_SEED: [u8; 32] = [
    0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36,
    0x37, 0x38, 0x39, 0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x30, 0x31, 0x32,
];

/// SM3 国标测试向量 (GB/T 32905-2016 示例 1: SM3("abc"))
const SM3_KAT_ABC: [u8; 32] = [
    0x66, 0xC7, 0xF0, 0xF4, 0x62, 0xEE, 0xED, 0xD9, 0xD1, 0xF2, 0xD4, 0x6B, 0xDC, 0x10, 0xE4, 0xE2,
    0x41, 0x67, 0xC4, 0x87, 0x5C, 0xF2, 0xF7, 0xA2, 0x29, 0x7D, 0xA0, 0x2B, 0x8F, 0x4B, 0xA8, 0xE0,
];

/// SM3 国标测试向量 (GB/T 32905-2016 示例 2: SM3("abcd" × 16))
const SM3_KAT_64B: [u8; 32] = [
    0xDE, 0xBE, 0x9F, 0xF9, 0x22, 0x75, 0xB8, 0xA1, 0x38, 0x60, 0x48, 0x89, 0xC1, 0x8E, 0x5A, 0x4D,
    0x6F, 0xDB, 0x70, 0xE5, 0x38, 0x7E, 0x57, 0x65, 0x29, 0x3D, 0xCB, 0xA3, 0x9C, 0x0C, 0x57, 0x32,
];

/// SM4 国标测试向量 (GB/T 32907-2016: key = plaintext)
const SM4_KEY: [u8; 16] = [
    0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54, 0x32, 0x10,
];

/// SM4-ECB KAT 密文
const SM4_ECB_KAT: [u8; 16] = [
    0x68, 0x1E, 0xDF, 0x34, 0xD2, 0x06, 0x96, 0x5E, 0x86, 0xB3, 0xE9, 0x4F, 0x53, 0x6E, 0x42, 0x46,
];

fn simulated_engines() -> Vec<Box<dyn CryptoEngine>> {
    vec![
        Box::new(SoftwareCryptoEngine::new()),
        Box::new(PhytiumCryptoEngine::new_simulated()),
        Box::new(KunpengKaeEngine::new_simulated()),
    ]
}

#[test]
fn test_sm3_kat_abc_all_engines() {
    for engine in simulated_engines().iter_mut() {
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
    for engine in simulated_engines().iter_mut() {
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
fn test_sm4_ecb_kat_all_engines() {
    // 国标测试向量: SM4-ECB(key, key) = 681EDF34D206965E86B3E94F536E4246
    for engine in simulated_engines().iter_mut() {
        let ct = engine
            .sm4_encrypt(&SM4_KEY, None, Sm4Mode::Ecb, &SM4_KEY)
            .unwrap();
        assert_eq!(
            ct,
            SM4_ECB_KAT,
            "SM4-ECB KAT failed for engine: {}",
            engine.name()
        );

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

    for engine in simulated_engines().iter_mut() {
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

#[test]
fn test_sm2_sign_verify_all_engines() {
    let mut rng = CsRng::from_seed(&TEST_FIXED_SEED);
    let kp = Sm2KeyPair::generate(&mut rng).unwrap();
    let sk_bytes = kp.private_key.to_bytes();
    let pk_bytes = kp.public_key.to_bytes_uncompressed();
    let msg = b"SM2 integration test message for v0.158.0";

    for engine in simulated_engines().iter_mut() {
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

#[test]
fn test_selector_hardware_first_strategy() {
    // 模拟选择器：硬件引擎优先（飞腾模拟引擎行为未变——WP-A.3 仅门控其构造器）
    let mut selector = create_simulated_selector();
    assert_eq!(selector.engine_count(), 2);
    assert!(selector.is_active_hardware());
    assert_eq!(selector.active_engine_name(), "Phytium D2000 Crypto Engine");

    // 功能正常
    let hash = selector.sm3_hash(b"test").unwrap();
    assert_eq!(hash, crate::sm3::hash(b"test"));
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
    let mut rng = CsRng::from_seed(&TEST_FIXED_SEED);
    let kp = Sm2KeyPair::generate(&mut rng).unwrap();
    let sk = kp.private_key.to_bytes();
    let pk = kp.public_key.to_bytes_uncompressed();

    let sig = selector.sm2_sign(&sk, data).unwrap();
    let valid = selector.sm2_verify(&pk, data, &sig).unwrap();
    assert!(valid);
}

#[test]
fn test_engine_metadata_honest_self_report() {
    let sw = SoftwareCryptoEngine::new();
    assert_eq!(sw.name(), "Software Crypto (Pure Rust)");
    assert!(!sw.is_hardware());
    assert!(sw.is_available());

    // 飞腾模拟引擎：构造已门控（F-C2），报告语义不在本卷改动面。
    let phy = PhytiumCryptoEngine::new_simulated();
    assert_eq!(phy.name(), "Phytium D2000 Crypto Engine");
    assert!(phy.is_hardware());
    assert!(phy.is_available());

    // 鲲鹏模拟引擎（F-C1）：如实自报——非硬件、不可用、名字带 simulated 后缀。
    let kae = KunpengKaeEngine::new_simulated();
    assert_eq!(kae.name(), "Kunpeng 920 KAE (simulated)");
    assert!(!kae.is_hardware());
    assert!(!kae.is_available());

    // 软件选择器不受影响
    let sel = create_software_selector();
    assert_eq!(sel.active_engine_name(), "Software Crypto (Pure Rust)");
}

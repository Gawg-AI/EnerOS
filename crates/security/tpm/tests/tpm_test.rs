#![cfg(feature = "simulated")]
//! TPM 2.0 集成测试 (workspace v0.192.0).
//!
//! 验证完整启动链度量、密钥管理和平台完整性验证。

use eneros_tpm::{
    BootStage, KeyManager, PcrManager, PcrStrategy, SimulatedTpm, TpmDriver, TpmMeasurement,
    PCR_BOOT, PCR_SECURE_BOOT,
};

// ============================================================
// 完整启动链度量测试
// ============================================================

#[test]
fn test_full_boot_chain_measurement() {
    let mut tpm = SimulatedTpm::new();
    tpm.init().unwrap();

    let mut measurement = TpmMeasurement::new(&mut tpm);

    // 模拟完整启动链
    measurement
        .measure(BootStage::Bootloader, b"EnerOS Bootloader v1.0", "firmware")
        .unwrap();
    measurement
        .measure(BootStage::Kernel, b"EnerOS Kernel image", "kernel")
        .unwrap();
    measurement
        .measure(BootStage::KernelConfig, b"device-tree-blob", "dtb")
        .unwrap();
    measurement
        .measure(BootStage::Userland, b"runtime + agents", "userland")
        .unwrap();
    measurement
        .measure(
            BootStage::SecureBootPolicy,
            b"secure boot policy v1",
            "policy",
        )
        .unwrap();

    // 验证日志
    assert_eq!(measurement.log_count(), 5);
    let log = measurement.measurement_log();
    assert_eq!(log[0].stage, BootStage::Bootloader);
    assert_eq!(log[4].stage, BootStage::SecureBootPolicy);

    // PCR 应被修改
    let pcr0 = tpm.pcr_read(PCR_BOOT).unwrap();
    assert_ne!(pcr0, [0u8; 32]);
    let pcr7 = tpm.pcr_read(PCR_SECURE_BOOT).unwrap();
    assert_ne!(pcr7, [0u8; 32]);
}

// ============================================================
// 平台完整性验证测试
// ============================================================

#[test]
fn test_platform_integrity_verification_pass() {
    let mut tpm = SimulatedTpm::new();
    tpm.init().unwrap();

    // 度量已知数据
    let boot_data = b"trusted bootloader";
    let digest = eneros_crypto::sm3::hash(boot_data);
    tpm.pcr_extend(PCR_BOOT, &digest).unwrap();

    // 读取 PCR 0 当前值作为期望值
    let pcr0_value = tpm.pcr_read(PCR_BOOT).unwrap();
    let strategy = PcrStrategy::new().with_boot(pcr0_value);

    // 验证应通过
    let mut measurement = TpmMeasurement::new(&mut tpm);
    let result = measurement.verify_integrity(&strategy).unwrap();
    assert!(result);
}

#[test]
fn test_platform_integrity_verification_fail() {
    let mut tpm = SimulatedTpm::new();
    tpm.init().unwrap();

    let mut measurement = TpmMeasurement::new(&mut tpm);
    measurement
        .measure(BootStage::Bootloader, b"actual bootloader", "boot")
        .unwrap();

    // 期望值不匹配
    let strategy = PcrStrategy::new().with_boot([0xDE; 32]);
    let result = measurement.verify_integrity(&strategy).unwrap();
    assert!(!result);
}

// ============================================================
// 密钥管理集成测试
// ============================================================

#[test]
fn test_key_management_full_workflow() {
    let mut tpm = SimulatedTpm::new();
    tpm.init().unwrap();

    let mut key_mgr = KeyManager::new(&mut tpm);

    // 初始化密钥层次
    key_mgr.init_hierarchy(b"owner_auth").unwrap();
    assert!(key_mgr.hierarchy().srk.is_some());
    assert!(key_mgr.hierarchy().ak.is_some());

    // 创建 Agent 密钥
    let agent_key = key_mgr.create_agent_key(b"agent1").unwrap();

    // 签名
    let sig = key_mgr.sign(agent_key, b"agent message").unwrap();
    assert_eq!(sig.len(), 64);

    // 证明（使用 AK）
    let attestation = key_mgr.attest(b"platform state").unwrap();
    assert_eq!(attestation.len(), 64);

    // Seal/Unseal
    let secret = b"encryption key material";
    let sealed = key_mgr.seal(secret).unwrap();
    let unsealed = key_mgr.unseal(&sealed).unwrap();
    assert_eq!(unsealed, secret);
}

// ============================================================
// 远程证明测试
// ============================================================

#[test]
fn test_remote_attestation_quote() {
    let mut tpm = SimulatedTpm::new();
    tpm.init().unwrap();

    let mut measurement = TpmMeasurement::new(&mut tpm);
    measurement
        .measure(BootStage::Bootloader, b"boot", "firmware")
        .unwrap();
    measurement
        .measure(BootStage::SecureBootPolicy, b"policy", "secure boot")
        .unwrap();

    // 生成 Quote
    let quote = measurement.generate_quote(b"verifier_nonce_123").unwrap();
    assert_ne!(quote.quote_digest, [0u8; 32]);
    assert_eq!(quote.measurement_count, 2);
    assert_eq!(quote.nonce, b"verifier_nonce_123");
}

// ============================================================
// PCR 操作集成测试
// ============================================================

#[test]
fn test_pcr_extend_chain() {
    let mut tpm = SimulatedTpm::new();
    tpm.init().unwrap();

    let mut mgr = PcrManager::new(&mut tpm);

    // 连续扩展 3 次
    mgr.extend(PCR_BOOT, b"stage1").unwrap();
    let pcr_after_1 = mgr.read(PCR_BOOT).unwrap();

    mgr.extend(PCR_BOOT, b"stage2").unwrap();
    let pcr_after_2 = mgr.read(PCR_BOOT).unwrap();

    mgr.extend(PCR_BOOT, b"stage3").unwrap();
    let pcr_after_3 = mgr.read(PCR_BOOT).unwrap();

    // 每次扩展后 PCR 值应不同
    assert_ne!(pcr_after_1, pcr_after_2);
    assert_ne!(pcr_after_2, pcr_after_3);
    assert_ne!(pcr_after_1, pcr_after_3);
}

// ============================================================
// 故障注入测试
// ============================================================

#[test]
fn test_tpm_not_initialized_error() {
    let mut tpm = SimulatedTpm::new();
    // 未初始化时操作应失败
    assert!(tpm.pcr_read(0).is_err());
    assert!(tpm.get_random(16).is_err());
}

#[test]
fn test_invalid_pcr_index() {
    let mut tpm = SimulatedTpm::new();
    tpm.init().unwrap();

    assert!(tpm.pcr_read(24).is_err());
    assert!(tpm.pcr_read(100).is_err());
}

// ============================================================
// v0.158.0 回归测试（国密功能不退化）
// ============================================================

#[test]
fn test_crypto_regression_sm3() {
    // 确保 eneros-crypto SM3 功能正常
    let hash = eneros_crypto::sm3::hash(b"abc");
    let expected: [u8; 32] = [
        0x66, 0xC7, 0xF0, 0xF4, 0x62, 0xEE, 0xED, 0xD9, 0xD1, 0xF2, 0xD4, 0x6B, 0xDC, 0x10, 0xE4,
        0xE2, 0x41, 0x67, 0xC4, 0x87, 0x5C, 0xF2, 0xF7, 0xA2, 0x29, 0x7D, 0xA0, 0x2B, 0x8F, 0x4B,
        0xA8, 0xE0,
    ];
    assert_eq!(hash, expected);
}

//! PCR 操作与度量策略 (v0.159.0).
//!
//! 实现 PCR（Platform Configuration Register）管理和度量策略。
//! PCR 是 TPM 中只能扩展不能直接写入的寄存器，用于记录平台状态度量。

use alloc::vec::Vec;

use crate::commands::PCR_COUNT;
use crate::driver::TpmDriver;
use crate::error::TpmError;

// ============================================================
// PCR Index Definitions (TCG 规范)
// ============================================================

/// PCR 0: 启动代码（Bootloader/Firmware）
pub const PCR_BOOT: u32 = 0;
/// PCR 1: 内核配置
pub const PCR_KERNEL_CONFIG: u32 = 1;
/// PCR 2: 用户态组件/驱动
pub const PCR_USERLAND: u32 = 2;
/// PCR 3: 安全策略
pub const PCR_SECURITY_POLICY: u32 = 3;
/// PCR 4: 内核镜像
pub const PCR_KERNEL_IMAGE: u32 = 4;
/// PCR 5: 文件系统/分区表
pub const PCR_FILESYSTEM: u32 = 5;
/// PCR 6: 平台特定
pub const PCR_PLATFORM: u32 = 6;
/// PCR 7: Secure Boot 策略
pub const PCR_SECURE_BOOT: u32 = 7;
/// PCR 8~15: 应用定义
pub const PCR_APP_BASE: u32 = 8;

// ============================================================
// PCR Strategy
// ============================================================

/// PCR 度量策略.
///
/// 定义各 PCR 寄存器的用途和期望值。
#[derive(Debug, Clone)]
pub struct PcrStrategy {
    /// PCR 0: 启动代码度量期望值
    pub pcr0_boot: [u8; 32],
    /// PCR 1: 内核配置度量期望值
    pub pcr1_kernel_config: [u8; 32],
    /// PCR 2: 用户态组件度量期望值
    pub pcr2_userland: [u8; 32],
    /// PCR 7: Secure Boot 状态期望值
    pub pcr7_secure_boot: [u8; 32],
}

impl PcrStrategy {
    /// 创建默认策略（全零 = 未配置）.
    pub fn new() -> Self {
        Self {
            pcr0_boot: [0u8; 32],
            pcr1_kernel_config: [0u8; 32],
            pcr2_userland: [0u8; 32],
            pcr7_secure_boot: [0u8; 32],
        }
    }

    /// 设置 PCR 0 期望值.
    pub fn with_boot(mut self, hash: [u8; 32]) -> Self {
        self.pcr0_boot = hash;
        self
    }

    /// 设置 PCR 7 期望值.
    pub fn with_secure_boot(mut self, hash: [u8; 32]) -> Self {
        self.pcr7_secure_boot = hash;
        self
    }

    /// 获取指定 PCR 的期望值.
    pub fn expected_value(&self, pcr_idx: u32) -> Option<&[u8; 32]> {
        match pcr_idx {
            PCR_BOOT => Some(&self.pcr0_boot),
            PCR_KERNEL_CONFIG => Some(&self.pcr1_kernel_config),
            PCR_USERLAND => Some(&self.pcr2_userland),
            PCR_SECURE_BOOT => Some(&self.pcr7_secure_boot),
            _ => None,
        }
    }
}

impl Default for PcrStrategy {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// PCR Manager
// ============================================================

/// PCR 管理器.
///
/// 封装 PCR 读取/扩展/验证操作。
pub struct PcrManager<'a> {
    driver: &'a mut dyn TpmDriver,
}

impl<'a> PcrManager<'a> {
    /// 创建 PCR 管理器.
    pub fn new(driver: &'a mut dyn TpmDriver) -> Self {
        Self { driver }
    }

    /// 读取指定 PCR 值.
    pub fn read(&mut self, pcr_idx: u32) -> Result<[u8; 32], TpmError> {
        self.driver.pcr_read(pcr_idx)
    }

    /// 扩展 PCR（度量数据）.
    ///
    /// PCR = SM3(PCR ‖ SM3(data))
    pub fn extend(&mut self, pcr_idx: u32, data: &[u8]) -> Result<(), TpmError> {
        // 先计算数据的 SM3 哈希
        let digest = eneros_crypto::sm3::hash(data);
        self.driver.pcr_extend(pcr_idx, &digest)
    }

    /// 使用预计算的 digest 扩展 PCR.
    pub fn extend_digest(&mut self, pcr_idx: u32, digest: &[u8; 32]) -> Result<(), TpmError> {
        self.driver.pcr_extend(pcr_idx, digest)
    }

    /// 读取所有 PCR 值.
    pub fn read_all(&mut self) -> Result<[[u8; 32]; PCR_COUNT], TpmError> {
        let mut pcrs = [[0u8; 32]; PCR_COUNT];
        for (i, pcr) in pcrs.iter_mut().enumerate() {
            *pcr = self.driver.pcr_read(i as u32)?;
        }
        Ok(pcrs)
    }

    /// 验证 PCR 值是否匹配策略.
    pub fn verify(&mut self, strategy: &PcrStrategy) -> Result<Vec<PcrVerifyResult>, TpmError> {
        let mut results = Vec::new();
        let pcrs_to_check = [PCR_BOOT, PCR_KERNEL_CONFIG, PCR_USERLAND, PCR_SECURE_BOOT];

        for &idx in &pcrs_to_check {
            if let Some(expected) = strategy.expected_value(idx) {
                // 跳过全零（未配置）
                if *expected == [0u8; 32] {
                    continue;
                }
                let actual = self.driver.pcr_read(idx)?;
                let matched = actual == *expected;
                results.push(PcrVerifyResult {
                    pcr_index: idx,
                    expected: *expected,
                    actual,
                    matched,
                });
            }
        }
        Ok(results)
    }

    /// 验证平台完整性（所有配置的 PCR 必须匹配）.
    pub fn verify_integrity(&mut self, strategy: &PcrStrategy) -> Result<bool, TpmError> {
        let results = self.verify(strategy)?;
        Ok(results.iter().all(|r| r.matched))
    }
}

/// PCR 验证结果.
#[derive(Debug, Clone)]
pub struct PcrVerifyResult {
    /// PCR 索引
    pub pcr_index: u32,
    /// 期望值
    pub expected: [u8; 32],
    /// 实际值
    pub actual: [u8; 32],
    /// 是否匹配
    pub matched: bool,
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::SimulatedTpm;

    #[test]
    fn test_pcr_strategy_default() {
        let strategy = PcrStrategy::new();
        assert_eq!(strategy.pcr0_boot, [0u8; 32]);
        assert!(strategy.expected_value(0).is_some());
        assert!(strategy.expected_value(99).is_none());
    }

    #[test]
    fn test_pcr_manager_extend_and_read() {
        let mut tpm = SimulatedTpm::new();
        tpm.init().unwrap();

        let mut mgr = PcrManager::new(&mut tpm);
        // 初始 PCR 0 全零
        let pcr0 = mgr.read(PCR_BOOT).unwrap();
        assert_eq!(pcr0, [0u8; 32]);

        // 扩展
        mgr.extend(PCR_BOOT, b"bootloader binary").unwrap();
        let pcr0_after = mgr.read(PCR_BOOT).unwrap();
        assert_ne!(pcr0_after, [0u8; 32]);
    }

    #[test]
    fn test_pcr_extend_deterministic() {
        // 相同输入应产生相同 PCR 值
        let mut tpm1 = SimulatedTpm::new();
        tpm1.init().unwrap();
        let mut mgr1 = PcrManager::new(&mut tpm1);
        mgr1.extend(PCR_BOOT, b"same data").unwrap();
        let pcr1 = mgr1.read(PCR_BOOT).unwrap();

        let mut tpm2 = SimulatedTpm::new();
        tpm2.init().unwrap();
        let mut mgr2 = PcrManager::new(&mut tpm2);
        mgr2.extend(PCR_BOOT, b"same data").unwrap();
        let pcr2 = mgr2.read(PCR_BOOT).unwrap();

        assert_eq!(pcr1, pcr2);
    }

    #[test]
    fn test_pcr_extend_order_matters() {
        // 不同顺序扩展应产生不同结果
        let mut tpm1 = SimulatedTpm::new();
        tpm1.init().unwrap();
        let mut mgr1 = PcrManager::new(&mut tpm1);
        mgr1.extend(PCR_BOOT, b"first").unwrap();
        mgr1.extend(PCR_BOOT, b"second").unwrap();
        let pcr1 = mgr1.read(PCR_BOOT).unwrap();

        let mut tpm2 = SimulatedTpm::new();
        tpm2.init().unwrap();
        let mut mgr2 = PcrManager::new(&mut tpm2);
        mgr2.extend(PCR_BOOT, b"second").unwrap();
        mgr2.extend(PCR_BOOT, b"first").unwrap();
        let pcr2 = mgr2.read(PCR_BOOT).unwrap();

        assert_ne!(pcr1, pcr2);
    }

    #[test]
    fn test_pcr_verify_integrity_pass() {
        let mut tpm = SimulatedTpm::new();
        tpm.init().unwrap();

        // 先扩展得到已知 PCR 值
        let mut mgr = PcrManager::new(&mut tpm);
        mgr.extend(PCR_BOOT, b"known bootloader").unwrap();
        let expected_pcr0 = mgr.read(PCR_BOOT).unwrap();

        // 构造匹配策略
        let strategy = PcrStrategy::new().with_boot(expected_pcr0);
        let result = mgr.verify_integrity(&strategy).unwrap();
        assert!(result);
    }

    #[test]
    fn test_pcr_verify_integrity_fail() {
        let mut tpm = SimulatedTpm::new();
        tpm.init().unwrap();

        let mut mgr = PcrManager::new(&mut tpm);
        mgr.extend(PCR_BOOT, b"actual bootloader").unwrap();

        // 策略期望不同的值
        let strategy = PcrStrategy::new().with_boot([0xFF; 32]);
        let result = mgr.verify_integrity(&strategy).unwrap();
        assert!(!result);
    }

    #[test]
    fn test_pcr_read_all() {
        let mut tpm = SimulatedTpm::new();
        tpm.init().unwrap();

        let mut mgr = PcrManager::new(&mut tpm);
        let all = mgr.read_all().unwrap();
        assert_eq!(all.len(), PCR_COUNT);
        // 初始全零
        for pcr in &all {
            assert_eq!(*pcr, [0u8; 32]);
        }
    }

    #[test]
    fn test_pcr_constants() {
        assert_eq!(PCR_BOOT, 0);
        assert_eq!(PCR_SECURE_BOOT, 7);
        assert_eq!(PCR_APP_BASE, 8);
    }
}

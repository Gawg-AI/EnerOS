//! TPM 度量服务 (workspace v0.192.0).
//!
//! 提供启动链度量和平台完整性验证服务。
//! 度量日志可审计，支持远程证明（Remote Attestation）扩展。

use alloc::string::String;
use alloc::vec::Vec;

use crate::driver::TpmDriver;
use crate::error::TpmError;
use crate::pcr::*;

// ============================================================
// Boot Stage Definitions
// ============================================================

/// 启动阶段.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootStage {
    /// Bootloader / Firmware
    Bootloader,
    /// 内核镜像
    Kernel,
    /// 内核配置/设备树
    KernelConfig,
    /// 用户态运行时
    Userland,
    /// Agent 组件
    Agent,
    /// Secure Boot 策略
    SecureBootPolicy,
}

impl BootStage {
    /// 获取对应的 PCR 索引.
    pub fn pcr_index(&self) -> u32 {
        match self {
            BootStage::Bootloader => PCR_BOOT,
            BootStage::Kernel => PCR_KERNEL_IMAGE,
            BootStage::KernelConfig => PCR_KERNEL_CONFIG,
            BootStage::Userland => PCR_USERLAND,
            BootStage::Agent => PCR_APP_BASE,
            BootStage::SecureBootPolicy => PCR_SECURE_BOOT,
        }
    }

    /// 阶段名称.
    pub fn name(&self) -> &'static str {
        match self {
            BootStage::Bootloader => "Bootloader",
            BootStage::Kernel => "Kernel",
            BootStage::KernelConfig => "KernelConfig",
            BootStage::Userland => "Userland",
            BootStage::Agent => "Agent",
            BootStage::SecureBootPolicy => "SecureBootPolicy",
        }
    }
}

// ============================================================
// Measurement Entry
// ============================================================

/// 度量日志条目.
#[derive(Debug, Clone)]
pub struct MeasurementEntry {
    /// 序号
    pub sequence: u32,
    /// 启动阶段
    pub stage: BootStage,
    /// PCR 索引
    pub pcr_index: u32,
    /// 数据摘要 (SM3)
    pub digest: [u8; 32],
    /// 描述信息
    pub description: String,
}

// ============================================================
// TPM Measurement Service
// ============================================================

/// TPM 度量服务.
///
/// 管理启动链度量和完整性验证。
pub struct TpmMeasurement<'a> {
    driver: &'a mut dyn TpmDriver,
    /// 度量日志
    log: Vec<MeasurementEntry>,
    /// 序号计数器
    sequence: u32,
}

impl<'a> TpmMeasurement<'a> {
    /// 创建度量服务.
    pub fn new(driver: &'a mut dyn TpmDriver) -> Self {
        Self {
            driver,
            log: Vec::new(),
            sequence: 0,
        }
    }

    /// 度量启动阶段数据.
    ///
    /// 计算数据 SM3 摘要并扩展到对应 PCR。
    pub fn measure(
        &mut self,
        stage: BootStage,
        data: &[u8],
        description: &str,
    ) -> Result<[u8; 32], TpmError> {
        let pcr_idx = stage.pcr_index();
        let digest = eneros_crypto::sm3::hash(data);

        // 扩展 PCR
        self.driver.pcr_extend(pcr_idx, &digest)?;

        // 记录日志
        self.sequence += 1;
        self.log.push(MeasurementEntry {
            sequence: self.sequence,
            stage,
            pcr_index: pcr_idx,
            digest,
            description: String::from(description),
        });

        Ok(digest)
    }

    /// 度量完整启动链.
    pub fn measure_boot_chain(
        &mut self,
        bootloader: &[u8],
        kernel: &[u8],
        config: &[u8],
        userland: &[u8],
    ) -> Result<(), TpmError> {
        self.measure(BootStage::Bootloader, bootloader, "Bootloader image")?;
        self.measure(BootStage::Kernel, kernel, "Kernel image")?;
        self.measure(BootStage::KernelConfig, config, "Kernel config / DTB")?;
        self.measure(BootStage::Userland, userland, "Userland runtime")?;
        Ok(())
    }

    /// 验证平台完整性.
    pub fn verify_integrity(&mut self, strategy: &PcrStrategy) -> Result<bool, TpmError> {
        let mut mgr = PcrManager::new(self.driver);
        mgr.verify_integrity(strategy)
    }

    /// 获取度量日志.
    pub fn measurement_log(&self) -> &[MeasurementEntry] {
        &self.log
    }

    /// 获取度量日志条目数.
    pub fn log_count(&self) -> usize {
        self.log.len()
    }

    /// 生成证明报告（用于远程证明）.
    pub fn generate_quote(&mut self, nonce: &[u8]) -> Result<QuoteReport, TpmError> {
        // 读取关键 PCR 值
        let pcr0 = self.driver.pcr_read(PCR_BOOT)?;
        let pcr7 = self.driver.pcr_read(PCR_SECURE_BOOT)?;

        // 构造 quote 数据
        let mut quote_data = Vec::new();
        quote_data.extend_from_slice(&pcr0);
        quote_data.extend_from_slice(&pcr7);
        quote_data.extend_from_slice(nonce);

        // 计算 quote 摘要
        let quote_digest = eneros_crypto::sm3::hash(&quote_data);

        Ok(QuoteReport {
            pcr0_boot: pcr0,
            pcr7_secure_boot: pcr7,
            nonce: nonce.to_vec(),
            quote_digest,
            measurement_count: self.log.len() as u32,
        })
    }
}

/// 远程证明 Quote 报告.
#[derive(Debug, Clone)]
pub struct QuoteReport {
    /// PCR 0 值（启动代码度量）
    pub pcr0_boot: [u8; 32],
    /// PCR 7 值（Secure Boot 状态）
    pub pcr7_secure_boot: [u8; 32],
    /// 挑战 Nonce
    pub nonce: Vec<u8>,
    /// Quote 摘要
    pub quote_digest: [u8; 32],
    /// 度量条目数
    pub measurement_count: u32,
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::SimulatedTpm;

    #[test]
    fn test_boot_stage_pcr_mapping() {
        assert_eq!(BootStage::Bootloader.pcr_index(), 0);
        assert_eq!(BootStage::Kernel.pcr_index(), 4);
        assert_eq!(BootStage::SecureBootPolicy.pcr_index(), 7);
    }

    #[test]
    fn test_measurement_single() {
        let mut tpm = SimulatedTpm::new();
        tpm.init().unwrap();

        let mut svc = TpmMeasurement::new(&mut tpm);
        let digest = svc
            .measure(BootStage::Bootloader, b"boot code", "test bootloader")
            .unwrap();

        assert_ne!(digest, [0u8; 32]);
        assert_eq!(svc.log_count(), 1);
        assert_eq!(svc.measurement_log()[0].stage, BootStage::Bootloader);
    }

    #[test]
    fn test_measurement_boot_chain() {
        let mut tpm = SimulatedTpm::new();
        tpm.init().unwrap();

        let mut svc = TpmMeasurement::new(&mut tpm);
        svc.measure_boot_chain(b"bootloader", b"kernel image", b"device tree", b"userland")
            .unwrap();

        assert_eq!(svc.log_count(), 4);
        // PCR 0, 4, 1, 2 应被扩展
        let pcr0 = tpm.pcr_read(0).unwrap();
        assert_ne!(pcr0, [0u8; 32]);
    }

    #[test]
    fn test_measurement_log_ordering() {
        let mut tpm = SimulatedTpm::new();
        tpm.init().unwrap();

        let mut svc = TpmMeasurement::new(&mut tpm);
        svc.measure(BootStage::Bootloader, b"stage0", "first")
            .unwrap();
        svc.measure(BootStage::Kernel, b"stage1", "second").unwrap();
        svc.measure(BootStage::Userland, b"stage2", "third")
            .unwrap();

        let log = svc.measurement_log();
        assert_eq!(log[0].sequence, 1);
        assert_eq!(log[1].sequence, 2);
        assert_eq!(log[2].sequence, 3);
    }

    #[test]
    fn test_generate_quote() {
        let mut tpm = SimulatedTpm::new();
        tpm.init().unwrap();

        let mut svc = TpmMeasurement::new(&mut tpm);
        svc.measure(BootStage::Bootloader, b"boot", "boot measure")
            .unwrap();

        let quote = svc.generate_quote(b"challenge_nonce").unwrap();
        assert_ne!(quote.quote_digest, [0u8; 32]);
        assert_eq!(quote.measurement_count, 1);
        assert_eq!(quote.nonce, b"challenge_nonce");
    }

    #[test]
    fn test_measurement_deterministic() {
        // 相同数据度量应产生相同摘要
        let mut tpm1 = SimulatedTpm::new();
        tpm1.init().unwrap();
        let mut svc1 = TpmMeasurement::new(&mut tpm1);
        let d1 = svc1
            .measure(BootStage::Bootloader, b"same", "test")
            .unwrap();

        let mut tpm2 = SimulatedTpm::new();
        tpm2.init().unwrap();
        let mut svc2 = TpmMeasurement::new(&mut tpm2);
        let d2 = svc2
            .measure(BootStage::Bootloader, b"same", "test")
            .unwrap();

        assert_eq!(d1, d2);
    }
}

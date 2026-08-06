//! 启动链验证报告 (workspace v0.192.0).
//!
//! 记录 Secure Boot 全链验证的结果。

/// 启动链验证报告.
///
/// 记录每个启动阶段的验证状态和 TPM PCR 度量状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootChainReport {
    /// Bootloader 验签通过
    pub bootloader_verified: bool,
    /// 内核验签通过
    pub kernel_verified: bool,
    /// 用户态运行时验签通过
    pub runtime_verified: bool,
    /// TPM PCR 扩展完成
    pub pcr_extended: bool,
    /// 验签通过的阶段总数
    pub stages_verified: u32,
    /// 信任根有效
    pub root_of_trust_valid: bool,
}

impl BootChainReport {
    /// 创建空报告（全部未验证）.
    pub fn new() -> Self {
        Self {
            bootloader_verified: false,
            kernel_verified: false,
            runtime_verified: false,
            pcr_extended: false,
            stages_verified: 0,
            root_of_trust_valid: false,
        }
    }

    /// 全链验证是否通过.
    pub fn all_verified(&self) -> bool {
        self.bootloader_verified && self.kernel_verified && self.runtime_verified
    }

    /// 是否可启动（全链通过，TPM 可选）.
    pub fn can_boot(&self) -> bool {
        self.root_of_trust_valid && self.all_verified()
    }
}

impl Default for BootChainReport {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_report_new() {
        let report = BootChainReport::new();
        assert!(!report.all_verified());
        assert!(!report.can_boot());
        assert_eq!(report.stages_verified, 0);
    }

    #[test]
    fn test_report_all_verified() {
        let report = BootChainReport {
            bootloader_verified: true,
            kernel_verified: true,
            runtime_verified: true,
            pcr_extended: true,
            stages_verified: 3,
            root_of_trust_valid: true,
        };
        assert!(report.all_verified());
        assert!(report.can_boot());
    }

    #[test]
    fn test_report_partial_verified() {
        let report = BootChainReport {
            bootloader_verified: true,
            kernel_verified: false,
            runtime_verified: false,
            pcr_extended: false,
            stages_verified: 1,
            root_of_trust_valid: true,
        };
        assert!(!report.all_verified());
        assert!(!report.can_boot());
    }

    #[test]
    fn test_report_can_boot_without_tpm() {
        // TPM 不可用时仍可启动（降级模式）
        let report = BootChainReport {
            bootloader_verified: true,
            kernel_verified: true,
            runtime_verified: true,
            pcr_extended: false, // TPM 不可用
            stages_verified: 3,
            root_of_trust_valid: true,
        };
        assert!(report.can_boot()); // 仍可启动
    }
}

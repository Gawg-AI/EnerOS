//! 支持/EOL 策略 (OPS-015).
//!
//! MASTER §21.14 / G9: "support/EOL policy — LTS/漏洞/备件承诺".
//!
//! 冻结每个发布版本的支持等级、生命周期日期、漏洞响应 SLA 和备件供应承诺。
//! 客户必须能在部署前知晓：版本支持到何时、漏洞多久修复、备件供应多久。
//!
//! # 证据推导
//!
//! EOL 策略合规结论由证据推导：
//! 1. 每个在役版本有明确的支持等级和日期
//! 2. 每个在役版本未超过 security_eol（仍在安全支持期）
//! 3. 漏洞 SLA 矩阵覆盖所有 CVSS 级别
//! 4. 备件供应承诺覆盖在役硬件且未过期
//! 5. 无在役版本处于 EOL 状态

use alloc::string::String;
use alloc::vec::Vec;

/// 支持等级.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportTier {
    /// 完整支持（功能 + 安全 + 补丁）.
    Full,
    /// 长期支持（仅安全 + 关键修复）.
    Lts,
    /// 仅安全支持（仅安全补丁）.
    SecurityOnly,
    /// 终止支持（不再维护）.
    Eol,
}

impl SupportTier {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Lts => "lts",
            Self::SecurityOnly => "security-only",
            Self::Eol => "eol",
        }
    }

    /// 是否仍在支持（非 EOL）.
    pub fn is_supported(&self) -> bool {
        !matches!(self, Self::Eol)
    }
}

/// CVSS 严重度级别.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CvssLevel {
    Low,
    Medium,
    High,
    Critical,
}

/// 漏洞响应 SLA（秒）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VulnerabilitySla {
    pub level: CvssLevel,
    /// 确认时限（秒）.
    pub ack_deadline_secs: u64,
    /// 补丁发布时限（秒）.
    pub patch_deadline_secs: u64,
}

impl VulnerabilitySla {
    pub fn new(level: CvssLevel, ack_deadline_secs: u64, patch_deadline_secs: u64) -> Self {
        Self {
            level,
            ack_deadline_secs,
            patch_deadline_secs,
        }
    }
}

/// 版本支持承诺.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionSupport {
    /// 版本号（如 "1.0.0"）.
    pub version: String,
    /// 支持等级.
    pub tier: SupportTier,
    /// GA 日期（Unix 秒）.
    pub ga_date: u64,
    /// 完整支持结束日期（Unix 秒）.
    pub full_support_end: u64,
    /// 安全支持结束日期（Unix 秒）.
    pub security_eol: u64,
}

impl VersionSupport {
    pub fn new(
        version: &str,
        tier: SupportTier,
        ga_date: u64,
        full_support_end: u64,
        security_eol: u64,
    ) -> Self {
        Self {
            version: String::from(version),
            tier,
            ga_date,
            full_support_end,
            security_eol,
        }
    }

    /// 在给定时间是否仍在安全支持期内.
    pub fn is_within_security_support(&self, now: u64) -> bool {
        self.tier.is_supported() && now < self.security_eol
    }

    /// 在给定时间是否已过安全 EOL.
    pub fn is_past_eol(&self, now: u64) -> bool {
        now >= self.security_eol || self.tier == SupportTier::Eol
    }
}

/// 备件供应承诺.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SparePartCommitment {
    /// 硬件型号/板卡 ID.
    pub hardware_id: String,
    /// 备件供应开始日期（Unix 秒）.
    pub available_from: u64,
    /// 备件供应结束日期（Unix 秒）.
    pub available_until: u64,
    /// 最小库存数量承诺.
    pub min_stock: u32,
}

impl SparePartCommitment {
    pub fn new(
        hardware_id: &str,
        available_from: u64,
        available_until: u64,
        min_stock: u32,
    ) -> Self {
        Self {
            hardware_id: String::from(hardware_id),
            available_from,
            available_until,
            min_stock,
        }
    }

    /// 在给定时间备件供应是否仍有效.
    pub fn is_available(&self, now: u64) -> bool {
        now >= self.available_from && now < self.available_until
    }
}

/// EOL/支持策略.
///
/// 汇总版本支持、漏洞 SLA、备件承诺，计算合规结论。结论**由证据推导**。
pub struct EolPolicy {
    versions: Vec<VersionSupport>,
    sla_matrix: Vec<VulnerabilitySla>,
    spare_parts: Vec<SparePartCommitment>,
    /// 在役硬件 ID 清单（必须全部有备件承诺）.
    active_hardware: Vec<String>,
}

impl EolPolicy {
    pub fn new(
        versions: Vec<VersionSupport>,
        sla_matrix: Vec<VulnerabilitySla>,
        spare_parts: Vec<SparePartCommitment>,
        active_hardware: Vec<String>,
    ) -> Self {
        Self {
            versions,
            sla_matrix,
            spare_parts,
            active_hardware,
        }
    }

    /// 缺少 SLA 的 CVSS 级别.
    pub fn missing_sla_levels(&self) -> Vec<CvssLevel> {
        let required = [
            CvssLevel::Low,
            CvssLevel::Medium,
            CvssLevel::High,
            CvssLevel::Critical,
        ];
        required
            .iter()
            .filter(|lvl| !self.sla_matrix.iter().any(|sla| sla.level == **lvl))
            .copied()
            .collect()
    }

    /// 已过 EOL 但仍标记为在役的版本.
    pub fn expired_in_service_versions(&self, now: u64) -> Vec<&String> {
        self.versions
            .iter()
            .filter(|v| v.is_past_eol(now))
            .map(|v| &v.version)
            .collect()
    }

    /// 缺少备件承诺的在役硬件.
    pub fn hardware_without_spare_commitment(&self) -> Vec<&String> {
        self.active_hardware
            .iter()
            .filter(|hw| !self.spare_parts.iter().any(|sp| sp.hardware_id == **hw))
            .collect()
    }

    /// 备件供应已过期的在役硬件.
    pub fn expired_spare_commitments(&self, now: u64) -> Vec<&String> {
        self.active_hardware
            .iter()
            .filter(|hw| {
                // MSRV 1.75: Option::is_none_or stabilized in 1.82, use map+unwrap_or.
                self.spare_parts
                    .iter()
                    .find(|sp| sp.hardware_id == **hw)
                    .map(|sp| !sp.is_available(now))
                    .unwrap_or(true)
            })
            .collect()
    }

    /// 策略是否合规（由证据推导）.
    ///
    /// 条件：
    /// 1. 非空（至少有版本支持记录）
    /// 2. 无在役版本已过 EOL
    /// 3. SLA 矩阵覆盖所有 CVSS 级别
    /// 4. 所有在役硬件有备件承诺
    /// 5. 所有备件承诺在当前时间有效
    pub fn is_compliant(&self, now: u64) -> bool {
        if self.versions.is_empty() {
            return false;
        }
        self.expired_in_service_versions(now).is_empty()
            && self.missing_sla_levels().is_empty()
            && self.hardware_without_spare_commitment().is_empty()
            && self.expired_spare_commitments(now).is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sla_matrix() -> Vec<VulnerabilitySla> {
        vec![
            VulnerabilitySla::new(CvssLevel::Low, 7 * 86400, 90 * 86400),
            VulnerabilitySla::new(CvssLevel::Medium, 3 * 86400, 30 * 86400),
            VulnerabilitySla::new(CvssLevel::High, 86400, 7 * 86400),
            VulnerabilitySla::new(CvssLevel::Critical, 4 * 3600, 24 * 3600),
        ]
    }

    const NOW: u64 = 1_750_000_000; // 2025-06 左右

    #[test]
    fn t1_compliant_full_policy() {
        let versions = vec![
            VersionSupport::new(
                "1.0.0",
                SupportTier::Full,
                1_740_000_000,
                1_780_000_000,
                1_820_000_000,
            ),
            VersionSupport::new(
                "0.9.0",
                SupportTier::Lts,
                1_720_000_000,
                1_760_000_000,
                1_800_000_000,
            ),
        ];
        let spares = vec![SparePartCommitment::new(
            "d2000-board-rev1",
            1_740_000_000,
            1_900_000_000,
            50,
        )];
        let policy = EolPolicy::new(
            versions,
            sla_matrix(),
            spares,
            vec![String::from("d2000-board-rev1")],
        );
        assert!(policy.is_compliant(NOW));
    }

    #[test]
    fn t2_fails_when_version_past_eol() {
        let versions = vec![VersionSupport::new(
            "0.5.0",
            SupportTier::Eol,
            1_700_000_000,
            1_720_000_000,
            1_740_000_000,
        )];
        let policy = EolPolicy::new(versions, sla_matrix(), Vec::new(), Vec::new());
        assert!(!policy.is_compliant(NOW));
        assert_eq!(policy.expired_in_service_versions(NOW).len(), 1);
    }

    #[test]
    fn t3_fails_when_sla_level_missing() {
        let versions = vec![VersionSupport::new(
            "1.0.0",
            SupportTier::Full,
            1_740_000_000,
            1_780_000_000,
            1_820_000_000,
        )];
        let mut sla = sla_matrix();
        sla.retain(|s| s.level != CvssLevel::Critical); // 缺 Critical SLA
        let policy = EolPolicy::new(versions, sla, Vec::new(), Vec::new());
        assert!(!policy.is_compliant(NOW));
        assert!(policy.missing_sla_levels().contains(&CvssLevel::Critical));
    }

    #[test]
    fn t4_fails_when_hardware_without_spare() {
        let versions = vec![VersionSupport::new(
            "1.0.0",
            SupportTier::Full,
            1_740_000_000,
            1_780_000_000,
            1_820_000_000,
        )];
        let policy = EolPolicy::new(
            versions,
            sla_matrix(),
            Vec::new(),
            vec![String::from("d2000-board-rev1")],
        );
        assert!(!policy.is_compliant(NOW));
        assert_eq!(policy.hardware_without_spare_commitment().len(), 1);
    }

    #[test]
    fn t5_fails_when_spare_expired() {
        let versions = vec![VersionSupport::new(
            "1.0.0",
            SupportTier::Full,
            1_740_000_000,
            1_780_000_000,
            1_820_000_000,
        )];
        let spares = vec![SparePartCommitment::new(
            "d2000",
            1_700_000_000,
            1_730_000_000,
            10,
        )]; // 已过期
        let policy = EolPolicy::new(versions, sla_matrix(), spares, vec![String::from("d2000")]);
        assert!(!policy.is_compliant(NOW));
        assert_eq!(policy.expired_spare_commitments(NOW).len(), 1);
    }

    #[test]
    fn t6_empty_policy_fails() {
        let policy = EolPolicy::new(Vec::new(), sla_matrix(), Vec::new(), Vec::new());
        assert!(!policy.is_compliant(NOW));
    }

    #[test]
    fn t7_security_only_still_supported() {
        let v = VersionSupport::new(
            "0.8.0",
            SupportTier::SecurityOnly,
            1_710_000_000,
            1_740_000_000,
            1_780_000_000,
        );
        assert!(v.is_within_security_support(NOW));
        assert!(!v.is_past_eol(NOW));
    }

    #[test]
    fn t8_lts_tier_is_supported() {
        assert!(SupportTier::Lts.is_supported());
        assert!(SupportTier::Full.is_supported());
        assert!(SupportTier::SecurityOnly.is_supported());
        assert!(!SupportTier::Eol.is_supported());
    }

    #[test]
    fn t9_multiple_hardware_all_covered() {
        let versions = vec![VersionSupport::new(
            "1.0.0",
            SupportTier::Full,
            1_740_000_000,
            1_780_000_000,
            1_820_000_000,
        )];
        let spares = vec![
            SparePartCommitment::new("d2000-rev1", 1_740_000_000, 1_900_000_000, 50),
            SparePartCommitment::new("d2000-rev2", 1_740_000_000, 1_920_000_000, 30),
        ];
        let active = vec![String::from("d2000-rev1"), String::from("d2000-rev2")];
        let policy = EolPolicy::new(versions, sla_matrix(), spares, active);
        assert!(policy.is_compliant(NOW));
    }

    #[test]
    fn t10_version_within_full_support_window() {
        let v = VersionSupport::new(
            "1.0.0",
            SupportTier::Full,
            1_740_000_000,
            1_780_000_000,
            1_820_000_000,
        );
        assert!(v.is_within_security_support(NOW));
        // full_support_end 之后但仍 security_eol 之前
        assert!(v.is_within_security_support(1_790_000_000));
        // security_eol 之后
        assert!(!v.is_within_security_support(1_830_000_000));
    }
}

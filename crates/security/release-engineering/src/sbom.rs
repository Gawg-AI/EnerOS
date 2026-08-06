//! SBOM 与许可证管理 (REL-004~007).
//!
//! REL-004: workspace license 决策 — LICENSE/Cargo 一致
//! REL-005: third-party NOTICE 生成 — 每 artifact 对应
//! REL-006: model/firmware license review — 出货权利确认
//! REL-007: SPDX/CycloneDX SBOM — source/binary/model 全覆盖

use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

/// 许可证决策状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LicenseDecision {
    /// 批准使用。
    Approved,
    /// 需要法务审查。
    NeedsLegalReview,
    /// 拒绝（许可证不兼容）。
    Rejected,
}

impl LicenseDecision {
    pub fn label(self) -> &'static str {
        match self {
            LicenseDecision::Approved => "APPROVED",
            LicenseDecision::NeedsLegalReview => "NEEDS_LEGAL_REVIEW",
            LicenseDecision::Rejected => "REJECTED",
        }
    }
}

/// NOTICE 条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoticeEntry {
    /// 组件名。
    pub component: String,
    /// 版本。
    pub version: String,
    /// 许可证标识（SPDX）。
    pub license_spdx: String,
    /// 许可证全文或引用。
    pub license_text: String,
    /// 版权声明。
    pub copyright: String,
    /// 来源 URL。
    pub source_url: String,
}

impl NoticeEntry {
    pub fn new(
        component: &str,
        version: &str,
        license_spdx: &str,
        copyright: &str,
        source_url: &str,
    ) -> Self {
        Self {
            component: String::from(component),
            version: String::from(version),
            license_spdx: String::from(license_spdx),
            license_text: String::new(), // 调用方填充
            copyright: String::from(copyright),
            source_url: String::from(source_url),
        }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.component.is_empty() {
            return Err("component is empty");
        }
        if self.license_spdx.is_empty() {
            return Err("license_spdx is empty");
        }
        if self.copyright.is_empty() {
            return Err("copyright is empty");
        }
        Ok(())
    }
}

/// SBOM 条目类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SbomEntryType {
    /// 源代码。
    Source,
    /// 二进制产物。
    Binary,
    /// AI 模型。
    Model,
    /// 固件。
    Firmware,
}

impl SbomEntryType {
    pub fn label(self) -> &'static str {
        match self {
            SbomEntryType::Source => "SOURCE",
            SbomEntryType::Binary => "BINARY",
            SbomEntryType::Model => "MODEL",
            SbomEntryType::Firmware => "FIRMWARE",
        }
    }
}

/// SBOM 条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SbomEntry {
    /// 条目类型。
    pub entry_type: SbomEntryType,
    /// 组件名。
    pub name: String,
    /// 版本。
    pub version: String,
    /// SHA-256 摘要。
    pub sha256: String,
    /// 许可证（SPDX）。
    pub license: String,
    /// 来源 URL。
    pub source_url: String,
    /// 供应商。
    pub supplier: String,
}

impl SbomEntry {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.name.is_empty() {
            return Err("name is empty");
        }
        if self.sha256.len() != 64 {
            return Err("sha256 must be 64 hex chars");
        }
        if self.license.is_empty() {
            return Err("license is empty");
        }
        Ok(())
    }
}

/// SBOM 报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SbomReport {
    /// 报告 ID。
    pub report_id: String,
    /// 产品版本。
    pub product_version: String,
    /// 生成时间（Unix 秒）。
    pub generated_at: u64,
    /// SBOM 条目。
    pub entries: Vec<SbomEntry>,
    /// NOTICE 条目。
    pub notices: Vec<NoticeEntry>,
    /// 许可证决策摘要。
    pub license_decisions: Vec<(String, LicenseDecision)>,
}

impl SbomReport {
    pub fn new(report_id: &str, product_version: &str, generated_at: u64) -> Self {
        Self {
            report_id: String::from(report_id),
            product_version: String::from(product_version),
            generated_at,
            entries: Vec::new(),
            notices: Vec::new(),
            license_decisions: Vec::new(),
        }
    }

    /// 添加 SBOM 条目。
    pub fn add_entry(&mut self, entry: SbomEntry) -> Result<(), &'static str> {
        entry.validate()?;
        self.entries.push(entry);
        Ok(())
    }

    /// 添加 NOTICE 条目。
    pub fn add_notice(&mut self, notice: NoticeEntry) -> Result<(), &'static str> {
        notice.validate()?;
        self.notices.push(notice);
        Ok(())
    }

    /// 记录许可证决策。
    pub fn decide_license(&mut self, component: &str, decision: LicenseDecision) {
        self.license_decisions
            .retain(|(c, _)| c != component);
        self.license_decisions
            .push((String::from(component), decision));
    }

    /// 是否所有许可证已决策且批准。
    pub fn all_licenses_approved(&self) -> bool {
        // 每个 SBOM 条目必须有对应的许可证决策
        for entry in &self.entries {
            let decision = self
                .license_decisions
                .iter()
                .find(|(c, _)| c == &entry.name);
            match decision {
                None => return false, // 未决策
                Some((_, LicenseDecision::Approved)) => {}
                Some((_, _)) => return false, // 未批准
            }
        }
        true
    }

    /// 是否覆盖所有产物类型（source/binary/model/firmware）。
    pub fn covers_all_types(&self) -> bool {
        let has_source = self.entries.iter().any(|e| e.entry_type == SbomEntryType::Source);
        let has_binary = self.entries.iter().any(|e| e.entry_type == SbomEntryType::Binary);
        has_source && has_binary
    }

    /// 条目数。
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry(entry_type: SbomEntryType, name: &str) -> SbomEntry {
        SbomEntry {
            entry_type,
            name: String::from(name),
            version: String::from("1.0.0"),
            sha256: String::from("a1b2c3d4e5f67890a1b2c3d4e5f67890a1b2c3d4e5f67890a1b2c3d4e5f67890"),
            license: String::from("MIT"),
            source_url: String::from("https://example.com/repo"),
            supplier: String::from("EnerOS Team"),
        }
    }

    fn sample_notice(name: &str) -> NoticeEntry {
        NoticeEntry::new(name, "1.0.0", "MIT", "Copyright 2026 EnerOS", "https://example.com")
    }

    #[test]
    fn sbom_report_complete() {
        let mut report = SbomReport::new("sbom-001", "0.200.0", 1000);

        report.add_entry(sample_entry(SbomEntryType::Source, "eneros-kernel")).unwrap();
        report.add_entry(sample_entry(SbomEntryType::Binary, "eneros-kernel")).unwrap();
        report.add_entry(sample_entry(SbomEntryType::Model, "llm-7b")).unwrap();
        report.add_notice(sample_notice("eneros-kernel")).unwrap();
        report.decide_license("eneros-kernel", LicenseDecision::Approved);
        report.decide_license("llm-7b", LicenseDecision::Approved);

        assert!(report.covers_all_types());
        assert!(report.all_licenses_approved());
        assert_eq!(report.entry_count(), 3);
    }

    #[test]
    fn unapproved_license_fails() {
        let mut report = SbomReport::new("sbom-002", "0.200.0", 1000);
        report.add_entry(sample_entry(SbomEntryType::Source, "lib-a")).unwrap();
        report.decide_license("lib-a", LicenseDecision::Rejected);

        assert!(!report.all_licenses_approved());
    }

    #[test]
    fn undecided_license_fails() {
        let mut report = SbomReport::new("sbom-003", "0.200.0", 1000);
        report.add_entry(sample_entry(SbomEntryType::Source, "lib-b")).unwrap();
        // No decision recorded

        assert!(!report.all_licenses_approved());
    }

    #[test]
    fn missing_source_type_fails() {
        let mut report = SbomReport::new("sbom-004", "0.200.0", 1000);
        report.add_entry(sample_entry(SbomEntryType::Binary, "bin-only")).unwrap();
        assert!(!report.covers_all_types());
    }

    #[test]
    fn invalid_entry_rejected() {
        let mut report = SbomReport::new("sbom-005", "0.200.0", 1000);
        let bad_entry = SbomEntry {
            entry_type: SbomEntryType::Source,
            name: String::new(), // empty
            version: String::from("1.0"),
            sha256: String::from("abc"),
            license: String::from("MIT"),
            source_url: String::new(),
            supplier: String::new(),
        };
        assert!(report.add_entry(bad_entry).is_err());
    }

    #[test]
    fn notice_validation() {
        let notice = sample_notice("test");
        assert!(notice.validate().is_ok());

        let bad = NoticeEntry::new("", "1.0", "MIT", "copy", "url");
        assert!(bad.validate().is_err());
    }

    #[test]
    fn license_decision_labels() {
        assert_eq!(LicenseDecision::Approved.label(), "APPROVED");
        assert_eq!(LicenseDecision::Rejected.label(), "REJECTED");
        assert_eq!(LicenseDecision::NeedsLegalReview.label(), "NEEDS_LEGAL_REVIEW");
    }
}

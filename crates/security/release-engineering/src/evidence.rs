//! 发布验证与证据包 (REL-011~012).
//!
//! REL-011: release verifier 独立工具 — 反向验证全部产物
//! REL-012: release evidence bundle — requirement 到 artifact

use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

/// 验证检查项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationCheck {
    /// 产物签名有效。
    SignatureValid,
    /// 产物 SHA-256 匹配 manifest。
    DigestMatchesManifest,
    /// SBOM 完整且许可证已批准。
    SbomComplete,
    /// 来源记录已签名。
    ProvenanceSigned,
    /// 无 Mock 后端（生产构建）。
    NoMockBackends,
    /// 无 skip/timeout/硬编码通过。
    NoFakeTests,
    /// 交叉编译通过。
    CrossCompileOk,
    /// 安全扫描无 Critical/High。
    SecurityScanClean,
    /// 可复现构建（双 builder digest 一致）。
    ReproducibleBuild,
}

impl VerificationCheck {
    pub fn label(self) -> &'static str {
        match self {
            VerificationCheck::SignatureValid => "SIGNATURE_VALID",
            VerificationCheck::DigestMatchesManifest => "DIGEST_MATCHES_MANIFEST",
            VerificationCheck::SbomComplete => "SBOM_COMPLETE",
            VerificationCheck::ProvenanceSigned => "PROVENANCE_SIGNED",
            VerificationCheck::NoMockBackends => "NO_MOCK_BACKENDS",
            VerificationCheck::NoFakeTests => "NO_FAKE_TESTS",
            VerificationCheck::CrossCompileOk => "CROSS_COMPILE_OK",
            VerificationCheck::SecurityScanClean => "SECURITY_SCAN_CLEAN",
            VerificationCheck::ReproducibleBuild => "REPRODUCIBLE_BUILD",
        }
    }

    pub fn all() -> &'static [VerificationCheck] {
        &[
            VerificationCheck::SignatureValid,
            VerificationCheck::DigestMatchesManifest,
            VerificationCheck::SbomComplete,
            VerificationCheck::ProvenanceSigned,
            VerificationCheck::NoMockBackends,
            VerificationCheck::NoFakeTests,
            VerificationCheck::CrossCompileOk,
            VerificationCheck::SecurityScanClean,
            VerificationCheck::ReproducibleBuild,
        ]
    }
}

/// 验证结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationResult {
    pub check: VerificationCheck,
    pub passed: bool,
    pub evidence: String,
}

impl VerificationResult {
    pub fn pass(check: VerificationCheck, evidence: &str) -> Self {
        Self { check, passed: true, evidence: String::from(evidence) }
    }
    pub fn fail(check: VerificationCheck, evidence: &str) -> Self {
        Self { check, passed: false, evidence: String::from(evidence) }
    }
}

/// 发布验证器（独立工具，非构建方）。
pub struct ReleaseVerifier {
    results: Vec<VerificationResult>,
}

impl Default for ReleaseVerifier {
    fn default() -> Self {
        Self::new()
    }
}

impl ReleaseVerifier {
    pub fn new() -> Self {
        Self { results: Vec::new() }
    }

    /// 添加验证结果。
    pub fn add_result(&mut self, result: VerificationResult) {
        self.results.push(result);
    }

    /// 是否所有检查通过。
    pub fn all_passed(&self) -> bool {
        !self.results.is_empty()
            && self.results.iter().all(|r| r.passed)
    }

    /// 失败检查数。
    pub fn failure_count(&self) -> usize {
        self.results.iter().filter(|r| !r.passed).count()
    }

    /// 是否所有检查项已完成。
    pub fn is_complete(&self) -> bool {
        // 每个 VerificationCheck 至少有一条结果
        VerificationCheck::all().iter().all(|check| {
            self.results.iter().any(|r| r.check == *check)
        })
    }

    /// 生成验证报告。
    pub fn report(&self) -> String {
        let pass_count = self.results.iter().filter(|r| r.passed).count();
        let fail_count = self.results.iter().filter(|r| !r.passed).count();
        format!(
            "verification: {}/{} passed, {} failed, complete={}",
            pass_count,
            self.results.len(),
            fail_count,
            self.is_complete()
        )
    }
}

/// 证据条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceItem {
    /// 需求 ID（如 "REL-010"）。
    pub requirement_id: String,
    /// 产物路径/标识。
    pub artifact: String,
    /// 证据类型。
    pub evidence_type: EvidenceType,
    /// 证据内容（hash、签名、日志摘要等）。
    pub content: String,
}

/// 证据类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceType {
    /// 构建日志。
    BuildLog,
    /// 签名报告。
    Signature,
    /// 测试报告。
    TestReport,
    /// SBOM。
    Sbom,
    /// 审计记录。
    AuditRecord,
    /// 截图/快照。
    Snapshot,
}

impl EvidenceType {
    pub fn label(self) -> &'static str {
        match self {
            EvidenceType::BuildLog => "BUILD_LOG",
            EvidenceType::Signature => "SIGNATURE",
            EvidenceType::TestReport => "TEST_REPORT",
            EvidenceType::Sbom => "SBOM",
            EvidenceType::AuditRecord => "AUDIT_RECORD",
            EvidenceType::Snapshot => "SNAPSHOT",
        }
    }
}

/// 发布证据包。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceBundle {
    /// 发布版本。
    pub release_version: String,
    /// 证据条目。
    pub items: Vec<EvidenceItem>,
    /// 生成时间。
    pub generated_at: u64,
    /// 签名（SM2）。
    pub signature: String,
}

impl EvidenceBundle {
    pub fn new(release_version: &str, generated_at: u64, signature: &str) -> Self {
        Self {
            release_version: String::from(release_version),
            items: Vec::new(),
            generated_at,
            signature: String::from(signature),
        }
    }

    pub fn add(&mut self, item: EvidenceItem) {
        self.items.push(item);
    }

    /// 查找需求的所有证据。
    pub fn find_for_requirement(&self, req_id: &str) -> Vec<&EvidenceItem> {
        self.items
            .iter()
            .filter(|i| i.requirement_id == req_id)
            .collect()
    }

    /// 是否每个需求都有证据。
    pub fn covers_all_requirements(&self, requirements: &[&str]) -> bool {
        requirements.iter().all(|req| {
            self.items.iter().any(|i| i.requirement_id == *req)
        })
    }

    /// 条目数。
    pub fn item_count(&self) -> usize {
        self.items.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifier_all_pass() {
        let mut v = ReleaseVerifier::new();
        for check in VerificationCheck::all() {
            v.add_result(VerificationResult::pass(*check, "verified"));
        }
        assert!(v.is_complete());
        assert!(v.all_passed());
        assert_eq!(v.failure_count(), 0);
    }

    #[test]
    fn verifier_with_failure() {
        let mut v = ReleaseVerifier::new();
        for check in VerificationCheck::all() {
            v.add_result(VerificationResult::pass(*check, "ok"));
        }
        // Override one with failure
        v.add_result(VerificationResult::fail(VerificationCheck::NoMockBackends, "mock found"));

        assert!(!v.all_passed());
        assert!(v.failure_count() >= 1);
    }

    #[test]
    fn verifier_incomplete() {
        let mut v = ReleaseVerifier::new();
        v.add_result(VerificationResult::pass(VerificationCheck::SignatureValid, "ok"));
        assert!(!v.is_complete());
    }

    #[test]
    fn evidence_bundle_covers_requirements() {
        let mut bundle = EvidenceBundle::new("0.200.0", 1000, "sm2-sig");
        bundle.add(EvidenceItem {
            requirement_id: String::from("REL-010"),
            artifact: String::from("signing-ceremony-log"),
            evidence_type: EvidenceType::AuditRecord,
            content: String::from("ceremony completed, sig=abc"),
        });
        bundle.add(EvidenceItem {
            requirement_id: String::from("REL-007"),
            artifact: String::from("sbom.json"),
            evidence_type: EvidenceType::Sbom,
            content: String::from("42 entries, all approved"),
        });

        assert!(bundle.covers_all_requirements(&["REL-010", "REL-007"]));
        assert!(!bundle.covers_all_requirements(&["REL-010", "REL-007", "REL-011"]));
    }

    #[test]
    fn evidence_find_for_requirement() {
        let mut bundle = EvidenceBundle::new("0.200.0", 1000, "sig");
        bundle.add(EvidenceItem {
            requirement_id: String::from("REL-010"),
            artifact: String::from("log1"),
            evidence_type: EvidenceType::AuditRecord,
            content: String::from("evidence 1"),
        });
        bundle.add(EvidenceItem {
            requirement_id: String::from("REL-010"),
            artifact: String::from("log2"),
            evidence_type: EvidenceType::Signature,
            content: String::from("evidence 2"),
        });

        let items = bundle.find_for_requirement("REL-010");
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn nine_verification_checks() {
        assert_eq!(VerificationCheck::all().len(), 9);
    }
}

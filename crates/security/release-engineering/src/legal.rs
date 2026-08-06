//! 法务审查与发布说明 (REL-013~014).
//!
//! REL-013: EULA/privacy/export/crypto review — 法务批准
//! REL-014: release notes/known limits — 支持矩阵一致

use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

/// 法务审查领域。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegalDomain {
    /// 最终用户许可协议。
    Eula,
    /// 隐私政策。
    Privacy,
    /// 出口管制。
    Export,
    /// 密码学合规。
    Crypto,
}

impl LegalDomain {
    pub fn label(self) -> &'static str {
        match self {
            LegalDomain::Eula => "EULA",
            LegalDomain::Privacy => "PRIVACY",
            LegalDomain::Export => "EXPORT",
            LegalDomain::Crypto => "CRYPTO",
        }
    }

    pub fn all() -> &'static [LegalDomain] {
        &[
            LegalDomain::Eula,
            LegalDomain::Privacy,
            LegalDomain::Export,
            LegalDomain::Crypto,
        ]
    }
}

/// 法务审查状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegalStatus {
    /// 待审查。
    Pending,
    /// 已批准。
    Approved,
    /// 需要修改。
    ChangesRequested,
    /// 拒绝。
    Rejected,
}

impl LegalStatus {
    pub fn label(self) -> &'static str {
        match self {
            LegalStatus::Pending => "PENDING",
            LegalStatus::Approved => "APPROVED",
            LegalStatus::ChangesRequested => "CHANGES_REQUESTED",
            LegalStatus::Rejected => "REJECTED",
        }
    }
}

/// 法务审查记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegalReview {
    /// 审查领域。
    pub domain: LegalDomain,
    /// 审查状态。
    pub status: LegalStatus,
    /// 审查人。
    pub reviewer: String,
    /// 审查时间。
    pub reviewed_at: u64,
    /// 审查意见。
    pub comments: String,
}

impl LegalReview {
    pub fn new(domain: LegalDomain, status: LegalStatus, reviewer: &str, reviewed_at: u64, comments: &str) -> Self {
        Self {
            domain,
            status,
            reviewer: String::from(reviewer),
            reviewed_at,
            comments: String::from(comments),
        }
    }

    pub fn is_approved(&self) -> bool {
        self.status == LegalStatus::Approved
    }
}

/// 法务审查集合。
pub struct LegalReviewSet {
    reviews: Vec<LegalReview>,
}

impl Default for LegalReviewSet {
    fn default() -> Self {
        Self::new()
    }
}

impl LegalReviewSet {
    pub fn new() -> Self {
        Self { reviews: Vec::new() }
    }

    pub fn add(&mut self, review: LegalReview) {
        self.reviews.retain(|r| r.domain != review.domain);
        self.reviews.push(review);
    }

    /// 是否所有领域已批准。
    pub fn all_approved(&self) -> bool {
        LegalDomain::all().iter().all(|domain| {
            self.reviews.iter().any(|r| r.domain == *domain && r.is_approved())
        })
    }

    /// 待审查领域数。
    pub fn pending_count(&self) -> usize {
        LegalDomain::all()
            .iter()
            .filter(|d| !self.reviews.iter().any(|r| r.domain == **d))
            .count()
    }
}

/// 已知限制。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownLimit {
    /// 限制描述。
    pub description: String,
    /// 影响范围。
    pub scope: String,
    /// 变通方案。
    pub workaround: String,
}

impl KnownLimit {
    pub fn new(description: &str, scope: &str, workaround: &str) -> Self {
        Self {
            description: String::from(description),
            scope: String::from(scope),
            workaround: String::from(workaround),
        }
    }
}

/// 发布说明。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseNotes {
    /// 版本号。
    pub version: String,
    /// 发布日期。
    pub release_date: u64,
    /// 新功能。
    pub new_features: Vec<String>,
    /// 修复。
    pub bug_fixes: Vec<String>,
    /// 已知限制。
    pub known_limits: Vec<KnownLimit>,
    /// 支持矩阵（平台 → 最低版本）。
    pub support_matrix: Vec<(String, String)>,
    /// 上一版本（用于升级路径）。
    pub previous_version: String,
}

impl ReleaseNotes {
    pub fn new(version: &str, release_date: u64, previous_version: &str) -> Self {
        Self {
            version: String::from(version),
            release_date,
            new_features: Vec::new(),
            bug_fixes: Vec::new(),
            known_limits: Vec::new(),
            support_matrix: Vec::new(),
            previous_version: String::from(previous_version),
        }
    }

    pub fn add_feature(&mut self, feature: &str) {
        self.new_features.push(String::from(feature));
    }

    pub fn add_fix(&mut self, fix: &str) {
        self.bug_fixes.push(String::from(fix));
    }

    pub fn add_limit(&mut self, limit: KnownLimit) {
        self.known_limits.push(limit);
    }

    pub fn add_support(&mut self, platform: &str, min_version: &str) {
        self.support_matrix.push((String::from(platform), String::from(min_version)));
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.version.is_empty() {
            return Err("version is empty");
        }
        if self.previous_version.is_empty() {
            return Err("previous_version is empty");
        }
        if self.support_matrix.is_empty() {
            return Err("support_matrix is empty");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_legal_domains_approved() {
        let mut set = LegalReviewSet::new();
        for domain in LegalDomain::all() {
            set.add(LegalReview::new(*domain, LegalStatus::Approved, "lawyer", 1000, "ok"));
        }
        assert!(set.all_approved());
    }

    #[test]
    fn pending_domain_not_approved() {
        let mut set = LegalReviewSet::new();
        set.add(LegalReview::new(LegalDomain::Eula, LegalStatus::Approved, "lawyer", 1000, "ok"));
        set.add(LegalReview::new(LegalDomain::Privacy, LegalStatus::Approved, "lawyer", 1000, "ok"));
        set.add(LegalReview::new(LegalDomain::Export, LegalStatus::Approved, "lawyer", 1000, "ok"));
        // Crypto not reviewed
        assert!(!set.all_approved());
        assert_eq!(set.pending_count(), 1);
    }

    #[test]
    fn rejected_domain_not_approved() {
        let mut set = LegalReviewSet::new();
        for domain in LegalDomain::all() {
            let status = if *domain == LegalDomain::Crypto {
                LegalStatus::Rejected
            } else {
                LegalStatus::Approved
            };
            set.add(LegalReview::new(*domain, status, "lawyer", 1000, "comment"));
        }
        assert!(!set.all_approved());
    }

    #[test]
    fn release_notes_validate() {
        let mut notes = ReleaseNotes::new("0.200.0", 1000, "0.199.0");
        notes.add_feature("G8 fleet operations");
        notes.add_fix("Fixed canary auto-stop threshold");
        notes.add_limit(KnownLimit::new("No hardware BSP", "physical I/O", "use QEMU virtual platform"));
        notes.add_support("qemu-aarch64", "0.200.0");
        assert!(notes.validate().is_ok());
    }

    #[test]
    fn release_notes_empty_support_matrix_rejected() {
        let notes = ReleaseNotes::new("0.200.0", 1000, "0.199.0");
        assert!(notes.validate().is_err());
    }

    #[test]
    fn release_notes_empty_version_rejected() {
        let notes = ReleaseNotes::new("", 1000, "0.199.0");
        assert!(notes.validate().is_err());
    }

    #[test]
    fn four_legal_domains() {
        assert_eq!(LegalDomain::all().len(), 4);
    }
}

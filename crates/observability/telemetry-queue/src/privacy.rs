//! 隐私配置与审计 (OBS-014).
//!
//! 客户可配置哪些遥测类别允许上报。
//! 所有配置变更记录在审计日志中，确保可追溯。
//!
//! 设计：
//! - 默认所有类别 opt-out（不上报），客户需显式 opt-in。
//! - 配置变更记录 actor、timestamp、before/after。
//! - 审计日志不可篡改（只能追加）。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::queue::TelemetryCategory;

/// 遥测同意状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelemetryConsent {
    /// 客户已同意上报此类别。
    OptedIn,
    /// 客户已拒绝上报此类别。
    OptedOut,
    /// 未配置（默认为 OptedOut）。
    Unset,
}

impl TelemetryConsent {
    pub fn is_allowed(self) -> bool {
        matches!(self, Self::OptedIn)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::OptedIn => "OPTED_IN",
            Self::OptedOut => "OPTED_OUT",
            Self::Unset => "UNSET",
        }
    }
}

/// 隐私配置 — 每个遥测类别的同意状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivacyConfig {
    /// 各类别的同意状态。
    consents: [TelemetryConsent; 8],
    /// 配置版本号（每次变更递增）。
    version: u32,
    /// 最后修改时间（Unix 秒）。
    last_modified: u64,
    /// 最后修改者。
    last_modified_by: String,
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            consents: [TelemetryConsent::Unset; 8],
            version: 0,
            last_modified: 0,
            last_modified_by: String::new(),
        }
    }
}

impl PrivacyConfig {
    /// 创建新配置（所有类别默认 Unset = 不上报）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 获取类别的同意状态。
    pub fn consent(&self, category: TelemetryCategory) -> TelemetryConsent {
        self.consents[category as usize]
    }

    /// 设置类别的同意状态。
    ///
    /// 返回变更前的状态（用于审计日志）。
    pub fn set_consent(
        &mut self,
        category: TelemetryCategory,
        consent: TelemetryConsent,
        actor: &str,
        now_unix: u64,
    ) -> TelemetryConsent {
        let old = self.consents[category as usize];
        self.consents[category as usize] = consent;
        self.version += 1;
        self.last_modified = now_unix;
        self.last_modified_by = String::from(actor);
        old
    }

    /// 批量设置同意状态。
    pub fn set_all(&mut self, consent: TelemetryConsent, actor: &str, now_unix: u64) {
        for c in &mut self.consents {
            *c = consent;
        }
        self.version += 1;
        self.last_modified = now_unix;
        self.last_modified_by = String::from(actor);
    }

    /// 返回允许上报的类别列表。
    pub fn allowed_categories(&self) -> Vec<TelemetryCategory> {
        let all = [
            TelemetryCategory::SystemMetrics,
            TelemetryCategory::PerformanceMetrics,
            TelemetryCategory::AlertEvents,
            TelemetryCategory::SecurityAudit,
            TelemetryCategory::AgentBehavior,
            TelemetryCategory::CrashDumpRef,
            TelemetryCategory::ConfigChanges,
            TelemetryCategory::NetworkStats,
        ];
        all.iter()
            .filter(|c| self.consent(**c).is_allowed())
            .copied()
            .collect()
    }

    /// 检查类别是否允许上报。
    pub fn is_allowed(&self, category: TelemetryCategory) -> bool {
        self.consent(category).is_allowed()
    }

    /// 配置版本号。
    pub fn version(&self) -> u32 {
        self.version
    }

    /// 生成配置 JSON。
    pub fn to_json(&self) -> String {
        let entries: Vec<String> = [
            TelemetryCategory::SystemMetrics,
            TelemetryCategory::PerformanceMetrics,
            TelemetryCategory::AlertEvents,
            TelemetryCategory::SecurityAudit,
            TelemetryCategory::AgentBehavior,
            TelemetryCategory::CrashDumpRef,
            TelemetryCategory::ConfigChanges,
            TelemetryCategory::NetworkStats,
        ]
        .iter()
        .map(|c| format!(r#""{}":"{}""#, c.name(), self.consent(*c).label()))
        .collect();
        format!(
            r#"{{"version":{},"last_modified":{},"modified_by":"{}","consents":{{{}}}}}"#,
            self.version,
            self.last_modified,
            self.last_modified_by,
            entries.join(","),
        )
    }
}

/// 隐私配置审计条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivacyAuditEntry {
    /// 序号（单调递增）。
    pub seq: u64,
    /// 变更时间（Unix 秒）。
    pub timestamp: u64,
    /// 执行变更的 actor。
    pub actor: String,
    /// 变更的类别。
    pub category: TelemetryCategory,
    /// 变更前的状态。
    pub before: TelemetryConsent,
    /// 变更后的状态。
    pub after: TelemetryConsent,
    /// 配置版本号（变更后）。
    pub config_version: u32,
}

/// 隐私审计日志（只追加）。
pub struct PrivacyAuditLog {
    entries: Vec<PrivacyAuditEntry>,
    next_seq: u64,
}

impl Default for PrivacyAuditLog {
    fn default() -> Self {
        Self::new()
    }
}

impl PrivacyAuditLog {
    /// 创建空审计日志。
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            next_seq: 1,
        }
    }

    /// 记录一条变更。
    pub fn record(
        &mut self,
        timestamp: u64,
        actor: &str,
        category: TelemetryCategory,
        before: TelemetryConsent,
        after: TelemetryConsent,
        config_version: u32,
    ) {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.entries.push(PrivacyAuditEntry {
            seq,
            timestamp,
            actor: String::from(actor),
            category,
            before,
            after,
            config_version,
        });
    }

    /// 返回所有审计条目。
    pub fn entries(&self) -> &[PrivacyAuditEntry] {
        &self.entries
    }

    /// 返回指定类别的变更历史。
    pub fn history_for(&self, category: TelemetryCategory) -> Vec<&PrivacyAuditEntry> {
        self.entries
            .iter()
            .filter(|e| e.category == category)
            .collect()
    }

    /// 条目数。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 生成审计日志 JSON。
    pub fn to_json(&self) -> String {
        let entries: Vec<String> = self
            .entries
            .iter()
            .map(|e| {
                format!(
                    r#"{{"seq":{},"ts":{},"actor":"{}","category":"{}","before":"{}","after":"{}","version":{}}}"#,
                    e.seq,
                    e.timestamp,
                    e.actor,
                    e.category.name(),
                    e.before.label(),
                    e.after.label(),
                    e.config_version,
                )
            })
            .collect();
        format!(r#"[{}]"#, entries.join(","))
    }
}

/// 隐私管理器 — 组合配置和审计日志。
pub struct PrivacyManager {
    config: PrivacyConfig,
    audit_log: PrivacyAuditLog,
}

impl Default for PrivacyManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PrivacyManager {
    /// 创建隐私管理器（默认所有类别 Unset = 不上报）。
    pub fn new() -> Self {
        Self {
            config: PrivacyConfig::new(),
            audit_log: PrivacyAuditLog::new(),
        }
    }

    /// 设置类别同意状态（自动记录审计）。
    pub fn set_consent(
        &mut self,
        category: TelemetryCategory,
        consent: TelemetryConsent,
        actor: &str,
        now_unix: u64,
    ) {
        let before = self.config.consent(category);
        self.config.set_consent(category, consent, actor, now_unix);
        self.audit_log.record(
            now_unix,
            actor,
            category,
            before,
            consent,
            self.config.version(),
        );
    }

    /// 批量设置所有类别（自动记录审计）。
    pub fn set_all(&mut self, consent: TelemetryConsent, actor: &str, now_unix: u64) {
        let categories = [
            TelemetryCategory::SystemMetrics,
            TelemetryCategory::PerformanceMetrics,
            TelemetryCategory::AlertEvents,
            TelemetryCategory::SecurityAudit,
            TelemetryCategory::AgentBehavior,
            TelemetryCategory::CrashDumpRef,
            TelemetryCategory::ConfigChanges,
            TelemetryCategory::NetworkStats,
        ];
        for cat in &categories {
            let before = self.config.consent(*cat);
            if before != consent {
                self.config.set_consent(*cat, consent, actor, now_unix);
                self.audit_log.record(
                    now_unix,
                    actor,
                    *cat,
                    before,
                    consent,
                    self.config.version(),
                );
            }
        }
    }

    /// 获取配置。
    pub fn config(&self) -> &PrivacyConfig {
        &self.config
    }

    /// 获取审计日志。
    pub fn audit_log(&self) -> &PrivacyAuditLog {
        &self.audit_log
    }

    /// 检查类别是否允许上报。
    pub fn is_allowed(&self, category: TelemetryCategory) -> bool {
        self.config.is_allowed(category)
    }

    /// 返回允许上报的类别列表。
    pub fn allowed_categories(&self) -> Vec<TelemetryCategory> {
        self.config.allowed_categories()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_opted_out() {
        let config = PrivacyConfig::new();
        // 默认所有类别 Unset = 不上报
        assert!(!config.is_allowed(TelemetryCategory::SystemMetrics));
        assert!(!config.is_allowed(TelemetryCategory::SecurityAudit));
    }

    #[test]
    fn opt_in_allows_category() {
        let mut mgr = PrivacyManager::new();
        mgr.set_consent(
            TelemetryCategory::SystemMetrics,
            TelemetryConsent::OptedIn,
            "admin",
            1000,
        );
        assert!(mgr.is_allowed(TelemetryCategory::SystemMetrics));
        assert!(!mgr.is_allowed(TelemetryCategory::AlertEvents));
    }

    #[test]
    fn opt_out_blocks_category() {
        let mut mgr = PrivacyManager::new();
        mgr.set_consent(
            TelemetryCategory::SystemMetrics,
            TelemetryConsent::OptedIn,
            "admin",
            1000,
        );
        mgr.set_consent(
            TelemetryCategory::SystemMetrics,
            TelemetryConsent::OptedOut,
            "admin",
            1001,
        );
        assert!(!mgr.is_allowed(TelemetryCategory::SystemMetrics));
    }

    #[test]
    fn audit_log_records_changes() {
        let mut mgr = PrivacyManager::new();
        mgr.set_consent(
            TelemetryCategory::AlertEvents,
            TelemetryConsent::OptedIn,
            "operator",
            1000,
        );
        mgr.set_consent(
            TelemetryCategory::AlertEvents,
            TelemetryConsent::OptedOut,
            "admin",
            1001,
        );

        let log = mgr.audit_log();
        assert_eq!(log.len(), 2);
        let history = log.history_for(TelemetryCategory::AlertEvents);
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].before, TelemetryConsent::Unset);
        assert_eq!(history[0].after, TelemetryConsent::OptedIn);
        assert_eq!(history[1].before, TelemetryConsent::OptedIn);
        assert_eq!(history[1].after, TelemetryConsent::OptedOut);
    }

    #[test]
    fn set_all_changes_all_categories() {
        let mut mgr = PrivacyManager::new();
        mgr.set_all(TelemetryConsent::OptedIn, "admin", 1000);

        assert!(mgr.is_allowed(TelemetryCategory::SystemMetrics));
        assert!(mgr.is_allowed(TelemetryCategory::SecurityAudit));
        assert!(mgr.is_allowed(TelemetryCategory::AgentBehavior));
        assert_eq!(mgr.allowed_categories().len(), 8);
    }

    #[test]
    fn config_version_increments() {
        let mut mgr = PrivacyManager::new();
        assert_eq!(mgr.config().version(), 0);
        mgr.set_consent(
            TelemetryCategory::SystemMetrics,
            TelemetryConsent::OptedIn,
            "admin",
            1000,
        );
        assert_eq!(mgr.config().version(), 1);
        mgr.set_consent(
            TelemetryCategory::AlertEvents,
            TelemetryConsent::OptedIn,
            "admin",
            1001,
        );
        assert_eq!(mgr.config().version(), 2);
    }

    #[test]
    fn audit_log_seq_increments() {
        let mut mgr = PrivacyManager::new();
        mgr.set_consent(
            TelemetryCategory::SystemMetrics,
            TelemetryConsent::OptedIn,
            "admin",
            1000,
        );
        mgr.set_consent(
            TelemetryCategory::AlertEvents,
            TelemetryConsent::OptedIn,
            "admin",
            1001,
        );
        let entries = mgr.audit_log().entries();
        assert!(entries[1].seq > entries[0].seq);
    }

    #[test]
    fn allowed_categories_filters_correctly() {
        let mut mgr = PrivacyManager::new();
        mgr.set_consent(
            TelemetryCategory::SystemMetrics,
            TelemetryConsent::OptedIn,
            "admin",
            1000,
        );
        mgr.set_consent(
            TelemetryCategory::SecurityAudit,
            TelemetryConsent::OptedIn,
            "admin",
            1001,
        );
        let allowed = mgr.allowed_categories();
        assert_eq!(allowed.len(), 2);
        assert!(allowed.contains(&TelemetryCategory::SystemMetrics));
        assert!(allowed.contains(&TelemetryCategory::SecurityAudit));
    }

    #[test]
    fn config_json_contains_all_categories() {
        let mut mgr = PrivacyManager::new();
        mgr.set_consent(
            TelemetryCategory::SystemMetrics,
            TelemetryConsent::OptedIn,
            "admin",
            1000,
        );
        let json = mgr.config().to_json();
        assert!(json.contains("system-metrics"));
        assert!(json.contains("OPTED_IN"));
        assert!(json.contains("security-audit"));
        assert!(json.contains("UNSET"));
    }

    #[test]
    fn audit_log_json_contains_entries() {
        let mut mgr = PrivacyManager::new();
        mgr.set_consent(
            TelemetryCategory::SecurityAudit,
            TelemetryConsent::OptedIn,
            "admin",
            1000,
        );
        let json = mgr.audit_log().to_json();
        assert!(json.contains("security-audit"));
        assert!(json.contains("OPTED_IN"));
        assert!(json.contains("admin"));
    }

    #[test]
    fn no_audit_entry_when_no_change() {
        let mut mgr = PrivacyManager::new();
        mgr.set_all(TelemetryConsent::OptedIn, "admin", 1000);
        assert_eq!(mgr.audit_log().len(), 8);

        // 再次设置为相同值 — 不应产生审计条目
        mgr.set_all(TelemetryConsent::OptedIn, "admin", 1001);
        assert_eq!(mgr.audit_log().len(), 8);
    }
}

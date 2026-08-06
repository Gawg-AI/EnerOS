//! 遥测隐私控制 (DATA-009).
//!
//! MASTER §21.17 / G8: "telemetry privacy controls — opt-in/脱敏".
//!
//! 遥测数据必须遵循 opt-in 原则：客户未同意的类别不外发，
//! 已外发数据必须经过脱敏（redaction）去除密钥/PII/凭证。
//!
//! # 证据推导
//!
//! 遥测隐私通过结论由证据推导：
//! 1. 每个外发类别有对应 opt-in 同意记录
//! 2. 脱敏规则覆盖所有已知敏感字段
//! 3. 脱敏验证样本中无泄露（leaked_count == 0）
//! 4. 无未同意类别出现在输出中

use alloc::string::String;
use alloc::vec::Vec;

/// 遥测隐私类别.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PrivacyCategory {
    /// 崩溃诊断（含堆栈，需脱敏路径参数）.
    CrashDiagnostics,
    /// 性能指标（CPU/内存/延迟）.
    Performance,
    /// 使用统计（功能调用次数）.
    UsageStats,
    /// 网络指标（吞吐/错误率）.
    NetworkMetrics,
    /// 设备健康（温度/电压）.
    DeviceHealth,
}

impl PrivacyCategory {
    /// 是否默认敏感（需显式 opt-in）.
    pub fn is_sensitive_by_default(&self) -> bool {
        matches!(self, Self::CrashDiagnostics | Self::UsageStats)
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::CrashDiagnostics => "crash-diagnostics",
            Self::Performance => "performance",
            Self::UsageStats => "usage-stats",
            Self::NetworkMetrics => "network-metrics",
            Self::DeviceHealth => "device-health",
        }
    }
}

/// 脱敏规则.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactionRule {
    /// 匹配的字段名（如 "password"、"api_key"、"token"）.
    pub field_pattern: String,
    /// 替换值.
    pub replacement: String,
}

impl RedactionRule {
    pub fn new(field_pattern: &str, replacement: &str) -> Self {
        Self {
            field_pattern: String::from(field_pattern),
            replacement: String::from(replacement),
        }
    }

    /// 字段名是否匹配此规则（子串匹配，大小写不敏感）.
    pub fn matches(&self, field_name: &str) -> bool {
        field_name
            .to_ascii_lowercase()
            .contains(&self.field_pattern.to_ascii_lowercase())
    }
}

/// 已知敏感字段清单（必须全部有脱敏规则覆盖）.
pub const KNOWN_SENSITIVE_FIELDS: &[&str] = &[
    "password",
    "api_key",
    "apikey",
    "token",
    "secret",
    "private_key",
    "cert",
    "session",
    "cookie",
    "ssn",
    "phone",
    "email",
];

/// 遥测同意记录.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryConsent {
    /// 客户/租户 ID.
    pub tenant_id: String,
    /// 已同意的类别.
    pub consented_categories: Vec<PrivacyCategory>,
    /// 同意时间（Unix 秒）.
    pub consented_at: u64,
    /// 同意来源（UI/合同/管理员）.
    pub source: String,
}

impl TelemetryConsent {
    pub fn new(
        tenant_id: &str,
        consented_categories: Vec<PrivacyCategory>,
        consented_at: u64,
        source: &str,
    ) -> Self {
        Self {
            tenant_id: String::from(tenant_id),
            consented_categories,
            consented_at,
            source: String::from(source),
        }
    }

    /// 是否同意指定类别.
    pub fn has_consent(&self, category: PrivacyCategory) -> bool {
        self.consented_categories.contains(&category)
    }

    /// 同意是否有效（非空 + 有来源）.
    pub fn is_valid(&self) -> bool {
        !self.consented_categories.is_empty() && !self.source.is_empty()
    }
}

/// 脱敏验证结果.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactionVerify {
    /// 验证的输出样本字段名.
    pub sampled_fields: Vec<String>,
    /// 检测到的泄露字段（未脱敏的敏感字段）.
    pub leaked_fields: Vec<String>,
    /// 应用的脱敏规则数.
    pub rules_applied: usize,
}

impl RedactionVerify {
    pub fn new(
        sampled_fields: Vec<String>,
        leaked_fields: Vec<String>,
        rules_applied: usize,
    ) -> Self {
        Self {
            sampled_fields,
            leaked_fields,
            rules_applied,
        }
    }

    /// 是否无泄露.
    pub fn is_clean(&self) -> bool {
        self.leaked_fields.is_empty()
    }
}

/// 遥测隐私报告.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryPrivacyReport {
    /// 实际外发的类别.
    pub emitted_categories: Vec<PrivacyCategory>,
    /// 未同意但外发的类别（违规）.
    pub unconsented_emitted: Vec<PrivacyCategory>,
    /// 未覆盖的敏感字段（缺脱敏规则）.
    pub uncovered_sensitive_fields: Vec<String>,
    /// 脱敏验证.
    pub redaction_verify: RedactionVerify,
}

impl TelemetryPrivacyReport {
    /// 是否通过（由证据推导）.
    pub fn is_pass(&self) -> bool {
        self.unconsented_emitted.is_empty()
            && self.uncovered_sensitive_fields.is_empty()
            && self.redaction_verify.is_clean()
    }
}

/// 遥测隐私控制器.
pub struct TelemetryPrivacy {
    consent: TelemetryConsent,
    redaction_rules: Vec<RedactionRule>,
}

impl TelemetryPrivacy {
    pub fn new(consent: TelemetryConsent, redaction_rules: Vec<RedactionRule>) -> Self {
        Self {
            consent,
            redaction_rules,
        }
    }

    /// 缺少脱敏规则的敏感字段.
    pub fn uncovered_sensitive_fields(&self) -> Vec<String> {
        KNOWN_SENSITIVE_FIELDS
            .iter()
            .filter(|field| !self.redaction_rules.iter().any(|rule| rule.matches(field)))
            .map(|s| String::from(*s))
            .collect()
    }

    /// 评估一批外发数据的隐私合规.
    ///
    /// `emitted_categories` 为实际外发类别，
    /// `sampled_fields` 为输出样本中出现的字段名，
    /// `rules_applied` 为实际应用的脱敏规则数.
    pub fn evaluate(
        &self,
        emitted_categories: Vec<PrivacyCategory>,
        sampled_fields: Vec<String>,
        rules_applied: usize,
    ) -> TelemetryPrivacyReport {
        let unconsented_emitted = emitted_categories
            .iter()
            .filter(|c| !self.consent.has_consent(**c))
            .copied()
            .collect();

        let leaked_fields = sampled_fields
            .iter()
            .filter(|field| {
                KNOWN_SENSITIVE_FIELDS
                    .iter()
                    .any(|s| field.to_ascii_lowercase().contains(s))
            })
            .cloned()
            .collect();

        let uncovered = self.uncovered_sensitive_fields();

        TelemetryPrivacyReport {
            emitted_categories,
            unconsented_emitted,
            uncovered_sensitive_fields: uncovered,
            redaction_verify: RedactionVerify::new(sampled_fields, leaked_fields, rules_applied),
        }
    }

    /// 整体是否通过（由证据推导）.
    ///
    /// 条件：
    /// 1. 同意记录有效
    /// 2. 所有已知敏感字段有脱敏规则覆盖
    pub fn is_configured(&self) -> bool {
        self.consent.is_valid() && self.uncovered_sensitive_fields().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Vec<RedactionRule> {
        vec![
            RedactionRule::new("password", "***REDACTED***"),
            RedactionRule::new("api_key", "***REDACTED***"),
            RedactionRule::new("apikey", "***REDACTED***"),
            RedactionRule::new("token", "***REDACTED***"),
            RedactionRule::new("secret", "***REDACTED***"),
            RedactionRule::new("private_key", "***REDACTED***"),
            RedactionRule::new("cert", "***REDACTED***"),
            RedactionRule::new("session", "***REDACTED***"),
            RedactionRule::new("cookie", "***REDACTED***"),
            RedactionRule::new("ssn", "***REDACTED***"),
            RedactionRule::new("phone", "***REDACTED***"),
            RedactionRule::new("email", "***REDACTED***"),
        ]
    }

    #[test]
    fn t1_pass_when_consented_and_redacted() {
        let consent = TelemetryConsent::new(
            "tenant-1",
            vec![
                PrivacyCategory::Performance,
                PrivacyCategory::NetworkMetrics,
            ],
            1_700_000_000,
            "admin-ui",
        );
        let tp = TelemetryPrivacy::new(consent, rules());
        assert!(tp.is_configured());
        let report = tp.evaluate(
            vec![PrivacyCategory::Performance],
            vec![String::from("cpu_usage"), String::from("mem_bytes")],
            0,
        );
        assert!(report.is_pass());
    }

    #[test]
    fn t2_fail_when_unconsented_category_emitted() {
        let consent = TelemetryConsent::new(
            "tenant-1",
            vec![PrivacyCategory::Performance],
            1_700_000_000,
            "admin-ui",
        );
        let tp = TelemetryPrivacy::new(consent, rules());
        let report = tp.evaluate(
            vec![
                PrivacyCategory::Performance,
                PrivacyCategory::CrashDiagnostics,
            ],
            vec![String::from("cpu")],
            0,
        );
        assert!(!report.is_pass());
        assert_eq!(
            report.unconsented_emitted,
            vec![PrivacyCategory::CrashDiagnostics]
        );
    }

    #[test]
    fn t3_fail_when_sensitive_field_leaked() {
        let consent = TelemetryConsent::new(
            "tenant-1",
            vec![PrivacyCategory::CrashDiagnostics],
            1_700_000_000,
            "admin-ui",
        );
        let tp = TelemetryPrivacy::new(consent, rules());
        let report = tp.evaluate(
            vec![PrivacyCategory::CrashDiagnostics],
            vec![String::from("stack_trace"), String::from("api_key_plain")],
            0,
        );
        assert!(!report.is_pass());
        assert_eq!(
            report.redaction_verify.leaked_fields,
            vec![String::from("api_key_plain")]
        );
    }

    #[test]
    fn t4_fail_when_redaction_rule_missing() {
        let consent = TelemetryConsent::new(
            "tenant-1",
            vec![PrivacyCategory::Performance],
            1_700_000_000,
            "admin-ui",
        );
        // 缺少 password 规则
        let mut r = rules();
        r.retain(|x| x.field_pattern != "password");
        let tp = TelemetryPrivacy::new(consent, r);
        assert!(!tp.is_configured());
        assert!(tp
            .uncovered_sensitive_fields()
            .contains(&String::from("password")));
    }

    #[test]
    fn t5_fail_when_consent_invalid() {
        let consent = TelemetryConsent::new("tenant-1", Vec::new(), 1_700_000_000, "");
        let tp = TelemetryPrivacy::new(consent, rules());
        assert!(!tp.is_configured());
    }

    #[test]
    fn t6_redaction_rule_case_insensitive_match() {
        let rule = RedactionRule::new("API_KEY", "***");
        assert!(rule.matches("user_api_key"));
        assert!(rule.matches("API_KEY_FIELD"));
        assert!(!rule.matches("cpu_usage"));
    }

    #[test]
    fn t7_sensitive_category_needs_explicit_consent() {
        assert!(PrivacyCategory::CrashDiagnostics.is_sensitive_by_default());
        assert!(PrivacyCategory::UsageStats.is_sensitive_by_default());
        assert!(!PrivacyCategory::Performance.is_sensitive_by_default());
    }

    #[test]
    fn t8_clean_when_no_sensitive_fields_in_output() {
        let consent = TelemetryConsent::new(
            "t1",
            vec![PrivacyCategory::DeviceHealth],
            1_700_000_000,
            "contract",
        );
        let tp = TelemetryPrivacy::new(consent, rules());
        let report = tp.evaluate(
            vec![PrivacyCategory::DeviceHealth],
            vec![String::from("temperature"), String::from("voltage")],
            0,
        );
        assert!(report.is_pass());
        assert!(report.redaction_verify.is_clean());
    }

    #[test]
    fn t9_multiple_leaked_fields_all_detected() {
        let consent = TelemetryConsent::new(
            "t1",
            vec![PrivacyCategory::CrashDiagnostics],
            1_700_000_000,
            "admin",
        );
        let tp = TelemetryPrivacy::new(consent, rules());
        let report = tp.evaluate(
            vec![PrivacyCategory::CrashDiagnostics],
            vec![
                String::from("user_password"),
                String::from("auth_token"),
                String::from("client_secret"),
                String::from("stack"),
            ],
            0,
        );
        assert!(!report.is_pass());
        assert_eq!(report.redaction_verify.leaked_fields.len(), 3);
    }

    #[test]
    fn t10_consent_has_category_check() {
        let consent = TelemetryConsent::new(
            "t1",
            vec![PrivacyCategory::Performance, PrivacyCategory::UsageStats],
            1_700_000_000,
            "ui",
        );
        assert!(consent.has_consent(PrivacyCategory::Performance));
        assert!(consent.has_consent(PrivacyCategory::UsageStats));
        assert!(!consent.has_consent(PrivacyCategory::CrashDiagnostics));
    }
}

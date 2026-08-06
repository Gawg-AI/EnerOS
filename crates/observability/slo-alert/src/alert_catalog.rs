//! 告警目录 (OBS-012).
//!
//! 每条告警绑定 owner、recovery runbook 和定期演练状态。
//! 告警由实际事件触发，禁止硬编码告警状态。

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::runbook::{Runbook, RunbookStep};

/// 告警严重级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AlertSeverity {
    /// 信息（无需立即处理）。
    Info,
    /// 警告（需在 4h 内处理）。
    Warning,
    /// 严重（需在 1h 内处理）。
    Critical,
    /// 紧急（需立即处理，控制回路受影响）。
    Emergency,
}

impl AlertSeverity {
    pub fn label(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warning => "WARNING",
            Self::Critical => "CRITICAL",
            Self::Emergency => "EMERGENCY",
        }
    }

    /// 响应时限（秒）。
    pub fn response_sla_secs(self) -> u64 {
        match self {
            Self::Info => 24 * 60 * 60,   // 24h
            Self::Warning => 4 * 60 * 60, // 4h
            Self::Critical => 60 * 60,    // 1h
            Self::Emergency => 5 * 60,    // 5min
        }
    }
}

/// 告警状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertStatus {
    /// 活跃（未确认）。
    Firing,
    /// 已确认（有人在处理）。
    Acknowledged,
    /// 已解决。
    Resolved,
    /// 已抑制（维护窗口内）。
    Suppressed,
}

impl AlertStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Firing => "FIRING",
            Self::Acknowledged => "ACKNOWLEDGED",
            Self::Resolved => "RESOLVED",
            Self::Suppressed => "SUPPRESSED",
        }
    }
}

/// 告警定义（目录条目）。
#[derive(Debug, Clone)]
pub struct Alert {
    /// 告警 ID（稳定标识符）。
    pub id: &'static str,
    /// 告警名称。
    pub name: &'static str,
    /// 严重级别。
    pub severity: AlertSeverity,
    /// 负责 owner（团队/角色）。
    pub owner: &'static str,
    /// 关联的 SLO 基线 ID（可选）。
    pub related_slo: Option<&'static str>,
    /// 关联的 runbook。
    pub runbook: Runbook,
    /// 上次演练时间（Unix 秒，0 = 未演练）。
    pub last_drill_unix: u64,
    /// 演练周期（秒，超过此时间需重新演练）。
    pub drill_interval_secs: u64,
}

impl Alert {
    /// 检查是否需要演练。
    pub fn needs_drill(&self, now_unix: u64) -> bool {
        if self.last_drill_unix == 0 {
            return true;
        }
        now_unix.saturating_sub(self.last_drill_unix) > self.drill_interval_secs
    }

    /// 记录演练完成。
    pub fn record_drill(&mut self, now_unix: u64) {
        self.last_drill_unix = now_unix;
    }

    /// 生成告警目录 JSON。
    pub fn to_catalog_json(&self) -> String {
        format!(
            r#"{{"id":"{}","name":"{}","severity":"{}","owner":"{}","slo":"{}","runbook_steps":{},"last_drill":{},"needs_drill":{}}}"#,
            self.id,
            self.name,
            self.severity.label(),
            self.owner,
            self.related_slo.unwrap_or("null"),
            self.runbook.step_count(),
            self.last_drill_unix,
            self.last_drill_unix == 0,
        )
    }
}

/// 告警目录 — 所有告警定义的注册表。
pub struct AlertCatalog {
    alerts: Vec<Alert>,
}

impl Default for AlertCatalog {
    fn default() -> Self {
        Self::new()
    }
}

impl AlertCatalog {
    /// 创建空目录。
    pub fn new() -> Self {
        Self { alerts: Vec::new() }
    }

    /// 创建包含标准 EnerOS 告警的目录。
    pub fn with_standard_alerts() -> Self {
        let mut catalog = Self::new();
        catalog.register(standard_alerts());
        catalog
    }

    /// 注册告警。
    pub fn register(&mut self, alerts: Vec<Alert>) {
        self.alerts.extend(alerts);
    }

    /// 按 ID 查找告警。
    pub fn find(&self, id: &str) -> Option<&Alert> {
        self.alerts.iter().find(|a| a.id == id)
    }

    /// 按 ID 查找告警（可变）。
    pub fn find_mut(&mut self, id: &str) -> Option<&mut Alert> {
        self.alerts.iter_mut().find(|a| a.id == id)
    }

    /// 返回所有需要演练的告警。
    pub fn alerts_needing_drill(&self, now_unix: u64) -> Vec<&Alert> {
        self.alerts
            .iter()
            .filter(|a| a.needs_drill(now_unix))
            .collect()
    }

    /// 返回指定严重级别的告警。
    pub fn by_severity(&self, severity: AlertSeverity) -> Vec<&Alert> {
        self.alerts
            .iter()
            .filter(|a| a.severity == severity)
            .collect()
    }

    /// 返回告警数量。
    pub fn len(&self) -> usize {
        self.alerts.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.alerts.is_empty()
    }

    /// 生成完整目录 JSON。
    pub fn to_catalog_json(&self) -> String {
        let entries: Vec<String> = self.alerts.iter().map(|a| a.to_catalog_json()).collect();
        format!(r#"[{}]"#, entries.join(","))
    }
}

/// 标准 EnerOS 告警集合。
fn standard_alerts() -> Vec<Alert> {
    vec![
        Alert {
            id: "ALR-001",
            name: "Control loop deadline miss",
            severity: AlertSeverity::Emergency,
            owner: "control-team",
            related_slo: Some("control-loop-latency"),
            runbook: Runbook::new(
                "RNB-001",
                vec![
                    RunbookStep::new(
                        "Check control loop timing logs",
                        "Verify deadline-miss count in telemetry",
                    ),
                    RunbookStep::new(
                        "Identify blocking component",
                        "Check ISR/IPC/safety-validator timing",
                    ),
                    RunbookStep::new(
                        "Throttle AI workers",
                        "Reduce AI inference concurrency to free CPU",
                    ),
                    RunbookStep::new(
                        "Verify control plane health",
                        "Ensure actuator commands are flowing",
                    ),
                    RunbookStep::new(
                        "Escalate if unresolved",
                        "Page control-team on-call if miss rate > 1000/million",
                    ),
                ],
            ),
            last_drill_unix: 0,
            drill_interval_secs: 90 * 24 * 60 * 60, // 90 days
        },
        Alert {
            id: "ALR-002",
            name: "Agent process crash",
            severity: AlertSeverity::Critical,
            owner: "agent-team",
            related_slo: Some("agent-response"),
            runbook: Runbook::new(
                "RNB-002",
                vec![
                    RunbookStep::new("Collect crash dump", "Extract crash-dump.bin from device"),
                    RunbookStep::new(
                        "Symbolize crash dump",
                        "Run: xtask symbolize crash-dump.bin agent.elf",
                    ),
                    RunbookStep::new(
                        "Verify build-id match",
                        "Ensure crash dump build-id matches deployed ELF",
                    ),
                    RunbookStep::new(
                        "Identify root cause",
                        "Analyze stack trace for panic location",
                    ),
                    RunbookStep::new("Restart agent", "Use: agent start --id <agent-id>"),
                    RunbookStep::new(
                        "Escalate if recurring",
                        "File issue if same agent crashes > 3 times in 24h",
                    ),
                ],
            ),
            last_drill_unix: 0,
            drill_interval_secs: 90 * 24 * 60 * 60,
        },
        Alert {
            id: "ALR-003",
            name: "SLO violation — IPC latency",
            severity: AlertSeverity::Critical,
            owner: "runtime-team",
            related_slo: Some("ipc-round-trip"),
            runbook: Runbook::new(
                "RNB-003",
                vec![
                    RunbookStep::new(
                        "Check IPC metrics",
                        "Query SLO dashboard for ipc-round-trip P99",
                    ),
                    RunbookStep::new(
                        "Identify slow endpoint",
                        "Check per-endpoint IPC latency breakdown",
                    ),
                    RunbookStep::new(
                        "Check for IPC flood",
                        "Verify no agent is flooding IPC channel",
                    ),
                    RunbookStep::new(
                        "Apply rate limiting",
                        "Use management API to throttle offending agent",
                    ),
                    RunbookStep::new("Verify recovery", "Confirm P99 returns below 2ms target"),
                ],
            ),
            last_drill_unix: 0,
            drill_interval_secs: 90 * 24 * 60 * 60,
        },
        Alert {
            id: "ALR-004",
            name: "Disk space low",
            severity: AlertSeverity::Warning,
            owner: "ops-team",
            related_slo: None,
            runbook: Runbook::new(
                "RNB-004",
                vec![
                    RunbookStep::new("Check disk usage", "Run: status (check disk_usage field)"),
                    RunbookStep::new(
                        "Trigger log rotation",
                        "Force log rotation via management API",
                    ),
                    RunbookStep::new(
                        "Clean old metrics",
                        "Run: config set --key retention.days --value 7",
                    ),
                    RunbookStep::new(
                        "Export support bundle",
                        "Run: support-bundle export (frees space)",
                    ),
                    RunbookStep::new(
                        "Escalate if < 10% free",
                        "Plan storage expansion if chronic",
                    ),
                ],
            ),
            last_drill_unix: 0,
            drill_interval_secs: 180 * 24 * 60 * 60, // 180 days
        },
        Alert {
            id: "ALR-005",
            name: "AI inference timeout",
            severity: AlertSeverity::Warning,
            owner: "ai-team",
            related_slo: Some("llm-inference"),
            runbook: Runbook::new(
                "RNB-005",
                vec![
                    RunbookStep::new(
                        "Check AI engine status",
                        "Verify llama.cpp context is loaded",
                    ),
                    RunbookStep::new(
                        "Check model file integrity",
                        "Verify GGUF file hash matches manifest",
                    ),
                    RunbookStep::new(
                        "Reduce context length",
                        "Shorten prompt to reduce inference time",
                    ),
                    RunbookStep::new(
                        "Fallback to Solver-only",
                        "Enable L1 path (Solver without LLM)",
                    ),
                    RunbookStep::new(
                        "Escalate if persistent",
                        "Check for OOM or model corruption",
                    ),
                ],
            ),
            last_drill_unix: 0,
            drill_interval_secs: 90 * 24 * 60 * 60,
        },
        Alert {
            id: "ALR-006",
            name: "Security certificate expiring",
            severity: AlertSeverity::Critical,
            owner: "security-team",
            related_slo: None,
            runbook: Runbook::new(
                "RNB-006",
                vec![
                    RunbookStep::new(
                        "Check cert expiry",
                        "Query certificate store for expiry < 30 days",
                    ),
                    RunbookStep::new("Generate new CSR", "Create CSR with device identity"),
                    RunbookStep::new("Submit to CA", "Send CSR to organizational CA for signing"),
                    RunbookStep::new(
                        "Deploy new certificate",
                        "Install via: config set --key cert.path --value <path>",
                    ),
                    RunbookStep::new("Verify chain", "Validate new certificate chain"),
                    RunbookStep::new(
                        "Revoke old certificate",
                        "Run: security revoke --cert-id <old-cert-id>",
                    ),
                ],
            ),
            last_drill_unix: 0,
            drill_interval_secs: 365 * 24 * 60 * 60, // 365 days
        },
        Alert {
            id: "ALR-007",
            name: "Gateway authorization failure spike",
            severity: AlertSeverity::Critical,
            owner: "security-team",
            related_slo: Some("gateway-authorize"),
            runbook: Runbook::new(
                "RNB-007",
                vec![
                    RunbookStep::new(
                        "Check auth failure rate",
                        "Query audit log for auth failures > 10/min",
                    ),
                    RunbookStep::new(
                        "Identify source",
                        "Check if failures come from same agent/IP",
                    ),
                    RunbookStep::new(
                        "Check for credential expiry",
                        "Verify agent leases are still valid",
                    ),
                    RunbookStep::new(
                        "Block if attack",
                        "Revoke capabilities if malicious pattern detected",
                    ),
                    RunbookStep::new(
                        "Notify security team",
                        "Escalate to security-team if attack confirmed",
                    ),
                ],
            ),
            last_drill_unix: 0,
            drill_interval_secs: 90 * 24 * 60 * 60,
        },
        Alert {
            id: "ALR-008",
            name: "Memory budget exceeded",
            severity: AlertSeverity::Critical,
            owner: "runtime-team",
            related_slo: None,
            runbook: Runbook::new(
                "RNB-008",
                vec![
                    RunbookStep::new(
                        "Check memory usage",
                        "Run: status (check memory_usage field)",
                    ),
                    RunbookStep::new("Identify top consumer", "Check per-partition memory usage"),
                    RunbookStep::new(
                        "Trigger OOM handler",
                        "Verify OOM handler froze non-critical agents",
                    ),
                    RunbookStep::new(
                        "Reduce Agent count",
                        "Stop non-essential agents to free memory",
                    ),
                    RunbookStep::new(
                        "Check for leak",
                        "Monitor memory growth trend over 30 minutes",
                    ),
                ],
            ),
            last_drill_unix: 0,
            drill_interval_secs: 90 * 24 * 60 * 60,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_catalog_has_alerts() {
        let catalog = AlertCatalog::with_standard_alerts();
        assert!(catalog.len() >= 8);
    }

    #[test]
    fn find_alert_by_id() {
        let catalog = AlertCatalog::with_standard_alerts();
        let alert = catalog.find("ALR-001").expect("ALR-001 should exist");
        assert_eq!(alert.name, "Control loop deadline miss");
        assert_eq!(alert.severity, AlertSeverity::Emergency);
    }

    #[test]
    fn all_alerts_have_owners() {
        let catalog = AlertCatalog::with_standard_alerts();
        for alert in &catalog.alerts {
            assert!(!alert.owner.is_empty(), "alert {} has no owner", alert.id);
        }
    }

    #[test]
    fn all_alerts_have_runbooks() {
        let catalog = AlertCatalog::with_standard_alerts();
        for alert in &catalog.alerts {
            assert!(
                alert.runbook.step_count() > 0,
                "alert {} has no runbook steps",
                alert.id
            );
        }
    }

    #[test]
    fn emergency_alerts_have_short_response_sla() {
        let catalog = AlertCatalog::with_standard_alerts();
        for alert in catalog.by_severity(AlertSeverity::Emergency) {
            assert!(alert.severity.response_sla_secs() <= 300); // 5 min
        }
    }

    #[test]
    fn needs_drill_when_never_drilled() {
        let catalog = AlertCatalog::with_standard_alerts();
        let needing = catalog.alerts_needing_drill(1_000_000);
        assert_eq!(needing.len(), catalog.len());
    }

    #[test]
    fn needs_drill_after_interval() {
        let mut catalog = AlertCatalog::with_standard_alerts();
        let now = 1_000_000_000;
        catalog.find_mut("ALR-001").unwrap().record_drill(now);

        // Within interval — no drill needed
        let needing = catalog.alerts_needing_drill(now + 1000);
        assert_eq!(needing.len(), catalog.len() - 1);

        // After interval (90 days) — drill needed
        let later = now + 91 * 24 * 60 * 60;
        let needing = catalog.alerts_needing_drill(later);
        assert_eq!(needing.len(), catalog.len());
    }

    #[test]
    fn record_drill_updates_timestamp() {
        let mut catalog = AlertCatalog::with_standard_alerts();
        let alert = catalog.find_mut("ALR-002").unwrap();
        assert_eq!(alert.last_drill_unix, 0);
        alert.record_drill(12345);
        assert_eq!(alert.last_drill_unix, 12345);
    }

    #[test]
    fn catalog_json_contains_all_alerts() {
        let catalog = AlertCatalog::with_standard_alerts();
        let json = catalog.to_catalog_json();
        assert!(json.contains("ALR-001"));
        assert!(json.contains("ALR-002"));
        assert!(json.contains("EMERGENCY"));
        assert!(json.contains("control-team"));
    }

    #[test]
    fn alert_json_contains_runbook_info() {
        let catalog = AlertCatalog::with_standard_alerts();
        let alert = catalog.find("ALR-003").unwrap();
        let json = alert.to_catalog_json();
        assert!(json.contains("runtime-team"));
        assert!(json.contains("ipc-round-trip"));
    }

    #[test]
    fn by_severity_filters_correctly() {
        let catalog = AlertCatalog::with_standard_alerts();
        let emergency = catalog.by_severity(AlertSeverity::Emergency);
        assert!(emergency
            .iter()
            .all(|a| a.severity == AlertSeverity::Emergency));
        assert!(!emergency.is_empty());

        let info = catalog.by_severity(AlertSeverity::Info);
        assert!(info.is_empty()); // No info-level alerts in standard set
    }

    #[test]
    fn all_emergency_alerts_have_control_related_slo() {
        let catalog = AlertCatalog::with_standard_alerts();
        for alert in catalog.by_severity(AlertSeverity::Emergency) {
            assert!(
                alert.related_slo.is_some(),
                "emergency alert {} must relate to an SLO",
                alert.id
            );
        }
    }
}

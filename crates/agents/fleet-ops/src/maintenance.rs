//! 维护窗口与 preflight 检查 (OPS-005).
//!
//! MASTER §21.14 OPS-005: "maintenance window/preflight — 控制状态不安全时拒绝"
//!
//! 原则：维护操作（升级、重启、配置变更）在控制状态不安全时必须拒绝。
//! 控制状态不安全 = 有活跃告警 / 控制回路未稳定 / Agent 健康率不足 /
//! 实时任务 pending / 安全 validator 异常。
//!
//! preflight 是 fail-closed：检查项无法判定时视为不安全，拒绝维护。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// 维护操作类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaintenanceAction {
    /// OS/固件升级。
    OsUpgrade,
    /// 配置变更。
    ConfigChange,
    /// Agent 升级/重启。
    AgentUpdate,
    /// 模型更新。
    ModelUpdate,
    /// 计划内重启。
    PlannedReboot,
    /// 证书轮换。
    CertRotation,
}

impl MaintenanceAction {
    pub fn label(self) -> &'static str {
        match self {
            MaintenanceAction::OsUpgrade => "OS_UPGRADE",
            MaintenanceAction::ConfigChange => "CONFIG_CHANGE",
            MaintenanceAction::AgentUpdate => "AGENT_UPDATE",
            MaintenanceAction::ModelUpdate => "MODEL_UPDATE",
            MaintenanceAction::PlannedReboot => "PLANNED_REBOOT",
            MaintenanceAction::CertRotation => "CERT_ROTATION",
        }
    }
}

/// 维护窗口。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaintenanceWindow {
    /// 窗口 ID。
    pub id: String,
    /// 开始时间（Unix 秒）。
    pub start_unix: u64,
    /// 结束时间（Unix 秒）。
    pub end_unix: u64,
    /// 维护操作。
    pub action: MaintenanceAction,
    /// 审批人。
    pub approved_by: String,
    /// 目标设备列表（空 = 全 fleet）。
    pub target_devices: Vec<String>,
}

impl MaintenanceWindow {
    /// 检查时间戳是否在窗口内。
    pub fn is_active(&self, now_unix: u64) -> bool {
        now_unix >= self.start_unix && now_unix <= self.end_unix
    }

    /// 校验窗口合法性。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.id.is_empty() {
            return Err("window id is empty");
        }
        if self.end_unix <= self.start_unix {
            return Err("end_unix must be > start_unix");
        }
        if self.approved_by.is_empty() {
            return Err("approved_by is empty");
        }
        Ok(())
    }
}

/// Preflight 检查项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreflightCheck {
    /// 无活跃 Emergency/Critical 告警。
    NoActiveCriticalAlerts,
    /// 控制回路稳定（无 deadline miss）。
    ControlLoopStable,
    /// Agent 健康率达标。
    AgentHealthRateOk,
    /// 无 pending 实时任务。
    NoPendingRealtimeTasks,
    /// 安全 validator 正常。
    SafetyValidatorOk,
    /// 备份已完成（变更前必须可回滚）。
    BackupCompleted,
    /// 回滚槽可用。
    RollbackSlotAvailable,
    /// 维护窗口有效。
    WithinMaintenanceWindow,
}

impl PreflightCheck {
    pub fn label(self) -> &'static str {
        match self {
            PreflightCheck::NoActiveCriticalAlerts => "NO_ACTIVE_CRITICAL_ALERTS",
            PreflightCheck::ControlLoopStable => "CONTROL_LOOP_STABLE",
            PreflightCheck::AgentHealthRateOk => "AGENT_HEALTH_RATE_OK",
            PreflightCheck::NoPendingRealtimeTasks => "NO_PENDING_REALTIME_TASKS",
            PreflightCheck::SafetyValidatorOk => "SAFETY_VALIDATOR_OK",
            PreflightCheck::BackupCompleted => "BACKUP_COMPLETED",
            PreflightCheck::RollbackSlotAvailable => "ROLLBACK_SLOT_AVAILABLE",
            PreflightCheck::WithinMaintenanceWindow => "WITHIN_MAINTENANCE_WINDOW",
        }
    }
}

/// Preflight 检查结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreflightResult {
    /// 检查项。
    pub check: PreflightCheck,
    /// 是否通过。
    pub passed: bool,
    /// 失败原因（通过时为空）。
    pub reason: String,
}

impl PreflightResult {
    pub fn pass(check: PreflightCheck) -> Self {
        Self {
            check,
            passed: true,
            reason: String::new(),
        }
    }

    pub fn fail(check: PreflightCheck, reason: &str) -> Self {
        Self {
            check,
            passed: false,
            reason: String::from(reason),
        }
    }
}

/// 系统状态快照（preflight 输入）。
///
/// 所有字段均为 fail-closed 语义：未知/无法判定时填 `false` 或 `0`，
/// preflight 会据此拒绝维护。
#[derive(Debug, Clone, Default)]
pub struct SystemSnapshot {
    /// 活跃 Emergency/Critical 告警数。
    pub active_critical_alerts: u32,
    /// 最近控制回路 deadline miss 次数。
    pub recent_deadline_misses: u64,
    /// Agent 总数。
    pub agent_total: u32,
    /// 健康 Agent 数。
    pub agent_healthy: u32,
    /// pending 实时任务数。
    pub pending_realtime_tasks: u32,
    /// 安全 validator 是否正常。
    pub safety_validator_ok: bool,
    /// 变更前备份是否完成。
    pub backup_completed: bool,
    /// 回滚槽是否可用。
    pub rollback_slot_available: bool,
    /// 指标是否采集成功（false = fail-closed 拒绝）。
    pub metrics_available: bool,
}

/// Preflight 检查器。
pub struct MaintenancePreflight {
    /// 当前维护窗口（None = 无窗口，拒绝所有维护）。
    window: Option<MaintenanceWindow>,
}

impl Default for MaintenancePreflight {
    fn default() -> Self {
        Self::new()
    }
}

impl MaintenancePreflight {
    pub fn new() -> Self {
        Self { window: None }
    }

    /// 设置维护窗口。
    pub fn set_window(&mut self, window: MaintenanceWindow) -> Result<(), &'static str> {
        window.validate()?;
        self.window = Some(window);
        Ok(())
    }

    /// 运行全部 preflight 检查。
    ///
    /// 任一检查失败即拒绝维护。fail-closed：指标不可用时所有检查视为失败。
    pub fn run(&self, snapshot: &SystemSnapshot, now_unix: u64) -> Vec<PreflightResult> {
        let mut results = Vec::new();
        let metrics_ok = snapshot.metrics_available;

        // 1. 无活跃严重告警
        results.push(if !metrics_ok {
            PreflightResult::fail(
                PreflightCheck::NoActiveCriticalAlerts,
                "metrics unavailable (fail-closed)",
            )
        } else if snapshot.active_critical_alerts > 0 {
            PreflightResult::fail(
                PreflightCheck::NoActiveCriticalAlerts,
                &format!("{} active critical alerts", snapshot.active_critical_alerts),
            )
        } else {
            PreflightResult::pass(PreflightCheck::NoActiveCriticalAlerts)
        });

        // 2. 控制回路稳定
        results.push(if !metrics_ok {
            PreflightResult::fail(
                PreflightCheck::ControlLoopStable,
                "metrics unavailable (fail-closed)",
            )
        } else if snapshot.recent_deadline_misses > 0 {
            PreflightResult::fail(
                PreflightCheck::ControlLoopStable,
                &format!("{} recent deadline misses", snapshot.recent_deadline_misses),
            )
        } else {
            PreflightResult::pass(PreflightCheck::ControlLoopStable)
        });

        // 3. Agent 健康率（要求 100%）
        results.push(if !metrics_ok {
            PreflightResult::fail(
                PreflightCheck::AgentHealthRateOk,
                "metrics unavailable (fail-closed)",
            )
        } else if snapshot.agent_total == 0 {
            PreflightResult::pass(PreflightCheck::AgentHealthRateOk) // no agents = ok
        } else if snapshot.agent_healthy < snapshot.agent_total {
            PreflightResult::fail(
                PreflightCheck::AgentHealthRateOk,
                &format!(
                    "{}/{} agents healthy",
                    snapshot.agent_healthy, snapshot.agent_total
                ),
            )
        } else {
            PreflightResult::pass(PreflightCheck::AgentHealthRateOk)
        });

        // 4. 无 pending 实时任务
        results.push(if !metrics_ok {
            PreflightResult::fail(
                PreflightCheck::NoPendingRealtimeTasks,
                "metrics unavailable (fail-closed)",
            )
        } else if snapshot.pending_realtime_tasks > 0 {
            PreflightResult::fail(
                PreflightCheck::NoPendingRealtimeTasks,
                &format!("{} pending realtime tasks", snapshot.pending_realtime_tasks),
            )
        } else {
            PreflightResult::pass(PreflightCheck::NoPendingRealtimeTasks)
        });

        // 5. 安全 validator 正常
        results.push(if !snapshot.safety_validator_ok {
            PreflightResult::fail(PreflightCheck::SafetyValidatorOk, "safety validator not ok")
        } else {
            PreflightResult::pass(PreflightCheck::SafetyValidatorOk)
        });

        // 6. 备份已完成
        results.push(if !snapshot.backup_completed {
            PreflightResult::fail(PreflightCheck::BackupCompleted, "backup not completed")
        } else {
            PreflightResult::pass(PreflightCheck::BackupCompleted)
        });

        // 7. 回滚槽可用
        results.push(if !snapshot.rollback_slot_available {
            PreflightResult::fail(
                PreflightCheck::RollbackSlotAvailable,
                "rollback slot unavailable",
            )
        } else {
            PreflightResult::pass(PreflightCheck::RollbackSlotAvailable)
        });

        // 8. 维护窗口有效
        results.push(match &self.window {
            None => PreflightResult::fail(
                PreflightCheck::WithinMaintenanceWindow,
                "no maintenance window set",
            ),
            Some(w) => {
                if !w.is_active(now_unix) {
                    PreflightResult::fail(
                        PreflightCheck::WithinMaintenanceWindow,
                        "outside maintenance window",
                    )
                } else {
                    PreflightResult::pass(PreflightCheck::WithinMaintenanceWindow)
                }
            }
        });

        results
    }

    /// 判定是否允许执行维护（所有检查通过）。
    pub fn can_proceed(&self, snapshot: &SystemSnapshot, now_unix: u64) -> bool {
        self.run(snapshot, now_unix).iter().all(|r| r.passed)
    }

    /// 返回失败的检查项（拥有版本，避免悬垂引用）。
    pub fn collect_failures(
        &self,
        snapshot: &SystemSnapshot,
        now_unix: u64,
    ) -> Vec<PreflightResult> {
        self.run(snapshot, now_unix)
            .into_iter()
            .filter(|r| !r.passed)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn healthy_snapshot() -> SystemSnapshot {
        SystemSnapshot {
            active_critical_alerts: 0,
            recent_deadline_misses: 0,
            agent_total: 8,
            agent_healthy: 8,
            pending_realtime_tasks: 0,
            safety_validator_ok: true,
            backup_completed: true,
            rollback_slot_available: true,
            metrics_available: true,
        }
    }

    fn valid_window(now: u64) -> MaintenanceWindow {
        MaintenanceWindow {
            id: String::from("win-001"),
            start_unix: now,
            end_unix: now + 3600,
            action: MaintenanceAction::OsUpgrade,
            approved_by: String::from("admin"),
            target_devices: Vec::new(),
        }
    }

    #[test]
    fn all_clear_allows_maintenance() {
        let mut pf = MaintenancePreflight::new();
        pf.set_window(valid_window(1000)).unwrap();
        let snap = healthy_snapshot();
        assert!(pf.can_proceed(&snap, 1000));
    }

    #[test]
    fn active_alerts_block_maintenance() {
        let mut pf = MaintenancePreflight::new();
        pf.set_window(valid_window(1000)).unwrap();
        let mut snap = healthy_snapshot();
        snap.active_critical_alerts = 1;
        assert!(!pf.can_proceed(&snap, 1000));

        let failures = pf.collect_failures(&snap, 1000);
        assert!(failures
            .iter()
            .any(|f| f.check == PreflightCheck::NoActiveCriticalAlerts));
    }

    #[test]
    fn deadline_misses_block_maintenance() {
        let mut pf = MaintenancePreflight::new();
        pf.set_window(valid_window(1000)).unwrap();
        let mut snap = healthy_snapshot();
        snap.recent_deadline_misses = 5;
        assert!(!pf.can_proceed(&snap, 1000));
    }

    #[test]
    fn unhealthy_agents_block_maintenance() {
        let mut pf = MaintenancePreflight::new();
        pf.set_window(valid_window(1000)).unwrap();
        let mut snap = healthy_snapshot();
        snap.agent_healthy = 6; // 6/8
        assert!(!pf.can_proceed(&snap, 1000));
    }

    #[test]
    fn no_agents_is_ok() {
        let mut pf = MaintenancePreflight::new();
        pf.set_window(valid_window(1000)).unwrap();
        let mut snap = healthy_snapshot();
        snap.agent_total = 0;
        snap.agent_healthy = 0;
        assert!(pf.can_proceed(&snap, 1000));
    }

    #[test]
    fn pending_realtime_tasks_block() {
        let mut pf = MaintenancePreflight::new();
        pf.set_window(valid_window(1000)).unwrap();
        let mut snap = healthy_snapshot();
        snap.pending_realtime_tasks = 1;
        assert!(!pf.can_proceed(&snap, 1000));
    }

    #[test]
    fn safety_validator_failure_blocks() {
        let mut pf = MaintenancePreflight::new();
        pf.set_window(valid_window(1000)).unwrap();
        let mut snap = healthy_snapshot();
        snap.safety_validator_ok = false;
        assert!(!pf.can_proceed(&snap, 1000));
    }

    #[test]
    fn missing_backup_blocks() {
        let mut pf = MaintenancePreflight::new();
        pf.set_window(valid_window(1000)).unwrap();
        let mut snap = healthy_snapshot();
        snap.backup_completed = false;
        assert!(!pf.can_proceed(&snap, 1000));
    }

    #[test]
    fn no_rollback_slot_blocks() {
        let mut pf = MaintenancePreflight::new();
        pf.set_window(valid_window(1000)).unwrap();
        let mut snap = healthy_snapshot();
        snap.rollback_slot_available = false;
        assert!(!pf.can_proceed(&snap, 1000));
    }

    #[test]
    fn no_window_blocks_all_maintenance() {
        let pf = MaintenancePreflight::new();
        let snap = healthy_snapshot();
        assert!(!pf.can_proceed(&snap, 1000));
    }

    #[test]
    fn outside_window_blocks() {
        let mut pf = MaintenancePreflight::new();
        pf.set_window(valid_window(1000)).unwrap(); // window: 1000..4600
        let snap = healthy_snapshot();
        assert!(!pf.can_proceed(&snap, 5000)); // outside
    }

    #[test]
    fn metrics_unavailable_fail_closed() {
        let mut pf = MaintenancePreflight::new();
        pf.set_window(valid_window(1000)).unwrap();
        let mut snap = healthy_snapshot();
        snap.metrics_available = false;
        // fail-closed: cannot proceed when metrics unavailable
        assert!(!pf.can_proceed(&snap, 1000));

        let failures = pf.collect_failures(&snap, 1000);
        // Multiple checks should fail due to metrics unavailable
        assert!(failures.len() >= 4);
    }

    #[test]
    fn window_validate_rejects_bad() {
        let mut pf = MaintenancePreflight::new();
        let bad_window = MaintenanceWindow {
            id: String::new(),
            start_unix: 1000,
            end_unix: 2000,
            action: MaintenanceAction::OsUpgrade,
            approved_by: String::from("admin"),
            target_devices: Vec::new(),
        };
        assert!(pf.set_window(bad_window).is_err());

        let bad_window2 = MaintenanceWindow {
            id: String::from("win-002"),
            start_unix: 2000,
            end_unix: 1000, // end < start
            action: MaintenanceAction::OsUpgrade,
            approved_by: String::from("admin"),
            target_devices: Vec::new(),
        };
        assert!(pf.set_window(bad_window2).is_err());
    }

    #[test]
    fn all_eight_checks_run() {
        let mut pf = MaintenancePreflight::new();
        pf.set_window(valid_window(1000)).unwrap();
        let snap = healthy_snapshot();
        let results = pf.run(&snap, 1000);
        assert_eq!(results.len(), 8); // all 8 preflight checks
    }
}

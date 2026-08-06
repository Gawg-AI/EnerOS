//! 灾难恢复演练 (OPS-016).
//!
//! MASTER §21.14 / G9: "disaster recovery drill — fleet/离线站点场景".
//!
//! 在 fleet 级别和离线站点场景下演练灾难恢复，验证 RTO/RTO 达标、
//! 数据丢失不超过 RPO、恢复流程完整可执行。
//!
//! # 证据推导
//!
//! DR 演练通过结论由证据推导：
//! 1. 每个场景完成全部恢复步骤（Detect→RestoreService）
//! 2. 每个场景 RTO 实测 ≤ 目标
//! 3. 每个场景 RPO 实测 ≤ 目标
//! 4. 无超出 RPO 的数据丢失
//! 5. 覆盖 fleet 级别和离线站点场景

use alloc::string::String;
use alloc::vec::Vec;

/// 灾难场景类型.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DrScenarioKind {
    /// 整站点丢失（硬件/设施损毁）.
    SiteLoss,
    /// 网络分区（站点与中心断连）.
    NetworkPartition,
    /// 数据损坏（存储腐化/勒索软件）.
    DataCorruption,
    /// 大规模设备故障（批量设备失联）.
    MassDeviceFailure,
    /// 云/中心平台中断.
    CloudOutage,
}

impl DrScenarioKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::SiteLoss => "site-loss",
            Self::NetworkPartition => "network-partition",
            Self::DataCorruption => "data-corruption",
            Self::MassDeviceFailure => "mass-device-failure",
            Self::CloudOutage => "cloud-outage",
        }
    }
}

/// 恢复步骤.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrStep {
    /// 检测灾难.
    Detect,
    /// 宣布灾难状态.
    Declare,
    /// 隔离故障域.
    Isolate,
    /// 执行恢复.
    Recover,
    /// 验证数据/服务完整性.
    Verify,
    /// 恢复正常服务.
    RestoreService,
}

impl DrStep {
    /// 步骤顺序序号.
    pub fn ordinal(&self) -> u8 {
        match self {
            Self::Detect => 0,
            Self::Declare => 1,
            Self::Isolate => 2,
            Self::Recover => 3,
            Self::Verify => 4,
            Self::RestoreService => 5,
        }
    }

    /// 全部步骤（按顺序）.
    pub fn all() -> [DrStep; 6] {
        [
            Self::Detect,
            Self::Declare,
            Self::Isolate,
            Self::Recover,
            Self::Verify,
            Self::RestoreService,
        ]
    }
}

/// 单步骤执行结果.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrStepResult {
    pub step: DrStep,
    /// 是否完成.
    pub completed: bool,
    /// 完成时间（Unix 秒）.
    pub completed_at: u64,
    /// 证据（操作日志摘要/哈希）.
    pub evidence: String,
}

impl DrStepResult {
    pub fn new(step: DrStep, completed: bool, completed_at: u64, evidence: &str) -> Self {
        Self {
            step,
            completed,
            completed_at,
            evidence: String::from(evidence),
        }
    }
}

/// 灾难恢复场景实例.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrScenario {
    /// 场景 ID.
    pub scenario_id: String,
    /// 场景类型.
    pub kind: DrScenarioKind,
    /// RTO 目标（秒）.
    pub rto_target_secs: u64,
    /// RPO 目标（秒）.
    pub rpo_target_secs: u64,
    /// 灾难发生时间（Unix 秒）.
    pub incident_at: u64,
    /// 恢复完成时间（Unix 秒）.
    pub restored_at: u64,
    /// 数据丢失量（秒，即最后有效备份到故障的时间差）.
    pub data_loss_secs: u64,
    /// 步骤执行结果.
    pub steps: Vec<DrStepResult>,
}

impl DrScenario {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        scenario_id: &str,
        kind: DrScenarioKind,
        rto_target_secs: u64,
        rpo_target_secs: u64,
        incident_at: u64,
        restored_at: u64,
        data_loss_secs: u64,
        steps: Vec<DrStepResult>,
    ) -> Self {
        Self {
            scenario_id: String::from(scenario_id),
            kind,
            rto_target_secs,
            rpo_target_secs,
            incident_at,
            restored_at,
            data_loss_secs,
            steps,
        }
    }

    /// RTO 实测（秒）.
    pub fn rto_actual_secs(&self) -> u64 {
        self.restored_at.saturating_sub(self.incident_at)
    }

    /// RPO 实测（数据丢失秒数）.
    pub fn rpo_actual_secs(&self) -> u64 {
        self.data_loss_secs
    }

    /// RTO 是否达标.
    pub fn rto_passed(&self) -> bool {
        self.rto_actual_secs() <= self.rto_target_secs
    }

    /// RPO 是否达标.
    pub fn rpo_passed(&self) -> bool {
        self.rpo_actual_secs() <= self.rpo_target_secs
    }

    /// 未完成的步骤.
    pub fn incomplete_steps(&self) -> Vec<DrStep> {
        DrStep::all()
            .iter()
            .filter(|s| !self.steps.iter().any(|r| r.step == **s && r.completed))
            .copied()
            .collect()
    }

    /// 步骤是否按顺序执行（后一步时间 ≥ 前一步）.
    pub fn steps_in_order(&self) -> bool {
        let mut prev_time = 0u64;
        for step in DrStep::all() {
            if let Some(r) = self.steps.iter().find(|r| r.step == step) {
                if r.completed && r.completed_at < prev_time {
                    return false;
                }
                if r.completed {
                    prev_time = r.completed_at;
                }
            }
        }
        true
    }

    /// 场景是否通过（由证据推导）.
    ///
    /// 条件：
    /// 1. 全部步骤完成
    /// 2. 步骤按顺序
    /// 3. RTO 达标
    /// 4. RPO 达标
    pub fn passed(&self) -> bool {
        self.incomplete_steps().is_empty()
            && self.steps_in_order()
            && self.rto_passed()
            && self.rpo_passed()
    }
}

/// 灾难恢复演练.
///
/// 汇总多个场景，计算演练结论。结论**由证据推导**。
pub struct DisasterRecoveryDrill {
    scenarios: Vec<DrScenario>,
}

impl DisasterRecoveryDrill {
    pub fn new(scenarios: Vec<DrScenario>) -> Self {
        Self { scenarios }
    }

    /// 未通过的场景.
    pub fn failed_scenarios(&self) -> Vec<&String> {
        self.scenarios
            .iter()
            .filter(|s| !s.passed())
            .map(|s| &s.scenario_id)
            .collect()
    }

    /// 覆盖的场景类型.
    pub fn covered_kinds(&self) -> Vec<DrScenarioKind> {
        self.scenarios.iter().map(|s| s.kind).collect()
    }

    /// 是否覆盖必需场景（fleet 级别 + 离线站点）.
    ///
    /// 必需：至少一个 NetworkPartition（离线站点）和一个 MassDeviceFailure 或 SiteLoss（fleet 级别）.
    pub fn covers_required_scenarios(&self) -> bool {
        let kinds = self.covered_kinds();
        kinds.contains(&DrScenarioKind::NetworkPartition)
            && (kinds.contains(&DrScenarioKind::MassDeviceFailure)
                || kinds.contains(&DrScenarioKind::SiteLoss))
    }

    /// 演练是否通过（由证据推导）.
    ///
    /// 条件：
    /// 1. 非空（至少有场景）
    /// 2. 所有场景通过
    /// 3. 覆盖必需场景（fleet + 离线站点）
    pub fn all_passed(&self) -> bool {
        if self.scenarios.is_empty() {
            return false;
        }
        self.failed_scenarios().is_empty() && self.covers_required_scenarios()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete_steps(incident_at: u64, restore_at: u64) -> Vec<DrStepResult> {
        let span = restore_at - incident_at;
        let step_times = [
            incident_at,
            incident_at + span / 6,
            incident_at + span / 3,
            incident_at + span / 2,
            incident_at + (span * 4) / 5,
            restore_at,
        ];
        DrStep::all()
            .iter()
            .zip(step_times.iter())
            .map(|(step, &t)| DrStepResult::new(*step, true, t, "drill-log"))
            .collect()
    }

    #[test]
    fn t1_scenario_passed_within_targets() {
        let steps = complete_steps(1_700_000_000, 1_700_000_900); // 900s 恢复
        let s = DrScenario::new(
            "dr1",
            DrScenarioKind::NetworkPartition,
            3600, // RTO 1h
            300,  // RPO 5min
            1_700_000_000,
            1_700_000_900,
            60, // 数据丢失 60s
            steps,
        );
        assert!(s.passed());
        assert!(s.rto_passed());
        assert!(s.rpo_passed());
    }

    #[test]
    fn t2_fails_when_rto_exceeded() {
        let steps = complete_steps(1_700_000_000, 1_700_004_000); // 4000s 恢复
        let s = DrScenario::new(
            "dr2",
            DrScenarioKind::SiteLoss,
            3600,
            300,
            1_700_000_000,
            1_700_004_000,
            60,
            steps,
        );
        assert!(!s.passed());
        assert!(!s.rto_passed());
    }

    #[test]
    fn t3_fails_when_rpo_exceeded() {
        let steps = complete_steps(1_700_000_000, 1_700_000_900);
        let s = DrScenario::new(
            "dr3",
            DrScenarioKind::DataCorruption,
            3600,
            300,
            1_700_000_000,
            1_700_000_900,
            600, // 数据丢失 600s > RPO 300
            steps,
        );
        assert!(!s.passed());
        assert!(!s.rpo_passed());
    }

    #[test]
    fn t4_fails_when_step_incomplete() {
        let mut steps = complete_steps(1_700_000_000, 1_700_000_900);
        // 标记 Verify 未完成
        steps
            .iter_mut()
            .find(|r| r.step == DrStep::Verify)
            .unwrap()
            .completed = false;
        let s = DrScenario::new(
            "dr4",
            DrScenarioKind::NetworkPartition,
            3600,
            300,
            1_700_000_000,
            1_700_000_900,
            60,
            steps,
        );
        assert!(!s.passed());
        assert_eq!(s.incomplete_steps(), vec![DrStep::Verify]);
    }

    #[test]
    fn t5_fails_when_steps_out_of_order() {
        let mut steps = complete_steps(1_700_000_000, 1_700_000_900);
        // 让 Declare 时间早于 Detect
        steps
            .iter_mut()
            .find(|r| r.step == DrStep::Declare)
            .unwrap()
            .completed_at = 1_699_999_000;
        let s = DrScenario::new(
            "dr5",
            DrScenarioKind::NetworkPartition,
            3600,
            300,
            1_700_000_000,
            1_700_000_900,
            60,
            steps,
        );
        assert!(!s.passed());
        assert!(!s.steps_in_order());
    }

    #[test]
    fn t6_drill_all_passed_with_required_scenarios() {
        let s1 = DrScenario::new(
            "dr1",
            DrScenarioKind::NetworkPartition,
            3600,
            300,
            1_700_000_000,
            1_700_000_900,
            60,
            complete_steps(1_700_000_000, 1_700_000_900),
        );
        let s2 = DrScenario::new(
            "dr2",
            DrScenarioKind::MassDeviceFailure,
            7200,
            600,
            1_700_000_000,
            1_700_001_000,
            120,
            complete_steps(1_700_000_000, 1_700_001_000),
        );
        let drill = DisasterRecoveryDrill::new(vec![s1, s2]);
        assert!(drill.all_passed());
    }

    #[test]
    fn t7_drill_fails_without_offline_scenario() {
        let s = DrScenario::new(
            "dr1",
            DrScenarioKind::DataCorruption,
            3600,
            300,
            1_700_000_000,
            1_700_000_900,
            60,
            complete_steps(1_700_000_000, 1_700_000_900),
        );
        let drill = DisasterRecoveryDrill::new(vec![s]);
        assert!(!drill.all_passed());
        assert!(!drill.covers_required_scenarios());
    }

    #[test]
    fn t8_drill_fails_when_one_scenario_fails() {
        let s1 = DrScenario::new(
            "dr1",
            DrScenarioKind::NetworkPartition,
            3600,
            300,
            1_700_000_000,
            1_700_000_900,
            60,
            complete_steps(1_700_000_000, 1_700_000_900),
        );
        let s2 = DrScenario::new(
            "dr2",
            DrScenarioKind::MassDeviceFailure,
            3600,
            300,
            1_700_000_000,
            1_700_005_000, // RTO 超标
            60,
            complete_steps(1_700_000_000, 1_700_005_000),
        );
        let drill = DisasterRecoveryDrill::new(vec![s1, s2]);
        assert!(!drill.all_passed());
        assert_eq!(drill.failed_scenarios().len(), 1);
    }

    #[test]
    fn t9_empty_drill_fails() {
        let drill = DisasterRecoveryDrill::new(Vec::new());
        assert!(!drill.all_passed());
    }

    #[test]
    fn t10_site_loss_covers_fleet_requirement() {
        let s1 = DrScenario::new(
            "dr1",
            DrScenarioKind::NetworkPartition,
            3600,
            300,
            1_700_000_000,
            1_700_000_900,
            60,
            complete_steps(1_700_000_000, 1_700_000_900),
        );
        let s2 = DrScenario::new(
            "dr2",
            DrScenarioKind::SiteLoss,
            7200,
            600,
            1_700_000_000,
            1_700_001_000,
            120,
            complete_steps(1_700_000_000, 1_700_001_000),
        );
        let drill = DisasterRecoveryDrill::new(vec![s1, s2]);
        assert!(drill.covers_required_scenarios());
        assert!(drill.all_passed());
    }

    #[test]
    fn t11_cloud_outage_scenario() {
        let s = DrScenario::new(
            "dr1",
            DrScenarioKind::CloudOutage,
            1800,
            300,
            1_700_000_000,
            1_700_000_600,
            30,
            complete_steps(1_700_000_000, 1_700_000_600),
        );
        assert!(s.passed());
        // 单独 CloudOutage 不满足 fleet + offline 必需场景
        let drill = DisasterRecoveryDrill::new(vec![s]);
        assert!(!drill.covers_required_scenarios());
    }
}

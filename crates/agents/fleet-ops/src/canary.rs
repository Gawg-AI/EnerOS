//! Canary 发布与自动停止 (OPS-004).
//!
//! MASTER §21.14 OPS-004: "canary/rollout/auto-stop — 坏版本不扩散"
//!
//! 流程：
//! 1. 将目标版本分批次推送到 fleet（canary 批次先于全量批次）。
//! 2. 每批次升级后收集健康指标（成功率、SLO 违约率、崩溃数）。
//! 3. 若任一批次触发 auto-stop 阈值，立即冻结后续批次。
//! 4. 已升级设备进入回滚候选；未升级设备保持原版本。
//!
//! 设计原则：坏版本绝不扩散到全 fleet。auto-stop 是 fail-closed 行为 —
//! 指标采集失败同样视为违约，触发停止。

use alloc::string::String;
use alloc::vec::Vec;

/// Canary 阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanaryPhase {
    /// 尚未开始。
    Pending,
    /// Canary 批次进行中（小批量验证）。
    Canary,
    /// 正在扩展到更大批次。
    Promoting,
    /// 全量完成。
    Complete,
    /// 因指标违约自动停止（fail-closed）。
    AutoStopped,
    /// 人工停止。
    ManualStopped,
}

impl CanaryPhase {
    /// 是否处于停止态（不再推进）。
    pub fn is_stopped(self) -> bool {
        matches!(self, CanaryPhase::AutoStopped | CanaryPhase::ManualStopped)
    }

    /// 人类可读标签（telemetry/审计）。
    pub fn label(self) -> &'static str {
        match self {
            CanaryPhase::Pending => "PENDING",
            CanaryPhase::Canary => "CANARY",
            CanaryPhase::Promoting => "PROMOTING",
            CanaryPhase::Complete => "COMPLETE",
            CanaryPhase::AutoStopped => "AUTO_STOPPED",
            CanaryPhase::ManualStopped => "MANUAL_STOPPED",
        }
    }
}

/// 自动停止原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoStopReason {
    /// 成功率低于阈值。
    FailureRateExceeded,
    /// SLO P99 违约率超阈值。
    SloBreach,
    /// 崩溃次数超阈值。
    CrashSpike,
    /// 健康指标采集失败（fail-closed）。
    MetricsUnavailable,
    /// 人工干预停止。
    ManualStop,
}

impl AutoStopReason {
    pub fn label(self) -> &'static str {
        match self {
            AutoStopReason::FailureRateExceeded => "FAILURE_RATE_EXCEEDED",
            AutoStopReason::SloBreach => "SLO_BREACH",
            AutoStopReason::CrashSpike => "CRASH_SPIKE",
            AutoStopReason::MetricsUnavailable => "METRICS_UNAVAILABLE",
            AutoStopReason::ManualStop => "MANUAL_STOP",
        }
    }
}

/// 发布策略。
///
/// 阈值是**冻结的上界**：实际指标超过即触发 auto-stop。
/// `auto_stop_enabled = false` 仅用于测试；生产环境必须为 true。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RolloutPolicy {
    /// Canary 批次大小（占 fleet 百分比，0~100）。
    pub canary_batch_percent: u8,
    /// 扩展批次大小（占 fleet 百分比）。
    pub promote_batch_percent: u8,
    /// 单批次最大失败率（百分比，0~100）。超过即停止。
    pub max_failure_rate_percent: u8,
    /// 单批次最大 SLO 违约率（每百万次操作）。
    pub max_slo_breach_per_million: u64,
    /// 单批次最大崩溃次数。
    pub max_crash_count: u32,
    /// 是否启用 auto-stop。生产环境必须为 true。
    pub auto_stop_enabled: bool,
}

impl Default for RolloutPolicy {
    /// 生产默认策略：10% canary → 50% promote → 100%，严格 auto-stop。
    fn default() -> Self {
        Self::production()
    }
}

impl RolloutPolicy {
    /// 生产策略（fail-closed）。
    pub const fn production() -> Self {
        RolloutPolicy {
            canary_batch_percent: 10,
            promote_batch_percent: 50,
            max_failure_rate_percent: 5,
            max_slo_breach_per_million: 1000,
            max_crash_count: 3,
            auto_stop_enabled: true,
        }
    }

    /// 测试策略（可关闭 auto-stop）。
    pub const fn testing() -> Self {
        RolloutPolicy {
            canary_batch_percent: 100,
            promote_batch_percent: 100,
            max_failure_rate_percent: 100,
            max_slo_breach_per_million: u64::MAX,
            max_crash_count: u32::MAX,
            auto_stop_enabled: false,
        }
    }

    /// 校验策略合法性。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.canary_batch_percent == 0 || self.canary_batch_percent > 100 {
            return Err("canary_batch_percent must be 1..=100");
        }
        if self.promote_batch_percent == 0 || self.promote_batch_percent > 100 {
            return Err("promote_batch_percent must be 1..=100");
        }
        if self.promote_batch_percent < self.canary_batch_percent {
            return Err("promote_batch_percent must be >= canary_batch_percent");
        }
        if self.max_failure_rate_percent > 100 {
            return Err("max_failure_rate_percent must be 0..=100");
        }
        Ok(())
    }
}

/// 单批次健康指标。
#[derive(Debug, Clone, Copy, Default)]
pub struct BatchHealth {
    /// 批次内设备总数。
    pub device_count: u32,
    /// 升级成功数。
    pub success_count: u32,
    /// 升级失败数。
    pub failure_count: u32,
    /// SLO 违约次数（本批次累计）。
    pub slo_breach_count: u64,
    /// 崩溃次数（本批次累计）。
    pub crash_count: u32,
    /// 指标是否可用（false = 采集失败，触发 fail-closed）。
    pub metrics_available: bool,
}

impl BatchHealth {
    /// 失败率（百分比，0~100）。指标不可用时返回 100（fail-closed）。
    pub fn failure_rate_percent(&self) -> u8 {
        if !self.metrics_available {
            return 100;
        }
        if self.device_count == 0 {
            return 0;
        }
        let rate = (self.failure_count as u64 * 100) / self.device_count as u64;
        if rate > u64::from(u8::MAX) {
            100
        } else {
            rate as u8
        }
    }

    /// 每百万次操作的 SLO 违约率。
    pub fn slo_breach_per_million(&self) -> u64 {
        if !self.metrics_available {
            return u64::MAX;
        }
        // 以设备数为基数 × 1000 次操作估算
        let base_ops = self.device_count as u64 * 1000;
        if base_ops == 0 {
            return 0;
        }
        (self.slo_breach_count * 1_000_000) / base_ops
    }

    /// 对照策略判定是否应 auto-stop。
    pub fn should_auto_stop(&self, policy: &RolloutPolicy) -> Option<AutoStopReason> {
        if !policy.auto_stop_enabled {
            return None;
        }
        if !self.metrics_available {
            return Some(AutoStopReason::MetricsUnavailable);
        }
        if self.failure_rate_percent() > policy.max_failure_rate_percent {
            return Some(AutoStopReason::FailureRateExceeded);
        }
        if self.slo_breach_per_million() > policy.max_slo_breach_per_million {
            return Some(AutoStopReason::SloBreach);
        }
        if self.crash_count > policy.max_crash_count {
            return Some(AutoStopReason::CrashSpike);
        }
        None
    }
}

/// Canary 发布实例。
pub struct CanaryRollout {
    /// 目标版本。
    target_version: String,
    /// 策略。
    policy: RolloutPolicy,
    /// 当前阶段。
    phase: CanaryPhase,
    /// fleet 总设备数。
    fleet_size: u32,
    /// 已升级设备数。
    upgraded_count: u32,
    /// 当前批次健康指标。
    current_batch_health: BatchHealth,
    /// auto-stop 原因（若触发）。
    auto_stop_reason: Option<AutoStopReason>,
    /// 历史批次记录。
    batch_history: Vec<(CanaryPhase, BatchHealth)>,
}

impl CanaryRollout {
    /// 创建 canary 发布。
    pub fn new(
        target_version: &str,
        policy: RolloutPolicy,
        fleet_size: u32,
    ) -> Result<Self, &'static str> {
        policy.validate()?;
        if fleet_size == 0 {
            return Err("fleet_size must be > 0");
        }
        Ok(Self {
            target_version: String::from(target_version),
            policy,
            phase: CanaryPhase::Pending,
            fleet_size,
            upgraded_count: 0,
            current_batch_health: BatchHealth::default(),
            auto_stop_reason: None,
            batch_history: Vec::new(),
        })
    }

    /// 启动 canary 批次。
    pub fn start_canary(&mut self) -> Result<(), &'static str> {
        if self.phase != CanaryPhase::Pending {
            return Err("canary already started");
        }
        self.phase = CanaryPhase::Canary;
        self.current_batch_health = BatchHealth {
            device_count: self.batch_device_count(CanaryPhase::Canary),
            ..Default::default()
        };
        Ok(())
    }

    /// 上报当前批次健康指标。
    ///
    /// 自动检查 auto-stop 阈值。若触发，立即冻结。
    pub fn report_health(&mut self, health: BatchHealth) -> Result<(), AutoStopReason> {
        if self.phase.is_stopped() || self.phase == CanaryPhase::Complete {
            return Ok(());
        }
        self.current_batch_health = health;

        if let Some(reason) = health.should_auto_stop(&self.policy) {
            self.auto_stop_reason = Some(reason);
            self.batch_history
                .push((self.phase, self.current_batch_health));
            self.phase = CanaryPhase::AutoStopped;
            return Err(reason);
        }
        Ok(())
    }

    /// 标记当前批次成功完成，进入下一阶段。
    pub fn advance(&mut self) -> Result<CanaryPhase, &'static str> {
        if self.phase.is_stopped() {
            return Err("rollout is stopped, cannot advance");
        }
        match self.phase {
            CanaryPhase::Pending => Err("must start_canary first"),
            CanaryPhase::Canary => {
                self.batch_history
                    .push((self.phase, self.current_batch_health));
                self.upgraded_count += self.current_batch_health.success_count;
                self.phase = CanaryPhase::Promoting;
                self.current_batch_health = BatchHealth {
                    device_count: self.batch_device_count(CanaryPhase::Promoting),
                    ..Default::default()
                };
                Ok(self.phase)
            }
            CanaryPhase::Promoting => {
                self.batch_history
                    .push((self.phase, self.current_batch_health));
                self.upgraded_count += self.current_batch_health.success_count;
                self.phase = CanaryPhase::Complete;
                Ok(self.phase)
            }
            CanaryPhase::Complete | CanaryPhase::AutoStopped | CanaryPhase::ManualStopped => {
                Err("already in terminal phase")
            }
        }
    }

    /// 人工停止发布。
    pub fn manual_stop(&mut self) -> Result<(), &'static str> {
        if self.phase == CanaryPhase::Complete {
            return Err("rollout already complete");
        }
        if self.phase.is_stopped() {
            return Err("rollout already stopped");
        }
        self.auto_stop_reason = Some(AutoStopReason::ManualStop);
        self.phase = CanaryPhase::ManualStopped;
        Ok(())
    }

    /// 计算批次设备数。
    fn batch_device_count(&self, phase: CanaryPhase) -> u32 {
        let percent = match phase {
            CanaryPhase::Canary => self.policy.canary_batch_percent,
            CanaryPhase::Promoting => self.policy.promote_batch_percent,
            _ => 100,
        };
        let count = (self.fleet_size as u64 * percent as u64) / 100;
        count.max(1) as u32
    }

    /// 当前阶段。
    pub fn phase(&self) -> CanaryPhase {
        self.phase
    }

    /// auto-stop 原因。
    pub fn auto_stop_reason(&self) -> Option<AutoStopReason> {
        self.auto_stop_reason
    }

    /// 目标版本。
    pub fn target_version(&self) -> &str {
        &self.target_version
    }

    /// 已升级设备数。
    pub fn upgraded_count(&self) -> u32 {
        self.upgraded_count
    }

    /// 剩余未升级设备数。
    pub fn remaining_count(&self) -> u32 {
        self.fleet_size.saturating_sub(self.upgraded_count)
    }

    /// 历史批次记录。
    pub fn batch_history(&self) -> &[(CanaryPhase, BatchHealth)] {
        &self.batch_history
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn healthy_batch(devices: u32) -> BatchHealth {
        BatchHealth {
            device_count: devices,
            success_count: devices,
            failure_count: 0,
            slo_breach_count: 0,
            crash_count: 0,
            metrics_available: true,
        }
    }

    #[test]
    fn policy_default_is_production_fail_closed() {
        let p = RolloutPolicy::default();
        assert!(p.auto_stop_enabled);
        assert_eq!(p.canary_batch_percent, 10);
        assert_eq!(p.max_failure_rate_percent, 5);
    }

    #[test]
    fn policy_validate_rejects_bad_values() {
        let mut p = RolloutPolicy::production();
        p.canary_batch_percent = 0;
        assert!(p.validate().is_err());

        let mut p = RolloutPolicy::production();
        p.promote_batch_percent = 5; // less than canary
        assert!(p.validate().is_err());
    }

    #[test]
    fn canary_full_success_path() {
        let mut rollout = CanaryRollout::new("0.200.0", RolloutPolicy::production(), 100).unwrap();
        assert_eq!(rollout.phase(), CanaryPhase::Pending);

        rollout.start_canary().unwrap();
        assert_eq!(rollout.phase(), CanaryPhase::Canary);

        // Canary batch: 10 devices, all healthy
        rollout.report_health(healthy_batch(10)).unwrap();
        rollout.advance().unwrap();
        assert_eq!(rollout.phase(), CanaryPhase::Promoting);
        assert_eq!(rollout.upgraded_count(), 10);

        // Promote batch: 50 devices, all healthy
        rollout.report_health(healthy_batch(50)).unwrap();
        rollout.advance().unwrap();
        assert_eq!(rollout.phase(), CanaryPhase::Complete);
        assert_eq!(rollout.upgraded_count(), 60);
    }

    #[test]
    fn auto_stop_on_failure_rate() {
        let mut rollout = CanaryRollout::new("0.200.0", RolloutPolicy::production(), 100).unwrap();
        rollout.start_canary().unwrap();

        let bad_health = BatchHealth {
            device_count: 10,
            success_count: 5,
            failure_count: 5, // 50% failure > 5% threshold
            slo_breach_count: 0,
            crash_count: 0,
            metrics_available: true,
        };

        let result = rollout.report_health(bad_health);
        assert!(result.is_err());
        assert_eq!(rollout.phase(), CanaryPhase::AutoStopped);
        assert_eq!(
            rollout.auto_stop_reason(),
            Some(AutoStopReason::FailureRateExceeded)
        );
    }

    #[test]
    fn auto_stop_on_crash_spike() {
        let mut rollout = CanaryRollout::new("0.200.0", RolloutPolicy::production(), 100).unwrap();
        rollout.start_canary().unwrap();

        let crashy = BatchHealth {
            device_count: 10,
            success_count: 10,
            failure_count: 0,
            slo_breach_count: 0,
            crash_count: 5, // > 3 threshold
            metrics_available: true,
        };

        let result = rollout.report_health(crashy);
        assert!(result.is_err());
        assert_eq!(rollout.phase(), CanaryPhase::AutoStopped);
        assert_eq!(rollout.auto_stop_reason(), Some(AutoStopReason::CrashSpike));
    }

    #[test]
    fn auto_stop_on_metrics_unavailable_fail_closed() {
        let mut rollout = CanaryRollout::new("0.200.0", RolloutPolicy::production(), 100).unwrap();
        rollout.start_canary().unwrap();

        let no_metrics = BatchHealth {
            device_count: 10,
            metrics_available: false,
            ..Default::default()
        };

        let result = rollout.report_health(no_metrics);
        assert!(result.is_err());
        assert_eq!(rollout.phase(), CanaryPhase::AutoStopped);
        assert_eq!(
            rollout.auto_stop_reason(),
            Some(AutoStopReason::MetricsUnavailable)
        );
    }

    #[test]
    fn auto_stop_on_slo_breach() {
        let mut rollout = CanaryRollout::new("0.200.0", RolloutPolicy::production(), 100).unwrap();
        rollout.start_canary().unwrap();

        // 10 devices × 1000 ops = 10000 base ops
        // breach_count = 20 → 20 * 1_000_000 / 10000 = 2000 per million > 1000 threshold
        let slo_bad = BatchHealth {
            device_count: 10,
            success_count: 10,
            failure_count: 0,
            slo_breach_count: 20,
            crash_count: 0,
            metrics_available: true,
        };

        let result = rollout.report_health(slo_bad);
        assert!(result.is_err());
        assert_eq!(rollout.auto_stop_reason(), Some(AutoStopReason::SloBreach));
    }

    #[test]
    fn manual_stop_freezes_rollout() {
        let mut rollout = CanaryRollout::new("0.200.0", RolloutPolicy::production(), 100).unwrap();
        rollout.start_canary().unwrap();
        rollout.manual_stop().unwrap();
        assert_eq!(rollout.phase(), CanaryPhase::ManualStopped);

        // Cannot advance after stop
        let result = rollout.advance();
        assert!(result.is_err());
    }

    #[test]
    fn cannot_advance_after_complete() {
        let mut rollout = CanaryRollout::new("0.200.0", RolloutPolicy::production(), 100).unwrap();
        rollout.start_canary().unwrap();
        rollout.report_health(healthy_batch(10)).unwrap();
        rollout.advance().unwrap();
        rollout.report_health(healthy_batch(50)).unwrap();
        rollout.advance().unwrap();
        assert_eq!(rollout.phase(), CanaryPhase::Complete);

        let result = rollout.advance();
        assert!(result.is_err());
    }

    #[test]
    fn cannot_start_canary_twice() {
        let mut rollout = CanaryRollout::new("0.200.0", RolloutPolicy::production(), 100).unwrap();
        rollout.start_canary().unwrap();
        let result = rollout.start_canary();
        assert!(result.is_err());
    }

    #[test]
    fn batch_history_recorded() {
        let mut rollout = CanaryRollout::new("0.200.0", RolloutPolicy::production(), 100).unwrap();
        rollout.start_canary().unwrap();
        rollout.report_health(healthy_batch(10)).unwrap();
        rollout.advance().unwrap();
        rollout.report_health(healthy_batch(50)).unwrap();
        rollout.advance().unwrap();

        let history = rollout.batch_history();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].0, CanaryPhase::Canary);
        assert_eq!(history[1].0, CanaryPhase::Promoting);
    }

    #[test]
    fn testing_policy_disables_auto_stop() {
        let mut rollout = CanaryRollout::new("0.200.0", RolloutPolicy::testing(), 100).unwrap();
        rollout.start_canary().unwrap();

        let terrible = BatchHealth {
            device_count: 10,
            success_count: 0,
            failure_count: 10,
            slo_breach_count: 1000,
            crash_count: 100,
            metrics_available: false,
        };

        // With auto_stop disabled, even terrible health does not stop
        let result = rollout.report_health(terrible);
        assert!(result.is_ok());
        assert_eq!(rollout.phase(), CanaryPhase::Canary);
    }

    #[test]
    fn remaining_count_decreases() {
        let mut rollout = CanaryRollout::new("0.200.0", RolloutPolicy::production(), 100).unwrap();
        assert_eq!(rollout.remaining_count(), 100);

        rollout.start_canary().unwrap();
        rollout.report_health(healthy_batch(10)).unwrap();
        rollout.advance().unwrap();
        assert_eq!(rollout.upgraded_count(), 10);
        assert_eq!(rollout.remaining_count(), 90);
    }

    #[test]
    fn zero_fleet_rejected() {
        let result = CanaryRollout::new("0.200.0", RolloutPolicy::production(), 0);
        assert!(result.is_err());
    }
}

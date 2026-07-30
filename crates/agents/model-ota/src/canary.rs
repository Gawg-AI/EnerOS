//! 灰度发布引擎 (v0.162.0).
//!
//! 支持按比例推送、分批验证、失败率超阈值自动暂停/回滚。
//! 4 阶段灰度：5% → 25% → 50% → 100%。

use alloc::string::String;
use alloc::vec::Vec;

use crate::OtaError;

/// 灰度发布 ID.
pub type CanaryId = u64;

// ============================================================
// Canary Policy
// ============================================================

/// 灰度阶段定义.
#[derive(Debug, Clone, PartialEq)]
pub struct CanaryStage {
    /// 阶段名称
    pub name: String,
    /// 推送比例 (0.0~1.0)
    pub percentage: f64,
    /// 最短观察时间（秒）
    pub min_duration_secs: u64,
}

/// 灰度发布策略.
#[derive(Debug, Clone, PartialEq)]
pub struct CanaryPolicy {
    /// 灰度阶段列表
    pub stages: Vec<CanaryStage>,
    /// 自动暂停失败率阈值 (0.0~1.0)
    pub auto_pause_threshold: f64,
    /// 自动回滚失败率阈值 (0.0~1.0)
    pub rollback_threshold: f64,
}

impl CanaryPolicy {
    /// 创建默认 4 阶段灰度策略 (5% → 25% → 50% → 100%).
    pub fn default_four_stage() -> Self {
        Self {
            stages: alloc::vec![
                CanaryStage {
                    name: String::from("canary_5pct"),
                    percentage: 0.05,
                    min_duration_secs: 300,
                },
                CanaryStage {
                    name: String::from("canary_25pct"),
                    percentage: 0.25,
                    min_duration_secs: 600,
                },
                CanaryStage {
                    name: String::from("canary_50pct"),
                    percentage: 0.50,
                    min_duration_secs: 600,
                },
                CanaryStage {
                    name: String::from("full_rollout"),
                    percentage: 1.0,
                    min_duration_secs: 300,
                },
            ],
            auto_pause_threshold: 0.05,
            rollback_threshold: 0.10,
        }
    }
}

// ============================================================
// Canary Status
// ============================================================

/// 灰度发布状态.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanaryStatus {
    /// 进行中
    Running,
    /// 已暂停
    Paused,
    /// 已回滚
    RolledBack,
    /// 已完成
    Completed,
}

/// 阶段健康度.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageHealth {
    /// 健康（失败率低于暂停阈值）
    Healthy,
    /// 警告（失败率超暂停阈值，低于回滚阈值）
    Warning,
    /// 危急（失败率超回滚阈值）
    Critical,
}

// ============================================================
// Stage Status
// ============================================================

/// 阶段执行状态.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageStatus {
    /// 阶段索引
    pub stage_index: usize,
    /// 推送设备数
    pub pushed_count: usize,
    /// 成功数
    pub success_count: usize,
    /// 失败数
    pub failure_count: usize,
    /// 开始时间
    pub started_at: u64,
    /// 是否完成
    pub completed: bool,
}

impl StageStatus {
    /// 失败率.
    pub fn failure_rate(&self) -> f64 {
        if self.pushed_count == 0 {
            return 0.0;
        }
        self.failure_count as f64 / self.pushed_count as f64
    }
}

// ============================================================
// Canary Deployment
// ============================================================

/// 灰度发布实例.
#[derive(Debug, Clone)]
pub struct CanaryDeployment {
    /// 发布 ID
    pub id: CanaryId,
    /// 补丁版本标识
    pub patch_version: String,
    /// 灰度策略
    pub policy: CanaryPolicy,
    /// 当前阶段索引
    pub current_stage: usize,
    /// 各阶段状态
    pub stage_status: Vec<StageStatus>,
    /// 发布状态
    pub status: CanaryStatus,
    /// 开始时间
    pub started_at: u64,
    /// 总设备数
    pub total_devices: usize,
}

impl CanaryDeployment {
    /// 当前阶段状态.
    pub fn current_stage_status(&self) -> Option<&StageStatus> {
        self.stage_status.last()
    }

    /// 当前阶段健康度.
    pub fn stage_health(&self) -> StageHealth {
        let stage = match self.stage_status.last() {
            Some(s) => s,
            None => return StageHealth::Healthy,
        };
        let rate = stage.failure_rate();
        if rate > self.policy.rollback_threshold {
            StageHealth::Critical
        } else if rate > self.policy.auto_pause_threshold {
            StageHealth::Warning
        } else {
            StageHealth::Healthy
        }
    }

    /// 是否可以推进到下一阶段.
    pub fn can_advance(&self, now: u64) -> bool {
        if self.status != CanaryStatus::Running {
            return false;
        }
        let stage = match self.stage_status.last() {
            Some(s) => s,
            None => return false,
        };
        // 检查最短观察时间
        let elapsed = now.saturating_sub(stage.started_at);
        let min_duration = self.policy.stages[self.current_stage].min_duration_secs;
        if elapsed < min_duration {
            return false;
        }
        // 检查健康度
        self.stage_health() == StageHealth::Healthy
    }
}

// ============================================================
// Canary Engine
// ============================================================

/// 灰度发布引擎.
///
/// 管理灰度发布的全生命周期：启动 → 监控 → 推进/暂停/回滚 → 完成。
pub struct CanaryEngine {
    /// 所有灰度发布实例
    deployments: Vec<CanaryDeployment>,
    /// 下一个发布 ID
    next_id: CanaryId,
}

impl CanaryEngine {
    /// 创建灰度引擎.
    pub fn new() -> Self {
        Self {
            deployments: Vec::new(),
            next_id: 1,
        }
    }

    /// 启动灰度发布.
    pub fn start_canary(
        &mut self,
        patch_version: &str,
        policy: CanaryPolicy,
        total_devices: usize,
        now: u64,
    ) -> Result<CanaryId, OtaError> {
        if policy.stages.is_empty() {
            return Err(OtaError::InvalidConfig);
        }

        let id = self.next_id;
        self.next_id += 1;

        // 计算第一阶段推送数
        let first_stage = &policy.stages[0];
        let push_count = {
            let n = (total_devices as f64 * first_stage.percentage) as usize;
            if (total_devices as f64 * first_stage.percentage) > n as f64 {
                n + 1
            } else {
                n
            }
        };

        let stage_status = StageStatus {
            stage_index: 0,
            pushed_count: push_count,
            success_count: 0,
            failure_count: 0,
            started_at: now,
            completed: false,
        };

        let deployment = CanaryDeployment {
            id,
            patch_version: String::from(patch_version),
            policy,
            current_stage: 0,
            stage_status: alloc::vec![stage_status],
            status: CanaryStatus::Running,
            started_at: now,
            total_devices,
        };

        self.deployments.push(deployment);
        Ok(id)
    }

    /// 报告设备更新结果.
    pub fn report_result(&mut self, id: CanaryId, success: bool) -> Result<(), OtaError> {
        let deployment = self
            .deployments
            .iter_mut()
            .find(|d| d.id == id)
            .ok_or(OtaError::InvalidConfig)?;

        if let Some(stage) = deployment.stage_status.last_mut() {
            if success {
                stage.success_count += 1;
            } else {
                stage.failure_count += 1;
            }
        }
        Ok(())
    }

    /// 检查并执行自动暂停/回滚.
    pub fn check_health(&mut self, id: CanaryId) -> Result<StageHealth, OtaError> {
        let deployment = self
            .deployments
            .iter_mut()
            .find(|d| d.id == id)
            .ok_or(OtaError::InvalidConfig)?;

        let health = deployment.stage_health();
        match health {
            StageHealth::Critical => {
                deployment.status = CanaryStatus::RolledBack;
            }
            StageHealth::Warning => {
                deployment.status = CanaryStatus::Paused;
            }
            StageHealth::Healthy => {}
        }
        Ok(health)
    }

    /// 推进到下一阶段.
    pub fn advance_stage(&mut self, id: CanaryId, now: u64) -> Result<bool, OtaError> {
        let deployment = self
            .deployments
            .iter_mut()
            .find(|d| d.id == id)
            .ok_or(OtaError::InvalidConfig)?;

        if deployment.status != CanaryStatus::Running {
            return Ok(false);
        }

        // 标记当前阶段完成
        if let Some(stage) = deployment.stage_status.last_mut() {
            stage.completed = true;
        }

        let next_idx = deployment.current_stage + 1;
        if next_idx >= deployment.policy.stages.len() {
            // 所有阶段完成
            deployment.status = CanaryStatus::Completed;
            return Ok(false);
        }

        // 计算下一阶段推送数
        let next_stage = &deployment.policy.stages[next_idx];
        let push_count = {
            let n = (deployment.total_devices as f64 * next_stage.percentage) as usize;
            if (deployment.total_devices as f64 * next_stage.percentage) > n as f64 {
                n + 1
            } else {
                n
            }
        };

        let stage_status = StageStatus {
            stage_index: next_idx,
            pushed_count: push_count,
            success_count: 0,
            failure_count: 0,
            started_at: now,
            completed: false,
        };

        deployment.current_stage = next_idx;
        deployment.stage_status.push(stage_status);
        Ok(true)
    }

    /// 暂停灰度发布.
    pub fn pause_canary(&mut self, id: CanaryId) -> Result<(), OtaError> {
        let deployment = self
            .deployments
            .iter_mut()
            .find(|d| d.id == id)
            .ok_or(OtaError::InvalidConfig)?;
        deployment.status = CanaryStatus::Paused;
        Ok(())
    }

    /// 恢复灰度发布.
    pub fn resume_canary(&mut self, id: CanaryId) -> Result<(), OtaError> {
        let deployment = self
            .deployments
            .iter_mut()
            .find(|d| d.id == id)
            .ok_or(OtaError::InvalidConfig)?;
        if deployment.status == CanaryStatus::Paused {
            deployment.status = CanaryStatus::Running;
        }
        Ok(())
    }

    /// 回滚灰度发布.
    pub fn rollback_canary(&mut self, id: CanaryId) -> Result<(), OtaError> {
        let deployment = self
            .deployments
            .iter_mut()
            .find(|d| d.id == id)
            .ok_or(OtaError::InvalidConfig)?;
        deployment.status = CanaryStatus::RolledBack;
        Ok(())
    }

    /// 获取灰度发布状态.
    pub fn get_status(&self, id: CanaryId) -> Option<CanaryStatus> {
        self.deployments
            .iter()
            .find(|d| d.id == id)
            .map(|d| d.status)
    }

    /// 获取灰度发布实例.
    pub fn get_deployment(&self, id: CanaryId) -> Option<&CanaryDeployment> {
        self.deployments.iter().find(|d| d.id == id)
    }

    /// 活跃发布数量.
    pub fn active_count(&self) -> usize {
        self.deployments
            .iter()
            .filter(|d| d.status == CanaryStatus::Running)
            .count()
    }
}

impl Default for CanaryEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_policy() {
        let policy = CanaryPolicy::default_four_stage();
        assert_eq!(policy.stages.len(), 4);
        assert!((policy.stages[0].percentage - 0.05).abs() < f64::EPSILON);
        assert!((policy.stages[3].percentage - 1.0).abs() < f64::EPSILON);
        assert!((policy.auto_pause_threshold - 0.05).abs() < f64::EPSILON);
        assert!((policy.rollback_threshold - 0.10).abs() < f64::EPSILON);
    }

    #[test]
    fn test_start_canary() {
        let mut engine = CanaryEngine::new();
        let policy = CanaryPolicy::default_four_stage();
        let id = engine.start_canary("2.0.0", policy, 100, 1000).unwrap();
        assert_eq!(id, 1);
        assert_eq!(engine.get_status(id), Some(CanaryStatus::Running));

        let dep = engine.get_deployment(id).unwrap();
        assert_eq!(dep.current_stage, 0);
        assert_eq!(dep.total_devices, 100);
        // 5% of 100 = 5
        assert_eq!(dep.stage_status[0].pushed_count, 5);
    }

    #[test]
    fn test_report_results() {
        let mut engine = CanaryEngine::new();
        let policy = CanaryPolicy::default_four_stage();
        let id = engine.start_canary("2.0.0", policy, 100, 1000).unwrap();

        engine.report_result(id, true).unwrap();
        engine.report_result(id, true).unwrap();
        engine.report_result(id, false).unwrap();

        let dep = engine.get_deployment(id).unwrap();
        let stage = dep.current_stage_status().unwrap();
        assert_eq!(stage.success_count, 2);
        assert_eq!(stage.failure_count, 1);
    }

    #[test]
    fn test_healthy_advance() {
        let mut engine = CanaryEngine::new();
        let policy = CanaryPolicy::default_four_stage();
        let id = engine.start_canary("2.0.0", policy, 100, 1000).unwrap();

        // 全部成功
        for _ in 0..5 {
            engine.report_result(id, true).unwrap();
        }

        // 健康检查
        let health = engine.check_health(id).unwrap();
        assert_eq!(health, StageHealth::Healthy);
        assert_eq!(engine.get_status(id), Some(CanaryStatus::Running));

        // 推进（需要超过最短观察时间）
        let advanced = engine.advance_stage(id, 2000).unwrap();
        assert!(advanced);

        let dep = engine.get_deployment(id).unwrap();
        assert_eq!(dep.current_stage, 1);
        // 25% of 100 = 25
        assert_eq!(dep.stage_status[1].pushed_count, 25);
    }

    #[test]
    fn test_auto_pause_on_warning() {
        let mut engine = CanaryEngine::new();
        let policy = CanaryPolicy::default_four_stage();
        // 200 设备，5% = 10 台推送
        let id = engine.start_canary("2.0.0", policy, 200, 1000).unwrap();

        // 10 台推送，1 台失败 = 10% 失败率
        // 10% == rollback_threshold(0.10)，不触发 Critical（需 >）
        // 10% > auto_pause_threshold(0.05)，触发 Warning
        for _ in 0..9 {
            engine.report_result(id, true).unwrap();
        }
        engine.report_result(id, false).unwrap();

        let health = engine.check_health(id).unwrap();
        assert_eq!(health, StageHealth::Warning);
        assert_eq!(engine.get_status(id), Some(CanaryStatus::Paused));
    }

    #[test]
    fn test_auto_rollback_on_critical() {
        let mut engine = CanaryEngine::new();
        let mut policy = CanaryPolicy::default_four_stage();
        policy.rollback_threshold = 0.10;
        let id = engine.start_canary("2.0.0", policy, 20, 1000).unwrap();

        // 5% of 20 = 1 台推送，1 台失败 = 100% > 10% 回滚阈值
        engine.report_result(id, false).unwrap();

        let health = engine.check_health(id).unwrap();
        assert_eq!(health, StageHealth::Critical);
        assert_eq!(engine.get_status(id), Some(CanaryStatus::RolledBack));
    }

    #[test]
    fn test_full_canary_lifecycle() {
        let mut engine = CanaryEngine::new();
        let policy = CanaryPolicy::default_four_stage();
        let id = engine.start_canary("2.0.0", policy, 100, 0).unwrap();

        // 阶段 1: 5%
        for _ in 0..5 {
            engine.report_result(id, true).unwrap();
        }
        assert!(engine.advance_stage(id, 1000).unwrap());

        // 阶段 2: 25%
        for _ in 0..25 {
            engine.report_result(id, true).unwrap();
        }
        assert!(engine.advance_stage(id, 2000).unwrap());

        // 阶段 3: 50%
        for _ in 0..50 {
            engine.report_result(id, true).unwrap();
        }
        assert!(engine.advance_stage(id, 3000).unwrap());

        // 阶段 4: 100%
        for _ in 0..100 {
            engine.report_result(id, true).unwrap();
        }
        // 最后阶段推进 → 完成
        let advanced = engine.advance_stage(id, 4000).unwrap();
        assert!(!advanced); // 无下一阶段
        assert_eq!(engine.get_status(id), Some(CanaryStatus::Completed));
    }

    #[test]
    fn test_pause_and_resume() {
        let mut engine = CanaryEngine::new();
        let policy = CanaryPolicy::default_four_stage();
        let id = engine.start_canary("2.0.0", policy, 100, 1000).unwrap();

        engine.pause_canary(id).unwrap();
        assert_eq!(engine.get_status(id), Some(CanaryStatus::Paused));

        engine.resume_canary(id).unwrap();
        assert_eq!(engine.get_status(id), Some(CanaryStatus::Running));
    }

    #[test]
    fn test_manual_rollback() {
        let mut engine = CanaryEngine::new();
        let policy = CanaryPolicy::default_four_stage();
        let id = engine.start_canary("2.0.0", policy, 100, 1000).unwrap();

        engine.rollback_canary(id).unwrap();
        assert_eq!(engine.get_status(id), Some(CanaryStatus::RolledBack));
    }

    #[test]
    fn test_stage_failure_rate() {
        let stage = StageStatus {
            stage_index: 0,
            pushed_count: 10,
            success_count: 9,
            failure_count: 1,
            started_at: 0,
            completed: false,
        };
        assert!((stage.failure_rate() - 0.1).abs() < f64::EPSILON);
    }

    #[test]
    fn test_stage_failure_rate_zero_pushed() {
        let stage = StageStatus {
            stage_index: 0,
            pushed_count: 0,
            success_count: 0,
            failure_count: 0,
            started_at: 0,
            completed: false,
        };
        assert!((stage.failure_rate() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_active_count() {
        let mut engine = CanaryEngine::new();
        let policy = CanaryPolicy::default_four_stage();
        let id1 = engine
            .start_canary("2.0.0", policy.clone(), 100, 1000)
            .unwrap();
        let _id2 = engine.start_canary("3.0.0", policy, 100, 1000).unwrap();

        assert_eq!(engine.active_count(), 2);

        engine.pause_canary(id1).unwrap();
        assert_eq!(engine.active_count(), 1);
    }

    #[test]
    fn test_empty_policy_rejected() {
        let mut engine = CanaryEngine::new();
        let policy = CanaryPolicy {
            stages: Vec::new(),
            auto_pause_threshold: 0.05,
            rollback_threshold: 0.10,
        };
        let result = engine.start_canary("2.0.0", policy, 100, 1000);
        assert_eq!(result, Err(OtaError::InvalidConfig));
    }
}

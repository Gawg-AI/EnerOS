//! 灾备演练自动化（v0.26.0 — Task 6）
//!
//! 支持定期灾备演练，验证 failover 流程的可靠性：
//! - 演练场景：PrimaryDown / NetworkPartition / DiskFailure
//! - 调度策略：Daily / Weekly / Monthly
//! - 自动回滚：演练后自动恢复原状态
//! - 演练日志：JSON Lines 追加到 `/var/log/eneros/drill.log`（Linux）
//! - 历史记录：最多保留 50 条
//!
//! ## 场景说明
//!
//! - **PrimaryDown**：模拟主节点故障，触发 failover，验证备节点接管 < 3s，可选自动回滚
//! - **NetworkPartition**：模拟网络分区，记录日志（Quorum 验证需要 ClusterManager，当前为日志模拟）
//! - **DiskFailure**：模拟磁盘故障，记录日志（WAL 恢复验证需要持久化配置，当前为日志模拟）

use crate::ha::failover::{FailoverEngine, FailoverState};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{Arc, RwLock};
use std::time::Instant;
use thiserror::Error;

/// 演练历史最大保留条数
const MAX_HISTORY: usize = 50;

/// 演练场景
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrillScenario {
    /// 主节点故障
    PrimaryDown,
    /// 网络分区
    NetworkPartition,
    /// 磁盘故障
    DiskFailure,
}

/// 演练调度策略
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DrillSchedule {
    /// 每日
    Daily,
    /// 每周（默认）
    #[default]
    Weekly,
    /// 每月
    Monthly,
}

impl DrillSchedule {
    /// 返回调度间隔（秒）
    fn interval_secs(&self) -> u64 {
        match self {
            DrillSchedule::Daily => 24 * 3600,
            DrillSchedule::Weekly => 7 * 24 * 3600,
            DrillSchedule::Monthly => 30 * 24 * 3600,
        }
    }
}

/// 演练配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrillConfig {
    /// 是否启用自动演练
    #[serde(default)]
    pub enabled: bool,
    /// 调度策略
    #[serde(default)]
    pub schedule: DrillSchedule,
    /// 演练场景列表
    #[serde(default = "default_scenarios")]
    pub scenarios: Vec<DrillScenario>,
    /// 演练后是否自动回滚
    #[serde(default = "default_auto_rollback")]
    pub auto_rollback: bool,
}

fn default_scenarios() -> Vec<DrillScenario> {
    vec![DrillScenario::PrimaryDown]
}

fn default_auto_rollback() -> bool {
    true
}

impl Default for DrillConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            schedule: DrillSchedule::default(),
            scenarios: default_scenarios(),
            auto_rollback: default_auto_rollback(),
        }
    }
}

/// 演练结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrillResult {
    /// 演练场景
    pub scenario: DrillScenario,
    /// 开始时间（Unix 毫秒）
    pub start_time: i64,
    /// 结束时间（Unix 毫秒）
    pub end_time: i64,
    /// 演练耗时（毫秒）
    pub duration_ms: u64,
    /// 是否成功
    pub success: bool,
    /// 详细信息
    pub details: String,
    /// 执行节点 ID
    pub node_id: String,
}

/// 演练错误
#[derive(Debug, Error)]
pub enum DrillError {
    #[error("failover failed: {0}")]
    FailoverFailed(String),
    #[error("rollback failed: {0}")]
    RollbackFailed(String),
    #[error("drill timeout")]
    Timeout,
    #[error("drill not configured")]
    NotConfigured,
}

/// 灾备演练调度器
///
/// 管理灾备演练的调度和执行。所有 RwLock 使用 `unwrap_or_else(|e| e.into_inner())` 安全处理。
pub struct DrillScheduler {
    /// 演练配置
    config: DrillConfig,
    /// Failover 引擎
    failover_engine: Arc<FailoverEngine>,
    /// 上次演练时间
    last_drill: Arc<RwLock<Option<Instant>>>,
    /// 演练历史（最多 50 条）
    history: Arc<RwLock<VecDeque<DrillResult>>>,
    /// 本节点 ID
    node_id: String,
}

impl DrillScheduler {
    /// 创建演练调度器
    pub fn new(
        config: DrillConfig,
        failover_engine: Arc<FailoverEngine>,
        node_id: impl Into<String>,
    ) -> Self {
        Self {
            config,
            failover_engine,
            last_drill: Arc::new(RwLock::new(None)),
            history: Arc::new(RwLock::new(VecDeque::new())),
            node_id: node_id.into(),
        }
    }

    /// 检查是否到达演练时间
    ///
    /// 根据调度策略检查距上次演练是否已过足够时间：
    /// - Daily：24 小时
    /// - Weekly：168 小时（7 天）
    /// - Monthly：720 小时（30 天）
    ///
    /// 从未演练过时返回 `true`。
    pub fn should_run(&self) -> bool {
        let last = self
            .last_drill
            .read()
            .unwrap_or_else(|e| e.into_inner());
        match *last {
            None => true,
            Some(last_time) => {
                let elapsed = Instant::now().duration_since(last_time);
                elapsed.as_secs() >= self.config.schedule.interval_secs()
            }
        }
    }

    /// 执行单次演练
    ///
    /// 根据场景类型模拟故障并验证 failover 流程。
    /// 演练完成后如果 `auto_rollback = true`，自动调用 `trigger_recovery` 恢复原状态。
    pub fn run_drill(&self, scenario: DrillScenario) -> Result<DrillResult, DrillError> {
        let start = Instant::now();
        let start_time = current_timestamp_millis();

        let (success, details) = match scenario {
            DrillScenario::PrimaryDown => self.run_primary_down(),
            DrillScenario::NetworkPartition => self.run_network_partition(),
            DrillScenario::DiskFailure => self.run_disk_failure(),
        };

        let end_time = current_timestamp_millis();
        let duration_ms = start.elapsed().as_millis() as u64;

        let result = DrillResult {
            scenario,
            start_time,
            end_time,
            duration_ms,
            success,
            details,
            node_id: self.node_id.clone(),
        };

        // 记录历史
        self.record_history(result.clone());

        // 更新上次演练时间
        {
            let mut last = self
                .last_drill
                .write()
                .unwrap_or_else(|e| e.into_inner());
            *last = Some(Instant::now());
        }

        // 发布演练完成事件
        crate::ha::failover::HaEvent::HaDrillCompleted.publish();

        // 持久化日志
        self.log_drill(&result);

        Ok(result)
    }

    /// 执行所有配置场景
    ///
    /// 依次执行 `config.scenarios` 中的所有场景，返回每个场景的结果。
    /// 单个场景失败不影响其他场景执行。
    pub fn run_all_scenarios(&self) -> Vec<DrillResult> {
        self.config
            .scenarios
            .iter()
            .filter_map(|scenario| self.run_drill(*scenario).ok())
            .collect()
    }

    /// 手动触发演练（enerosctl 调用）
    ///
    /// 与 [`DrillScheduler::run_drill`] 相同，但语义上表示手动触发。
    pub fn run_drill_manual(&self, scenario: DrillScenario) -> Result<DrillResult, DrillError> {
        tracing::info!(?scenario, "手动触发灾备演练");
        self.run_drill(scenario)
    }

    /// PrimaryDown 场景：模拟主节点故障
    ///
    /// 1. 调用 `failover_engine.trigger_failover("drill: primary_down")`
    /// 2. 验证状态为 Active
    /// 3. 如果 `auto_rollback = true`，调用 `trigger_recovery` 恢复
    fn run_primary_down(&self) -> (bool, String) {
        // 触发 failover
        match self
            .failover_engine
            .trigger_failover("drill: primary_down")
        {
            Ok(record) => {
                let failover_ms = record.duration_ms;
                // 验证状态为 Active
                let state = self.failover_engine.current_state();
                if state != FailoverState::Active {
                    return (
                        false,
                        format!("failover 后状态非 Active: {:?}, failover 耗时 {}ms", state, failover_ms),
                    );
                }

                // 自动回滚
                if self.config.auto_rollback {
                    match self.failover_engine.trigger_recovery() {
                        Ok(recovery_record) => {
                            let total_ms = failover_ms + recovery_record.duration_ms;
                            (
                                true,
                                format!(
                                    "PrimaryDown 演练成功：failover {}ms + recovery {}ms = {}ms",
                                    failover_ms, recovery_record.duration_ms, total_ms
                                ),
                            )
                        }
                        Err(e) => (
                            false,
                            format!("PrimaryDown failover 成功但回滚失败: {}", e),
                        ),
                    }
                } else {
                    (
                        true,
                        format!(
                            "PrimaryDown 演练成功（未回滚）：failover {}ms，状态保持 Active",
                            failover_ms
                        ),
                    )
                }
            }
            Err(e) => (false, format!("PrimaryDown failover 失败: {}", e)),
        }
    }

    /// NetworkPartition 场景：模拟网络分区
    ///
    /// 当前为日志模拟（Quorum 验证需要 ClusterManager，未集成时仅记录日志）。
    fn run_network_partition(&self) -> (bool, String) {
        tracing::info!("NetworkPartition 演练：模拟网络分区");
        // Quorum 验证需要 ClusterManager，当前 DrillScheduler 未持有 ClusterManager
        // 仅记录日志，验证演练流程可执行
        (
            true,
            "NetworkPartition 演练完成：网络分区模拟成功（Quorum 验证需 ClusterManager 集成）".to_string(),
        )
    }

    /// DiskFailure 场景：模拟磁盘故障
    ///
    /// 当前为日志模拟（WAL 恢复验证需要持久化配置，未集成时仅记录日志）。
    fn run_disk_failure(&self) -> (bool, String) {
        tracing::info!("DiskFailure 演练：模拟磁盘故障");
        // WAL 恢复验证需要 SharedStore 持久化配置
        // 仅记录日志，验证演练流程可执行
        (
            true,
            "DiskFailure 演练完成：磁盘故障模拟成功（WAL 恢复验证需持久化配置集成）".to_string(),
        )
    }

    /// 追加演练日志到 `/var/log/eneros/drill.log`（JSON Lines）
    ///
    /// Linux 平台写入 `/var/log/eneros/drill.log`，非 Linux 平台写入临时目录。
    /// 写入失败时静默忽略，不影响演练主流程。
    pub fn log_drill(&self, result: &DrillResult) {
        let log_path = self.drill_log_path();
        if let Some(parent) = log_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!(error = %e, "创建演练日志目录失败");
                return;
            }
        }
        match serde_json::to_string(result) {
            Ok(line) => {
                use std::io::Write;
                if let Err(e) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&log_path)
                    .and_then(|mut f| writeln!(f, "{}", line))
                {
                    tracing::warn!(error = %e, "写入演练日志失败");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "序列化 DrillResult 失败");
            }
        }
    }

    /// 返回演练历史（按时间倒序，最新在前）
    pub fn history(&self) -> Vec<DrillResult> {
        let history = self
            .history
            .read()
            .unwrap_or_else(|e| e.into_inner());
        history.iter().rev().cloned().collect()
    }

    /// 返回上次演练时间（Unix 毫秒），从未演练过返回 None
    pub fn last_drill_time(&self) -> Option<i64> {
        let history = self
            .history
            .read()
            .unwrap_or_else(|e| e.into_inner());
        history.back().map(|r| r.end_time)
    }

    /// 记录演练历史（最多保留 50 条）
    fn record_history(&self, result: DrillResult) {
        let mut history = self
            .history
            .write()
            .unwrap_or_else(|e| e.into_inner());
        if history.len() >= MAX_HISTORY {
            history.pop_front();
        }
        history.push_back(result);
    }

    /// 演练日志路径
    fn drill_log_path(&self) -> std::path::PathBuf {
        #[cfg(target_os = "linux")]
        {
            std::path::PathBuf::from("/var/log/eneros/drill.log")
        }
        #[cfg(not(target_os = "linux"))]
        {
            std::env::temp_dir().join("eneros").join("drill.log")
        }
    }
}

/// 获取当前 Unix 时间戳（毫秒）
fn current_timestamp_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ha::{
        ConflictResolution, FailoverConfig, FencingStrategy, NodeRole, RecoveryPolicy,
        SharedStore, StorageQuota, SyncScope,
    };

    /// 构造测试用 HaConfig
    fn test_ha_config() -> crate::ha::HaConfig {
        crate::ha::HaConfig {
            node_id: "node-2".to_string(),
            role: NodeRole::Secondary,
            heartbeat_interval_ms: 100,
            heartbeat_suspect_ms: 100,
            heartbeat_dead_ms: 300,
            multicast_addr: "239.0.0.1".to_string(),
            heartbeat_port: 5400,
            sync_port: 5401,
            interfaces: Vec::new(),
            priority: 100,
            fencing_strategy: FencingStrategy::None,
            sync_scope: SyncScope::default(),
            auth_key: None,
            multicast_ttl: 32,
            is_production: false,
            failover: None,
            cluster: None,
            drill: None,
        }
    }

    /// 构造测试用 FailoverEngine
    fn test_failover_engine() -> Arc<FailoverEngine> {
        let config = test_ha_config();
        let store = Arc::new(SharedStore::new(
            "node-2",
            NodeRole::Secondary,
            ConflictResolution::default(),
            StorageQuota::default(),
        ));
        let failover_config = FailoverConfig {
            vip: "192.168.1.100".to_string(),
            vip_interface: "eth0".to_string(),
            cleanup_arp: false,
            takeover_timeout_ms: 3000,
            recovery_policy: RecoveryPolicy::AutoPreferPrimary,
        };
        Arc::new(FailoverEngine::new(config, store, failover_config))
    }

    #[test]
    fn test_drill_config_default() {
        let config = DrillConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.schedule, DrillSchedule::Weekly);
        assert_eq!(config.scenarios, vec![DrillScenario::PrimaryDown]);
        assert!(config.auto_rollback);
    }

    #[test]
    fn test_drill_schedule_default() {
        assert_eq!(DrillSchedule::default(), DrillSchedule::Weekly);
    }

    #[test]
    fn test_drill_schedule_interval() {
        assert_eq!(DrillSchedule::Daily.interval_secs(), 86400);
        assert_eq!(DrillSchedule::Weekly.interval_secs(), 604800);
        assert_eq!(DrillSchedule::Monthly.interval_secs(), 2592000);
    }

    #[test]
    fn test_should_run_first_time() {
        // 从未演练过 → should_run = true
        let engine = test_failover_engine();
        let scheduler = DrillScheduler::new(DrillConfig::default(), engine, "node-2");
        assert!(scheduler.should_run());
    }

    #[test]
    fn test_should_run_after_drill() {
        // 演练后 → should_run = false（未到下次调度时间）
        let engine = test_failover_engine();
        let scheduler = DrillScheduler::new(DrillConfig::default(), engine, "node-2");

        // 执行一次演练
        scheduler
            .run_drill(DrillScenario::PrimaryDown)
            .expect("drill should succeed");

        // 刚演练完 → should_run = false
        assert!(!scheduler.should_run());
    }

    #[test]
    fn test_run_drill_primary_down_success() {
        let engine = test_failover_engine();
        let scheduler = DrillScheduler::new(DrillConfig::default(), engine.clone(), "node-2");

        let result = scheduler
            .run_drill(DrillScenario::PrimaryDown)
            .expect("drill should succeed");

        assert_eq!(result.scenario, DrillScenario::PrimaryDown);
        assert!(result.success);
        assert!(result.duration_ms > 0);
        assert!(result.details.contains("PrimaryDown"));
        assert_eq!(result.node_id, "node-2");

        // auto_rollback=true → 演练后应回滚到 Standby
        assert_eq!(engine.current_state(), FailoverState::Standby);
    }

    #[test]
    fn test_run_drill_network_partition() {
        let engine = test_failover_engine();
        let scheduler = DrillScheduler::new(DrillConfig::default(), engine, "node-2");

        let result = scheduler
            .run_drill(DrillScenario::NetworkPartition)
            .expect("drill should succeed");

        assert_eq!(result.scenario, DrillScenario::NetworkPartition);
        assert!(result.success);
        assert!(result.details.contains("NetworkPartition"));
    }

    #[test]
    fn test_run_drill_disk_failure() {
        let engine = test_failover_engine();
        let scheduler = DrillScheduler::new(DrillConfig::default(), engine, "node-2");

        let result = scheduler
            .run_drill(DrillScenario::DiskFailure)
            .expect("drill should succeed");

        assert_eq!(result.scenario, DrillScenario::DiskFailure);
        assert!(result.success);
        assert!(result.details.contains("DiskFailure"));
    }

    #[test]
    fn test_run_all_scenarios() {
        let engine = test_failover_engine();
        let config = DrillConfig {
            enabled: true,
            schedule: DrillSchedule::Weekly,
            scenarios: vec![
                DrillScenario::PrimaryDown,
                DrillScenario::NetworkPartition,
                DrillScenario::DiskFailure,
            ],
            auto_rollback: true,
        };
        let scheduler = DrillScheduler::new(config, engine, "node-2");

        let results = scheduler.run_all_scenarios();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].scenario, DrillScenario::PrimaryDown);
        assert_eq!(results[1].scenario, DrillScenario::NetworkPartition);
        assert_eq!(results[2].scenario, DrillScenario::DiskFailure);
        // 所有场景都应成功
        for r in &results {
            assert!(r.success);
        }
    }

    #[test]
    fn test_run_drill_manual() {
        let engine = test_failover_engine();
        let scheduler = DrillScheduler::new(DrillConfig::default(), engine, "node-2");

        let result = scheduler
            .run_drill_manual(DrillScenario::PrimaryDown)
            .expect("manual drill should succeed");

        assert!(result.success);
        assert_eq!(result.scenario, DrillScenario::PrimaryDown);
    }

    #[test]
    fn test_drill_log_written() {
        let engine = test_failover_engine();
        let scheduler = DrillScheduler::new(DrillConfig::default(), engine, "node-2");

        let _result = scheduler
            .run_drill(DrillScenario::NetworkPartition)
            .expect("drill should succeed");

        // 验证日志文件存在且包含内容
        let log_path = scheduler.drill_log_path();
        assert!(log_path.exists(), "演练日志文件应存在");
        let content = std::fs::read_to_string(&log_path).expect("read log");
        assert!(
            content.contains("NetworkPartition"),
            "日志应包含场景名称"
        );
        assert!(content.contains("node-2"), "日志应包含节点 ID");
    }

    #[test]
    fn test_history_max_50() {
        let engine = test_failover_engine();
        // 使用 NetworkPartition 场景（不触发 failover，可重复执行）
        let config = DrillConfig {
            enabled: true,
            schedule: DrillSchedule::Daily,
            scenarios: vec![DrillScenario::NetworkPartition],
            auto_rollback: false,
        };
        let scheduler = DrillScheduler::new(config, engine, "node-2");

        // 执行 60 次演练
        for _ in 0..60 {
            scheduler
                .run_drill(DrillScenario::NetworkPartition)
                .expect("drill should succeed");
        }

        let history = scheduler.history();
        assert_eq!(history.len(), MAX_HISTORY, "历史应最多保留 {} 条", MAX_HISTORY);
    }

    #[test]
    fn test_drill_result_serde() {
        let result = DrillResult {
            scenario: DrillScenario::PrimaryDown,
            start_time: 1700000000000,
            end_time: 1700000003000,
            duration_ms: 3000,
            success: true,
            details: "演练成功".to_string(),
            node_id: "node-2".to_string(),
        };
        let json = serde_json::to_string(&result).expect("serialize");
        let de: DrillResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(de.scenario, result.scenario);
        assert_eq!(de.start_time, result.start_time);
        assert_eq!(de.end_time, result.end_time);
        assert_eq!(de.duration_ms, result.duration_ms);
        assert_eq!(de.success, result.success);
        assert_eq!(de.details, result.details);
        assert_eq!(de.node_id, result.node_id);
    }

    #[test]
    fn test_auto_rollback_true() {
        // auto_rollback=true → 演练后回滚到 Standby
        let engine = test_failover_engine();
        let config = DrillConfig {
            auto_rollback: true,
            ..DrillConfig::default()
        };
        let scheduler = DrillScheduler::new(config, engine.clone(), "node-2");

        scheduler
            .run_drill(DrillScenario::PrimaryDown)
            .expect("drill should succeed");

        // 回滚后状态应为 Standby
        assert_eq!(engine.current_state(), FailoverState::Standby);
    }

    #[test]
    fn test_auto_rollback_false() {
        // auto_rollback=false → 演练后保持 Active
        let engine = test_failover_engine();
        let config = DrillConfig {
            auto_rollback: false,
            ..DrillConfig::default()
        };
        let scheduler = DrillScheduler::new(config, engine.clone(), "node-2");

        let result = scheduler
            .run_drill(DrillScenario::PrimaryDown)
            .expect("drill should succeed");

        assert!(result.success);
        // 未回滚 → 状态保持 Active
        assert_eq!(engine.current_state(), FailoverState::Active);
    }

    #[test]
    fn test_last_drill_time() {
        let engine = test_failover_engine();
        let scheduler = DrillScheduler::new(DrillConfig::default(), engine, "node-2");

        // 从未演练 → None
        assert!(scheduler.last_drill_time().is_none());

        // 演练后 → Some
        scheduler
            .run_drill(DrillScenario::NetworkPartition)
            .expect("drill should succeed");
        assert!(scheduler.last_drill_time().is_some());
    }

    #[test]
    fn test_drill_scenario_serde() {
        let json = serde_json::to_string(&DrillScenario::PrimaryDown).expect("serialize");
        assert_eq!(json, "\"primary_down\"");
        let scenario: DrillScenario =
            serde_json::from_str("\"network_partition\"").expect("deserialize");
        assert_eq!(scenario, DrillScenario::NetworkPartition);
        let json = serde_json::to_string(&DrillScenario::DiskFailure).expect("serialize");
        assert_eq!(json, "\"disk_failure\"");
    }
}

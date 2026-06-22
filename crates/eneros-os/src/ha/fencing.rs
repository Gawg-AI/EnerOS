//! 脑裂防护（Fencing）模块（v0.25.1 — Task 5 安全加固 / v0.26.0 — Task 5 多节点支持 / v0.29.0 — Task T029-21 Quorum 校验）
//!
//! 提供脑裂（split-brain）防护框架：当主备节点失联且无法确认对方状态时，
//! 通过 fencing 策略强制隔离一方，防止双主写入导致数据损坏。
//!
//! 常见策略：
//! - **STONITH**（Shoot The Other Node In The Head）：通过远程电源开关/IPMI 强制关机
//! - **Disk Fencing**：SCSI Reservation 锁定共享存储访问
//! - **Network Fencing**：切断网络访问
//!
//! ## 脑裂检测
//!
//! [`FencingManager::detect_split_brain`] 基于心跳丢失节点列表和仲裁节点响应，
//! 判定本节点应保留 quorum 还是被 fencing：
//!
//! > **v0.26.0 变更**：移除 `dead_nodes.len() == 1` 约束，支持多节点集群。
//! > `dead_nodes` 可包含任意数量节点，[`SplitBrainResult::FencePeer`] 返回所有需 fencing 的节点列表。
//!
//! 1. `dead_nodes` 为空 → [`SplitBrainResult::NoSplitBrain`]
//! 2. 本节点在死节点列表中 → 本节点应被 fencing
//! 3. 有仲裁节点：超过半数可达 → 本节点有 quorum，对端应被 fencing；
//!    否则本节点应被 fencing
//! 4. 无仲裁节点（双节点）：Primary → 对端应被 fencing；
//!    Secondary → 保守策略，本节点应被 fencing
//!
//! ## 安全加固（v0.25.1）
//!
//! - 拒绝自 fencing（`target_node == self.node_id` → [`FencingError::InvalidTarget`]）
//! - 30 秒冷却期，防止 fencing 风暴（冷却期内返回 [`FencingResult::Skipped`]）
//! - 每次 fencing 记录追加写入 `/var/log/eneros/fencing.log`（JSON Lines，仅 Linux）
//! - `FencingRecord` 增加 `source_node` 字段以追溯发起方
//!
//! ## Quorum 校验（v0.29.0 — Task T029-21）
//!
//! [`FencingManager::fence`] 在执行实际 fencing 操作前校验 Quorum（多数派），
//! 无 Quorum 时返回 [`FencingError::NoQuorum`] 拒绝执行，防止脑裂场景下错误 fencing。
//! Quorum 状态由外部通过 [`FencingManager::update_quorum_state`] 更新，
//! 默认为单节点状态（持有 Quorum），保证未配置集群时 fencing 正常工作。
//!
//! ## 多节点支持（v0.26.0）
//!
//! - [`FencingManager::fence_all`]：批量 fencing 多个目标节点
//! - `detect_split_brain` 支持多节点 `dead_nodes` 列表
//!
//! > **注意**：SCSI/IPMI/Network fencing 当前为 stub 实现（返回 `NotConfigured`），
//! > 完整硬件驱动将在后续版本接入。

use crate::ha::NodeRole;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Instant;
use thiserror::Error;

/// Fencing 冷却期（秒）：同一目标节点在冷却期内重复 fencing 请求将被跳过
const FENCING_COOLDOWN_SECS: u64 = 30;

/// Fencing 策略
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FencingStrategy {
    /// 不启用 fencing（仅用于单节点测试，生产环境禁止使用）
    #[default]
    None,
    /// STONITH — 通过远程电源开关/IPMI 强制关机
    Stonith,
    /// 磁盘 fencing — 锁定共享存储
    Disk,
    /// 网络 fencing — 切断网络访问
    Network,
}

/// Fencing 错误
#[derive(Debug, Error)]
pub enum FencingError {
    #[error("fencing not supported on this platform")]
    Unsupported,
    #[error("fencing device error: {0}")]
    Device(String),
    #[error("fencing timeout")]
    Timeout,
    #[error("fencing strategy not configured")]
    NotConfigured,
    #[error("fencing failed: {0}")]
    Failed(String),
    /// 无效的 fencing 目标（例如试图 fencing 自身）
    #[error("invalid fencing target: {0}")]
    InvalidTarget(String),
    /// v0.25.x 仅支持双节点 HA，多节点场景不支持
    #[error("multi-node not supported in v0.25.x: {0}")]
    MultiNodeNotSupported(String),
    /// 无 Quorum（多数派），拒绝执行 fencing（v0.29.0 — Task T029-21）
    ///
    /// 脑裂防护：当本节点不持有 Quorum 时，禁止 fencing 操作，
    /// 防止脑裂场景下错误 fencing 导致数据损坏。
    #[error("no quorum: cannot fence without majority (alive={alive}, total={total})")]
    NoQuorum { alive: usize, total: usize },
}

/// 脑裂检测配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SplitBrainConfig {
    /// 心跳丢失阈值（超过此时间认为节点故障，毫秒）
    pub heartbeat_timeout_ms: u64,
    /// 仲裁节点列表（用于脑裂判定）
    pub quorum_nodes: Vec<String>,
    /// 仲裁超时（毫秒）
    pub quorum_timeout_ms: u64,
}

impl Default for SplitBrainConfig {
    fn default() -> Self {
        Self {
            heartbeat_timeout_ms: 300,
            quorum_nodes: vec![],
            quorum_timeout_ms: 1000,
        }
    }
}

/// Fencing 操作记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FencingRecord {
    /// 被 fencing 的节点 ID
    pub target_node: String,
    /// Fencing 策略
    pub strategy: FencingStrategy,
    /// 操作时间（Unix 毫秒）
    pub timestamp: i64,
    /// 操作结果
    pub result: FencingResult,
    /// 原因
    pub reason: String,
    /// 发起 fencing 的节点 ID（v0.25.1 — 追溯发起方）
    pub source_node: String,
}

/// Fencing 操作结果
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FencingResult {
    /// 成功
    Success,
    /// 失败
    Failed,
    /// 未配置（stub 返回）
    NotConfigured,
    /// 跳过（策略为 None 或冷却期内）
    Skipped,
}

/// 脑裂检测结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SplitBrainResult {
    /// 没有脑裂
    NoSplitBrain,
    /// 对端节点应该被 fencing（v0.26.0：支持多节点列表）
    FencePeer(Vec<String>),
    /// 本节点应该被 fencing
    ShouldBeFenced,
}

/// Quorum 仲裁状态（v0.29.0 — Task T029-21）
///
/// 记录集群节点总数和存活数，用于 fencing 前的 Quorum 校验。
/// 由外部（如 [`crate::ha::cluster::ClusterManager`]）通过
/// [`FencingManager::update_quorum_state`] 定期更新。
///
/// # Quorum 判定
///
/// 存活节点数 > 总节点数 / 2 时持有 Quorum（多数派）。
/// 例如 3 节点集群需 2 节点存活，5 节点集群需 3 节点存活。
///
/// # 默认值
///
/// 默认为单节点状态（total=1, alive=1），即默认持有 Quorum。
/// 这保证未配置集群的独立 [`FencingManager`] 仍可正常执行 fencing。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuorumState {
    /// 集群总节点数（不含 Witness 仲裁节点）
    pub total_members: usize,
    /// 存活节点数（不含 Witness 仲裁节点）
    pub alive_members: usize,
}

impl QuorumState {
    /// 创建 Quorum 状态
    pub fn new(total_members: usize, alive_members: usize) -> Self {
        Self {
            total_members,
            alive_members,
        }
    }

    /// 默认单节点状态（total=1, alive=1），持有 Quorum
    fn single_node() -> Self {
        Self {
            total_members: 1,
            alive_members: 1,
        }
    }

    /// 是否持有 Quorum（存活节点 > 总节点数 / 2）
    ///
    /// 与 [`crate::ha::cluster::ClusterManager::has_quorum`] 算法一致：
    /// `alive_members > total_members / 2`
    pub fn has_quorum(&self) -> bool {
        self.alive_members > self.total_members / 2
    }
}

impl Default for QuorumState {
    fn default() -> Self {
        Self::single_node()
    }
}

/// Fencing 管理器
///
/// 管理脑裂检测与 fencing 操作分发。根据 [`FencingStrategy`] 将 fencing 请求
/// 路由到具体实现（SCSI/IPMI/Network），并记录操作历史。
///
/// 当前 SCSI/IPMI/Network 实现为 stub（返回 [`FencingResult::NotConfigured`]），
/// 完整硬件驱动将在后续版本接入。
///
/// # 安全机制（v0.25.1）
///
/// - 拒绝自 fencing
/// - 30 秒冷却期防止 fencing 风暴
/// - 操作记录持久化至 `/var/log/eneros/fencing.log`（仅 Linux）
///
/// # Quorum 校验（v0.29.0 — Task T029-21）
///
/// 执行 fencing 前校验本节点是否持有 Quorum（多数派）。无 Quorum 时拒绝
/// fencing 操作，防止脑裂场景下错误 fencing。Quorum 状态由外部通过
/// [`FencingManager::update_quorum_state`] 更新，默认为单节点（持有 Quorum）。
pub struct FencingManager {
    /// 配置的策略
    strategy: FencingStrategy,
    /// 本节点 ID
    node_id: String,
    /// 本节点角色
    role: NodeRole,
    /// Fencing 操作历史
    history: Arc<RwLock<Vec<FencingRecord>>>,
    /// 脑裂检测配置
    split_brain_config: SplitBrainConfig,
    /// 每个目标节点最近一次 fencing 时间（用于冷却期判定）
    last_fence_time: Arc<RwLock<HashMap<String, Instant>>>,
    /// Quorum 仲裁状态（v0.29.0 — Task T029-21）
    ///
    /// 由外部（如 ClusterManager）通过 [`FencingManager::update_quorum_state`]
    /// 更新。默认为单节点状态（持有 Quorum），保证未配置集群时 fencing 正常工作。
    quorum_state: Arc<RwLock<QuorumState>>,
}

impl FencingManager {
    /// 创建 fencing 管理器
    ///
    /// # 参数
    /// - `strategy`: fencing 策略
    /// - `node_id`: 本节点 ID
    /// - `role`: 本节点角色（Primary/Secondary）
    /// - `config`: 脑裂检测配置
    pub fn new(
        strategy: FencingStrategy,
        node_id: impl Into<String>,
        role: NodeRole,
        config: SplitBrainConfig,
    ) -> Self {
        Self {
            strategy,
            node_id: node_id.into(),
            role,
            history: Arc::new(RwLock::new(Vec::new())),
            split_brain_config: config,
            last_fence_time: Arc::new(RwLock::new(HashMap::new())),
            quorum_state: Arc::new(RwLock::new(QuorumState::default())),
        }
    }

    /// 返回当前策略
    pub fn strategy(&self) -> FencingStrategy {
        self.strategy
    }

    /// 更新 Quorum 仲裁状态（v0.29.0 — Task T029-21）
    ///
    /// 由外部（如 [`crate::ha::cluster::ClusterManager`]）在心跳监测发现
    /// 节点状态变化时调用，将最新的集群节点总数和存活数同步给 FencingManager。
    ///
    /// # 参数
    /// - `total_members`: 集群总节点数（不含 Witness 仲裁节点）
    /// - `alive_members`: 存活节点数（不含 Witness 仲裁节点）
    pub fn update_quorum_state(&self, total_members: usize, alive_members: usize) {
        let mut state = self
            .quorum_state
            .write()
            .unwrap_or_else(|e| e.into_inner());
        state.total_members = total_members;
        state.alive_members = alive_members;
        tracing::debug!(
            total_members,
            alive_members,
            has_quorum = state.has_quorum(),
            "FencingManager Quorum 状态更新"
        );
    }

    /// 返回当前 Quorum 状态快照（v0.29.0 — Task T029-21）
    pub fn quorum_state(&self) -> QuorumState {
        *self
            .quorum_state
            .read()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// 校验 Quorum（v0.29.0 — Task T029-21）
    ///
    /// 判断本节点是否持有 Quorum（多数派）。无 Quorum 时返回错误，
    /// 拒绝执行 fencing 操作，防止脑裂场景下错误 fencing 导致数据损坏。
    ///
    /// # 算法
    ///
    /// 存活节点数 > 总节点数 / 2 时有 Quorum（与
    /// [`crate::ha::cluster::ClusterManager::has_quorum`] 一致）。
    ///
    /// # 返回
    /// - `Ok(())`：有 Quorum，可以执行 fencing
    /// - `Err(FencingError::NoQuorum)`：无 Quorum，拒绝 fencing
    pub fn check_quorum(&self) -> Result<(), FencingError> {
        let state = self
            .quorum_state
            .read()
            .unwrap_or_else(|e| e.into_inner());
        if state.has_quorum() {
            Ok(())
        } else {
            tracing::warn!(
                total_members = state.total_members,
                alive_members = state.alive_members,
                "fencing 拒绝执行：无 Quorum（脑裂防护）"
            );
            Err(FencingError::NoQuorum {
                alive: state.alive_members,
                total: state.total_members,
            })
        }
    }

    /// 检测脑裂
    ///
    /// 基于心跳丢失节点列表和仲裁节点响应判定脑裂状态。
    ///
    /// # 参数
    /// - `dead_nodes`: 心跳丢失的节点列表（v0.26.0：支持多节点）
    /// - `quorum_responses`: 仲裁节点响应（node_id → bool，true 表示可达）
    ///
    /// # 算法
    /// 1. `dead_nodes` 为空 → [`SplitBrainResult::NoSplitBrain`]
    /// 2. 本节点在 `dead_nodes` 中 → [`SplitBrainResult::ShouldBeFenced`]
    /// 3. 有仲裁节点：超过半数可达 → [`SplitBrainResult::FencePeer`]（所有死节点应被 fencing）；
    ///    否则 → [`SplitBrainResult::ShouldBeFenced`]
    /// 4. 无仲裁节点：Primary → [`SplitBrainResult::FencePeer`]（所有死节点）；
    ///    Secondary → [`SplitBrainResult::ShouldBeFenced`]（保守策略）
    pub fn detect_split_brain(
        &self,
        dead_nodes: &[String],
        quorum_responses: &HashMap<String, bool>,
    ) -> Result<SplitBrainResult, FencingError> {
        // v0.26.0：空死节点列表 → 无脑裂
        if dead_nodes.is_empty() {
            return Ok(SplitBrainResult::NoSplitBrain);
        }

        // 本节点在死节点列表中 → 本节点应被 fencing
        if dead_nodes.iter().any(|n| n == &self.node_id) {
            return Ok(SplitBrainResult::ShouldBeFenced);
        }

        // 有仲裁节点：按 quorum 判定
        if !self.split_brain_config.quorum_nodes.is_empty() {
            let total = self.split_brain_config.quorum_nodes.len();
            let reachable = self
                .split_brain_config
                .quorum_nodes
                .iter()
                .filter(|q| quorum_responses.get(*q).copied().unwrap_or(false))
                .count();

            if reachable * 2 > total {
                // 超过半数可达 → 本节点有 quorum → 所有死节点应被 fencing
                return Ok(SplitBrainResult::FencePeer(dead_nodes.to_vec()));
            } else {
                // 半数及以下可达 → 本节点无 quorum → 本节点应被 fencing
                return Ok(SplitBrainResult::ShouldBeFenced);
            }
        }

        // 无仲裁节点：按角色判定
        match self.role {
            NodeRole::Primary => Ok(SplitBrainResult::FencePeer(dead_nodes.to_vec())),
            NodeRole::Secondary => Ok(SplitBrainResult::ShouldBeFenced),
        }
    }

    /// 执行 Fencing
    ///
    /// 按当前策略分发到具体实现，记录操作到历史并持久化至日志，返回结果。
    ///
    /// # 安全校验
    /// 1. 拒绝自 fencing（`target_node == self.node_id` → [`FencingError::InvalidTarget`]）
    /// 2. Quorum 校验（v0.29.0 — Task T029-21）：无 Quorum 时拒绝 fencing
    ///    （[`FencingError::NoQuorum`]），防止脑裂场景下错误 fencing
    /// 3. 30 秒冷却期：冷却期内对同一目标的重复请求返回 [`FencingResult::Skipped`]
    ///
    /// # 参数
    /// - `target_node`: 被 fencing 的节点 ID
    /// - `reason`: fencing 原因（记录到历史）
    pub fn fence(&self, target_node: &str, reason: &str) -> Result<FencingResult, FencingError> {
        // 校验 1：禁止 fencing 自身
        if target_node == self.node_id {
            return Err(FencingError::InvalidTarget(format!(
                "cannot fence self (node_id={})",
                self.node_id
            )));
        }

        // 校验 2：Quorum 校验（v0.29.0 — Task T029-21）
        // 无 Quorum（多数派）时拒绝 fencing，防止脑裂场景下错误 fencing。
        // Quorum 状态由外部通过 update_quorum_state() 更新，默认单节点持有 Quorum。
        self.check_quorum()?;

        // 校验 3：冷却期检查：同一目标节点在 FENCING_COOLDOWN_SECS 内重复请求将被跳过
        let now = Instant::now();
        let in_cooldown = {
            let mut last_times = self
                .last_fence_time
                .write()
                .unwrap_or_else(|e| e.into_inner());
            let in_cd = match last_times.get(target_node) {
                Some(last) => now.duration_since(*last).as_secs() < FENCING_COOLDOWN_SECS,
                None => false,
            };
            // 不在冷却期才记录本次时间（冷却期内不刷新时间戳）
            if !in_cd {
                last_times.insert(target_node.to_string(), now);
            }
            in_cd
        };

        if in_cooldown {
            let record = FencingRecord {
                target_node: target_node.to_string(),
                strategy: self.strategy,
                timestamp: current_timestamp_millis(),
                result: FencingResult::Skipped,
                reason: format!("cooldown period ({}s)", FENCING_COOLDOWN_SECS),
                source_node: self.node_id.clone(),
            };
            self.history
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .push(record.clone());
            persist_fence_record(&record);
            return Ok(FencingResult::Skipped);
        }

        let result = match self.strategy {
            FencingStrategy::None => FencingResult::Skipped,
            FencingStrategy::Stonith => self.fence_ipmi(target_node),
            FencingStrategy::Disk => self.fence_scsi(target_node),
            FencingStrategy::Network => self.fence_network(target_node),
        };

        let record = FencingRecord {
            target_node: target_node.to_string(),
            strategy: self.strategy,
            timestamp: current_timestamp_millis(),
            result: result.clone(),
            reason: reason.to_string(),
            source_node: self.node_id.clone(),
        };
        self.history
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .push(record.clone());
        persist_fence_record(&record);

        Ok(result)
    }

    /// 批量 Fencing（v0.26.0 — Task 5）
    ///
    /// 对每个目标节点独立执行 [`FencingManager::fence`]，返回每个节点的结果。
    /// 某个节点 fencing 失败不影响其他节点的 fencing 执行。
    ///
    /// # 参数
    /// - `target_nodes`: 需要 fencing 的节点 ID 列表
    /// - `reason`: fencing 原因（记录到所有节点的历史）
    pub fn fence_all(
        &self,
        target_nodes: &[String],
        reason: &str,
    ) -> Vec<Result<FencingResult, FencingError>> {
        target_nodes
            .iter()
            .map(|node| self.fence(node, reason))
            .collect()
    }

    /// SCSI Reservation fencing（stub）
    ///
    /// 通过 SCSI 持久预留锁定共享存储访问。当前为 stub，返回
    /// [`FencingResult::NotConfigured`]，完整实现将在后续版本接入。
    fn fence_scsi(&self, _target_node: &str) -> FencingResult {
        FencingResult::NotConfigured
    }

    /// IPMI/PDU fencing（stub）
    ///
    /// 通过 IPMI 远程电源开关强制关机（STONITH）。当前为 stub，返回
    /// [`FencingResult::NotConfigured`]，完整实现将在后续版本接入。
    fn fence_ipmi(&self, _target_node: &str) -> FencingResult {
        FencingResult::NotConfigured
    }

    /// 网络隔离 fencing（stub）
    ///
    /// 切断目标节点的网络访问。当前为 stub，返回
    /// [`FencingResult::NotConfigured`]，完整实现将在后续版本接入。
    fn fence_network(&self, _target_node: &str) -> FencingResult {
        FencingResult::NotConfigured
    }

    /// 获取 Fencing 操作历史
    pub fn history(&self) -> Vec<FencingRecord> {
        self.history
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
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

/// 将 fencing 记录追加写入 `/var/log/eneros/fencing.log`（JSON Lines）。
///
/// 仅 Linux 平台执行持久化；非 Linux 平台（测试环境）跳过。
/// 写入失败（目录不存在/权限不足）时静默忽略，不影响 fencing 主流程。
#[cfg(target_os = "linux")]
fn persist_fence_record(record: &FencingRecord) {
    use std::fs::OpenOptions;
    use std::io::Write;
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open("/var/log/eneros/fencing.log")
    {
        if let Ok(json) = serde_json::to_string(record) {
            let _ = writeln!(file, "{}", json);
        }
    }
}

/// 非 Linux 平台：跳过持久化（测试环境）。
#[cfg(not(target_os = "linux"))]
fn persist_fence_record(_record: &FencingRecord) {}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fencing_strategy_default() {
        let strategy = FencingStrategy::default();
        assert_eq!(strategy, FencingStrategy::None);
    }

    #[test]
    fn test_detect_split_brain_empty_returns_no_split() {
        // v0.26.0：空死节点列表 → NoSplitBrain（不再返回错误）
        let manager = FencingManager::new(
            FencingStrategy::None,
            "node-1",
            NodeRole::Primary,
            SplitBrainConfig::default(),
        );
        let dead_nodes: Vec<String> = vec![];
        let quorum_responses = HashMap::new();
        let result = manager.detect_split_brain(&dead_nodes, &quorum_responses);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), SplitBrainResult::NoSplitBrain);
    }

    #[test]
    fn test_detect_split_brain_multi_node_supported() {
        // v0.26.0：多节点 dead_nodes 应被支持（不再返回 MultiNodeNotSupported）
        let manager = FencingManager::new(
            FencingStrategy::None,
            "node-1",
            NodeRole::Primary,
            SplitBrainConfig::default(),
        );
        let dead_nodes = vec!["n2".to_string(), "n3".to_string()];
        let quorum_responses = HashMap::new();
        let result = manager
            .detect_split_brain(&dead_nodes, &quorum_responses)
            .unwrap();
        // Primary 无仲裁节点 → 所有死节点应被 fencing
        assert_eq!(result, SplitBrainResult::FencePeer(dead_nodes));
    }

    #[test]
    fn test_detect_split_brain_primary() {
        // Primary 检测到对端故障，无仲裁节点 → 对端应被 fencing
        let manager = FencingManager::new(
            FencingStrategy::None,
            "node-1",
            NodeRole::Primary,
            SplitBrainConfig::default(),
        );
        let dead_nodes = vec!["node-2".to_string()];
        let quorum_responses = HashMap::new();
        let result = manager
            .detect_split_brain(&dead_nodes, &quorum_responses)
            .unwrap();
        assert_eq!(
            result,
            SplitBrainResult::FencePeer(vec!["node-2".to_string()])
        );
    }

    #[test]
    fn test_detect_split_brain_secondary() {
        // Secondary 检测到对端故障，无仲裁节点 → 保守策略，本节点应被 fencing
        let manager = FencingManager::new(
            FencingStrategy::None,
            "node-2",
            NodeRole::Secondary,
            SplitBrainConfig::default(),
        );
        let dead_nodes = vec!["node-1".to_string()];
        let quorum_responses = HashMap::new();
        let result = manager
            .detect_split_brain(&dead_nodes, &quorum_responses)
            .unwrap();
        assert_eq!(result, SplitBrainResult::ShouldBeFenced);
    }

    #[test]
    fn test_detect_split_brain_with_quorum() {
        // 3 个仲裁节点
        let config = SplitBrainConfig {
            quorum_nodes: vec![
                "q1".to_string(),
                "q2".to_string(),
                "q3".to_string(),
            ],
            ..Default::default()
        };
        let manager = FencingManager::new(
            FencingStrategy::None,
            "node-1",
            NodeRole::Primary,
            config,
        );
        let dead_nodes = vec!["node-2".to_string()];

        // 2/3 可达 → 超过半数 → 对端应被 fencing
        let mut responses = HashMap::new();
        responses.insert("q1".to_string(), true);
        responses.insert("q2".to_string(), true);
        responses.insert("q3".to_string(), false);
        let result = manager
            .detect_split_brain(&dead_nodes, &responses)
            .unwrap();
        assert_eq!(
            result,
            SplitBrainResult::FencePeer(vec!["node-2".to_string()])
        );

        // 1/3 可达 → 半数及以下 → 本节点应被 fencing
        let mut responses2 = HashMap::new();
        responses2.insert("q1".to_string(), true);
        responses2.insert("q2".to_string(), false);
        responses2.insert("q3".to_string(), false);
        let result2 = manager
            .detect_split_brain(&dead_nodes, &responses2)
            .unwrap();
        assert_eq!(result2, SplitBrainResult::ShouldBeFenced);

        // 本节点在死节点列表中 → 本节点应被 fencing（即使有 quorum）
        let dead_self = vec!["node-1".to_string()];
        let result3 = manager
            .detect_split_brain(&dead_self, &responses)
            .unwrap();
        assert_eq!(result3, SplitBrainResult::ShouldBeFenced);
    }

    #[test]
    fn test_fence_rejects_self_fencing() {
        // 禁止 fencing 自身
        let manager = FencingManager::new(
            FencingStrategy::Stonith,
            "node-1",
            NodeRole::Primary,
            SplitBrainConfig::default(),
        );
        let result = manager.fence("node-1", "self-fence attempt");
        assert!(matches!(result, Err(FencingError::InvalidTarget(_))));

        // 自 fencing 不应记录历史
        assert!(manager.history().is_empty());
    }

    #[test]
    fn test_fence_none_strategy() {
        let manager = FencingManager::new(
            FencingStrategy::None,
            "node-1",
            NodeRole::Primary,
            SplitBrainConfig::default(),
        );
        let result = manager.fence("node-2", "split-brain detected").unwrap();
        assert_eq!(result, FencingResult::Skipped);

        // 历史应记录此次操作
        let history = manager.history();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].target_node, "node-2");
        assert_eq!(history[0].strategy, FencingStrategy::None);
        assert_eq!(history[0].result, FencingResult::Skipped);
        assert_eq!(history[0].reason, "split-brain detected");
        assert_eq!(history[0].source_node, "node-1");
    }

    #[test]
    fn test_fence_scsi_stub() {
        let manager = FencingManager::new(
            FencingStrategy::Disk,
            "node-1",
            NodeRole::Primary,
            SplitBrainConfig::default(),
        );
        let result = manager.fence_scsi("node-2");
        assert_eq!(result, FencingResult::NotConfigured);
    }

    #[test]
    fn test_fence_ipmi_stub() {
        let manager = FencingManager::new(
            FencingStrategy::Stonith,
            "node-1",
            NodeRole::Primary,
            SplitBrainConfig::default(),
        );
        let result = manager.fence_ipmi("node-2");
        assert_eq!(result, FencingResult::NotConfigured);
    }

    #[test]
    fn test_fence_network_stub() {
        let manager = FencingManager::new(
            FencingStrategy::Network,
            "node-1",
            NodeRole::Primary,
            SplitBrainConfig::default(),
        );
        let result = manager.fence_network("node-2");
        assert_eq!(result, FencingResult::NotConfigured);
    }

    #[test]
    fn test_fence_cooldown_period() {
        // 30 秒冷却期：第一次 fence 成功，冷却期内第二次返回 Skipped
        let manager = FencingManager::new(
            FencingStrategy::Stonith,
            "node-1",
            NodeRole::Primary,
            SplitBrainConfig::default(),
        );

        // 第一次 fence（Stonith stub → NotConfigured）
        let result1 = manager.fence("node-2", "split-brain").unwrap();
        assert_eq!(result1, FencingResult::NotConfigured);

        // 30 秒内第二次 fence 同一节点 → Skipped（冷却期）
        let result2 = manager.fence("node-2", "retry").unwrap();
        assert_eq!(result2, FencingResult::Skipped);

        // 历史应记录两次
        let history = manager.history();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].result, FencingResult::NotConfigured);
        assert_eq!(history[1].result, FencingResult::Skipped);
        assert!(history[1].reason.contains("cooldown"));
        assert_eq!(history[1].source_node, "node-1");
    }

    #[test]
    fn test_fence_record_includes_source_node() {
        // FencingRecord 必须包含 source_node 字段
        let manager = FencingManager::new(
            FencingStrategy::Stonith,
            "node-1",
            NodeRole::Primary,
            SplitBrainConfig::default(),
        );
        manager.fence("node-2", "split-brain").unwrap();

        let history = manager.history();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].source_node, "node-1");
        assert_eq!(history[0].target_node, "node-2");
    }

    #[test]
    fn test_fence_history() {
        let manager = FencingManager::new(
            FencingStrategy::Stonith,
            "node-1",
            NodeRole::Primary,
            SplitBrainConfig::default(),
        );

        // 初始历史为空
        assert!(manager.history().is_empty());

        // 使用不同节点避免冷却期触发 Skipped
        manager.fence("node-2", "heartbeat lost").unwrap();
        manager.fence("node-3", "quorum lost").unwrap();
        manager.fence("node-4", "retry").unwrap();

        let history = manager.history();
        assert_eq!(history.len(), 3);

        // 验证第一条记录
        assert_eq!(history[0].target_node, "node-2");
        assert_eq!(history[0].strategy, FencingStrategy::Stonith);
        assert_eq!(history[0].result, FencingResult::NotConfigured);
        assert_eq!(history[0].reason, "heartbeat lost");
        assert_eq!(history[0].source_node, "node-1");

        // 验证第二条记录
        assert_eq!(history[1].target_node, "node-3");
        assert_eq!(history[1].reason, "quorum lost");

        // 验证第三条记录
        assert_eq!(history[2].target_node, "node-4");
        assert_eq!(history[2].reason, "retry");

        // 时间戳应非递减
        assert!(history[1].timestamp >= history[0].timestamp);
        assert!(history[2].timestamp >= history[1].timestamp);
    }

    #[test]
    fn test_fence_history_persistence() {
        // 验证 fence 操作可重复执行且历史累积；
        // Linux 下同时验证日志持久化路径不 panic（写入失败静默忽略）。
        let manager = FencingManager::new(
            FencingStrategy::Stonith,
            "node-1",
            NodeRole::Primary,
            SplitBrainConfig::default(),
        );

        // 使用不同节点避免冷却期
        manager.fence("node-2", "first").unwrap();
        manager.fence("node-3", "second").unwrap();

        let history = manager.history();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].target_node, "node-2");
        assert_eq!(history[1].target_node, "node-3");
        assert_eq!(history[0].source_node, "node-1");
        assert_eq!(history[1].source_node, "node-1");
    }

    #[test]
    fn test_fence_all_batch_execution() {
        // v0.26.0 — Task 5：批量 fencing 多个目标节点
        let manager = FencingManager::new(
            FencingStrategy::Stonith,
            "node-1",
            NodeRole::Primary,
            SplitBrainConfig::default(),
        );

        // fence 3 个不同节点（避免冷却期）
        let targets = vec![
            "node-2".to_string(),
            "node-3".to_string(),
            "node-4".to_string(),
        ];
        let results = manager.fence_all(&targets, "batch fencing");

        // 每个节点都应返回结果（Stonith stub → NotConfigured）
        assert_eq!(results.len(), 3);
        for r in &results {
            assert!(r.is_ok());
            assert_eq!(r.as_ref().unwrap(), &FencingResult::NotConfigured);
        }

        // 历史应记录 3 次操作
        let history = manager.history();
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].target_node, "node-2");
        assert_eq!(history[1].target_node, "node-3");
        assert_eq!(history[2].target_node, "node-4");
        // 所有记录的 reason 应为 "batch fencing"
        for record in &history {
            assert_eq!(record.reason, "batch fencing");
            assert_eq!(record.source_node, "node-1");
        }
    }

    #[test]
    fn test_fence_all_includes_self_fencing_error() {
        // fence_all 中包含自身节点时，自身节点返回 InvalidTarget 错误，
        // 其他节点正常执行
        let manager = FencingManager::new(
            FencingStrategy::Stonith,
            "node-1",
            NodeRole::Primary,
            SplitBrainConfig::default(),
        );

        let targets = vec![
            "node-2".to_string(),
            "node-1".to_string(), // 自身 → InvalidTarget
            "node-3".to_string(),
        ];
        let results = manager.fence_all(&targets, "mixed");

        assert_eq!(results.len(), 3);
        assert!(results[0].is_ok()); // node-2 成功
        assert!(matches!(results[1], Err(FencingError::InvalidTarget(_)))); // node-1 自 fencing 被拒
        assert!(results[2].is_ok()); // node-3 成功

        // 历史只记录 2 次（自身 fencing 不记录）
        let history = manager.history();
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn test_fence_all_empty() {
        // 空目标列表 → 空结果
        let manager = FencingManager::new(
            FencingStrategy::Stonith,
            "node-1",
            NodeRole::Primary,
            SplitBrainConfig::default(),
        );
        let results = manager.fence_all(&[], "empty");
        assert!(results.is_empty());
        assert!(manager.history().is_empty());
    }

    // ========================================================================
    // v0.29.0 — Task T029-21：Quorum 校验测试
    // ========================================================================

    #[test]
    fn test_quorum_state_has_quorum() {
        // 单节点（默认）：1 > 0 → 有 Quorum
        let state = QuorumState::new(1, 1);
        assert!(state.has_quorum());

        // 3 节点 2 存活：2 > 1 → 有 Quorum
        let state = QuorumState::new(3, 2);
        assert!(state.has_quorum());

        // 3 节点 1 存活：1 > 1 为假 → 无 Quorum
        let state = QuorumState::new(3, 1);
        assert!(!state.has_quorum());

        // 2 节点 1 存活：1 > 1 为假 → 无 Quorum（脑裂场景）
        let state = QuorumState::new(2, 1);
        assert!(!state.has_quorum());

        // 5 节点 3 存活：3 > 2 → 有 Quorum
        let state = QuorumState::new(5, 3);
        assert!(state.has_quorum());

        // 5 节点 2 存活：2 > 2 为假 → 无 Quorum
        let state = QuorumState::new(5, 2);
        assert!(!state.has_quorum());

        // 0 节点（空集群）：0 > 0 为假 → 无 Quorum
        let state = QuorumState::new(0, 0);
        assert!(!state.has_quorum());
    }

    #[test]
    fn test_quorum_state_default_single_node() {
        // 默认 QuorumState 应为单节点状态（持有 Quorum）
        let state = QuorumState::default();
        assert_eq!(state.total_members, 1);
        assert_eq!(state.alive_members, 1);
        assert!(state.has_quorum());
    }

    #[test]
    fn test_fence_default_quorum_allows_fencing() {
        // 默认状态（单节点持有 Quorum）下 fencing 应正常执行（向后兼容）
        let manager = FencingManager::new(
            FencingStrategy::Stonith,
            "node-1",
            NodeRole::Primary,
            SplitBrainConfig::default(),
        );
        // 默认 quorum_state 应为单节点（持有 Quorum）
        let state = manager.quorum_state();
        assert!(state.has_quorum());

        // fencing 应正常执行（Stonith stub → NotConfigured）
        let result = manager.fence("node-2", "split-brain").unwrap();
        assert_eq!(result, FencingResult::NotConfigured);

        // 历史应记录此次操作
        let history = manager.history();
        assert_eq!(history.len(), 1);
    }

    #[test]
    fn test_fence_rejected_without_quorum() {
        // 无 Quorum 时 fencing 应被拒绝
        let manager = FencingManager::new(
            FencingStrategy::Stonith,
            "node-1",
            NodeRole::Primary,
            SplitBrainConfig::default(),
        );

        // 模拟 3 节点集群仅 1 节点存活（无 Quorum，脑裂场景）
        manager.update_quorum_state(3, 1);
        assert!(!manager.quorum_state().has_quorum());

        // fencing 应被拒绝，返回 NoQuorum 错误
        let result = manager.fence("node-2", "split-brain");
        assert!(matches!(result, Err(FencingError::NoQuorum { alive: 1, total: 3 })));

        // 无 Quorum 拒绝不应记录历史
        assert!(manager.history().is_empty());
    }

    #[test]
    fn test_fence_with_quorum_succeeds() {
        // 有 Quorum 时 fencing 应正常执行
        let manager = FencingManager::new(
            FencingStrategy::Stonith,
            "node-1",
            NodeRole::Primary,
            SplitBrainConfig::default(),
        );

        // 模拟 3 节点集群 2 节点存活（有 Quorum）
        manager.update_quorum_state(3, 2);
        assert!(manager.quorum_state().has_quorum());

        // fencing 应正常执行（Stonith stub → NotConfigured）
        let result = manager.fence("node-2", "split-brain").unwrap();
        assert_eq!(result, FencingResult::NotConfigured);

        // 历史应记录此次操作
        let history = manager.history();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].target_node, "node-2");
        assert_eq!(history[0].result, FencingResult::NotConfigured);
    }

    #[test]
    fn test_fence_quorum_recovery() {
        // Quorum 丢失后恢复，fencing 应可执行
        let manager = FencingManager::new(
            FencingStrategy::Stonith,
            "node-1",
            NodeRole::Primary,
            SplitBrainConfig::default(),
        );

        // 阶段 1：有 Quorum（3 节点 3 存活）
        manager.update_quorum_state(3, 3);
        assert!(manager.quorum_state().has_quorum());

        // 阶段 2：Quorum 丢失（3 节点 1 存活，脑裂）
        manager.update_quorum_state(3, 1);
        assert!(!manager.quorum_state().has_quorum());
        let result = manager.fence("node-2", "split-brain");
        assert!(matches!(result, Err(FencingError::NoQuorum { .. })));
        assert!(manager.history().is_empty());

        // 阶段 3：Quorum 恢复（3 节点 2 存活）
        manager.update_quorum_state(3, 2);
        assert!(manager.quorum_state().has_quorum());
        let result = manager.fence("node-2", "quorum recovered").unwrap();
        assert_eq!(result, FencingResult::NotConfigured);

        // 历史应只记录恢复后的 1 次操作
        let history = manager.history();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].target_node, "node-2");
        assert_eq!(history[0].reason, "quorum recovered");
    }

    #[test]
    fn test_check_quorum_method() {
        // 直接测试 check_quorum() 方法
        let manager = FencingManager::new(
            FencingStrategy::Stonith,
            "node-1",
            NodeRole::Primary,
            SplitBrainConfig::default(),
        );

        // 默认状态（单节点）→ 有 Quorum
        assert!(manager.check_quorum().is_ok());

        // 3 节点 2 存活 → 有 Quorum
        manager.update_quorum_state(3, 2);
        assert!(manager.check_quorum().is_ok());

        // 3 节点 1 存活 → 无 Quorum
        manager.update_quorum_state(3, 1);
        let result = manager.check_quorum();
        assert!(matches!(result, Err(FencingError::NoQuorum { alive: 1, total: 3 })));

        // 2 节点 1 存活（脑裂）→ 无 Quorum
        manager.update_quorum_state(2, 1);
        let result = manager.check_quorum();
        assert!(matches!(result, Err(FencingError::NoQuorum { alive: 1, total: 2 })));
    }

    #[test]
    fn test_update_quorum_state() {
        // 测试 update_quorum_state() 方法
        let manager = FencingManager::new(
            FencingStrategy::Stonith,
            "node-1",
            NodeRole::Primary,
            SplitBrainConfig::default(),
        );

        // 初始默认状态
        let state = manager.quorum_state();
        assert_eq!(state.total_members, 1);
        assert_eq!(state.alive_members, 1);

        // 更新为 5 节点 3 存活
        manager.update_quorum_state(5, 3);
        let state = manager.quorum_state();
        assert_eq!(state.total_members, 5);
        assert_eq!(state.alive_members, 3);
        assert!(state.has_quorum());

        // 更新为 5 节点 2 存活（无 Quorum）
        manager.update_quorum_state(5, 2);
        let state = manager.quorum_state();
        assert_eq!(state.total_members, 5);
        assert_eq!(state.alive_members, 2);
        assert!(!state.has_quorum());
    }

    #[test]
    fn test_fence_self_fencing_checked_before_quorum() {
        // self-fencing 检查应在 Quorum 校验之前（更基础的校验优先）
        let manager = FencingManager::new(
            FencingStrategy::Stonith,
            "node-1",
            NodeRole::Primary,
            SplitBrainConfig::default(),
        );

        // 无 Quorum 状态
        manager.update_quorum_state(3, 1);
        assert!(!manager.quorum_state().has_quorum());

        // 试图 self-fencing 应返回 InvalidTarget（而非 NoQuorum），
        // 因为 self-fencing 检查在 Quorum 校验之前
        let result = manager.fence("node-1", "self-fence");
        assert!(matches!(result, Err(FencingError::InvalidTarget(_))));
        assert!(!matches!(result, Err(FencingError::NoQuorum { .. })));

        // 不应记录历史
        assert!(manager.history().is_empty());
    }

    #[test]
    fn test_fence_all_respects_quorum() {
        // fence_all 在无 Quorum 时所有节点都应返回 NoQuorum 错误
        let manager = FencingManager::new(
            FencingStrategy::Stonith,
            "node-1",
            NodeRole::Primary,
            SplitBrainConfig::default(),
        );

        // 无 Quorum（3 节点 1 存活）
        manager.update_quorum_state(3, 1);

        let targets = vec![
            "node-2".to_string(),
            "node-3".to_string(),
            "node-4".to_string(),
        ];
        let results = manager.fence_all(&targets, "batch");

        // 所有节点都应返回 NoQuorum 错误
        assert_eq!(results.len(), 3);
        for r in &results {
            assert!(matches!(r, Err(FencingError::NoQuorum { .. })));
        }

        // 无 Quorum 拒绝不应记录历史
        assert!(manager.history().is_empty());
    }

    #[test]
    fn test_fence_two_node_split_brain_no_quorum() {
        // 双节点脑裂场景：1/2 存活 → 无 Quorum → 拒绝 fencing
        // 这是 T029-21 的核心场景：防止脑裂时错误 fencing
        let manager = FencingManager::new(
            FencingStrategy::Stonith,
            "node-1",
            NodeRole::Primary,
            SplitBrainConfig::default(),
        );

        // 双节点集群，对端失联，仅本节点存活 → 无 Quorum
        manager.update_quorum_state(2, 1);
        assert!(!manager.quorum_state().has_quorum());

        // 即使 Primary 角色试图 fencing 对端，也应被拒绝
        let result = manager.fence("node-2", "peer heartbeat lost");
        assert!(matches!(result, Err(FencingError::NoQuorum { alive: 1, total: 2 })));
        assert!(manager.history().is_empty());
    }

    // ===== T030-07: 覆盖率补充测试 =====

    /// 验证 `strategy()` 访问器返回当前配置的 fencing 策略。
    #[test]
    fn test_strategy_accessor() {
        let manager = FencingManager::new(
            FencingStrategy::Stonith,
            "node-1",
            NodeRole::Primary,
            SplitBrainConfig::default(),
        );
        assert_eq!(manager.strategy(), FencingStrategy::Stonith);

        let manager2 = FencingManager::new(
            FencingStrategy::Disk,
            "node-2",
            NodeRole::Secondary,
            SplitBrainConfig::default(),
        );
        assert_eq!(manager2.strategy(), FencingStrategy::Disk);
    }

    /// 验证 `quorum_state()` 访问器返回当前 Quorum 状态快照。
    #[test]
    fn test_quorum_state_accessor() {
        let manager = FencingManager::new(
            FencingStrategy::None,
            "node-1",
            NodeRole::Primary,
            SplitBrainConfig::default(),
        );
        // 默认为单节点状态（total=1, alive=1）
        let state = manager.quorum_state();
        assert_eq!(state.total_members, 1);
        assert_eq!(state.alive_members, 1);
        assert!(state.has_quorum());

        // 更新为 3 节点集群，2 节点存活
        manager.update_quorum_state(3, 2);
        let state = manager.quorum_state();
        assert_eq!(state.total_members, 3);
        assert_eq!(state.alive_members, 2);
        assert!(state.has_quorum());
    }

    /// 验证 `QuorumState::new()` 构造函数。
    #[test]
    fn test_quorum_state_new_constructor() {
        let state = QuorumState::new(5, 3);
        assert_eq!(state.total_members, 5);
        assert_eq!(state.alive_members, 3);
        assert!(state.has_quorum()); // 3 > 5/2=2

        let state2 = QuorumState::new(4, 2);
        assert_eq!(state2.total_members, 4);
        assert_eq!(state2.alive_members, 2);
        assert!(!state2.has_quorum()); // 2 > 4/2=2 → false（需严格大于）
    }

    /// 验证 `SplitBrainConfig::default()` 的默认值。
    #[test]
    fn test_split_brain_config_default() {
        let config = SplitBrainConfig::default();
        assert_eq!(config.heartbeat_timeout_ms, 300);
        assert!(config.quorum_nodes.is_empty());
        assert_eq!(config.quorum_timeout_ms, 1000);
    }

    /// 验证 `FencingStrategy` 的所有变体 serde 序列化。
    #[test]
    fn test_fencing_strategy_serde_all_variants() {
        for strategy in [
            FencingStrategy::None,
            FencingStrategy::Stonith,
            FencingStrategy::Disk,
            FencingStrategy::Network,
        ] {
            let json = serde_json::to_string(&strategy).expect("serialize");
            let deserialized: FencingStrategy =
                serde_json::from_str(&json).expect("deserialize");
            assert_eq!(strategy, deserialized);
        }
    }
}

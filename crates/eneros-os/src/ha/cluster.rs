//! 多节点集群管理（v0.26.0 — Task 5）
//!
//! 支持 >2 节点集群和 Quorum 仲裁：
//! - 集群成员管理（Primary/Secondary/Witness 角色）
//! - 多数派 Quorum 仲裁（Majority 策略）
//! - Leader 选举（优先级最高的存活节点）
//! - Witness 节点投票（打破偶数节点僵局）
//!
//! ## Quorum 算法
//!
//! - `total_members`：非 Witness 成员数
//! - `alive_count`：存活（Alive）的非 Witness 成员数
//! - `witness_count`：Witness 节点数
//! - 有多数派：`alive_count + witness_count > (total_members + witness_count) / 2`
//! - Leader：优先级最高的存活非 Witness 成员
//!
//! Witness 节点视为始终存活（轻量仲裁节点，不承载数据），用于在偶数节点集群中打破僵局。

use crate::ha::NodeState;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};

/// 集群成员角色
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClusterMemberRole {
    /// 主节点
    Primary,
    /// 备节点
    Secondary,
    /// 仲裁节点（不承载数据，仅参与 Quorum 投票）
    Witness,
}

/// 集群成员
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterMember {
    /// 节点 ID
    pub node_id: String,
    /// 成员角色
    pub role: ClusterMemberRole,
    /// 优先级（数字越大优先级越高，Leader 选举时使用）
    pub priority: u32,
    /// 节点状态（复用 heartbeat::NodeState）
    #[serde(default)]
    pub state: NodeState,
    /// 最后一次心跳时间（Unix 毫秒），None 表示尚未收到心跳
    #[serde(default)]
    pub last_heartbeat: Option<i64>,
}

/// Quorum 策略
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum QuorumPolicy {
    /// 多数派策略（默认）：存活节点 > 总节点/2 时有 Quorum
    #[default]
    Majority,
    /// 手动策略：不自动计算 Quorum，由运维手动指定 Leader
    Manual,
}

/// 集群配置
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClusterConfig {
    /// 集群成员列表（含 Primary/Secondary/Witness 角色）
    #[serde(default)]
    pub members: Vec<ClusterMember>,
    /// Witness 节点 ID 列表（独立于 members，用于快速查找）
    #[serde(default)]
    pub witness: Vec<String>,
    /// Quorum 策略
    #[serde(default)]
    pub quorum_policy: QuorumPolicy,
}

/// Quorum 仲裁结果
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuorumResult {
    /// 选举出 Leader 节点
    Leader(String),
    /// 无多数派
    NoQuorum,
    /// 脑裂（无法判定）
    SplitBrain,
}

/// 成员状态（用于事件回调，覆盖成员生命周期中的关键状态）
///
/// 与 [`NodeState`] 的区别：`NodeState` 描述心跳层面的运行时状态（Alive/Suspect/Dead），
/// `MemberStatus` 额外包含成员加入（Joined）和主动离开（Left）两个拓扑事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberStatus {
    /// 新加入集群
    Joined,
    /// 存活（从 Suspect/Dead 恢复或初始 Alive）
    Alive,
    /// 怀疑下线（心跳超时）
    Suspect,
    /// 已下线（心跳彻底超时）
    Dead,
    /// 主动离开集群
    Left,
}

/// 成员变更事件
///
/// 当集群成员加入、离开或状态发生迁移时触发，携带事件发生后的集群快照信息。
/// 回调实现应快速处理（如推送到 channel），避免阻塞集群管理主流程。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberEvent {
    /// 发生变更的成员节点 ID
    pub member_id: String,
    /// 变更后的成员状态
    pub status: MemberStatus,
    /// 事件发生时间戳（UTC）
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// 事件发生后的集群成员总数（含 Witness）
    pub cluster_size: usize,
}

/// 成员变更回调函数类型
///
/// 回调以 `Arc<dyn Fn>` 形式注册，要求 `Send + Sync` 以便在多线程上下文中调用。
/// 回调执行应快速完成（如发送到 channel），重逻辑应由回调内部异步派发。
pub type MemberCallback = Arc<dyn Fn(MemberEvent) + Send + Sync>;

/// 集群管理器
///
/// 管理多节点集群的成员状态和 Quorum 仲裁。
/// 所有 RwLock 使用 `unwrap_or_else(|e| e.into_inner())` 安全处理（v0.25.1 规范）。
pub struct ClusterManager {
    /// 集群配置（运行时可变，例如成员状态更新）
    config: Arc<RwLock<ClusterConfig>>,
    /// 本节点 ID
    local_node_id: String,
    /// 成员变更回调列表（成员加入/离开/状态迁移时触发）
    callbacks: Arc<RwLock<Vec<MemberCallback>>>,
}

impl ClusterManager {
    /// 创建集群管理器
    pub fn new(config: ClusterConfig, local_node_id: impl Into<String>) -> Self {
        Self {
            config: Arc::new(RwLock::new(config)),
            local_node_id: local_node_id.into(),
            callbacks: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// 注册成员变更回调
    ///
    /// 回调在成员加入（Joined）、离开（Left）或状态迁移（Alive/Suspect/Dead）时触发。
    /// 回调执行应快速完成，重逻辑应异步派发，避免阻塞集群管理主流程。
    pub fn register_member_callback(&self, callback: MemberCallback) {
        let mut callbacks = self
            .callbacks
            .write()
            .unwrap_or_else(|e| e.into_inner());
        callbacks.push(callback);
    }

    /// 更新成员状态
    ///
    /// 在 members 列表中查找指定 node_id 并更新其 state。
    /// 如果节点不在 members 中，记录警告日志但不报错（可能是 witness 节点）。
    /// 当状态发生迁移时（如 Alive → Dead），触发对应的 [`MemberEvent`] 回调。
    pub fn update_member_state(&self, node_id: &str, state: NodeState) {
        let (old_state, cluster_size) = {
            let mut config = self
                .config
                .write()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(member) = config.members.iter_mut().find(|m| m.node_id == node_id) {
                let old = member.state;
                member.state = state;
                tracing::info!(node_id = node_id, ?state, "集群成员状态更新");
                (Some(old), config.members.len())
            } else {
                tracing::warn!(node_id = node_id, "更新成员状态失败：节点不在 members 列表中");
                (None, config.members.len())
            }
        };

        // 状态发生迁移时触发回调
        if let Some(old) = old_state {
            if old != state {
                let status = match state {
                    NodeState::Alive => MemberStatus::Alive,
                    NodeState::Suspect => MemberStatus::Suspect,
                    NodeState::Dead => MemberStatus::Dead,
                };
                self.notify_member_event(MemberEvent {
                    member_id: node_id.to_string(),
                    status,
                    timestamp: chrono::Utc::now(),
                    cluster_size,
                });
            }
        }
    }

    /// 添加新成员到集群
    ///
    /// 将成员加入 members 列表，并触发 `MemberStatus::Joined` 事件。
    /// 如果成员已存在（node_id 重复），记录警告日志且不触发事件。
    pub fn add_member(&self, member: ClusterMember) {
        let node_id = member.node_id.clone();
        let cluster_size = {
            let mut config = self
                .config
                .write()
                .unwrap_or_else(|e| e.into_inner());
            if config.members.iter().any(|m| m.node_id == node_id) {
                tracing::warn!(
                    node_id = %node_id,
                    "添加成员失败：节点已存在"
                );
                return;
            }
            tracing::info!(node_id = %node_id, ?member.role, "集群成员加入");
            config.members.push(member);
            config.members.len()
        };

        self.notify_member_event(MemberEvent {
            member_id: node_id,
            status: MemberStatus::Joined,
            timestamp: chrono::Utc::now(),
            cluster_size,
        });
    }

    /// 从集群移除成员
    ///
    /// 从 members 列表中移除指定 node_id 的成员，并触发 `MemberStatus::Left` 事件。
    /// 如果成员不存在，记录警告日志且不触发事件。
    pub fn remove_member(&self, node_id: &str) {
        let cluster_size = {
            let mut config = self
                .config
                .write()
                .unwrap_or_else(|e| e.into_inner());
            let before = config.members.len();
            config.members.retain(|m| m.node_id != node_id);
            if config.members.len() == before {
                tracing::warn!(node_id = node_id, "移除成员失败：节点不在 members 列表中");
                return;
            }
            tracing::info!(node_id = node_id, "集群成员离开");
            config.members.len()
        };

        self.notify_member_event(MemberEvent {
            member_id: node_id.to_string(),
            status: MemberStatus::Left,
            timestamp: chrono::Utc::now(),
            cluster_size,
        });
    }

    /// 更新成员角色
    pub fn update_member_role(&self, node_id: &str, role: ClusterMemberRole) {
        let mut config = self
            .config
            .write()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(member) = config.members.iter_mut().find(|m| m.node_id == node_id) {
            member.role = role;
            tracing::info!(node_id = node_id, ?role, "集群成员角色更新");
        } else {
            tracing::warn!(node_id = node_id, "更新成员角色失败：节点不在 members 列表中");
        }
    }

    /// 计算 Quorum 仲裁结果
    ///
    /// Majority 策略：存活投票节点（alive_count + witness_count）> 总投票节点（total_members + witness_count）/ 2 时，
    /// 返回 Leader（优先级最高的存活非 Witness 成员）；否则 NoQuorum。
    ///
    /// Manual 策略：始终返回 NoQuorum（由运维手动指定 Leader）。
    pub fn quorum(&self) -> QuorumResult {
        let config = self
            .config
            .read()
            .unwrap_or_else(|e| e.into_inner());

        match config.quorum_policy {
            QuorumPolicy::Manual => QuorumResult::NoQuorum,
            QuorumPolicy::Majority => {
                let total = Self::count_non_witness_members(&config.members);
                let alive = Self::count_alive_non_witness_members(&config.members);
                let witness = config.witness.len();

                // 总投票节点 = 非 Witness 成员 + Witness 节点
                let total_voters = total + witness;
                // 存活投票节点 = 存活非 Witness 成员 + Witness（视为始终存活）
                let alive_voters = alive + witness;

                if total_voters == 0 {
                    return QuorumResult::NoQuorum;
                }

                if alive_voters > total_voters / 2 {
                    // 有多数派，选举优先级最高的存活非 Witness 成员为 Leader
                    let leader = config
                        .members
                        .iter()
                        .filter(|m| {
                            m.role != ClusterMemberRole::Witness
                                && m.state == NodeState::Alive
                        })
                        .max_by_key(|m| m.priority)
                        .map(|m| m.node_id.clone());
                    match leader {
                        Some(id) => QuorumResult::Leader(id),
                        None => QuorumResult::NoQuorum,
                    }
                } else {
                    QuorumResult::NoQuorum
                }
            }
        }
    }

    /// 判断指定节点是否为 Quorum Leader
    pub fn is_leader(&self, node_id: &str) -> bool {
        matches!(self.quorum(), QuorumResult::Leader(ref id) if id == node_id)
    }

    /// Witness 节点数量
    pub fn witness_count(&self) -> usize {
        self.config
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .witness
            .len()
    }

    /// 存活成员数量（不含 Witness）
    pub fn alive_count(&self) -> usize {
        let config = self
            .config
            .read()
            .unwrap_or_else(|e| e.into_inner());
        Self::count_alive_non_witness_members(&config.members)
    }

    /// 总成员数量（不含 Witness）
    pub fn total_members(&self) -> usize {
        let config = self
            .config
            .read()
            .unwrap_or_else(|e| e.into_inner());
        Self::count_non_witness_members(&config.members)
    }

    /// 是否有 Quorum（alive_count > total_members / 2）
    ///
    /// 注意：此方法仅基于非 Witness 成员计算，不考虑 Witness 投票。
    /// 如需包含 Witness 投票的完整 Quorum 判定，使用 [`ClusterManager::quorum`]。
    pub fn has_quorum(&self) -> bool {
        let total = self.total_members();
        let alive = self.alive_count();
        alive > total / 2
    }

    /// 返回 Dead 状态的成员 ID 列表
    pub fn dead_members(&self) -> Vec<String> {
        self.config
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .members
            .iter()
            .filter(|m| m.state == NodeState::Dead)
            .map(|m| m.node_id.clone())
            .collect()
    }

    /// 返回所有成员快照
    pub fn members(&self) -> Vec<ClusterMember> {
        self.config
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .members
            .clone()
    }

    /// 返回本节点 ID
    pub fn local_node_id(&self) -> &str {
        &self.local_node_id
    }

    /// 通知所有已注册的回调：成员变更事件发生
    ///
    /// 回调执行策略：优先在 tokio 运行时中异步派发（`Handle::spawn`），避免阻塞集群管理主流程；
    /// 若不在 tokio 运行时上下文（如单元测试），则同步执行回调。
    /// 回调实现应快速返回，重逻辑应内部异步派发。
    fn notify_member_event(&self, event: MemberEvent) {
        let callbacks: Vec<MemberCallback> = {
            let guard = self
                .callbacks
                .read()
                .unwrap_or_else(|e| e.into_inner());
            guard.iter().cloned().collect()
        };

        for callback in callbacks {
            // 尝试在 tokio 运行时中异步派发，避免阻塞集群管理主流程
            match tokio::runtime::Handle::try_current() {
                Ok(handle) => {
                    let event = event.clone();
                    handle.spawn(async move {
                        callback(event);
                    });
                }
                Err(_) => {
                    // 不在 tokio 运行时中（如单元测试），同步执行
                    callback(event.clone());
                }
            }
        }
    }

    /// 统计非 Witness 成员数
    fn count_non_witness_members(members: &[ClusterMember]) -> usize {
        members
            .iter()
            .filter(|m| m.role != ClusterMemberRole::Witness)
            .count()
    }

    /// 统计存活的非 Witness 成员数
    fn count_alive_non_witness_members(members: &[ClusterMember]) -> usize {
        members
            .iter()
            .filter(|m| {
                m.role != ClusterMemberRole::Witness && m.state == NodeState::Alive
            })
            .count()
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造测试用成员
    fn member(node_id: &str, role: ClusterMemberRole, priority: u32) -> ClusterMember {
        ClusterMember {
            node_id: node_id.to_string(),
            role,
            priority,
            state: NodeState::Alive,
            last_heartbeat: None,
        }
    }

    /// 构造 3 节点集群配置（node-1 Primary, node-2/node-3 Secondary）
    fn three_node_config() -> ClusterConfig {
        ClusterConfig {
            members: vec![
                member("node-1", ClusterMemberRole::Primary, 100),
                member("node-2", ClusterMemberRole::Secondary, 50),
                member("node-3", ClusterMemberRole::Secondary, 30),
            ],
            witness: vec![],
            quorum_policy: QuorumPolicy::Majority,
        }
    }

    #[test]
    fn test_cluster_config_default() {
        let config = ClusterConfig::default();
        assert!(config.members.is_empty());
        assert!(config.witness.is_empty());
        assert_eq!(config.quorum_policy, QuorumPolicy::Majority);
    }

    #[test]
    fn test_quorum_policy_default() {
        assert_eq!(QuorumPolicy::default(), QuorumPolicy::Majority);
    }

    #[test]
    fn test_three_node_quorum_two_alive() {
        // 3 节点集群，2/3 存活 → 有 Quorum，Leader 为优先级最高的 node-1
        let config = three_node_config();
        let manager = ClusterManager::new(config, "node-1");

        // 初始全部 Alive
        let result = manager.quorum();
        assert_eq!(result, QuorumResult::Leader("node-1".to_string()));

        // node-3 Dead，2/3 存活 → 仍有 Quorum
        manager.update_member_state("node-3", NodeState::Dead);
        let result = manager.quorum();
        assert_eq!(result, QuorumResult::Leader("node-1".to_string()));
        assert!(manager.has_quorum());
    }

    #[test]
    fn test_two_node_quorum_one_alive_no_quorum() {
        // 2 节点集群，1/2 存活 → 无 Quorum
        let config = ClusterConfig {
            members: vec![
                member("node-1", ClusterMemberRole::Primary, 100),
                member("node-2", ClusterMemberRole::Secondary, 50),
            ],
            witness: vec![],
            quorum_policy: QuorumPolicy::Majority,
        };
        let manager = ClusterManager::new(config, "node-1");

        // node-2 Dead，1/2 存活 → 无 Quorum
        manager.update_member_state("node-2", NodeState::Dead);
        let result = manager.quorum();
        assert_eq!(result, QuorumResult::NoQuorum);
        assert!(!manager.has_quorum());
    }

    #[test]
    fn test_witness_voting_breaks_tie() {
        // 2 成员 + 1 witness：1 成员 Dead 时，witness 投票打破僵局
        let config = ClusterConfig {
            members: vec![
                member("node-1", ClusterMemberRole::Primary, 100),
                member("node-2", ClusterMemberRole::Secondary, 50),
            ],
            witness: vec!["witness-1".to_string()],
            quorum_policy: QuorumPolicy::Majority,
        };
        let manager = ClusterManager::new(config, "node-1");

        // witness_count = 1
        assert_eq!(manager.witness_count(), 1);

        // node-2 Dead：alive=1, witness=1, total=2, total_voters=3, alive_voters=2 > 1 → Quorum
        manager.update_member_state("node-2", NodeState::Dead);
        let result = manager.quorum();
        assert_eq!(result, QuorumResult::Leader("node-1".to_string()));

        // has_quorum 仅基于成员（1 > 1 为假），但 quorum() 包含 witness 投票
        assert!(!manager.has_quorum());
    }

    #[test]
    fn test_leader_election_highest_priority() {
        // 3 节点集群，node-1（priority=100）Dead → Leader 应为 node-2（priority=50）
        let config = three_node_config();
        let manager = ClusterManager::new(config, "node-2");

        manager.update_member_state("node-1", NodeState::Dead);
        let result = manager.quorum();
        assert_eq!(result, QuorumResult::Leader("node-2".to_string()));
        assert!(manager.is_leader("node-2"));
        assert!(!manager.is_leader("node-3"));
    }

    #[test]
    fn test_dead_members_query() {
        let config = three_node_config();
        let manager = ClusterManager::new(config, "node-1");

        // 初始无 Dead 成员
        assert!(manager.dead_members().is_empty());

        // node-2 和 node-3 Dead
        manager.update_member_state("node-2", NodeState::Dead);
        manager.update_member_state("node-3", NodeState::Dead);

        let dead = manager.dead_members();
        assert_eq!(dead.len(), 2);
        assert!(dead.contains(&"node-2".to_string()));
        assert!(dead.contains(&"node-3".to_string()));
    }

    #[test]
    fn test_has_quorum() {
        let config = three_node_config();
        let manager = ClusterManager::new(config, "node-1");

        // 3/3 存活 → has_quorum
        assert!(manager.has_quorum());

        // 2/3 存活 → has_quorum（2 > 1）
        manager.update_member_state("node-3", NodeState::Dead);
        assert!(manager.has_quorum());

        // 1/3 存活 → 无 Quorum（1 > 1 为假）
        manager.update_member_state("node-2", NodeState::Dead);
        assert!(!manager.has_quorum());
    }

    #[test]
    fn test_update_member_state_and_role() {
        let config = three_node_config();
        let manager = ClusterManager::new(config, "node-1");

        // 更新状态
        manager.update_member_state("node-2", NodeState::Suspect);
        let members = manager.members();
        let node2 = members.iter().find(|m| m.node_id == "node-2").unwrap();
        assert_eq!(node2.state, NodeState::Suspect);

        // 更新角色
        manager.update_member_role("node-2", ClusterMemberRole::Primary);
        let members = manager.members();
        let node2 = members.iter().find(|m| m.node_id == "node-2").unwrap();
        assert_eq!(node2.role, ClusterMemberRole::Primary);

        // 更新不存在的节点不应 panic
        manager.update_member_state("nonexistent", NodeState::Dead);
        manager.update_member_role("nonexistent", ClusterMemberRole::Witness);
    }

    #[test]
    fn test_quorum_result_serde() {
        // Leader
        let leader = QuorumResult::Leader("node-1".to_string());
        let json = serde_json::to_string(&leader).expect("serialize");
        let de: QuorumResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(de, leader);

        // NoQuorum
        let no_quorum = QuorumResult::NoQuorum;
        let json = serde_json::to_string(&no_quorum).expect("serialize");
        assert!(json.contains("no_quorum"));
        let de: QuorumResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(de, no_quorum);

        // SplitBrain
        let split = QuorumResult::SplitBrain;
        let json = serde_json::to_string(&split).expect("serialize");
        assert!(json.contains("split_brain"));
        let de: QuorumResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(de, split);
    }

    #[test]
    fn test_cluster_member_role_serde() {
        let json = serde_json::to_string(&ClusterMemberRole::Primary).expect("serialize");
        assert_eq!(json, "\"primary\"");
        let role: ClusterMemberRole = serde_json::from_str("\"secondary\"").expect("deserialize");
        assert_eq!(role, ClusterMemberRole::Secondary);
        let json = serde_json::to_string(&ClusterMemberRole::Witness).expect("serialize");
        assert_eq!(json, "\"witness\"");
    }

    #[test]
    fn test_quorum_manual_policy() {
        // Manual 策略始终返回 NoQuorum
        let config = ClusterConfig {
            members: vec![
                member("node-1", ClusterMemberRole::Primary, 100),
                member("node-2", ClusterMemberRole::Secondary, 50),
            ],
            witness: vec![],
            quorum_policy: QuorumPolicy::Manual,
        };
        let manager = ClusterManager::new(config, "node-1");

        let result = manager.quorum();
        assert_eq!(result, QuorumResult::NoQuorum);
    }

    #[test]
    fn test_local_node_id() {
        let manager = ClusterManager::new(three_node_config(), "node-1");
        assert_eq!(manager.local_node_id(), "node-1");
    }

    #[test]
    fn test_alive_count_and_total_members() {
        let config = three_node_config();
        let manager = ClusterManager::new(config, "node-1");

        // 3 个非 Witness 成员，全部 Alive
        assert_eq!(manager.total_members(), 3);
        assert_eq!(manager.alive_count(), 3);

        // node-3 Dead
        manager.update_member_state("node-3", NodeState::Dead);
        assert_eq!(manager.total_members(), 3);
        assert_eq!(manager.alive_count(), 2);
    }

    #[test]
    fn test_witness_member_excluded_from_counts() {
        // members 中包含 Witness 角色的成员，应被排除在 total_members/alive_count 之外
        let config = ClusterConfig {
            members: vec![
                member("node-1", ClusterMemberRole::Primary, 100),
                member("node-2", ClusterMemberRole::Secondary, 50),
                member("w-1", ClusterMemberRole::Witness, 0),
            ],
            witness: vec!["w-1".to_string()],
            quorum_policy: QuorumPolicy::Majority,
        };
        let manager = ClusterManager::new(config, "node-1");

        // total_members 不含 Witness
        assert_eq!(manager.total_members(), 2);
        assert_eq!(manager.alive_count(), 2);
        // witness_count 来自 witness 列表
        assert_eq!(manager.witness_count(), 1);
    }

    // ========================================================================
    // 成员变更回调测试（T029-22）
    // ========================================================================

    /// 测试辅助：创建事件收集器（Arc<Mutex<Vec<MemberEvent>>>）
    fn event_collector() -> Arc<std::sync::Mutex<Vec<MemberEvent>>> {
        Arc::new(std::sync::Mutex::new(Vec::new()))
    }

    /// 测试辅助：将收集器包装为 MemberCallback
    fn make_callback(
        collector: Arc<std::sync::Mutex<Vec<MemberEvent>>>,
    ) -> MemberCallback {
        Arc::new(move |event: MemberEvent| {
            collector
                .lock()
                .unwrap()
                .push(event);
        })
    }

    #[test]
    fn test_callback_on_member_join() {
        // 注册回调后，添加新成员应触发 Joined 事件
        let config = ClusterConfig {
            members: vec![member("node-1", ClusterMemberRole::Primary, 100)],
            witness: vec![],
            quorum_policy: QuorumPolicy::Majority,
        };
        let manager = ClusterManager::new(config, "node-1");

        let collector = event_collector();
        manager.register_member_callback(make_callback(collector.clone()));

        // 添加 node-2
        manager.add_member(member("node-2", ClusterMemberRole::Secondary, 50));

        let events = collector.lock().unwrap();
        assert_eq!(events.len(), 1, "应触发 1 个 Joined 事件");
        assert_eq!(events[0].member_id, "node-2");
        assert_eq!(events[0].status, MemberStatus::Joined);
        assert_eq!(events[0].cluster_size, 2, "加入后集群大小应为 2");
    }

    #[test]
    fn test_callback_on_member_leave() {
        // 注册回调后，移除成员应触发 Left 事件
        let config = ClusterConfig {
            members: vec![
                member("node-1", ClusterMemberRole::Primary, 100),
                member("node-2", ClusterMemberRole::Secondary, 50),
            ],
            witness: vec![],
            quorum_policy: QuorumPolicy::Majority,
        };
        let manager = ClusterManager::new(config, "node-1");

        let collector = event_collector();
        manager.register_member_callback(make_callback(collector.clone()));

        // 移除 node-2
        manager.remove_member("node-2");

        let events = collector.lock().unwrap();
        assert_eq!(events.len(), 1, "应触发 1 个 Left 事件");
        assert_eq!(events[0].member_id, "node-2");
        assert_eq!(events[0].status, MemberStatus::Left);
        assert_eq!(events[0].cluster_size, 1, "离开后集群大小应为 1");
    }

    #[test]
    fn test_callback_on_state_transition_dead() {
        // 状态从 Alive → Dead 应触发 Dead 事件
        let config = three_node_config();
        let manager = ClusterManager::new(config, "node-1");

        let collector = event_collector();
        manager.register_member_callback(make_callback(collector.clone()));

        manager.update_member_state("node-2", NodeState::Dead);

        let events = collector.lock().unwrap();
        assert_eq!(events.len(), 1, "应触发 1 个 Dead 事件");
        assert_eq!(events[0].member_id, "node-2");
        assert_eq!(events[0].status, MemberStatus::Dead);
        assert_eq!(events[0].cluster_size, 3);
    }

    #[test]
    fn test_callback_on_state_transition_suspect_and_recover() {
        // Alive → Suspect → Alive 应分别触发 Suspect 和 Alive 事件
        let config = three_node_config();
        let manager = ClusterManager::new(config, "node-1");

        let collector = event_collector();
        manager.register_member_callback(make_callback(collector.clone()));

        // Alive → Suspect
        manager.update_member_state("node-2", NodeState::Suspect);
        // Suspect → Alive（恢复）
        manager.update_member_state("node-2", NodeState::Alive);

        let events = collector.lock().unwrap();
        assert_eq!(events.len(), 2, "应触发 2 个事件（Suspect + Alive）");
        assert_eq!(events[0].status, MemberStatus::Suspect);
        assert_eq!(events[1].status, MemberStatus::Alive);
        assert_eq!(events[0].member_id, "node-2");
        assert_eq!(events[1].member_id, "node-2");
    }

    #[test]
    fn test_callback_no_event_on_same_state() {
        // 状态未变化时不应触发回调
        let config = three_node_config();
        let manager = ClusterManager::new(config, "node-1");

        let collector = event_collector();
        manager.register_member_callback(make_callback(collector.clone()));

        // node-1 初始为 Alive，再次设置为 Alive 不应触发事件
        manager.update_member_state("node-1", NodeState::Alive);

        let events = collector.lock().unwrap();
        assert!(events.is_empty(), "状态未变化时不应触发回调");
    }

    #[test]
    fn test_callback_no_event_on_nonexistent_member() {
        // 更新不存在的成员状态不应触发回调
        let config = three_node_config();
        let manager = ClusterManager::new(config, "node-1");

        let collector = event_collector();
        manager.register_member_callback(make_callback(collector.clone()));

        manager.update_member_state("nonexistent", NodeState::Dead);

        let events = collector.lock().unwrap();
        assert!(events.is_empty(), "不存在的成员不应触发回调");
    }

    #[test]
    fn test_multiple_callbacks_all_triggered() {
        // 注册多个回调，所有回调都应被触发
        let config = three_node_config();
        let manager = ClusterManager::new(config, "node-1");

        let collector1 = event_collector();
        let collector2 = event_collector();
        let collector3 = event_collector();
        manager.register_member_callback(make_callback(collector1.clone()));
        manager.register_member_callback(make_callback(collector2.clone()));
        manager.register_member_callback(make_callback(collector3.clone()));

        manager.update_member_state("node-2", NodeState::Dead);

        assert_eq!(collector1.lock().unwrap().len(), 1, "回调1应被触发");
        assert_eq!(collector2.lock().unwrap().len(), 1, "回调2应被触发");
        assert_eq!(collector3.lock().unwrap().len(), 1, "回调3应被触发");
    }

    #[test]
    fn test_callback_event_timestamp_and_cluster_size() {
        // 验证事件携带正确的时间戳和集群大小
        let config = ClusterConfig {
            members: vec![member("node-1", ClusterMemberRole::Primary, 100)],
            witness: vec![],
            quorum_policy: QuorumPolicy::Majority,
        };
        let manager = ClusterManager::new(config, "node-1");

        let collector = event_collector();
        manager.register_member_callback(make_callback(collector.clone()));

        let before = chrono::Utc::now();
        manager.add_member(member("node-2", ClusterMemberRole::Secondary, 50));
        let after = chrono::Utc::now();

        let events = collector.lock().unwrap();
        assert_eq!(events.len(), 1);
        // 时间戳应在事件触发前后范围内
        assert!(
            events[0].timestamp >= before && events[0].timestamp <= after,
            "时间戳应在合理范围内"
        );
        // 集群大小为加入后的成员数
        assert_eq!(events[0].cluster_size, 2);
    }

    #[test]
    fn test_add_duplicate_member_no_event() {
        // 添加已存在的成员不应触发事件
        let config = ClusterConfig {
            members: vec![member("node-1", ClusterMemberRole::Primary, 100)],
            witness: vec![],
            quorum_policy: QuorumPolicy::Majority,
        };
        let manager = ClusterManager::new(config, "node-1");

        let collector = event_collector();
        manager.register_member_callback(make_callback(collector.clone()));

        // 再次添加 node-1 应失败且不触发事件
        manager.add_member(member("node-1", ClusterMemberRole::Secondary, 50));

        let events = collector.lock().unwrap();
        assert!(events.is_empty(), "重复添加不应触发 Joined 事件");
    }

    #[test]
    fn test_remove_nonexistent_member_no_event() {
        // 移除不存在的成员不应触发事件
        let config = three_node_config();
        let manager = ClusterManager::new(config, "node-1");

        let collector = event_collector();
        manager.register_member_callback(make_callback(collector.clone()));

        manager.remove_member("nonexistent");

        let events = collector.lock().unwrap();
        assert!(events.is_empty(), "移除不存在的成员不应触发 Left 事件");
    }

    #[test]
    fn test_member_status_serde() {
        // 验证 MemberStatus 序列化/反序列化
        let statuses = vec![
            MemberStatus::Joined,
            MemberStatus::Alive,
            MemberStatus::Suspect,
            MemberStatus::Dead,
            MemberStatus::Left,
        ];
        for status in statuses {
            let json = serde_json::to_string(&status).expect("serialize");
            let de: MemberStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(de, status, "序列化往返应保持一致: {}", json);
        }
        // 验证 snake_case 格式
        assert_eq!(
            serde_json::to_string(&MemberStatus::Joined).unwrap(),
            "\"joined\""
        );
        assert_eq!(
            serde_json::to_string(&MemberStatus::Left).unwrap(),
            "\"left\""
        );
    }

    #[test]
    fn test_member_event_serde() {
        // 验证 MemberEvent 序列化/反序列化
        let event = MemberEvent {
            member_id: "node-1".to_string(),
            status: MemberStatus::Dead,
            timestamp: chrono::Utc::now(),
            cluster_size: 3,
        };
        let json = serde_json::to_string(&event).expect("serialize");
        let de: MemberEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(de.member_id, event.member_id);
        assert_eq!(de.status, event.status);
        assert_eq!(de.cluster_size, event.cluster_size);
    }

    #[test]
    fn test_full_member_lifecycle_callbacks() {
        // 完整生命周期：加入 → 怀疑 → 下线 → 恢复 → 离开
        let config = ClusterConfig {
            members: vec![member("node-1", ClusterMemberRole::Primary, 100)],
            witness: vec![],
            quorum_policy: QuorumPolicy::Majority,
        };
        let manager = ClusterManager::new(config, "node-1");

        let collector = event_collector();
        manager.register_member_callback(make_callback(collector.clone()));

        // node-2 加入
        manager.add_member(member("node-2", ClusterMemberRole::Secondary, 50));
        // node-2 怀疑
        manager.update_member_state("node-2", NodeState::Suspect);
        // node-2 下线
        manager.update_member_state("node-2", NodeState::Dead);
        // node-2 恢复
        manager.update_member_state("node-2", NodeState::Alive);
        // node-2 离开
        manager.remove_member("node-2");

        let events = collector.lock().unwrap();
        assert_eq!(events.len(), 5, "完整生命周期应触发 5 个事件");
        assert_eq!(events[0].status, MemberStatus::Joined);
        assert_eq!(events[1].status, MemberStatus::Suspect);
        assert_eq!(events[2].status, MemberStatus::Dead);
        assert_eq!(events[3].status, MemberStatus::Alive);
        assert_eq!(events[4].status, MemberStatus::Left);

        // 验证集群大小变化
        assert_eq!(events[0].cluster_size, 2, "加入后 2 个成员");
        assert_eq!(events[4].cluster_size, 1, "离开后 1 个成员");
    }

    // ===== T030-07: 覆盖率补充测试 =====

    /// 验证 `members()` 访问器返回所有成员的快照。
    #[test]
    fn test_members_accessor() {
        let config = ClusterConfig {
            members: vec![
                member("node-1", ClusterMemberRole::Primary, 100),
                member("node-2", ClusterMemberRole::Secondary, 50),
            ],
            witness: vec![],
            quorum_policy: QuorumPolicy::Majority,
        };
        let manager = ClusterManager::new(config, "node-1");

        let members = manager.members();
        assert_eq!(members.len(), 2);
        // 验证成员 ID（顺序可能与添加顺序不同，但应包含两个成员）
        let ids: Vec<&str> = members.iter().map(|m| m.node_id.as_str()).collect();
        assert!(ids.contains(&"node-1"));
        assert!(ids.contains(&"node-2"));
    }

    /// 验证 `members()` 在空集群时返回空列表。
    #[test]
    fn test_members_empty_cluster() {
        let config = ClusterConfig {
            members: vec![],
            witness: vec![],
            quorum_policy: QuorumPolicy::Majority,
        };
        let manager = ClusterManager::new(config, "node-1");
        let members = manager.members();
        assert!(members.is_empty());
    }

    /// 验证 `is_leader()` 对非 leader 节点返回 false。
    #[test]
    fn test_is_leader_false_for_non_leader() {
        let config = ClusterConfig {
            members: vec![
                member("node-1", ClusterMemberRole::Primary, 100),
                member("node-2", ClusterMemberRole::Secondary, 50),
            ],
            witness: vec![],
            quorum_policy: QuorumPolicy::Majority,
        };
        let manager = ClusterManager::new(config, "node-1");

        // node-1 是 Primary，应为 leader
        assert!(manager.is_leader("node-1"));
        // node-2 是 Secondary，不应为 leader
        assert!(!manager.is_leader("node-2"));
    }

    /// 验证 `witness_count()` 返回正确的 witness 节点数量。
    #[test]
    fn test_witness_count_with_witness_nodes() {
        let config = ClusterConfig {
            members: vec![
                member("node-1", ClusterMemberRole::Primary, 100),
                member("node-2", ClusterMemberRole::Secondary, 50),
            ],
            witness: vec!["witness-1".to_string(), "witness-2".to_string()],
            quorum_policy: QuorumPolicy::Majority,
        };
        let manager = ClusterManager::new(config, "node-1");
        assert_eq!(manager.witness_count(), 2);
    }
}

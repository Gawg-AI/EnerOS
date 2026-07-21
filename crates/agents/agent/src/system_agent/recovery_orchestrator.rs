//! 故障恢复编排器 — Agent 崩溃后的有序恢复调度
//!
//! 用于 v0.42.0：当一个或多个 Agent 崩溃时，按依赖关系和优先级有序调度恢复。
//! 依赖图保证上游 Agent 先恢复；优先级保证关键 Agent 优先恢复。
//!
//! # 偏差声明
//!
//! - **D1**: 使用 `BTreeMap` + `BTreeSet` + `VecDeque` 替代蓝图中的
//!   `HashMap`/`HashSet`（no_std 约束，避免引入 hashbrown）。
//! - **D3**: 实现蓝图中声明但未实现的 `schedule_recovery(agent)` + `pending_count()`
//!   接口（蓝图 §v0.42.0 接口声明存在但 key code 未实现）。
//! - **D4**: `process_next()` 按优先级排序后选可恢复的 Agent（蓝图未明确排序逻辑，
//!   本实现按 `RecoveryPriority` 排序：Critical > High > Normal > Low）。
//!
//! # no_std 合规
//!
//! 本模块仅使用 `alloc::*` / `core::*`，无 `std::*`，无 `panic!`/`todo!`/`unimplemented!`。

use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::vec::Vec;

use crate::id::AgentId;
use crate::system_agent::dependency::DependencyGraph;
use crate::types::AgentType;

/// 恢复优先级（4 级，Critical 最高，Low 最低）
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RecoveryPriority {
    /// 低优先级（如 CloudCoord / Custom）
    Low,
    /// 普通优先级（如 Market / Twin / EdgeCoord）
    Normal,
    /// 高优先级（如 Device / Grid / Energy）
    High,
    /// 关键优先级（如 System）
    Critical,
}

/// 根据 AgentType 推导默认恢复优先级（D4）
///
/// 优先级映射：
/// - `System` → Critical（系统 Agent 必须最先恢复）
/// - `Device` / `Grid` / `Energy` → High（基础设施 Agent）
/// - `Market` / `Twin` / `EdgeCoord` → Normal（业务/协调 Agent）
/// - `CloudCoord` / `Custom(_)` → Low（云端/扩展 Agent）
pub fn priority_of(agent_type: AgentType) -> RecoveryPriority {
    match agent_type {
        AgentType::System => RecoveryPriority::Critical,
        AgentType::Device | AgentType::Grid | AgentType::Energy => RecoveryPriority::High,
        AgentType::Market | AgentType::Twin | AgentType::EdgeCoord => RecoveryPriority::Normal,
        AgentType::CloudCoord | AgentType::Custom(_) => RecoveryPriority::Low,
    }
}

/// 故障恢复编排器
///
/// 管理多个崩溃 Agent 的有序恢复。核心数据结构：
/// - `dependency_graph`: 依赖关系图（来自 v0.42.0 DependencyGraph）
/// - `queue`: 待恢复 Agent 队列
/// - `in_progress`: 正在恢复中的 Agent 集合
/// - `recovered`: 已恢复的 Agent 集合
/// - `failed`: 恢复失败的 Agent 集合
/// - `agent_types`: Agent ID → AgentType 映射（用于优先级查询）
#[derive(Debug, Clone, Default)]
pub struct RecoveryOrchestrator {
    /// 依赖关系图
    dependency_graph: DependencyGraph,
    /// 待恢复队列
    queue: VecDeque<AgentId>,
    /// 正在恢复中的 Agent 集合
    in_progress: BTreeSet<AgentId>,
    /// 已恢复的 Agent 集合
    recovered: BTreeSet<AgentId>,
    /// 恢复失败的 Agent 集合
    failed: BTreeSet<AgentId>,
    /// Agent ID → AgentType 映射（用于优先级查询）
    agent_types: BTreeMap<AgentId, AgentType>,
}

impl RecoveryOrchestrator {
    /// 创建空编排器
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加依赖关系：`agent` 依赖 `depends_on`（即 depends_on 必须先恢复）
    ///
    /// 同时记录 agent_type 用于优先级排序。
    pub fn add_dependency(&mut self, agent: AgentId, depends_on: AgentId, agent_type: AgentType) {
        self.dependency_graph.add_dependency(agent, depends_on);
        self.agent_types.insert(agent, agent_type);
        // 确保 depends_on 也有类型记录（若未显式注册，默认 Custom(0)）
        self.agent_types
            .entry(depends_on)
            .or_insert(AgentType::Custom(0));
    }

    /// 注册 Agent 类型（无依赖关系，仅用于优先级查询）
    pub fn register_agent(&mut self, agent: AgentId, agent_type: AgentType) {
        self.agent_types.insert(agent, agent_type);
    }

    /// 调度单个 Agent 恢复（D3）
    ///
    /// 将 agent 加入待恢复队列。实际恢复需调用 `process_next()` 取出并处理。
    pub fn schedule_recovery(&mut self, agent: AgentId, agent_type: AgentType) {
        self.agent_types.insert(agent, agent_type);
        if !self.queue.contains(&agent)
            && !self.in_progress.contains(&agent)
            && !self.recovered.contains(&agent)
            && !self.failed.contains(&agent)
        {
            self.queue.push_back(agent);
        }
    }

    /// 批量调度多个 Agent 恢复
    ///
    /// `agents` 与 `agent_types` 长度必须相同。
    pub fn schedule_batch(&mut self, agents: &[AgentId], agent_types: &[AgentType]) {
        for (agent, agent_type) in agents.iter().zip(agent_types.iter()) {
            self.schedule_recovery(*agent, *agent_type);
        }
    }

    /// 取出下一个可恢复的 Agent（D4：按优先级排序）
    ///
    /// 从队列中选择优先级最高且依赖已满足（can_recover）的 Agent，标记为 in_progress 并返回。
    /// 若队列中所有 Agent 的依赖均未满足，返回 None（等待上游恢复）。
    /// 若队列为空，返回 None。
    pub fn process_next(&mut self) -> Option<AgentId> {
        if self.queue.is_empty() {
            return None;
        }

        // 收集队列中所有可恢复的 Agent 及其优先级
        let mut candidates: Vec<(AgentId, RecoveryPriority)> = Vec::new();
        let mut remaining: VecDeque<AgentId> = VecDeque::new();

        while let Some(agent) = self.queue.pop_front() {
            if self.dependency_graph.can_recover(agent) {
                let priority = self
                    .agent_types
                    .get(&agent)
                    .map(|&t| priority_of(t))
                    .unwrap_or(RecoveryPriority::Low);
                candidates.push((agent, priority));
            } else {
                remaining.push_back(agent);
            }
        }

        // 将不可恢复的放回队列
        self.queue = remaining;

        if candidates.is_empty() {
            return None;
        }

        // 按优先级降序排序（Critical 在前）— RecoveryPriority 的 Ord 是 Low < Normal < High < Critical
        candidates.sort_by_key(|b| core::cmp::Reverse(b.1));

        // 取出最高优先级的 Agent
        let (agent, _) = candidates.remove(0);

        // 将剩余的可恢复 Agent 放回队列（保持无序，下次 process_next 再排序）
        for (a, _) in candidates {
            self.queue.push_back(a);
        }

        // 标记为 in_progress
        self.in_progress.insert(agent);
        Some(agent)
    }

    /// 通知 Agent 恢复成功
    ///
    /// 将 Agent 从 in_progress 移至 recovered，并在依赖图中标记。
    pub fn on_agent_recovered(&mut self, agent: AgentId) {
        self.in_progress.remove(&agent);
        self.recovered.insert(agent);
        self.dependency_graph.mark_recovered(agent);
    }

    /// 通知 Agent 恢复失败
    ///
    /// 将 Agent 从 in_progress 移至 failed，并在依赖图中标记。
    /// 失败的 Agent 不阻塞下游（D7：下游仍可降级恢复）。
    pub fn on_agent_failed(&mut self, agent: AgentId) {
        self.in_progress.remove(&agent);
        self.failed.insert(agent);
        self.dependency_graph.mark_failed(agent);
    }

    /// 待恢复 Agent 数量（队列 + in_progress）（D3）
    pub fn pending_count(&self) -> usize {
        self.queue.len() + self.in_progress.len()
    }

    /// 所有 Agent 是否都已处理完成（队列为空且无 in_progress）
    pub fn is_complete(&self) -> bool {
        self.queue.is_empty() && self.in_progress.is_empty()
    }

    /// 获取已恢复 Agent 集合
    pub fn recovered(&self) -> &BTreeSet<AgentId> {
        &self.recovered
    }

    /// 获取恢复失败 Agent 集合
    pub fn failed(&self) -> &BTreeSet<AgentId> {
        &self.failed
    }

    /// 获取正在恢复中 Agent 集合
    pub fn in_progress(&self) -> &BTreeSet<AgentId> {
        &self.in_progress
    }

    /// 获取依赖图引用
    pub fn dependency_graph(&self) -> &DependencyGraph {
        &self.dependency_graph
    }

    /// 获取队列长度
    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_orchestrator() {
        let mut orch = RecoveryOrchestrator::new();
        assert_eq!(orch.pending_count(), 0);
        assert!(orch.is_complete());
        assert!(orch.process_next().is_none());
    }

    #[test]
    fn test_priority_of_mapping() {
        assert_eq!(priority_of(AgentType::System), RecoveryPriority::Critical);
        assert_eq!(priority_of(AgentType::Device), RecoveryPriority::High);
        assert_eq!(priority_of(AgentType::Grid), RecoveryPriority::High);
        assert_eq!(priority_of(AgentType::Energy), RecoveryPriority::High);
        assert_eq!(priority_of(AgentType::Market), RecoveryPriority::Normal);
        assert_eq!(priority_of(AgentType::Twin), RecoveryPriority::Normal);
        assert_eq!(priority_of(AgentType::EdgeCoord), RecoveryPriority::Normal);
        assert_eq!(priority_of(AgentType::CloudCoord), RecoveryPriority::Low);
        assert_eq!(priority_of(AgentType::Custom(42)), RecoveryPriority::Low);
    }

    #[test]
    fn test_priority_ordering() {
        assert!(RecoveryPriority::Critical > RecoveryPriority::High);
        assert!(RecoveryPriority::High > RecoveryPriority::Normal);
        assert!(RecoveryPriority::Normal > RecoveryPriority::Low);
    }

    #[test]
    fn test_single_agent_recovery() {
        let mut orch = RecoveryOrchestrator::new();
        orch.schedule_recovery(AgentId(1), AgentType::System);

        assert_eq!(orch.pending_count(), 1);
        let next = orch.process_next();
        assert_eq!(next, Some(AgentId(1)));
        assert!(orch.in_progress().contains(&AgentId(1)));

        orch.on_agent_recovered(AgentId(1));
        assert!(orch.recovered().contains(&AgentId(1)));
        assert!(orch.is_complete());
    }

    #[test]
    fn test_ordered_recovery_with_dependency() {
        let mut orch = RecoveryOrchestrator::new();
        // agent 2 depends on agent 1
        orch.add_dependency(AgentId(2), AgentId(1), AgentType::Device);
        orch.schedule_recovery(AgentId(1), AgentType::System);
        orch.schedule_recovery(AgentId(2), AgentType::Device);

        // First process: only agent 1 can be recovered (agent 2 has unfinished dep)
        let first = orch.process_next();
        assert_eq!(first, Some(AgentId(1)));

        // agent 2 still blocked
        let second = orch.process_next();
        assert_eq!(second, None);

        // agent 1 recovered -> agent 2 can now be recovered
        orch.on_agent_recovered(AgentId(1));
        let third = orch.process_next();
        assert_eq!(third, Some(AgentId(2)));

        orch.on_agent_recovered(AgentId(2));
        assert!(orch.is_complete());
    }

    #[test]
    fn test_dependency_blocked() {
        let mut orch = RecoveryOrchestrator::new();
        orch.add_dependency(AgentId(2), AgentId(1), AgentType::Device);
        orch.schedule_recovery(AgentId(2), AgentType::Device);
        // agent 1 not scheduled -> agent 2 blocked
        let next = orch.process_next();
        assert_eq!(next, None);
        assert_eq!(orch.pending_count(), 1); // agent 2 still in queue
    }

    #[test]
    fn test_failed_dependency_not_blocked() {
        // D7: failed dependency does NOT block downstream
        let mut orch = RecoveryOrchestrator::new();
        orch.add_dependency(AgentId(2), AgentId(1), AgentType::Device);
        orch.schedule_recovery(AgentId(1), AgentType::System);
        orch.schedule_recovery(AgentId(2), AgentType::Device);

        // agent 1 fails
        let first = orch.process_next();
        assert_eq!(first, Some(AgentId(1)));
        orch.on_agent_failed(AgentId(1));

        // agent 2 can still be recovered (failed dep not blocking)
        let second = orch.process_next();
        assert_eq!(second, Some(AgentId(2)));
    }

    #[test]
    fn test_priority_ordering_in_process_next() {
        // Schedule multiple agents with no deps, ensure Critical comes first
        let mut orch = RecoveryOrchestrator::new();
        orch.schedule_recovery(AgentId(1), AgentType::Custom(0)); // Low
        orch.schedule_recovery(AgentId(2), AgentType::System); // Critical
        orch.schedule_recovery(AgentId(3), AgentType::Device); // High
        orch.schedule_recovery(AgentId(4), AgentType::Market); // Normal

        let order: Vec<AgentId> = [
            orch.process_next(),
            orch.process_next(),
            orch.process_next(),
            orch.process_next(),
        ]
        .into_iter()
        .map(|x| x.unwrap())
        .collect();

        // Critical (agent 2) first
        assert_eq!(order[0], AgentId(2));
        // High (agent 3) second
        assert_eq!(order[1], AgentId(3));
        // Normal (agent 4) third
        assert_eq!(order[2], AgentId(4));
        // Low (agent 1) last
        assert_eq!(order[3], AgentId(1));
    }

    #[test]
    fn test_batch_schedule() {
        let mut orch = RecoveryOrchestrator::new();
        let agents = [AgentId(1), AgentId(2), AgentId(3)];
        let types = [AgentType::System, AgentType::Device, AgentType::Market];
        orch.schedule_batch(&agents, &types);

        assert_eq!(orch.pending_count(), 3);
        // Process all
        let mut count = 0;
        while orch.process_next().is_some() {
            count += 1;
        }
        assert_eq!(count, 3);
    }

    #[test]
    fn test_is_complete_after_all_recovered() {
        let mut orch = RecoveryOrchestrator::new();
        orch.schedule_recovery(AgentId(1), AgentType::System);
        orch.schedule_recovery(AgentId(2), AgentType::Device);

        // Not complete while pending
        assert!(!orch.is_complete());

        let a = orch.process_next().unwrap();
        let b = orch.process_next().unwrap();

        // Still not complete while in_progress
        assert!(!orch.is_complete());

        orch.on_agent_recovered(a);
        assert!(!orch.is_complete());

        orch.on_agent_recovered(b);
        assert!(orch.is_complete());
    }

    #[test]
    fn test_pending_count() {
        let mut orch = RecoveryOrchestrator::new();
        orch.schedule_recovery(AgentId(1), AgentType::System);
        orch.schedule_recovery(AgentId(2), AgentType::Device);
        assert_eq!(orch.pending_count(), 2);

        let _ = orch.process_next();
        // 1 in_progress + 1 in queue
        assert_eq!(orch.pending_count(), 2);

        orch.on_agent_recovered(AgentId(1));
        // 0 in_progress + 1 in queue
        assert_eq!(orch.pending_count(), 1);
    }

    #[test]
    fn test_on_agent_failed() {
        let mut orch = RecoveryOrchestrator::new();
        orch.schedule_recovery(AgentId(1), AgentType::System);
        let agent = orch.process_next().unwrap();
        orch.on_agent_failed(agent);

        assert!(orch.failed().contains(&agent));
        assert!(!orch.in_progress().contains(&agent));
        assert!(!orch.recovered().contains(&agent));
        assert!(orch.is_complete()); // no pending, no in_progress
    }

    #[test]
    fn test_schedule_recovery_idempotent() {
        let mut orch = RecoveryOrchestrator::new();
        orch.schedule_recovery(AgentId(1), AgentType::System);
        orch.schedule_recovery(AgentId(1), AgentType::System); // duplicate
        assert_eq!(orch.pending_count(), 1);
    }
}

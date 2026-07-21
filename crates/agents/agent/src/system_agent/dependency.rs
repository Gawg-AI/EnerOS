//! 依赖图 — Agent 间恢复依赖关系建模
//!
//! 用于故障恢复编排（v0.42.0）：当一个 Agent 崩溃时，可能需要等待其依赖的 Agent 先恢复
//! 才能被恢复。本模块提供有向无环图（DAG）建模与拓扑排序能力。
//!
//! # 偏差声明
//!
//! - **D1**: 使用 `BTreeMap<AgentId, Vec<AgentId>>` + `BTreeSet<AgentId>` 替代蓝图中的
//!   `HashMap`/`HashSet`（no_std 约束，避免引入 hashbrown 外部依赖）。
//! - **D6**: `topological_sort` 采用 Kahn 算法（入度统计 + 队列），检测到环时返回
//!   `Err(AgentError::CircularDependency)`。
//! - **D7**: `can_recover` 判定逻辑 — 依赖已 `recovered` 或 `failed` 均视为可恢复
//!   （失败依赖不阻塞下游 — 蓝图 §故障恢复的降级策略：上游 Agent 恢复失败时，下游
//!   Agent 仍可降级运行）。
//!
//! # no_std 合规
//!
//! 本模块仅使用 `alloc::*` / `core::*`，无 `std::*`，无 `panic!`/`todo!`/`unimplemented!`。

use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::vec::Vec;

use crate::error::AgentError;
use crate::id::AgentId;

/// Agent 恢复依赖图（有向无环图）
///
/// 边的语义：`add_dependency(agent, depends_on)` 表示 `agent` 依赖 `depends_on`，
/// 即 `depends_on` 必须先恢复，`agent` 才能开始恢复。
///
/// # 数据结构（D1）
///
/// - `dependencies`: key=agent, value=该 agent 直接依赖的所有上游 agent 列表
/// - `recovered`: 已恢复的 agent 集合
/// - `failed`: 恢复失败的 agent 集合
#[derive(Debug, Clone, Default)]
pub struct DependencyGraph {
    /// agent → 其直接依赖的上游 agent 列表
    dependencies: BTreeMap<AgentId, Vec<AgentId>>,
    /// 已恢复的 agent 集合
    recovered: BTreeSet<AgentId>,
    /// 恢复失败的 agent 集合
    failed: BTreeSet<AgentId>,
}

impl DependencyGraph {
    /// 创建空依赖图
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加依赖关系：`agent` 依赖 `depends_on`
    ///
    /// 重复添加同一依赖关系会被去重（线性扫描，因 no_std 无 HashSet）。
    pub fn add_dependency(&mut self, agent: AgentId, depends_on: AgentId) {
        let deps = self.dependencies.entry(agent).or_default();
        if !deps.contains(&depends_on) {
            deps.push(depends_on);
        }
        // 确保 depends_on 也在 dependencies map 中有 entry（即使无依赖）
        self.dependencies.entry(depends_on).or_default();
    }

    /// 拓扑排序（D6：Kahn 算法）
    ///
    /// 返回按依赖顺序排序的 Agent 列表（被依赖的 Agent 在前）。
    /// 检测到环时返回 `Err(AgentError::CircularDependency)`。
    pub fn topological_sort(&self) -> Result<Vec<AgentId>, AgentError> {
        // 收集所有节点（包括仅作为 depends_on 出现的）
        let mut all_nodes: BTreeSet<AgentId> = BTreeSet::new();
        for (agent, deps) in &self.dependencies {
            all_nodes.insert(*agent);
            for d in deps {
                all_nodes.insert(*d);
            }
        }

        // 计算入度（每个节点的依赖数 = 入度）
        let mut in_degree: BTreeMap<AgentId, u32> = BTreeMap::new();
        for node in &all_nodes {
            in_degree.insert(*node, 0);
        }
        for (agent, deps) in &self.dependencies {
            // agent 的入度 = 它的依赖数
            in_degree.insert(*agent, deps.len() as u32);
        }

        // 入度为 0 的节点入队
        let mut queue: VecDeque<AgentId> = VecDeque::new();
        for (node, &deg) in &in_degree {
            if deg == 0 {
                queue.push_back(*node);
            }
        }

        // Kahn 算法主循环
        let mut result: Vec<AgentId> = Vec::new();
        while let Some(node) = queue.pop_front() {
            result.push(node);
            // 找到所有以 node 为依赖的 agent，减少它们的入度
            for (agent, deps) in &self.dependencies {
                if deps.contains(&node) {
                    if let Some(deg) = in_degree.get_mut(agent) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push_back(*agent);
                        }
                    }
                }
            }
        }

        // 环检测：若结果不包含所有节点，说明存在环
        if result.len() != all_nodes.len() {
            Err(AgentError::CircularDependency)
        } else {
            Ok(result)
        }
    }

    /// 检测是否存在环
    pub fn has_cycle(&self) -> bool {
        self.topological_sort().is_err()
    }

    /// 判断 agent 是否可恢复（D7）
    ///
    /// 可恢复条件：agent 的所有依赖要么已 `recovered`，要么已 `failed`
    /// （失败的依赖不阻塞 — 蓝图降级策略）。
    /// 若 agent 无依赖，则总是可恢复。
    /// 若 agent 已在 `recovered` 或 `failed` 集合中，返回 false（已完成无需重复）。
    pub fn can_recover(&self, agent: AgentId) -> bool {
        // 已恢复或已失败的 agent 不再可恢复
        if self.recovered.contains(&agent) || self.failed.contains(&agent) {
            return false;
        }
        // 检查所有依赖
        match self.dependencies.get(&agent) {
            None => true, // 无依赖条目，可恢复
            Some(deps) => deps
                .iter()
                .all(|d| self.recovered.contains(d) || self.failed.contains(d)),
        }
    }

    /// 标记 agent 已恢复
    pub fn mark_recovered(&mut self, agent: AgentId) {
        self.recovered.insert(agent);
    }

    /// 标记 agent 恢复失败
    pub fn mark_failed(&mut self, agent: AgentId) {
        self.failed.insert(agent);
    }

    /// 获取已恢复的 agent 集合
    pub fn recovered(&self) -> &BTreeSet<AgentId> {
        &self.recovered
    }

    /// 获取恢复失败的 agent 集合
    pub fn failed(&self) -> &BTreeSet<AgentId> {
        &self.failed
    }

    /// 获取 agent 的直接依赖列表（返回克隆以避免借用问题）
    pub fn dependencies_of(&self, agent: AgentId) -> Vec<AgentId> {
        self.dependencies.get(&agent).cloned().unwrap_or_default()
    }

    /// 获取图中所有节点
    pub fn all_nodes(&self) -> Vec<AgentId> {
        let mut nodes: BTreeSet<AgentId> = BTreeSet::new();
        for (agent, deps) in &self.dependencies {
            nodes.insert(*agent);
            for d in deps {
                nodes.insert(*d);
            }
        }
        nodes.into_iter().collect()
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_macros)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_graph() {
        let g = DependencyGraph::new();
        assert!(g.topological_sort().unwrap().is_empty());
        assert!(!g.has_cycle());
        assert!(g.recovered().is_empty());
        assert!(g.failed().is_empty());
    }

    #[test]
    fn test_add_dependency_basic() {
        let mut g = DependencyGraph::new();
        g.add_dependency(AgentId(2), AgentId(1));
        // agent 2 depends on agent 1
        let deps = g.dependencies_of(AgentId(2));
        assert_eq!(deps, vec![AgentId(1)]);
        // agent 1 has no dependencies
        let deps1 = g.dependencies_of(AgentId(1));
        assert!(deps1.is_empty());
    }

    #[test]
    fn test_add_dependency_dedup() {
        let mut g = DependencyGraph::new();
        g.add_dependency(AgentId(2), AgentId(1));
        g.add_dependency(AgentId(2), AgentId(1)); // duplicate
        let deps = g.dependencies_of(AgentId(2));
        assert_eq!(deps.len(), 1);
    }

    #[test]
    fn test_topological_sort_simple() {
        let mut g = DependencyGraph::new();
        g.add_dependency(AgentId(2), AgentId(1)); // 2 depends on 1
        g.add_dependency(AgentId(3), AgentId(2)); // 3 depends on 2
        let sorted = g.topological_sort().unwrap();
        // 1 must come before 2, 2 must come before 3
        let pos1 = sorted.iter().position(|&x| x == AgentId(1)).unwrap();
        let pos2 = sorted.iter().position(|&x| x == AgentId(2)).unwrap();
        let pos3 = sorted.iter().position(|&x| x == AgentId(3)).unwrap();
        assert!(pos1 < pos2);
        assert!(pos2 < pos3);
    }

    #[test]
    fn test_topological_sort_multi_level() {
        let mut g = DependencyGraph::new();
        // Diamond: 4 -> 2 -> 1, 4 -> 3 -> 1
        g.add_dependency(AgentId(2), AgentId(1));
        g.add_dependency(AgentId(3), AgentId(1));
        g.add_dependency(AgentId(4), AgentId(2));
        g.add_dependency(AgentId(4), AgentId(3));
        let sorted = g.topological_sort().unwrap();
        assert_eq!(sorted.len(), 4);
        // 1 must be first (only one with no deps)
        assert_eq!(sorted[0], AgentId(1));
        // 4 must be last (depends on 2 and 3)
        assert_eq!(sorted[3], AgentId(4));
    }

    #[test]
    fn test_cycle_detection() {
        let mut g = DependencyGraph::new();
        g.add_dependency(AgentId(1), AgentId(2)); // 1 depends on 2
        g.add_dependency(AgentId(2), AgentId(1)); // 2 depends on 1 -> cycle
        assert!(g.has_cycle());
        match g.topological_sort() {
            Err(AgentError::CircularDependency) => (),
            other => panic!("expected CircularDependency, got {:?}", other),
        }
    }

    #[test]
    fn test_can_recover_no_deps() {
        let g = DependencyGraph::new();
        // Agent not in graph, no deps -> can recover
        assert!(g.can_recover(AgentId(1)));
    }

    #[test]
    fn test_can_recover_with_deps_recovered() {
        let mut g = DependencyGraph::new();
        g.add_dependency(AgentId(2), AgentId(1));
        g.mark_recovered(AgentId(1));
        assert!(g.can_recover(AgentId(2)));
    }

    #[test]
    fn test_can_recover_blocked_by_unfinished_dep() {
        let mut g = DependencyGraph::new();
        g.add_dependency(AgentId(2), AgentId(1));
        // agent 1 not recovered/failed -> agent 2 cannot recover
        assert!(!g.can_recover(AgentId(2)));
    }

    #[test]
    fn test_can_recover_failed_dep_not_blocked() {
        // D7: failed dependencies do NOT block downstream recovery
        let mut g = DependencyGraph::new();
        g.add_dependency(AgentId(2), AgentId(1));
        g.mark_failed(AgentId(1));
        assert!(g.can_recover(AgentId(2)));
    }

    #[test]
    fn test_can_recover_already_recovered() {
        let mut g = DependencyGraph::new();
        g.mark_recovered(AgentId(1));
        assert!(!g.can_recover(AgentId(1)));
    }

    #[test]
    fn test_can_recover_already_failed() {
        let mut g = DependencyGraph::new();
        g.mark_failed(AgentId(1));
        assert!(!g.can_recover(AgentId(1)));
    }

    #[test]
    fn test_mark_recovered_and_failed() {
        let mut g = DependencyGraph::new();
        g.mark_recovered(AgentId(1));
        g.mark_failed(AgentId(2));
        assert!(g.recovered().contains(&AgentId(1)));
        assert!(g.failed().contains(&AgentId(2)));
        assert!(!g.recovered().contains(&AgentId(2)));
        assert!(!g.failed().contains(&AgentId(1)));
    }

    #[test]
    fn test_all_nodes() {
        let mut g = DependencyGraph::new();
        g.add_dependency(AgentId(2), AgentId(1));
        g.add_dependency(AgentId(3), AgentId(2));
        let nodes = g.all_nodes();
        assert_eq!(nodes.len(), 3);
        assert!(nodes.contains(&AgentId(1)));
        assert!(nodes.contains(&AgentId(2)));
        assert!(nodes.contains(&AgentId(3)));
    }

    #[test]
    fn test_self_cycle_detected() {
        let mut g = DependencyGraph::new();
        g.add_dependency(AgentId(1), AgentId(1)); // self-cycle
        assert!(g.has_cycle());
    }
}

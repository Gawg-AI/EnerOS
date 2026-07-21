//! Agent 注册表 — AgentRegistry 双索引设计
//!
//! 提供 Agent 的注册 / 注销 / 查询 / 枚举 / 统计能力，是 Agent 生命周期管理的基础。
//!
//! # 设计：双索引
//! - 主索引 `agents: BTreeMap<AgentId, AgentDescriptor>`：按 ID 存储描述符，
//!   提供 O(log n) 的按 ID 增删查。
//! - 副索引 `by_type: BTreeMap<AgentType, Vec<AgentId>>`：按类型反向索引 ID 列表，
//!   加速 `find_by_type` / `count_by_type` 查询。
//!
//! # 偏差声明
//! - **D1 BTreeMap 偏差**：no_std 环境无标准 `HashMap`（缺少哈希随机源），
//!   主/副索引均采用 `BTreeMap`。代价为 O(log n) 访问，收益是键的确定性升序遍历
//!   （`AgentId` 升序），便于调试与快照一致性。要求 `AgentId` / `AgentType` 实现 `Ord`。
//! - **D2 无锁偏差**：当前实现面向单线程非并发访问，内部不加锁。并发安全将由
//!   上层（v0.35+ 调度器 / 运行时）通过外部同步原语（如 `spin::Mutex`）包装提供，
//!   本 crate 保持零外部依赖不变。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::{AgentDescriptor, AgentError, AgentId, AgentType};

/// Agent 注册表（双索引）.
///
/// 主索引按 [`AgentId`] 存储描述符，副索引按 [`AgentType`] 反向索引 ID 列表。
#[derive(Default, Debug)]
pub struct AgentRegistry {
    agents: BTreeMap<AgentId, AgentDescriptor>,
    by_type: BTreeMap<AgentType, Vec<AgentId>>,
}

impl AgentRegistry {
    /// 创建空注册表.
    pub fn new() -> Self {
        AgentRegistry {
            agents: BTreeMap::new(),
            by_type: BTreeMap::new(),
        }
    }

    /// 注册 Agent.
    ///
    /// 若 `agent_id` 已存在返回 [`AgentError::AlreadyRegistered`]；
    /// 否则写入主索引并更新类型副索引，返回该 Agent ID。
    pub fn register(&mut self, desc: AgentDescriptor) -> Result<AgentId, AgentError> {
        if self.agents.contains_key(&desc.agent_id) {
            return Err(AgentError::AlreadyRegistered);
        }
        let id = desc.agent_id;
        let agent_type = desc.agent_type;
        self.agents.insert(id, desc);
        self.by_type.entry(agent_type).or_default().push(id);
        Ok(id)
    }

    /// 注销 Agent.
    ///
    /// 若 ID 不存在返回 [`AgentError::AgentNotFound`]；
    /// 否则从主索引移除，并清理类型副索引中的对应 ID。
    pub fn unregister(&mut self, id: AgentId) -> Result<(), AgentError> {
        let desc = self.agents.remove(&id).ok_or(AgentError::AgentNotFound)?;
        if let Some(ids) = self.by_type.get_mut(&desc.agent_type) {
            ids.retain(|&x| x != id);
        }
        Ok(())
    }

    /// 按 ID 获取 Agent 描述符（不可变引用）.
    pub fn get(&self, id: AgentId) -> Option<&AgentDescriptor> {
        self.agents.get(&id)
    }

    /// 按 ID 获取 Agent 描述符（可变引用）.
    pub fn get_mut(&mut self, id: AgentId) -> Option<&mut AgentDescriptor> {
        self.agents.get_mut(&id)
    }

    /// 判断 ID 是否已注册.
    pub fn exists(&self, id: AgentId) -> bool {
        self.agents.contains_key(&id)
    }

    /// 按类型查找 Agent（结果按 `AgentId` 升序）.
    pub fn find_by_type(&self, agent_type: AgentType) -> Vec<&AgentDescriptor> {
        self.by_type
            .get(&agent_type)
            .map(|ids| ids.iter().filter_map(|id| self.agents.get(id)).collect())
            .unwrap_or_default()
    }

    /// 按名称查找首个匹配的 Agent.
    pub fn find_by_name(&self, name: &str) -> Option<&AgentDescriptor> {
        self.agents.values().find(|a| a.name == name)
    }

    /// 列出所有 Agent（按 `AgentId` 升序）.
    pub fn list_all(&self) -> Vec<&AgentDescriptor> {
        self.agents.values().collect()
    }

    /// 列出所有存活 Agent（`is_alive` 为真）.
    pub fn list_alive(&self) -> Vec<&AgentDescriptor> {
        self.agents.values().filter(|a| a.is_alive()).collect()
    }

    /// 已注册 Agent 总数.
    pub fn count(&self) -> usize {
        self.agents.len()
    }

    /// 按类型统计 Agent 数量.
    pub fn count_by_type(&self, agent_type: AgentType) -> usize {
        self.by_type.get(&agent_type).map_or(0, |v| v.len())
    }

    /// 生成注册表统计快照.
    pub fn stats(&self) -> RegistryStats {
        let total = self.agents.len();
        let alive = self.agents.values().filter(|a| a.is_alive()).count();
        let mut by_type = BTreeMap::new();
        for (agent_type, ids) in &self.by_type {
            by_type.insert(*agent_type, ids.len());
        }
        RegistryStats {
            total,
            alive,
            by_type,
        }
    }
}

/// 注册表统计快照.
#[derive(Clone, Debug)]
pub struct RegistryStats {
    /// Agent 总数
    pub total: usize,
    /// 存活 Agent 数
    pub alive: usize,
    /// 按类型分组的数量
    pub by_type: BTreeMap<AgentType, usize>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentState;

    #[test]
    fn test_register_and_get() {
        let mut reg = AgentRegistry::new();
        let desc = AgentDescriptor::new(AgentType::Energy, "e1", 0);
        let id = reg.register(desc).unwrap();
        assert!(reg.get(id).is_some());
        assert_eq!(reg.get(id).unwrap().name, "e1");
    }

    #[test]
    fn test_register_duplicate_rejected() {
        let mut reg = AgentRegistry::new();
        let desc1 = AgentDescriptor::new(AgentType::Energy, "e1", 0);
        let mut desc2 = AgentDescriptor::new(AgentType::Energy, "e2", 0);
        desc2.agent_id = desc1.agent_id;
        reg.register(desc1).unwrap();
        let err = reg.register(desc2).unwrap_err();
        assert_eq!(err, AgentError::AlreadyRegistered);
    }

    #[test]
    fn test_unregister_existing() {
        let mut reg = AgentRegistry::new();
        let id = reg
            .register(AgentDescriptor::new(AgentType::Energy, "e1", 0))
            .unwrap();
        reg.unregister(id).unwrap();
        assert!(reg.get(id).is_none());
    }

    #[test]
    fn test_unregister_nonexistent() {
        let mut reg = AgentRegistry::new();
        let id = AgentId::generate();
        let err = reg.unregister(id).unwrap_err();
        assert_eq!(err, AgentError::AgentNotFound);
    }

    #[test]
    fn test_unregister_cleans_type_index() {
        let mut reg = AgentRegistry::new();
        let id1 = reg
            .register(AgentDescriptor::new(AgentType::Energy, "e1", 0))
            .unwrap();
        reg.register(AgentDescriptor::new(AgentType::Energy, "e2", 0))
            .unwrap();
        reg.unregister(id1).unwrap();
        assert_eq!(reg.count_by_type(AgentType::Energy), 1);
        assert_eq!(reg.find_by_type(AgentType::Energy).len(), 1);
    }

    #[test]
    fn test_find_by_type_returns_sorted_by_id() {
        let mut reg = AgentRegistry::new();
        let id1 = reg
            .register(AgentDescriptor::new(AgentType::Energy, "e1", 0))
            .unwrap();
        let id2 = reg
            .register(AgentDescriptor::new(AgentType::Energy, "e2", 0))
            .unwrap();
        let id3 = reg
            .register(AgentDescriptor::new(AgentType::Energy, "e3", 0))
            .unwrap();
        let found = reg.find_by_type(AgentType::Energy);
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].agent_id, id1);
        assert_eq!(found[1].agent_id, id2);
        assert_eq!(found[2].agent_id, id3);
        assert!(found[0].agent_id < found[1].agent_id);
        assert!(found[1].agent_id < found[2].agent_id);
    }

    #[test]
    fn test_find_by_type_empty() {
        let reg = AgentRegistry::new();
        let found = reg.find_by_type(AgentType::Energy);
        assert!(found.is_empty());
    }

    #[test]
    fn test_find_by_name() {
        let mut reg = AgentRegistry::new();
        let id = reg
            .register(AgentDescriptor::new(AgentType::Energy, "my-agent", 0))
            .unwrap();
        let found = reg.find_by_name("my-agent");
        assert!(found.is_some());
        assert_eq!(found.unwrap().agent_id, id);
    }

    #[test]
    fn test_find_by_name_not_found() {
        let reg = AgentRegistry::new();
        assert!(reg.find_by_name("nonexistent").is_none());
    }

    #[test]
    fn test_list_all_sorted() {
        let mut reg = AgentRegistry::new();
        let id1 = reg
            .register(AgentDescriptor::new(AgentType::Energy, "e1", 0))
            .unwrap();
        let id2 = reg
            .register(AgentDescriptor::new(AgentType::Device, "d1", 0))
            .unwrap();
        let id3 = reg
            .register(AgentDescriptor::new(AgentType::Market, "m1", 0))
            .unwrap();
        let all = reg.list_all();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].agent_id, id1);
        assert_eq!(all[1].agent_id, id2);
        assert_eq!(all[2].agent_id, id3);
        assert!(all[0].agent_id < all[1].agent_id);
        assert!(all[1].agent_id < all[2].agent_id);
    }

    #[test]
    fn test_list_alive_filters_dead() {
        let mut reg = AgentRegistry::new();
        let mut d1 = AgentDescriptor::new(AgentType::Energy, "e1", 0);
        d1.state = AgentState::Dead;
        let d2 = AgentDescriptor::new(AgentType::Energy, "e2", 0); // Created (not alive)
        let mut d3 = AgentDescriptor::new(AgentType::Energy, "e3", 0);
        d3.state = AgentState::Running;
        reg.register(d1).unwrap();
        reg.register(d2).unwrap();
        let id3 = reg.register(d3).unwrap();
        let alive = reg.list_alive();
        assert_eq!(alive.len(), 1);
        assert_eq!(alive[0].agent_id, id3);
    }

    #[test]
    fn test_count_and_count_by_type() {
        let mut reg = AgentRegistry::new();
        reg.register(AgentDescriptor::new(AgentType::Energy, "e1", 0))
            .unwrap();
        reg.register(AgentDescriptor::new(AgentType::Energy, "e2", 0))
            .unwrap();
        reg.register(AgentDescriptor::new(AgentType::Device, "d1", 0))
            .unwrap();
        assert_eq!(reg.count(), 3);
        assert_eq!(reg.count_by_type(AgentType::Energy), 2);
        assert_eq!(reg.count_by_type(AgentType::Device), 1);
        assert_eq!(reg.count_by_type(AgentType::Market), 0);
    }

    #[test]
    fn test_exists() {
        let mut reg = AgentRegistry::new();
        let id = reg
            .register(AgentDescriptor::new(AgentType::Energy, "e1", 0))
            .unwrap();
        assert!(reg.exists(id));
        reg.unregister(id).unwrap();
        assert!(!reg.exists(id));
    }

    #[test]
    fn test_stats() {
        let mut reg = AgentRegistry::new();
        let mut e1 = AgentDescriptor::new(AgentType::Energy, "e1", 0);
        e1.state = AgentState::Running;
        let mut e2 = AgentDescriptor::new(AgentType::Energy, "e2", 0);
        e2.state = AgentState::Dead;
        let mut d1 = AgentDescriptor::new(AgentType::Device, "d1", 0);
        d1.state = AgentState::Running;
        reg.register(e1).unwrap();
        reg.register(e2).unwrap();
        reg.register(d1).unwrap();
        let stats = reg.stats();
        assert_eq!(stats.total, 3);
        assert_eq!(stats.alive, 2);
        assert_eq!(stats.by_type.get(&AgentType::Energy).copied(), Some(2));
        assert_eq!(stats.by_type.get(&AgentType::Device).copied(), Some(1));
    }

    #[test]
    fn test_get_mut_updates_descriptor() {
        let mut reg = AgentRegistry::new();
        let id = reg
            .register(AgentDescriptor::new(AgentType::Energy, "e1", 0))
            .unwrap();
        {
            let desc = reg.get_mut(id).unwrap();
            desc.state = AgentState::Running;
        }
        let desc = reg.get(id).unwrap();
        assert_eq!(desc.state, AgentState::Running);
    }

    #[test]
    fn test_empty_registry() {
        let reg = AgentRegistry::new();
        assert_eq!(reg.count(), 0);
        assert!(reg.list_all().is_empty());
        assert_eq!(reg.stats().total, 0);
    }

    #[test]
    fn test_register_multiple_types() {
        let mut reg = AgentRegistry::new();
        reg.register(AgentDescriptor::new(AgentType::System, "s1", 0))
            .unwrap();
        reg.register(AgentDescriptor::new(AgentType::Energy, "e1", 0))
            .unwrap();
        reg.register(AgentDescriptor::new(AgentType::Market, "m1", 0))
            .unwrap();
        reg.register(AgentDescriptor::new(AgentType::Grid, "g1", 0))
            .unwrap();
        assert_eq!(reg.find_by_type(AgentType::System).len(), 1);
        assert_eq!(reg.find_by_type(AgentType::Energy).len(), 1);
        assert_eq!(reg.find_by_type(AgentType::Market).len(), 1);
        assert_eq!(reg.find_by_type(AgentType::Grid).len(), 1);
    }

    #[test]
    fn test_unregister_all_then_register() {
        let mut reg = AgentRegistry::new();
        let id1 = reg
            .register(AgentDescriptor::new(AgentType::Energy, "e1", 0))
            .unwrap();
        reg.unregister(id1).unwrap();
        let id2 = reg
            .register(AgentDescriptor::new(AgentType::Energy, "e2", 0))
            .unwrap();
        assert!(reg.exists(id2));
        assert_ne!(id1, id2);
        assert_eq!(reg.count(), 1);
    }
}

//! Agent 描述符 — AgentDescriptor 核心数据结构
//!
//! 定义 Agent 的完整属性（13 字段），是 Agent Runtime 的基础数据结构。
//! 所有后续 Agent 管理功能（注册表 / 生命周期 / 心跳 / 能力管理）都基于此结构。

use alloc::string::String;
use alloc::vec::Vec;

use crate::id::AgentId;
use crate::types::{AgentState, AgentType, CapabilityRef, TrustLevel};

/// Agent 描述符（13 字段）.
///
/// 定义 Agent 的完整属性，是 Agent Runtime 的基础数据结构。
/// 所有后续 Agent 管理功能（注册表 / 生命周期 / 心跳 / 能力管理）都基于此结构。
#[derive(Clone, Debug)]
pub struct AgentDescriptor {
    /// Agent 唯一标识符
    pub agent_id: AgentId,
    /// Agent 类型
    pub agent_type: AgentType,
    /// Agent 名称
    pub name: String,
    /// 生命周期状态
    pub state: AgentState,
    /// 优先级（0~255，越大越高）
    pub priority: u8,
    /// 内存配额（字节）
    pub mem_quota: usize,
    /// CPU 配额（百分比，0~100）
    pub cpu_quota: u8,
    /// 信任等级
    pub trust_level: TrustLevel,
    /// 已授予能力列表
    pub capabilities: Vec<CapabilityRef>,
    /// 父 Agent ID（None 表示顶层 Agent）
    pub parent: Option<AgentId>,
    /// 创建时间戳（外部提供）
    pub created_at: u64,
    /// 重启次数
    pub restart_count: u32,
    /// 最后心跳时间戳（0 表示尚未心跳）
    pub last_heartbeat: u64,
}

impl AgentDescriptor {
    /// 创建新的 Agent 描述符.
    ///
    /// 根据 `agent_type` 自动设置优先级、配额和信任等级。
    /// `state` 初始化为 `Created`，`capabilities` 为空。
    ///
    /// # 参数
    /// * `agent_type` - Agent 类型
    /// * `name` - Agent 名称
    /// * `now` - 当前时间戳（由外部提供，遵循 no_std 惯例）
    pub fn new(agent_type: AgentType, name: &str, now: u64) -> Self {
        let (priority, mem_quota, cpu_quota, trust_level) = Self::defaults_for_type(&agent_type);
        AgentDescriptor {
            agent_id: AgentId::generate(),
            agent_type,
            name: String::from(name),
            state: AgentState::Created,
            priority,
            mem_quota,
            cpu_quota,
            trust_level,
            capabilities: Vec::new(),
            parent: None,
            created_at: now,
            restart_count: 0,
            last_heartbeat: 0,
        }
    }

    /// 检查 Agent 是否存活（已启动且未死亡）.
    ///
    /// `Created` 和 `Dead` 视为非存活；其余状态视为存活。
    pub fn is_alive(&self) -> bool {
        !matches!(self.state, AgentState::Dead | AgentState::Created)
    }

    /// 检查 Agent 是否有权访问资源.
    ///
    /// 当前实现基于信任等级阈值（`trust_level >= Verified`）。
    /// v0.39.0 能力系统实现后将替换为 capability-based 检查。
    pub fn can_access(&self, _resource: &str) -> bool {
        self.trust_level >= TrustLevel::Verified
    }

    /// 检查请求的资源是否在配额范围内.
    ///
    /// `mem` 与 `cpu` 均须不超过对应配额。
    pub fn check_quota(&self, mem: usize, cpu: u8) -> bool {
        mem <= self.mem_quota && cpu <= self.cpu_quota
    }

    /// 根据 Agent 类型获取默认配额 (priority, mem_quota, cpu_quota, trust_level).
    ///
    /// 映射规则（蓝图）：
    /// - System → 优先级 255, 256MB, CPU 30%, TrustLevel::System
    /// - Energy → 优先级 200, 128MB, CPU 25%, TrustLevel::Trusted
    /// - Market / Grid → 优先级 150, 16MB, CPU 10%, TrustLevel::Trusted
    /// - Device → 优先级 100, 32MB, CPU 10%, TrustLevel::Trusted
    /// - 其他（Twin/EdgeCoord/CloudCoord/Custom）→ 优先级 50, 16MB, CPU 10%, TrustLevel::Verified
    fn defaults_for_type(agent_type: &AgentType) -> (u8, usize, u8, TrustLevel) {
        match agent_type {
            AgentType::System => (255, 256 * 1024 * 1024, 30, TrustLevel::System),
            AgentType::Energy => (200, 128 * 1024 * 1024, 25, TrustLevel::Trusted),
            AgentType::Market | AgentType::Grid => (150, 16 * 1024 * 1024, 10, TrustLevel::Trusted),
            AgentType::Device => (100, 32 * 1024 * 1024, 10, TrustLevel::Trusted),
            _ => (50, 16 * 1024 * 1024, 10, TrustLevel::Verified),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: usize = 1024 * 1024;

    #[test]
    fn test_new_system_defaults() {
        let a = AgentDescriptor::new(AgentType::System, "sys", 1000);
        assert_eq!(a.priority, 255);
        assert_eq!(a.mem_quota, 256 * MB);
        assert_eq!(a.cpu_quota, 30);
        assert_eq!(a.trust_level, TrustLevel::System);
        assert_eq!(a.state, AgentState::Created);
        assert_eq!(a.agent_type, AgentType::System);
        assert_eq!(a.name, "sys");
        assert_eq!(a.created_at, 1000);
    }

    #[test]
    fn test_new_energy_defaults() {
        let a = AgentDescriptor::new(AgentType::Energy, "energy", 1000);
        assert_eq!(a.priority, 200);
        assert_eq!(a.mem_quota, 128 * MB);
        assert_eq!(a.cpu_quota, 25);
        assert_eq!(a.trust_level, TrustLevel::Trusted);
    }

    #[test]
    fn test_new_market_defaults() {
        let a = AgentDescriptor::new(AgentType::Market, "market", 1000);
        assert_eq!(a.priority, 150);
        assert_eq!(a.mem_quota, 16 * MB);
        assert_eq!(a.cpu_quota, 10);
        assert_eq!(a.trust_level, TrustLevel::Trusted);
    }

    #[test]
    fn test_new_grid_defaults() {
        let a = AgentDescriptor::new(AgentType::Grid, "grid", 1000);
        assert_eq!(a.priority, 150);
        assert_eq!(a.mem_quota, 16 * MB);
        assert_eq!(a.cpu_quota, 10);
        assert_eq!(a.trust_level, TrustLevel::Trusted);
    }

    #[test]
    fn test_new_device_defaults() {
        let a = AgentDescriptor::new(AgentType::Device, "dev", 1000);
        assert_eq!(a.priority, 100);
        assert_eq!(a.mem_quota, 32 * MB);
        assert_eq!(a.cpu_quota, 10);
        assert_eq!(a.trust_level, TrustLevel::Trusted);
    }

    #[test]
    fn test_new_twin_defaults() {
        let a = AgentDescriptor::new(AgentType::Twin, "twin", 1000);
        assert_eq!(a.priority, 50);
        assert_eq!(a.mem_quota, 16 * MB);
        assert_eq!(a.cpu_quota, 10);
        assert_eq!(a.trust_level, TrustLevel::Verified);
    }

    #[test]
    fn test_new_custom_defaults() {
        let a = AgentDescriptor::new(AgentType::Custom(42), "custom", 1000);
        assert_eq!(a.priority, 50);
        assert_eq!(a.mem_quota, 16 * MB);
        assert_eq!(a.cpu_quota, 10);
        assert_eq!(a.trust_level, TrustLevel::Verified);
    }

    #[test]
    fn test_is_alive_all_states() {
        let mk = |state| {
            let mut a = AgentDescriptor::new(AgentType::Energy, "e", 0);
            a.state = state;
            a
        };
        assert!(!mk(AgentState::Created).is_alive());
        assert!(mk(AgentState::Ready).is_alive());
        assert!(mk(AgentState::Running).is_alive());
        assert!(mk(AgentState::Suspended).is_alive());
        assert!(mk(AgentState::Error).is_alive());
        assert!(mk(AgentState::Recovering).is_alive());
        assert!(!mk(AgentState::Dead).is_alive());
    }

    #[test]
    fn test_check_quota_boundaries() {
        let a = AgentDescriptor::new(AgentType::Device, "dev", 0);
        // Device: mem 32MB, cpu 10
        // exactly equal -> true
        assert!(a.check_quota(32 * MB, 10));
        // exceeds mem -> false
        assert!(!a.check_quota(32 * MB + 1, 5));
        // exceeds cpu -> false
        assert!(!a.check_quota(1024, 11));
        // zero request -> true
        assert!(a.check_quota(0, 0));
    }

    #[test]
    fn test_can_access_all_trust_levels() {
        let mk = |tl| {
            let mut a = AgentDescriptor::new(AgentType::Energy, "e", 0);
            a.trust_level = tl;
            a
        };
        assert!(!mk(TrustLevel::Untrusted).can_access("res"));
        assert!(mk(TrustLevel::Verified).can_access("res"));
        assert!(mk(TrustLevel::Trusted).can_access("res"));
        assert!(mk(TrustLevel::System).can_access("res"));
    }

    #[test]
    fn test_agent_id_nonzero_after_new() {
        let a = AgentDescriptor::new(AgentType::System, "sys", 0);
        assert_ne!(a.agent_id, AgentId::ZERO);
        assert_ne!(a.agent_id.0, 0);
    }

    #[test]
    fn test_capabilities_empty_after_new() {
        let a = AgentDescriptor::new(AgentType::System, "sys", 0);
        assert!(a.capabilities.is_empty());
    }

    #[test]
    fn test_parent_none_after_new() {
        let a = AgentDescriptor::new(AgentType::System, "sys", 0);
        assert!(a.parent.is_none());
    }

    #[test]
    fn test_descriptor_clone() {
        let a1 = AgentDescriptor::new(AgentType::Energy, "e", 5000);
        let a2 = a1.clone();
        assert_eq!(a1.agent_id, a2.agent_id);
        assert_eq!(a1.name, a2.name);
        assert_eq!(a1.agent_type, a2.agent_type);
        assert_eq!(a1.state, a2.state);
        assert_eq!(a1.priority, a2.priority);
    }

    #[test]
    fn test_restart_count_and_heartbeat_init() {
        let a = AgentDescriptor::new(AgentType::System, "sys", 42);
        assert_eq!(a.restart_count, 0);
        assert_eq!(a.last_heartbeat, 0);
    }
}

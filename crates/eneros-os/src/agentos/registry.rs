//! Agent 进程注册表
//!
//! 记录所有 Agent 进程的元数据（PID/状态/权限/类型），不持有实例引用。
//! 这是 AgentOS 内核的中央注册表，供 Supervisor/Scheduler/AuthorityEnforcer 共享查询。

use chrono::{DateTime, Utc};
use eneros_core::AuthorityLevel;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Agent 类型分类（OS 级别，对应 7 种专业 Agent）
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentType {
    /// 调度 Agent — 经济调度与负荷均衡
    Dispatch,
    /// 预测 Agent — 负荷/新能源预测
    Forecast,
    /// 运维 Agent — 故障诊断与恢复
    Operation,
    /// 自愈 Agent — 实时自愈（RT 进程，SCHED_FIFO）
    SelfHealing,
    /// 交易 Agent — 能源市场
    Trading,
    /// 规划 Agent — 扩展与重构
    Planning,
    /// 自定义 Agent
    Custom(String),
}

/// Agent 进程运行状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentStatus {
    /// 正在启动
    Starting,
    /// 正常运行
    Running,
    /// 已停止
    Stopped,
    /// 崩溃（等待重启）
    Crashed,
    /// 降级模式（崩溃频率过高）
    Degraded,
}

/// Agent 进程元数据（注册表中存储的信息）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    /// Agent 唯一标识
    pub agent_id: String,
    /// OS 进程 ID
    pub pid: u32,
    /// Agent 类型
    pub agent_type: AgentType,
    /// 权限级别
    pub authority: AuthorityLevel,
    /// 当前状态
    pub status: AgentStatus,
    /// 启动时间
    pub started_at: DateTime<Utc>,
    /// 二进制路径
    pub binary: String,
    /// 崩溃次数
    pub crash_count: u32,
}

/// Agent 进程注册表
///
/// 线程安全（`RwLock<HashMap>`），只存储元数据，不持有 Agent 实例引用。
/// 供 AgentSupervisor/AgentScheduler/AuthorityEnforcer 共享查询。
#[derive(Debug)]
pub struct AgentRegistry {
    agents: RwLock<HashMap<String, AgentInfo>>,
}

impl AgentRegistry {
    /// 创建空注册表
    pub fn new() -> Self {
        Self {
            agents: RwLock::new(HashMap::new()),
        }
    }

    /// 注册一个 Agent 进程
    pub fn register(&self, info: AgentInfo) -> Result<(), RegistryError> {
        let mut agents = self.agents.write();
        if agents.contains_key(&info.agent_id) {
            return Err(RegistryError::AlreadyRegistered(info.agent_id));
        }
        agents.insert(info.agent_id.clone(), info);
        Ok(())
    }

    /// 查询 Agent 信息
    pub fn lookup(&self, agent_id: &str) -> Option<AgentInfo> {
        self.agents.read().get(agent_id).cloned()
    }

    /// 列举所有 Agent
    pub fn list(&self) -> Vec<AgentInfo> {
        self.agents.read().values().cloned().collect()
    }

    /// 注销 Agent
    pub fn unregister(&self, agent_id: &str) -> Result<(), RegistryError> {
        let mut agents = self.agents.write();
        agents
            .remove(agent_id)
            .ok_or_else(|| RegistryError::NotFound(agent_id.to_string()))?;
        Ok(())
    }

    /// 更新 Agent 状态
    pub fn update_status(&self, agent_id: &str, status: AgentStatus) -> Result<(), RegistryError> {
        let mut agents = self.agents.write();
        let info = agents
            .get_mut(agent_id)
            .ok_or_else(|| RegistryError::NotFound(agent_id.to_string()))?;
        info.status = status;
        Ok(())
    }

    /// 更新 Agent PID（重启后 PID 变化）
    pub fn update_pid(&self, agent_id: &str, pid: u32) -> Result<(), RegistryError> {
        let mut agents = self.agents.write();
        let info = agents
            .get_mut(agent_id)
            .ok_or_else(|| RegistryError::NotFound(agent_id.to_string()))?;
        info.pid = pid;
        info.started_at = Utc::now();
        Ok(())
    }

    /// 记录崩溃
    pub fn record_crash(&self, agent_id: &str) -> Result<(), RegistryError> {
        let mut agents = self.agents.write();
        let info = agents
            .get_mut(agent_id)
            .ok_or_else(|| RegistryError::NotFound(agent_id.to_string()))?;
        info.crash_count += 1;
        info.status = AgentStatus::Crashed;
        Ok(())
    }
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// 注册表错误
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("agent '{0}' already registered")]
    AlreadyRegistered(String),
    #[error("agent '{0}' not found")]
    NotFound(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_agent_info(id: &str) -> AgentInfo {
        AgentInfo {
            agent_id: id.to_string(),
            pid: 1000,
            agent_type: AgentType::Dispatch,
            authority: AuthorityLevel::Operator,
            status: AgentStatus::Starting,
            started_at: Utc::now(),
            binary: "/bin/eneros-dispatch-agent".to_string(),
            crash_count: 0,
        }
    }

    #[test]
    fn test_register_and_lookup() {
        let reg = AgentRegistry::new();
        let info = test_agent_info("agent-1");
        reg.register(info.clone()).unwrap();
        let found = reg.lookup("agent-1").unwrap();
        assert_eq!(found.agent_id, "agent-1");
        assert_eq!(found.pid, 1000);
    }

    #[test]
    fn test_duplicate_register_fails() {
        let reg = AgentRegistry::new();
        let info = test_agent_info("agent-1");
        reg.register(info).unwrap();
        let result = reg.register(test_agent_info("agent-1"));
        assert!(result.is_err());
    }

    #[test]
    fn test_list() {
        let reg = AgentRegistry::new();
        reg.register(test_agent_info("agent-1")).unwrap();
        reg.register(test_agent_info("agent-2")).unwrap();
        assert_eq!(reg.list().len(), 2);
    }

    #[test]
    fn test_unregister() {
        let reg = AgentRegistry::new();
        reg.register(test_agent_info("agent-1")).unwrap();
        reg.unregister("agent-1").unwrap();
        assert!(reg.lookup("agent-1").is_none());
    }

    #[test]
    fn test_update_status() {
        let reg = AgentRegistry::new();
        reg.register(test_agent_info("agent-1")).unwrap();
        reg.update_status("agent-1", AgentStatus::Running).unwrap();
        assert_eq!(reg.lookup("agent-1").unwrap().status, AgentStatus::Running);
    }

    #[test]
    fn test_record_crash() {
        let reg = AgentRegistry::new();
        reg.register(test_agent_info("agent-1")).unwrap();
        reg.record_crash("agent-1").unwrap();
        let info = reg.lookup("agent-1").unwrap();
        assert_eq!(info.crash_count, 1);
        assert_eq!(info.status, AgentStatus::Crashed);
    }

    #[test]
    fn test_lookup_not_found() {
        let reg = AgentRegistry::new();
        assert!(reg.lookup("nonexistent").is_none());
    }

    #[test]
    fn test_unregister_not_found() {
        let reg = AgentRegistry::new();
        assert!(reg.unregister("nonexistent").is_err());
    }
}

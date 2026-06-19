//! Agent 调度策略 — SCHED_FIFO RT 调度 + CPU 隔离
//!
//! 为 RT Agent（如 self_healing）设置 SCHED_FIFO 实时调度策略，
//! 普通 Agent 使用 SCHED_OTHER。
//!
//! 在 Linux 上：调用 sched_setscheduler() + sched_setaffinity()
//! 在非 Linux 上：仅记录策略，不真正设置

use crate::agentos::registry::{AgentRegistry, AgentType};
use crate::rt::runtime::{RtConfig, RtRuntime};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// 调度策略
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SchedulingPolicy {
    /// 普通调度（SCHED_OTHER），无 RT 优先级
    Normal,
    /// 实时调度（SCHED_FIFO），指定优先级 1-99
    Realtime {
        /// SCHED_FIFO 优先级（1-99，越高越优先）
        priority: u32,
        /// 绑定的 CPU 核（空表示不绑定）
        cpus: Vec<u32>,
        /// 是否锁定内存（mlockall）
        lock_memory: bool,
    },
}

impl SchedulingPolicy {
    /// 创建普通调度策略
    pub fn normal() -> Self {
        Self::Normal
    }

    /// 创建 RT 调度策略
    pub fn realtime(priority: u32, cpus: Vec<u32>, lock_memory: bool) -> Self {
        Self::Realtime {
            priority: priority.clamp(1, 99),
            cpus,
            lock_memory,
        }
    }

    /// 是否为 RT 策略
    pub fn is_realtime(&self) -> bool {
        matches!(self, SchedulingPolicy::Realtime { .. })
    }

    /// 根据 Agent 类型获取默认调度策略
    pub fn default_for_agent_type(agent_type: &AgentType) -> Self {
        match agent_type {
            // 自愈 Agent 是 RT 进程，需要 SCHED_FIFO
            AgentType::SelfHealing => Self::realtime(80, vec![2, 3], true),
            // 其他 Agent 使用普通调度
            _ => Self::normal(),
        }
    }
}

impl Default for SchedulingPolicy {
    fn default() -> Self {
        Self::normal()
    }
}

/// 调度错误
#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    #[error("agent '{0}' not found in registry")]
    NotFound(String),
    #[error("scheduling operation failed: {0}")]
    SchedFailed(String),
    #[error("invalid priority: {0} (must be 1-99)")]
    InvalidPriority(u32),
    #[error("unsupported on this platform")]
    Unsupported,
}

/// Agent 调度器
///
/// 管理 Agent 进程的调度策略：
/// - RT Agent：SCHED_FIFO + CPU 隔离 + mlockall
/// - 普通 Agent：SCHED_OTHER
pub struct AgentScheduler {
    registry: Arc<AgentRegistry>,
    /// 已应用的调度策略（agent_id → policy）
    policies: parking_lot::RwLock<std::collections::HashMap<String, SchedulingPolicy>>,
}

impl AgentScheduler {
    /// 创建调度器，共享 AgentRegistry
    pub fn new(registry: Arc<AgentRegistry>) -> Self {
        Self {
            registry,
            policies: parking_lot::RwLock::new(std::collections::HashMap::new()),
        }
    }

    /// 为 Agent 设置调度策略
    ///
    /// 在 Linux 上：调用 sched_setscheduler() + sched_setaffinity() + mlockall()
    /// 在非 Linux 上：仅记录策略
    pub fn schedule(
        &self,
        agent_id: &str,
        policy: SchedulingPolicy,
    ) -> Result<(), SchedulerError> {
        let info = self
            .registry
            .lookup(agent_id)
            .ok_or_else(|| SchedulerError::NotFound(agent_id.to_string()))?;

        // 验证优先级
        if let SchedulingPolicy::Realtime { priority, .. } = &policy {
            if !(1..=99).contains(priority) {
                return Err(SchedulerError::InvalidPriority(*priority));
            }
        }

        #[cfg(target_os = "linux")]
        {
            self.apply_policy_linux(info.pid, &policy)?;
        }

        // 非 Linux 平台：info.pid 仅在 Linux 分支使用
        #[cfg(not(target_os = "linux"))]
        {
            let _ = &info;
        }

        self.policies
            .write()
            .insert(agent_id.to_string(), policy.clone());

        tracing::info!(
            "Scheduled agent '{}' with policy: {:?}",
            agent_id,
            policy
        );
        Ok(())
    }

    /// 根据 Agent 类型自动应用默认调度策略
    pub fn auto_schedule(&self, agent_id: &str) -> Result<(), SchedulerError> {
        let info = self
            .registry
            .lookup(agent_id)
            .ok_or_else(|| SchedulerError::NotFound(agent_id.to_string()))?;

        let policy = SchedulingPolicy::default_for_agent_type(&info.agent_type);
        self.schedule(agent_id, policy)
    }

    /// 紧急提升 Agent 优先级
    ///
    /// 将普通 Agent 临时提升为 RT 优先级（用于紧急情况）。
    pub fn preempt(&self, agent_id: &str, priority: u32) -> Result<(), SchedulerError> {
        if !(1..=99).contains(&priority) {
            return Err(SchedulerError::InvalidPriority(priority));
        }

        let info = self
            .registry
            .lookup(agent_id)
            .ok_or_else(|| SchedulerError::NotFound(agent_id.to_string()))?;

        let policy = SchedulingPolicy::realtime(priority, vec![], false);

        #[cfg(target_os = "linux")]
        {
            self.apply_policy_linux(info.pid, &policy)?;
        }

        // 非 Linux 平台：info.pid 仅在 Linux 分支使用
        #[cfg(not(target_os = "linux"))]
        {
            let _ = &info;
        }

        self.policies
            .write()
            .insert(agent_id.to_string(), policy.clone());

        tracing::warn!(
            "Preempted agent '{}' to RT priority {}",
            agent_id,
            priority
        );
        Ok(())
    }

    /// 恢复 Agent 为普通调度
    pub fn demote(&self, agent_id: &str) -> Result<(), SchedulerError> {
        self.schedule(agent_id, SchedulingPolicy::normal())
    }

    /// 获取 Agent 当前调度策略
    pub fn get_policy(&self, agent_id: &str) -> Option<SchedulingPolicy> {
        self.policies.read().get(agent_id).cloned()
    }

    /// 列出所有 RT Agent
    pub fn list_realtime_agents(&self) -> Vec<String> {
        self.policies
            .read()
            .iter()
            .filter(|(_, p)| p.is_realtime())
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// 列出所有已调度的 Agent
    pub fn list_scheduled(&self) -> Vec<String> {
        self.policies.read().keys().cloned().collect()
    }

    /// Linux: 应用调度策略到目标进程
    #[cfg(target_os = "linux")]
    fn apply_policy_linux(
        &self,
        pid: u32,
        policy: &SchedulingPolicy,
    ) -> Result<(), SchedulerError> {
        use std::mem;

        match policy {
            SchedulingPolicy::Normal => {
                // SCHED_OTHER
                let param = libc::sched_param { sched_priority: 0 };
                let ret = unsafe {
                    libc::sched_setscheduler(pid as i32, libc::SCHED_OTHER, &param)
                };
                if ret != 0 {
                    return Err(SchedulerError::SchedFailed(format!(
                        "sched_setscheduler SCHED_OTHER failed: {}",
                        std::io::Error::last_os_error()
                    )));
                }
            }
            SchedulingPolicy::Realtime {
                priority,
                cpus,
                lock_memory,
            } => {
                // SCHED_FIFO
                let param = libc::sched_param {
                    sched_priority: *priority as i32,
                };
                let ret = unsafe {
                    libc::sched_setscheduler(pid as i32, libc::SCHED_FIFO, &param)
                };
                if ret != 0 {
                    return Err(SchedulerError::SchedFailed(format!(
                        "sched_setscheduler SCHED_FIFO failed: {}",
                        std::io::Error::last_os_error()
                    )));
                }

                // CPU 亲和性
                if !cpus.is_empty() {
                    let mut cpuset: libc::cpu_set_t = unsafe { mem::zeroed() };
                    for &cpu in cpus {
                        unsafe { libc::CPU_SET(cpu as usize, &mut cpuset) };
                    }
                    let ret = unsafe {
                        libc::sched_setaffinity(
                            pid as i32,
                            mem::size_of::<libc::cpu_set_t>(),
                            &cpuset,
                        )
                    };
                    if ret != 0 {
                        return Err(SchedulerError::SchedFailed(format!(
                            "sched_setaffinity failed: {}",
                            std::io::Error::last_os_error()
                        )));
                    }
                }

                // mlockall
                if *lock_memory {
                    let ret = unsafe {
                        libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE)
                    };
                    if ret != 0 {
                        tracing::warn!(
                            "mlockall failed for agent pid={}: {}",
                            pid,
                            std::io::Error::last_os_error()
                        );
                        // mlockall 失败不致命，继续
                    }
                }
            }
        }
        Ok(())
    }
}

/// 从 SchedulingPolicy 创建 RtConfig（用于复用 RtRuntime）
pub fn policy_to_rt_config(policy: &SchedulingPolicy) -> Option<RtConfig> {
    match policy {
        SchedulingPolicy::Normal => None,
        SchedulingPolicy::Realtime {
            priority,
            cpus,
            lock_memory,
        } => Some(RtConfig {
            cpus: cpus.clone(),
            priority: *priority,
            lock_memory: *lock_memory,
            use_huge_pages: false,
        }),
    }
}

/// 从 RtRuntime 配置创建 SchedulingPolicy
pub fn rt_config_to_policy(config: &RtConfig) -> SchedulingPolicy {
    if config.priority > 0 {
        SchedulingPolicy::realtime(config.priority, config.cpus.clone(), config.lock_memory)
    } else {
        SchedulingPolicy::normal()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agentos::registry::{AgentInfo, AgentStatus};

    fn test_registry() -> Arc<AgentRegistry> {
        Arc::new(AgentRegistry::new())
    }

    fn register_agent(reg: &AgentRegistry, id: &str, agent_type: AgentType) {
        let info = AgentInfo {
            agent_id: id.to_string(),
            pid: 10000,
            agent_type,
            authority: eneros_core::AuthorityLevel::Operator,
            status: AgentStatus::Running,
            started_at: chrono::Utc::now(),
            binary: "/bin/test".to_string(),
            crash_count: 0,
        };
        reg.register(info).unwrap();
    }

    #[test]
    fn test_scheduling_policy_normal() {
        let policy = SchedulingPolicy::normal();
        assert!(!policy.is_realtime());
    }

    #[test]
    fn test_scheduling_policy_realtime() {
        let policy = SchedulingPolicy::realtime(80, vec![2, 3], true);
        assert!(policy.is_realtime());
        if let SchedulingPolicy::Realtime {
            priority,
            cpus,
            lock_memory,
        } = policy
        {
            assert_eq!(priority, 80);
            assert_eq!(cpus, vec![2, 3]);
            assert!(lock_memory);
        }
    }

    #[test]
    fn test_scheduling_priority_clamped() {
        let policy = SchedulingPolicy::realtime(150, vec![], false);
        if let SchedulingPolicy::Realtime { priority, .. } = policy {
            assert_eq!(priority, 99);
        }
    }

    #[test]
    fn test_default_for_self_healing_is_rt() {
        let policy = SchedulingPolicy::default_for_agent_type(&AgentType::SelfHealing);
        assert!(policy.is_realtime());
    }

    #[test]
    fn test_default_for_dispatch_is_normal() {
        let policy = SchedulingPolicy::default_for_agent_type(&AgentType::Dispatch);
        assert!(!policy.is_realtime());
    }

    #[test]
    fn test_schedule_not_found() {
        let reg = test_registry();
        let scheduler = AgentScheduler::new(reg);
        let result = scheduler.schedule("nonexistent", SchedulingPolicy::normal());
        assert!(result.is_err());
    }

    #[test]
    fn test_schedule_normal_succeeds() {
        let reg = test_registry();
        register_agent(&reg, "agent-1", AgentType::Dispatch);
        let scheduler = AgentScheduler::new(reg);

        let result = scheduler.schedule("agent-1", SchedulingPolicy::normal());
        assert!(result.is_ok());
        assert!(scheduler.get_policy("agent-1").is_some());
    }

    #[test]
    fn test_schedule_realtime_invalid_priority() {
        let reg = test_registry();
        register_agent(&reg, "agent-1", AgentType::SelfHealing);
        let scheduler = AgentScheduler::new(reg);

        // 优先级 0 无效
        let result = scheduler.schedule(
            "agent-1",
            SchedulingPolicy::Realtime {
                priority: 0,
                cpus: vec![],
                lock_memory: false,
            },
        );
        assert!(result.is_err());

        // 优先级 100 无效
        let result = scheduler.schedule(
            "agent-1",
            SchedulingPolicy::Realtime {
                priority: 100,
                cpus: vec![],
                lock_memory: false,
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_auto_schedule_self_healing() {
        let reg = test_registry();
        register_agent(&reg, "sh-agent", AgentType::SelfHealing);
        let scheduler = AgentScheduler::new(reg);

        scheduler.auto_schedule("sh-agent").unwrap();
        let policy = scheduler.get_policy("sh-agent").unwrap();
        assert!(policy.is_realtime());
    }

    #[test]
    fn test_auto_schedule_dispatch() {
        let reg = test_registry();
        register_agent(&reg, "disp-agent", AgentType::Dispatch);
        let scheduler = AgentScheduler::new(reg);

        scheduler.auto_schedule("disp-agent").unwrap();
        let policy = scheduler.get_policy("disp-agent").unwrap();
        assert!(!policy.is_realtime());
    }

    #[test]
    fn test_preempt_to_rt() {
        let reg = test_registry();
        register_agent(&reg, "agent-1", AgentType::Dispatch);
        let scheduler = AgentScheduler::new(reg);

        // 先设为普通
        scheduler.schedule("agent-1", SchedulingPolicy::normal()).unwrap();
        assert!(!scheduler.get_policy("agent-1").unwrap().is_realtime());

        // 紧急提升
        scheduler.preempt("agent-1", 90).unwrap();
        assert!(scheduler.get_policy("agent-1").unwrap().is_realtime());
    }

    #[test]
    fn test_preempt_invalid_priority() {
        let reg = test_registry();
        register_agent(&reg, "agent-1", AgentType::Dispatch);
        let scheduler = AgentScheduler::new(reg);

        assert!(scheduler.preempt("agent-1", 0).is_err());
        assert!(scheduler.preempt("agent-1", 100).is_err());
    }

    #[test]
    fn test_demote_to_normal() {
        let reg = test_registry();
        register_agent(&reg, "agent-1", AgentType::SelfHealing);
        let scheduler = AgentScheduler::new(reg);

        scheduler.auto_schedule("agent-1").unwrap();
        assert!(scheduler.get_policy("agent-1").unwrap().is_realtime());

        scheduler.demote("agent-1").unwrap();
        assert!(!scheduler.get_policy("agent-1").unwrap().is_realtime());
    }

    #[test]
    fn test_list_realtime_agents() {
        let reg = test_registry();
        register_agent(&reg, "rt-1", AgentType::SelfHealing);
        register_agent(&reg, "normal-1", AgentType::Dispatch);
        register_agent(&reg, "rt-2", AgentType::SelfHealing);
        let scheduler = AgentScheduler::new(reg);

        scheduler.auto_schedule("rt-1").unwrap();
        scheduler.auto_schedule("normal-1").unwrap();
        scheduler.auto_schedule("rt-2").unwrap();

        let mut rt_agents = scheduler.list_realtime_agents();
        rt_agents.sort();
        assert_eq!(rt_agents, vec!["rt-1", "rt-2"]);
    }

    #[test]
    fn test_policy_to_rt_config() {
        let policy = SchedulingPolicy::realtime(80, vec![2, 3], true);
        let config = policy_to_rt_config(&policy).unwrap();
        assert_eq!(config.priority, 80);
        assert_eq!(config.cpus, vec![2, 3]);
        assert!(config.lock_memory);
    }

    #[test]
    fn test_policy_to_rt_config_normal_returns_none() {
        let policy = SchedulingPolicy::normal();
        assert!(policy_to_rt_config(&policy).is_none());
    }

    #[test]
    fn test_rt_config_to_policy() {
        let config = RtConfig {
            cpus: vec![2, 3],
            priority: 80,
            lock_memory: true,
            use_huge_pages: false,
        };
        let policy = rt_config_to_policy(&config);
        assert!(policy.is_realtime());
    }
}

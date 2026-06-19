//! Agent 生命周期监督
//!
//! 负责 Agent 进程的 spawn/stop/restart，崩溃检测与自动重启。
//! 复用 eneros-init 的 RestartPolicy 和 crash_history 逻辑。

use crate::agentos::registry::{AgentInfo, AgentRegistry, AgentStatus, AgentType, RegistryError};
use chrono::Utc;
use eneros_core::AuthorityLevel;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Supervisor 错误
#[derive(Debug, thiserror::Error)]
pub enum SupervisorError {
    #[error("agent '{0}' not found in registry")]
    NotFound(String),
    #[error("failed to spawn agent '{0}': {1}")]
    SpawnFailed(String, String),
    #[error("agent '{0}' did not stop within timeout")]
    StopTimeout(String),
    #[error("agent '{0}' is in degraded mode (too many crashes)")]
    Degraded(String),
    #[error("registry error: {0}")]
    Registry(#[from] RegistryError),
}

/// Agent 启动配置
#[derive(Debug, Clone)]
pub struct AgentSpawnConfig {
    pub agent_id: String,
    pub agent_type: AgentType,
    pub authority: AuthorityLevel,
    pub binary: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
}

/// Agent 生命周期监督器
///
/// 管理 Agent 进程的启动、停止、重启，并跟踪崩溃历史。
/// 崩溃重启策略：5 次/分钟内降级为 Degraded 状态。
pub struct AgentSupervisor {
    registry: std::sync::Arc<AgentRegistry>,
    crash_history: parking_lot::Mutex<HashMap<String, Vec<Instant>>>,
    max_restarts_per_minute: usize,
}

impl AgentSupervisor {
    /// 创建新的 Supervisor，共享 AgentRegistry
    pub fn new(registry: std::sync::Arc<AgentRegistry>) -> Self {
        Self {
            registry,
            crash_history: parking_lot::Mutex::new(HashMap::new()),
            max_restarts_per_minute: 5,
        }
    }

    /// 启动一个 Agent 进程
    ///
    /// 在 Linux 上：fork/exec 创建子进程
    /// 在非 Linux 上：记录元数据但不真正 spawn（用于开发/测试）
    #[cfg(target_os = "linux")]
    pub fn spawn(&self, config: AgentSpawnConfig) -> Result<u32, SupervisorError> {
        use std::os::unix::process::CommandExt;
        use std::process::Command;

        let mut cmd = Command::new(&config.binary);
        cmd.args(&config.args);
        for (k, v) in &config.env {
            cmd.env(k, v);
        }

        let child = cmd
            .spawn()
            .map_err(|e| SupervisorError::SpawnFailed(config.agent_id.clone(), e.to_string()))?;

        let pid = child.id() as u32;
        let info = AgentInfo {
            agent_id: config.agent_id.clone(),
            pid,
            agent_type: config.agent_type,
            authority: config.authority,
            status: AgentStatus::Running,
            started_at: Utc::now(),
            binary: config.binary,
            crash_count: 0,
        };
        self.registry.register(info).map_err(|e| {
            SupervisorError::SpawnFailed(config.agent_id.clone(), e.to_string())
        })?;

        // 在 Linux 上我们不等待子进程，但需要避免 zombie
        // 实际部署中 eneros-init 的 reap_children 会处理
        std::mem::forget(child);

        Ok(pid)
    }

    /// 启动一个 Agent 进程（非 Linux 平台 stub）
    #[cfg(not(target_os = "linux"))]
    pub fn spawn(&self, config: AgentSpawnConfig) -> Result<u32, SupervisorError> {
        // 非 Linux 平台：模拟 spawn，使用伪 PID
        let fake_pid = (10000 + std::process::id()) as u32;
        let info = AgentInfo {
            agent_id: config.agent_id.clone(),
            pid: fake_pid,
            agent_type: config.agent_type,
            authority: config.authority,
            status: AgentStatus::Running,
            started_at: Utc::now(),
            binary: config.binary,
            crash_count: 0,
        };
        self.registry.register(info).map_err(|e| {
            SupervisorError::SpawnFailed(config.agent_id.clone(), e.to_string())
        })?;
        tracing::info!(
            "Agent '{}' simulated spawn (non-Linux platform, fake PID={})",
            config.agent_id,
            fake_pid
        );
        Ok(fake_pid)
    }

    /// 停止一个 Agent 进程
    ///
    /// 发送 SIGTERM → 等待 10s → SIGKILL
    #[cfg(target_os = "linux")]
    pub fn stop(&self, agent_id: &str) -> Result<(), SupervisorError> {
        let info = self
            .registry
            .lookup(agent_id)
            .ok_or_else(|| SupervisorError::NotFound(agent_id.to_string()))?;

        // SIGTERM
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(info.pid as i32),
            nix::sys::signal::Signal::SIGTERM,
        );

        // 等待退出（最多 10s）
        let timeout = Duration::from_secs(10);
        let start = Instant::now();
        while start.elapsed() < timeout {
            if !self.is_alive(info.pid) {
                self.registry
                    .update_status(agent_id, AgentStatus::Stopped)?;
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        // SIGKILL
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(info.pid as i32),
            nix::sys::signal::Signal::SIGKILL,
        );
        self.registry
            .update_status(agent_id, AgentStatus::Stopped)?;
        Ok(())
    }

    /// 停止一个 Agent 进程（非 Linux 平台 stub）
    #[cfg(not(target_os = "linux"))]
    pub fn stop(&self, agent_id: &str) -> Result<(), SupervisorError> {
        self.registry
            .lookup(agent_id)
            .ok_or_else(|| SupervisorError::NotFound(agent_id.to_string()))?;
        self.registry
            .update_status(agent_id, AgentStatus::Stopped)?;
        tracing::info!("Agent '{}' simulated stop (non-Linux)", agent_id);
        Ok(())
    }

    /// 重启一个 Agent 进程
    pub fn restart(&self, config: &AgentSpawnConfig) -> Result<u32, SupervisorError> {
        // 检查是否已注册
        if self.registry.lookup(&config.agent_id).is_some() {
            self.stop(&config.agent_id)?;
            self.registry.unregister(&config.agent_id)?;
        }
        self.spawn(config.clone())
    }

    /// 检查 Agent 进程是否存活
    #[cfg(target_os = "linux")]
    pub fn is_alive(&self, pid: u32) -> bool {
        // kill(pid, 0) 返回 0 表示进程存在
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid as i32),
            None,
        )
        .is_ok()
    }

    /// 检查 Agent 进程是否存活（非 Linux 平台 stub）
    #[cfg(not(target_os = "linux"))]
    pub fn is_alive(&self, _pid: u32) -> bool {
        false
    }

    /// 健康检查：返回 Agent 当前状态
    pub fn health_check(&self, agent_id: &str) -> Result<AgentStatus, SupervisorError> {
        let info = self
            .registry
            .lookup(agent_id)
            .ok_or_else(|| SupervisorError::NotFound(agent_id.to_string()))?;

        if info.status == AgentStatus::Running && !self.is_alive(info.pid) {
            // 进程已死但注册表还显示 Running → 标记为 Crashed
            self.registry.record_crash(agent_id)?;
            return Ok(AgentStatus::Crashed);
        }

        Ok(info.status)
    }

    /// 检查是否应该重启（基于崩溃频率）
    pub fn should_restart(&self, agent_id: &str) -> Result<bool, SupervisorError> {
        let _info = self
            .registry
            .lookup(agent_id)
            .ok_or_else(|| SupervisorError::NotFound(agent_id.to_string()))?;

        let mut history = self.crash_history.lock();
        let crashes = history.entry(agent_id.to_string()).or_default();

        // 清理 1 分钟前的记录
        let now = Instant::now();
        crashes.retain(|&t| now.duration_since(t) < Duration::from_secs(60));

        if crashes.len() >= self.max_restarts_per_minute {
            // 降级模式
            self.registry
                .update_status(agent_id, AgentStatus::Degraded)?;
            return Ok(false);
        }

        crashes.push(now);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(id: &str) -> AgentSpawnConfig {
        AgentSpawnConfig {
            agent_id: id.to_string(),
            agent_type: AgentType::Dispatch,
            authority: AuthorityLevel::Operator,
            binary: "/bin/test-agent".to_string(),
            args: vec![],
            env: HashMap::new(),
        }
    }

    #[test]
    fn test_spawn_registers_agent() {
        let registry = std::sync::Arc::new(AgentRegistry::new());
        let sup = AgentSupervisor::new(registry.clone());
        let config = test_config("test-agent-1");
        let pid = sup.spawn(config).unwrap();
        assert!(pid > 0);
        let info = registry.lookup("test-agent-1").unwrap();
        assert_eq!(info.status, AgentStatus::Running);
    }

    #[test]
    fn test_stop_updates_status() {
        let registry = std::sync::Arc::new(AgentRegistry::new());
        let sup = AgentSupervisor::new(registry.clone());
        sup.spawn(test_config("test-agent-2")).unwrap();
        sup.stop("test-agent-2").unwrap();
        let info = registry.lookup("test-agent-2").unwrap();
        assert_eq!(info.status, AgentStatus::Stopped);
    }

    #[test]
    fn test_health_check_not_found() {
        let registry = std::sync::Arc::new(AgentRegistry::new());
        let sup = AgentSupervisor::new(registry);
        assert!(sup.health_check("nonexistent").is_err());
    }

    #[test]
    fn test_should_restart_first_time() {
        let registry = std::sync::Arc::new(AgentRegistry::new());
        let sup = AgentSupervisor::new(registry.clone());
        sup.spawn(test_config("test-agent-3")).unwrap();
        assert!(sup.should_restart("test-agent-3").unwrap());
    }

    #[test]
    fn test_should_restart_degraded_after_max() {
        let registry = std::sync::Arc::new(AgentRegistry::new());
        let sup = AgentSupervisor::new(registry.clone());
        sup.spawn(test_config("test-agent-4")).unwrap();
        // 触发 5 次崩溃记录
        for _ in 0..5 {
            sup.should_restart("test-agent-4").unwrap();
        }
        // 第 6 次应该降级
        let result = sup.should_restart("test-agent-4").unwrap();
        assert!(!result);
        let info = registry.lookup("test-agent-4").unwrap();
        assert_eq!(info.status, AgentStatus::Degraded);
    }
}

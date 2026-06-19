//! 资源配额管理 — 基于 cgroups v2
//!
//! 为每个 Agent 进程创建独立 cgroup，限制 CPU/内存/PID 数量。
//! - CPU：通过 `cpu.max` 限制百分比
//! - 内存：通过 `memory.max` 限制字节数
//! - PID：通过 `pids.max` 限制进程数
//!
//! 非 Linux 平台提供 stub 实现，用于开发/测试。

use crate::agentos::registry::AgentRegistry;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

/// 资源配额配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaConfig {
    /// CPU 配额百分比（0-100，0 表示不限制）
    pub cpu_percent: u32,
    /// 内存上限（MB，0 表示不限制）
    pub memory_mb: u64,
    /// 最大 PID 数量（0 表示不限制）
    pub max_pids: u32,
}

impl Default for QuotaConfig {
    fn default() -> Self {
        Self {
            cpu_percent: 0,
            memory_mb: 0,
            max_pids: 0,
        }
    }
}

impl QuotaConfig {
    /// 创建无限制配额
    pub fn unlimited() -> Self {
        Self::default()
    }

    /// 创建受限配额
    pub fn limited(cpu_percent: u32, memory_mb: u64, max_pids: u32) -> Self {
        Self {
            cpu_percent,
            memory_mb,
            max_pids,
        }
    }

    /// 是否有任何限制
    pub fn has_limits(&self) -> bool {
        self.cpu_percent > 0 || self.memory_mb > 0 || self.max_pids > 0
    }
}

/// 资源使用情况快照
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResourceUsage {
    /// CPU 使用百分比（0-100）
    pub cpu_usage_percent: f64,
    /// 内存使用（MB）
    pub memory_usage_mb: f64,
    /// 内存上限（MB，0 表示无限制）
    pub memory_limit_mb: u64,
    /// 当前 PID 数量
    pub pid_count: u32,
}

/// 配额错误
#[derive(Debug, thiserror::Error)]
pub enum QuotaError {
    #[error("agent '{0}' not found in registry")]
    NotFound(String),
    #[error("cgroup operation failed: {0}")]
    CgroupFailed(String),
    #[error("cgroup already exists for agent '{0}'")]
    AlreadyExists(String),
    #[error("unsupported on this platform")]
    Unsupported,
}

/// 资源配额管理器
///
/// 基于 cgroups v2 为每个 Agent 创建独立 cgroup。
/// cgroup 路径：`/sys/fs/cgroup/eneros/agent-<id>/`
pub struct ResourceQuota {
    registry: Arc<AgentRegistry>,
    /// cgroup 根路径
    cgroup_root: PathBuf,
    /// 已创建的 cgroup（agent_id → 配置）
    configs: parking_lot::RwLock<std::collections::HashMap<String, QuotaConfig>>,
}

impl ResourceQuota {
    /// 创建资源配额管理器
    pub fn new(registry: Arc<AgentRegistry>) -> Self {
        Self {
            registry,
            cgroup_root: PathBuf::from("/sys/fs/cgroup/eneros"),
            configs: parking_lot::RwLock::new(std::collections::HashMap::new()),
        }
    }

    /// 创建并指定 cgroup 根路径（测试用）
    pub fn with_root(registry: Arc<AgentRegistry>, root: PathBuf) -> Self {
        Self {
            registry,
            cgroup_root: root,
            configs: parking_lot::RwLock::new(std::collections::HashMap::new()),
        }
    }

    /// 为 Agent 设置资源配额
    ///
    /// 在 Linux 上：创建 cgroup 目录，写入 cpu.max/memory.max/pids.max
    /// 在非 Linux 上：仅记录配置
    pub fn set_quota(
        &self,
        agent_id: &str,
        config: QuotaConfig,
    ) -> Result<(), QuotaError> {
        let info = self
            .registry
            .lookup(agent_id)
            .ok_or_else(|| QuotaError::NotFound(agent_id.to_string()))?;

        // 检查是否已存在
        {
            let configs = self.configs.read();
            if configs.contains_key(agent_id) {
                return Err(QuotaError::AlreadyExists(agent_id.to_string()));
            }
        }

        #[cfg(target_os = "linux")]
        {
            self.create_cgroup_linux(agent_id, info.pid, &config)?;
        }

        // 非 Linux 平台：info.pid 仅在 Linux 分支使用，避免未使用警告
        #[cfg(not(target_os = "linux"))]
        {
            let _ = &info;
        }

        // 记录配置
        self.configs
            .write()
            .insert(agent_id.to_string(), config.clone());

        tracing::info!(
            "Set quota for agent '{}': cpu={}%, mem={}MB, pids={}",
            agent_id,
            config.cpu_percent,
            config.memory_mb,
            config.max_pids
        );
        Ok(())
    }

    /// 更新 Agent 资源配额（覆盖已有配置）
    pub fn update_quota(
        &self,
        agent_id: &str,
        config: QuotaConfig,
    ) -> Result<(), QuotaError> {
        self.registry
            .lookup(agent_id)
            .ok_or_else(|| QuotaError::NotFound(agent_id.to_string()))?;

        #[cfg(target_os = "linux")]
        {
            self.update_cgroup_linux(agent_id, &config)?;
        }

        self.configs
            .write()
            .insert(agent_id.to_string(), config);
        Ok(())
    }

    /// 移除 Agent 的 cgroup（Agent 停止时调用）
    pub fn remove_quota(&self, agent_id: &str) -> Result<(), QuotaError> {
        self.registry
            .lookup(agent_id)
            .ok_or_else(|| QuotaError::NotFound(agent_id.to_string()))?;

        #[cfg(target_os = "linux")]
        {
            self.remove_cgroup_linux(agent_id)?;
        }

        self.configs.write().remove(agent_id);
        Ok(())
    }

    /// 查询 Agent 当前资源使用
    pub fn usage(&self, agent_id: &str) -> Result<ResourceUsage, QuotaError> {
        self.registry
            .lookup(agent_id)
            .ok_or_else(|| QuotaError::NotFound(agent_id.to_string()))?;

        let config = self
            .configs
            .read()
            .get(agent_id)
            .cloned()
            .unwrap_or_default();

        #[cfg(target_os = "linux")]
        {
            return self.read_usage_linux(agent_id, &config);
        }

        #[cfg(not(target_os = "linux"))]
        {
            // 非 Linux：返回模拟值
            Ok(ResourceUsage {
                cpu_usage_percent: 0.0,
                memory_usage_mb: 0.0,
                memory_limit_mb: config.memory_mb,
                pid_count: 1,
            })
        }
    }

    /// 获取 Agent 的配额配置
    pub fn get_config(&self, agent_id: &str) -> Option<QuotaConfig> {
        self.configs.read().get(agent_id).cloned()
    }

    /// 列出所有已配置配额的 Agent
    pub fn list_configured(&self) -> Vec<String> {
        self.configs.read().keys().cloned().collect()
    }

    /// cgroup 路径
    fn cgroup_path(&self, agent_id: &str) -> PathBuf {
        self.cgroup_root.join(format!("agent-{}", agent_id))
    }

    /// Linux: 创建 cgroup 并写入限制
    #[cfg(target_os = "linux")]
    fn create_cgroup_linux(
        &self,
        agent_id: &str,
        pid: u32,
        config: &QuotaConfig,
    ) -> Result<(), QuotaError> {
        use std::fs;

        // 确保根 cgroup 存在
        if !self.cgroup_root.exists() {
            fs::create_dir_all(&self.cgroup_root).map_err(|e| {
                QuotaError::CgroupFailed(format!("create root cgroup: {}", e))
            })?;
        }

        let path = self.cgroup_path(agent_id);
        fs::create_dir(&path).map_err(|e| {
            QuotaError::CgroupFailed(format!("create agent cgroup: {}", e))
        })?;

        // 将进程加入 cgroup
        let procs_file = path.join("cgroup.procs");
        fs::write(&procs_file, pid.to_string()).map_err(|e| {
            QuotaError::CgroupFailed(format!("write cgroup.procs: {}", e))
        })?;

        // 写入 CPU 限制
        if config.cpu_percent > 0 {
            // cpu.max 格式："quota period"（微秒）
            // 50% = "50000 100000"
            let quota = config.cpu_percent * 1000;
            let cpu_max = format!("{} {}", quota, 100_000);
            fs::write(path.join("cpu.max"), cpu_max).map_err(|e| {
                QuotaError::CgroupFailed(format!("write cpu.max: {}", e))
            })?;
        }

        // 写入内存限制
        if config.memory_mb > 0 {
            let bytes = config.memory_mb * 1024 * 1024;
            fs::write(path.join("memory.max"), bytes.to_string()).map_err(|e| {
                QuotaError::CgroupFailed(format!("write memory.max: {}", e))
            })?;
        }

        // 写入 PID 限制
        if config.max_pids > 0 {
            fs::write(path.join("pids.max"), config.max_pids.to_string()).map_err(|e| {
                QuotaError::CgroupFailed(format!("write pids.max: {}", e))
            })?;
        }

        Ok(())
    }

    /// Linux: 更新 cgroup 限制
    #[cfg(target_os = "linux")]
    fn update_cgroup_linux(
        &self,
        agent_id: &str,
        config: &QuotaConfig,
    ) -> Result<(), QuotaError> {
        use std::fs;

        let path = self.cgroup_path(agent_id);
        if !path.exists() {
            // 不存在则创建
            let info = self
                .registry
                .lookup(agent_id)
                .ok_or_else(|| QuotaError::NotFound(agent_id.to_string()))?;
            return self.create_cgroup_linux(agent_id, info.pid, config);
        }

        // CPU
        if config.cpu_percent > 0 {
            let quota = config.cpu_percent * 1000;
            let cpu_max = format!("{} {}", quota, 100_000);
            let _ = fs::write(path.join("cpu.max"), cpu_max);
        }

        // 内存
        if config.memory_mb > 0 {
            let bytes = config.memory_mb * 1024 * 1024;
            let _ = fs::write(path.join("memory.max"), bytes.to_string());
        }

        // PID
        if config.max_pids > 0 {
            let _ = fs::write(path.join("pids.max"), config.max_pids.to_string());
        }

        Ok(())
    }

    /// Linux: 移除 cgroup
    #[cfg(target_os = "linux")]
    fn remove_cgroup_linux(&self, agent_id: &str) -> Result<(), QuotaError> {
        use std::fs;

        let path = self.cgroup_path(agent_id);
        if path.exists() {
            // cgroup 必须无进程才能删除
            // 先尝试移除所有进程（写到 cgroup.kill）
            let _ = fs::write(path.join("cgroup.kill"), "1");
            fs::remove_dir(&path).map_err(|e| {
                QuotaError::CgroupFailed(format!("remove cgroup: {}", e))
            })?;
        }
        Ok(())
    }

    /// Linux: 读取 cgroup 使用统计
    #[cfg(target_os = "linux")]
    fn read_usage_linux(
        &self,
        agent_id: &str,
        config: &QuotaConfig,
    ) -> Result<ResourceUsage, QuotaError> {
        use std::fs;

        let path = self.cgroup_path(agent_id);
        if !path.exists() {
            return Ok(ResourceUsage {
                cpu_usage_percent: 0.0,
                memory_usage_mb: 0.0,
                memory_limit_mb: config.memory_mb,
                pid_count: 0,
            });
        }

        // 读取内存使用
        let memory_usage_mb = fs::read_to_string(path.join("memory.current"))
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .map(|bytes| bytes as f64 / (1024.0 * 1024.0))
            .unwrap_or(0.0);

        // 读取 PID 数量
        let pid_count = fs::read_to_string(path.join("pids.current"))
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(0);

        // CPU 使用率需要两次采样计算，这里返回 0（实时监控由其他模块负责）
        Ok(ResourceUsage {
            cpu_usage_percent: 0.0,
            memory_usage_mb,
            memory_limit_mb: config.memory_mb,
            pid_count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agentos::registry::{AgentInfo, AgentStatus, AgentType};
    use chrono::Utc;

    fn test_registry() -> Arc<AgentRegistry> {
        Arc::new(AgentRegistry::new())
    }

    fn register_agent(reg: &AgentRegistry, id: &str) {
        let info = AgentInfo {
            agent_id: id.to_string(),
            pid: 10000,
            agent_type: AgentType::Dispatch,
            authority: eneros_core::AuthorityLevel::Operator,
            status: AgentStatus::Running,
            started_at: Utc::now(),
            binary: "/bin/test".to_string(),
            crash_count: 0,
        };
        reg.register(info).unwrap();
    }

    #[test]
    fn test_quota_config_default_unlimited() {
        let config = QuotaConfig::default();
        assert!(!config.has_limits());
        assert_eq!(config.cpu_percent, 0);
        assert_eq!(config.memory_mb, 0);
        assert_eq!(config.max_pids, 0);
    }

    #[test]
    fn test_quota_config_limited() {
        let config = QuotaConfig::limited(50, 1024, 100);
        assert!(config.has_limits());
        assert_eq!(config.cpu_percent, 50);
        assert_eq!(config.memory_mb, 1024);
        assert_eq!(config.max_pids, 100);
    }

    #[test]
    fn test_set_quota_not_found() {
        let reg = test_registry();
        let quota = ResourceQuota::new(reg);
        let result = quota.set_quota("nonexistent", QuotaConfig::limited(50, 1024, 100));
        assert!(result.is_err());
    }

    #[test]
    fn test_set_quota_succeeds_non_linux() {
        let reg = test_registry();
        register_agent(&reg, "agent-1");
        let quota = ResourceQuota::new(reg);

        let config = QuotaConfig::limited(50, 1024, 100);
        let result = quota.set_quota("agent-1", config);
        assert!(result.is_ok());

        let stored = quota.get_config("agent-1").unwrap();
        assert_eq!(stored.cpu_percent, 50);
        assert_eq!(stored.memory_mb, 1024);
    }

    #[test]
    fn test_set_quota_duplicate_fails() {
        let reg = test_registry();
        register_agent(&reg, "agent-1");
        let quota = ResourceQuota::new(reg);

        quota
            .set_quota("agent-1", QuotaConfig::limited(50, 1024, 100))
            .unwrap();

        let result = quota.set_quota("agent-1", QuotaConfig::limited(30, 512, 50));
        assert!(result.is_err());
    }

    #[test]
    fn test_update_quota_overwrites() {
        let reg = test_registry();
        register_agent(&reg, "agent-1");
        let quota = ResourceQuota::new(reg);

        quota
            .set_quota("agent-1", QuotaConfig::limited(50, 1024, 100))
            .unwrap();

        let result = quota.update_quota("agent-1", QuotaConfig::limited(30, 512, 50));
        assert!(result.is_ok());

        let stored = quota.get_config("agent-1").unwrap();
        assert_eq!(stored.cpu_percent, 30);
        assert_eq!(stored.memory_mb, 512);
    }

    #[test]
    fn test_remove_quota() {
        let reg = test_registry();
        register_agent(&reg, "agent-1");
        let quota = ResourceQuota::new(reg);

        quota
            .set_quota("agent-1", QuotaConfig::limited(50, 1024, 100))
            .unwrap();
        assert!(quota.get_config("agent-1").is_some());

        quota.remove_quota("agent-1").unwrap();
        assert!(quota.get_config("agent-1").is_none());
    }

    #[test]
    fn test_usage_non_linux_returns_simulated() {
        let reg = test_registry();
        register_agent(&reg, "agent-1");
        let quota = ResourceQuota::new(reg);

        quota
            .set_quota("agent-1", QuotaConfig::limited(50, 1024, 100))
            .unwrap();

        let usage = quota.usage("agent-1").unwrap();
        assert_eq!(usage.memory_limit_mb, 1024);
        assert_eq!(usage.pid_count, 1); // 模拟值
    }

    #[test]
    fn test_list_configured() {
        let reg = test_registry();
        register_agent(&reg, "agent-1");
        register_agent(&reg, "agent-2");
        let quota = ResourceQuota::new(reg);

        quota
            .set_quota("agent-1", QuotaConfig::limited(50, 1024, 100))
            .unwrap();
        quota
            .set_quota("agent-2", QuotaConfig::limited(30, 512, 50))
            .unwrap();

        let mut list = quota.list_configured();
        list.sort();
        assert_eq!(list, vec!["agent-1", "agent-2"]);
    }

    #[test]
    fn test_with_custom_root() {
        let reg = test_registry();
        register_agent(&reg, "agent-1");
        let quota = ResourceQuota::with_root(reg, PathBuf::from("/tmp/test-cgroup"));

        assert!(quota.set_quota("agent-1", QuotaConfig::unlimited()).is_ok());
    }
}

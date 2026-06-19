//! 权限强制器 — Linux capabilities + seccomp
//!
//! 将 AuthorityLevel 映射为 Linux capabilities，OS 级别强制权限隔离。
//! - Observer：无 capabilities（只读）
//! - Operator：CAP_NET_BIND_SERVICE
//! - Supervisor：CAP_NET_BIND_SERVICE + CAP_SYS_ADMIN
//! - Emergency：CAP_NET_BIND_SERVICE + CAP_SYS_ADMIN + CAP_SYS_RAWIO
//!
//! 非 Linux 平台提供 stub 实现，用于开发/测试。

use crate::agentos::registry::AgentRegistry;
use eneros_core::AuthorityLevel;
use std::collections::HashMap;
use std::sync::Arc;

/// Linux capability 标识（数值与 linux/capability.h 对齐）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Capability {
    /// 绑定 1024 以下端口
    NetBindService = 10,
    /// 系统管理（挂载、交换等）
    SysAdmin = 21,
    /// 原始 I/O（访问 /dev/mem、/dev/kmem、串口）
    SysRawio = 17,
    /// 设置系统时间
    SysTime = 25,
    /// 网络管理（接口配置）
    NetAdmin = 12,
}

impl Capability {
    /// 转为 libc cap_flag_value 对应的 bit 位
    pub fn as_bit(self) -> u32 {
        1u32 << (self as u32)
    }
}

/// Capability 集合
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct CapabilitySet {
    caps: Vec<Capability>,
}

impl CapabilitySet {
    pub fn new() -> Self {
        Self { caps: Vec::new() }
    }

    pub fn from_iter<I: IntoIterator<Item = Capability>>(iter: I) -> Self {
        Self {
            caps: iter.into_iter().collect(),
        }
    }

    pub fn add(&mut self, cap: Capability) {
        if !self.caps.contains(&cap) {
            self.caps.push(cap);
        }
    }

    pub fn contains(&self, cap: Capability) -> bool {
        self.caps.contains(&cap)
    }

    pub fn is_empty(&self) -> bool {
        self.caps.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Capability> {
        self.caps.iter()
    }
}

/// AuthorityLevel → CapabilitySet 映射
///
/// 这是 AgentOS 权限模型的核心：应用层 AuthorityLevel 转换为 OS 级 capabilities。
pub fn authority_to_capabilities(level: AuthorityLevel) -> CapabilitySet {
    match level {
        AuthorityLevel::Observer => CapabilitySet::new(), // 只读，无 capabilities
        AuthorityLevel::Operator => CapabilitySet::from_iter([Capability::NetBindService]),
        AuthorityLevel::Supervisor => CapabilitySet::from_iter([
            Capability::NetBindService,
            Capability::SysAdmin,
        ]),
        AuthorityLevel::Emergency => CapabilitySet::from_iter([
            Capability::NetBindService,
            Capability::SysAdmin,
            Capability::SysRawio,
        ]),
    }
}

/// 权限强制器错误
#[derive(Debug, thiserror::Error)]
pub enum AuthorityError {
    #[error("agent '{0}' not found in registry")]
    NotFound(String),
    #[error("capability operation failed: {0}")]
    CapabilityFailed(String),
    #[error("permission denied: agent '{0}' lacks capability {1:?}")]
    PermissionDenied(String, Capability),
    #[error("unsupported on this platform")]
    Unsupported,
}

/// Agent 动作类型（用于权限检查）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentAction {
    /// 读取设备状态
    ReadDevice,
    /// 执行控制命令（写设备）
    ExecuteCommand,
    /// 高风险操作（切负荷、解列）
    HighRiskOperation,
    /// 紧急操作（绕过非关键安全检查）
    EmergencyOverride,
    /// 访问原始 I/O（串口、/dev/mem）
    RawIoAccess,
    /// 网络管理（接口配置）
    NetworkAdmin,
}

impl AgentAction {
    /// 返回执行此动作所需的最低 AuthorityLevel
    pub fn required_authority(&self) -> AuthorityLevel {
        match self {
            AgentAction::ReadDevice => AuthorityLevel::Observer,
            AgentAction::ExecuteCommand => AuthorityLevel::Operator,
            AgentAction::HighRiskOperation => AuthorityLevel::Supervisor,
            AgentAction::EmergencyOverride => AuthorityLevel::Emergency,
            AgentAction::RawIoAccess => AuthorityLevel::Emergency,
            AgentAction::NetworkAdmin => AuthorityLevel::Supervisor,
        }
    }

    /// 返回执行此动作所需的 capabilities
    pub fn required_capabilities(&self) -> CapabilitySet {
        match self {
            AgentAction::ReadDevice => CapabilitySet::new(),
            AgentAction::ExecuteCommand => {
                CapabilitySet::from_iter([Capability::NetBindService])
            }
            AgentAction::HighRiskOperation => {
                CapabilitySet::from_iter([Capability::NetBindService, Capability::SysAdmin])
            }
            AgentAction::EmergencyOverride => CapabilitySet::from_iter([
                Capability::NetBindService,
                Capability::SysAdmin,
                Capability::SysRawio,
            ]),
            AgentAction::RawIoAccess => CapabilitySet::from_iter([Capability::SysRawio]),
            AgentAction::NetworkAdmin => CapabilitySet::from_iter([Capability::NetAdmin]),
        }
    }
}

/// 权限强制器
///
/// 基于 AgentRegistry 查询 Agent 的 AuthorityLevel，映射为 capabilities，
/// 并检查 Agent 是否有权执行特定动作。
///
/// 在 Linux 上，spawn 时通过 capset() 真正设置进程 capabilities；
/// 在非 Linux 平台上，仅做应用层检查（用于开发/测试）。
pub struct AuthorityEnforcer {
    registry: Arc<AgentRegistry>,
    /// 已授予的 capabilities 缓存（agent_id → CapabilitySet）
    granted: parking_lot::RwLock<HashMap<String, CapabilitySet>>,
}

impl AuthorityEnforcer {
    /// 创建权限强制器，共享 AgentRegistry
    pub fn new(registry: Arc<AgentRegistry>) -> Self {
        Self {
            registry,
            granted: parking_lot::RwLock::new(HashMap::new()),
        }
    }

    /// 授予 Agent 一组 capabilities
    ///
    /// 在 Linux 上：调用 capset() 设置目标进程 capabilities
    /// 在非 Linux 上：仅记录到缓存
    pub fn grant(
        &self,
        agent_id: &str,
        caps: &CapabilitySet,
    ) -> Result<(), AuthorityError> {
        let info = self
            .registry
            .lookup(agent_id)
            .ok_or_else(|| AuthorityError::NotFound(agent_id.to_string()))?;

        // 验证：请求的 capabilities 不能超过 Agent AuthorityLevel 允许的范围
        let allowed = authority_to_capabilities(info.authority);
        for cap in caps.iter() {
            if !allowed.contains(*cap) {
                return Err(AuthorityError::PermissionDenied(
                    agent_id.to_string(),
                    *cap,
                ));
            }
        }

        // Linux: 真正调用 capset
        #[cfg(target_os = "linux")]
        {
            self.capset_linux(info.pid, caps)?;
        }

        // 记录到缓存
        let mut granted = self.granted.write();
        let entry = granted.entry(agent_id.to_string()).or_default();
        for cap in caps.iter() {
            entry.add(*cap);
        }

        tracing::info!(
            "Granted {} capabilities to agent '{}'",
            caps.caps.len(),
            agent_id
        );
        Ok(())
    }

    /// 撤销 Agent 的 capabilities
    pub fn revoke(&self, agent_id: &str, caps: &CapabilitySet) -> Result<(), AuthorityError> {
        self.registry
            .lookup(agent_id)
            .ok_or_else(|| AuthorityError::NotFound(agent_id.to_string()))?;

        #[cfg(target_os = "linux")]
        {
            // Linux: 从进程有效集中移除指定 capabilities
            // 注意：完全移除需要 capset() with inheritable/effective 清零
            self.capdrop_linux(agent_id, caps)?;
        }

        let mut granted = self.granted.write();
        if let Some(entry) = granted.get_mut(agent_id) {
            entry.caps.retain(|c| !caps.contains(*c));
        }
        Ok(())
    }

    /// 检查 Agent 是否有权执行某动作
    pub fn check(&self, agent_id: &str, action: &AgentAction) -> bool {
        let info = match self.registry.lookup(agent_id) {
            Some(i) => i,
            None => return false,
        };

        // 1. 检查 AuthorityLevel
        if info.authority < action.required_authority() {
            return false;
        }

        // 2. 检查 capabilities（缓存）
        let required_caps = action.required_capabilities();
        if required_caps.is_empty() {
            return true;
        }

        let granted = self.granted.read();
        match granted.get(agent_id) {
            Some(agent_caps) => {
                for cap in required_caps.iter() {
                    if !agent_caps.contains(*cap) {
                        return false;
                    }
                }
                true
            }
            None => false,
        }
    }

    /// 获取 Agent 当前持有的 capabilities
    pub fn current_capabilities(&self, agent_id: &str) -> CapabilitySet {
        self.granted
            .read()
            .get(agent_id)
            .cloned()
            .unwrap_or_default()
    }

    /// 根据 AuthorityLevel 自动授予对应 capabilities
    ///
    /// 在 Agent spawn 后调用，自动设置 OS 级权限。
    pub fn auto_grant(&self, agent_id: &str) -> Result<(), AuthorityError> {
        let info = self
            .registry
            .lookup(agent_id)
            .ok_or_else(|| AuthorityError::NotFound(agent_id.to_string()))?;

        let caps = authority_to_capabilities(info.authority);
        if caps.is_empty() {
            return Ok(());
        }
        self.grant(agent_id, &caps)
    }

    /// Linux: 调用 capset 设置进程 capabilities
    #[cfg(target_os = "linux")]
    fn capset_linux(&self, pid: u32, caps: &CapabilitySet) -> Result<(), AuthorityError> {
        // 构建 permitted + effective 位图
        let mut effective: u32 = 0;
        let mut permitted: u32 = 0;
        let mut inheritable: u32 = 0;
        for cap in caps.iter() {
            effective |= cap.as_bit();
            permitted |= cap.as_bit();
            inheritable |= cap.as_bit();
        }

        // libcap 调用：capset() 系统调用
        // 这里使用 libc 直接调用，避免引入 libcap 依赖
        // 注意：__user_cap_data_struct 在不同架构布局不同，这里使用最常见布局
        #[repr(C)]
        struct UserCapHeader {
            version: u32,
            pid: i32,
        }

        #[repr(C)]
        struct UserCapData {
            effective: u32,
            permitted: u32,
            inheritable: u32,
        }

        let header = UserCapHeader {
            version: 0x20080522, // _LINUX_CAPABILITY_VERSION_3
            pid: pid as i32,
        };
        let data = [UserCapData {
            effective,
            permitted,
            inheritable,
        }];

        let ret = unsafe {
            libc::syscall(
                libc::SYS_capset,
                &header as *const UserCapHeader,
                data.as_ptr() as *const UserCapData,
            )
        };
        if ret != 0 {
            return Err(AuthorityError::CapabilityFailed(format!(
                "capset failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(())
    }

    /// Linux: 从进程 capabilities 中移除指定的
    #[cfg(target_os = "linux")]
    fn capdrop_linux(&self, agent_id: &str, caps: &CapabilitySet) -> Result<(), AuthorityError> {
        let info = self
            .registry
            .lookup(agent_id)
            .ok_or_else(|| AuthorityError::NotFound(agent_id.to_string()))?;

        // 获取当前 caps，减去要移除的，再 capset
        let current = self.current_capabilities(agent_id);
        let mut remaining = CapabilitySet::new();
        for cap in current.iter() {
            if !caps.contains(*cap) {
                remaining.add(*cap);
            }
        }
        self.capset_linux(info.pid, &remaining)
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

    fn register_agent(reg: &AgentRegistry, id: &str, authority: AuthorityLevel) {
        let info = AgentInfo {
            agent_id: id.to_string(),
            pid: 10000,
            agent_type: AgentType::Dispatch,
            authority,
            status: AgentStatus::Running,
            started_at: Utc::now(),
            binary: "/bin/test".to_string(),
            crash_count: 0,
        };
        reg.register(info).unwrap();
    }

    #[test]
    fn test_authority_to_capabilities_observer() {
        let caps = authority_to_capabilities(AuthorityLevel::Observer);
        assert!(caps.is_empty());
    }

    #[test]
    fn test_authority_to_capabilities_operator() {
        let caps = authority_to_capabilities(AuthorityLevel::Operator);
        assert!(caps.contains(Capability::NetBindService));
        assert!(!caps.contains(Capability::SysAdmin));
    }

    #[test]
    fn test_authority_to_capabilities_supervisor() {
        let caps = authority_to_capabilities(AuthorityLevel::Supervisor);
        assert!(caps.contains(Capability::NetBindService));
        assert!(caps.contains(Capability::SysAdmin));
        assert!(!caps.contains(Capability::SysRawio));
    }

    #[test]
    fn test_authority_to_capabilities_emergency() {
        let caps = authority_to_capabilities(AuthorityLevel::Emergency);
        assert!(caps.contains(Capability::NetBindService));
        assert!(caps.contains(Capability::SysAdmin));
        assert!(caps.contains(Capability::SysRawio));
    }

    #[test]
    fn test_grant_respects_authority_level() {
        let reg = test_registry();
        register_agent(&reg, "observer-agent", AuthorityLevel::Observer);
        let enforcer = AuthorityEnforcer::new(reg);

        // Observer 不能被授予任何 capability
        let caps = CapabilitySet::from_iter([Capability::NetBindService]);
        let result = enforcer.grant("observer-agent", &caps);
        assert!(result.is_err());
    }

    #[test]
    fn test_grant_operator_succeeds() {
        let reg = test_registry();
        register_agent(&reg, "op-agent", AuthorityLevel::Operator);
        let enforcer = AuthorityEnforcer::new(reg);

        let caps = CapabilitySet::from_iter([Capability::NetBindService]);
        // 非 Linux 平台：grant 仅记录到缓存，不会失败
        let result = enforcer.grant("op-agent", &caps);
        assert!(result.is_ok());

        let current = enforcer.current_capabilities("op-agent");
        assert!(current.contains(Capability::NetBindService));
    }

    #[test]
    fn test_check_permission_observer_read() {
        let reg = test_registry();
        register_agent(&reg, "obs", AuthorityLevel::Observer);
        let enforcer = AuthorityEnforcer::new(reg);

        // Observer 可以读
        assert!(enforcer.check("obs", &AgentAction::ReadDevice));
        // Observer 不能执行命令
        assert!(!enforcer.check("obs", &AgentAction::ExecuteCommand));
    }

    #[test]
    fn test_check_permission_operator_with_caps() {
        let reg = test_registry();
        register_agent(&reg, "op", AuthorityLevel::Operator);
        let enforcer = AuthorityEnforcer::new(reg);

        // 先授予 capabilities
        enforcer
            .grant("op", &CapabilitySet::from_iter([Capability::NetBindService]))
            .unwrap();

        // 现在 Operator 可以执行命令
        assert!(enforcer.check("op", &AgentAction::ExecuteCommand));
        // 但不能执行高风险操作
        assert!(!enforcer.check("op", &AgentAction::HighRiskOperation));
    }

    #[test]
    fn test_check_permission_not_found() {
        let reg = test_registry();
        let enforcer = AuthorityEnforcer::new(reg);
        assert!(!enforcer.check("nonexistent", &AgentAction::ReadDevice));
    }

    #[test]
    fn test_auto_grant_observer_no_caps() {
        let reg = test_registry();
        register_agent(&reg, "obs", AuthorityLevel::Observer);
        let enforcer = AuthorityEnforcer::new(reg);

        // Observer auto_grant 应该是 no-op（无 caps）
        let result = enforcer.auto_grant("obs");
        assert!(result.is_ok());
        assert!(enforcer.current_capabilities("obs").is_empty());
    }

    #[test]
    fn test_auto_grant_supervisor() {
        let reg = test_registry();
        register_agent(&reg, "sup", AuthorityLevel::Supervisor);
        let enforcer = AuthorityEnforcer::new(reg);

        enforcer.auto_grant("sup").unwrap();
        let caps = enforcer.current_capabilities("sup");
        assert!(caps.contains(Capability::NetBindService));
        assert!(caps.contains(Capability::SysAdmin));
    }

    #[test]
    fn test_revoke_capability() {
        let reg = test_registry();
        register_agent(&reg, "sup", AuthorityLevel::Supervisor);
        let enforcer = AuthorityEnforcer::new(reg);

        enforcer.auto_grant("sup").unwrap();
        assert!(enforcer.current_capabilities("sup").contains(Capability::SysAdmin));

        enforcer
            .revoke("sup", &CapabilitySet::from_iter([Capability::SysAdmin]))
            .unwrap();
        assert!(!enforcer.current_capabilities("sup").contains(Capability::SysAdmin));
        assert!(enforcer.current_capabilities("sup").contains(Capability::NetBindService));
    }

    #[test]
    fn test_capability_set_operations() {
        let mut caps = CapabilitySet::new();
        assert!(caps.is_empty());

        caps.add(Capability::NetBindService);
        caps.add(Capability::SysAdmin);
        caps.add(Capability::NetBindService); // 去重
        assert_eq!(caps.caps.len(), 2);

        assert!(caps.contains(Capability::NetBindService));
        assert!(caps.contains(Capability::SysAdmin));
        assert!(!caps.contains(Capability::SysRawio));
    }

    #[test]
    fn test_agent_action_required_authority() {
        assert_eq!(
            AgentAction::ReadDevice.required_authority(),
            AuthorityLevel::Observer
        );
        assert_eq!(
            AgentAction::ExecuteCommand.required_authority(),
            AuthorityLevel::Operator
        );
        assert_eq!(
            AgentAction::HighRiskOperation.required_authority(),
            AuthorityLevel::Supervisor
        );
        assert_eq!(
            AgentAction::EmergencyOverride.required_authority(),
            AuthorityLevel::Emergency
        );
    }
}

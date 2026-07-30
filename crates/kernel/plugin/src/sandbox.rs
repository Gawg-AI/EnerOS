//! 插件沙箱隔离模块 (v0.166.0).
//!
//! 为每个插件提供独立的执行环境：
//! - 内存隔离：独立内存区域，越界访问被拦截
//! - 能力限制：仅授予声明的权限子集
//! - 资源配额：IPC 消息数/CPU 时间片限制
//!
//! 插件崩溃不影响宿主系统（蓝图 §9.4）。

use alloc::vec::Vec;

use crate::types::Permission;
use crate::PluginError;

/// 内存区域描述.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryRegion {
    /// 区域起始地址（虚拟）.
    pub base: usize,
    /// 区域大小（字节）.
    pub size: usize,
}

impl MemoryRegion {
    /// 创建内存区域.
    pub fn new(base: usize, size: usize) -> Self {
        Self { base, size }
    }

    /// 检查地址是否在区域内.
    pub fn contains(&self, addr: usize) -> bool {
        addr >= self.base && addr < self.base.saturating_add(self.size)
    }

    /// 检查 [addr, addr+len) 是否完全在区域内.
    pub fn contains_range(&self, addr: usize, len: usize) -> bool {
        let end = addr.saturating_add(len);
        addr >= self.base && end <= self.base.saturating_add(self.size)
    }
}

/// IPC 配额.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpcQuota {
    /// 最大消息数/秒.
    pub max_messages_per_sec: u32,
    /// 最大消息大小（字节）.
    pub max_message_size: usize,
    /// 当前周期已发送消息数.
    pub sent_this_period: u32,
}

impl IpcQuota {
    /// 创建 IPC 配额.
    pub fn new(max_messages_per_sec: u32, max_message_size: usize) -> Self {
        Self {
            max_messages_per_sec,
            max_message_size,
            sent_this_period: 0,
        }
    }

    /// 尝试消耗一个 IPC 消息配额.
    pub fn try_consume(&mut self, msg_size: usize) -> Result<(), PluginError> {
        if msg_size > self.max_message_size {
            return Err(PluginError::SandboxViolation);
        }
        if self.sent_this_period >= self.max_messages_per_sec {
            return Err(PluginError::QuotaExceeded);
        }
        self.sent_this_period += 1;
        Ok(())
    }

    /// 重置周期计数.
    pub fn reset_period(&mut self) {
        self.sent_this_period = 0;
    }
}

/// CPU 配额.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuQuota {
    /// 最大 CPU 时间片（微秒/调度周期）.
    pub max_time_slice_us: u64,
    /// 当前周期已消耗时间（微秒）.
    pub consumed_us: u64,
}

impl CpuQuota {
    /// 创建 CPU 配额.
    pub fn new(max_time_slice_us: u64) -> Self {
        Self {
            max_time_slice_us,
            consumed_us: 0,
        }
    }

    /// 记录 CPU 时间消耗.
    pub fn consume(&mut self, us: u64) -> Result<(), PluginError> {
        self.consumed_us = self.consumed_us.saturating_add(us);
        if self.consumed_us > self.max_time_slice_us {
            Err(PluginError::QuotaExceeded)
        } else {
            Ok(())
        }
    }

    /// 重置周期计数.
    pub fn reset_period(&mut self) {
        self.consumed_us = 0;
    }

    /// 剩余配额（微秒）.
    pub fn remaining(&self) -> u64 {
        self.max_time_slice_us.saturating_sub(self.consumed_us)
    }
}

/// 能力（Capability）— 沙箱内可执行的操作.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    /// 发送 IPC 消息.
    SendIpc,
    /// 接收 IPC 消息.
    RecvIpc,
    /// 读取文件.
    ReadFile,
    /// 写入文件.
    WriteFile,
    /// 访问网络设备.
    NetworkAccess,
    /// 访问硬件设备.
    DeviceAccess,
    /// 使用定时器.
    UseTimer,
}

impl Capability {
    /// 从权限映射到能力集.
    pub fn from_permission(perm: &Permission) -> Vec<Capability> {
        match perm {
            Permission::Network => alloc::vec![Capability::NetworkAccess],
            Permission::FileRead => alloc::vec![Capability::ReadFile],
            Permission::FileWrite => alloc::vec![Capability::WriteFile],
            Permission::DeviceAccess => alloc::vec![Capability::DeviceAccess],
            Permission::Ipc => alloc::vec![Capability::SendIpc, Capability::RecvIpc],
            Permission::Timer => alloc::vec![Capability::UseTimer],
            Permission::MemoryMap => alloc::vec![],
        }
    }
}

/// 默认 IPC 配额：100 消息/秒，最大 4KB/消息.
const DEFAULT_IPC_MAX_MSGS: u32 = 100;
const DEFAULT_IPC_MAX_SIZE: usize = 4096;

/// 默认 CPU 配额：10ms/调度周期.
const DEFAULT_CPU_QUOTA_US: u64 = 10_000;

/// 默认沙箱内存大小：1MB.
const DEFAULT_SANDBOX_MEMORY: usize = 1024 * 1024;

/// 插件沙箱 — 隔离执行环境.
///
/// 每个插件运行在独立沙箱中，具有：
/// - 独立内存区域（越界检测）
/// - 有限能力集（仅声明权限）
/// - IPC/CPU 配额（防资源耗尽）
#[derive(Debug, Clone)]
pub struct PluginSandbox {
    /// 独立内存区域.
    pub memory_region: MemoryRegion,
    /// 有限能力集.
    pub capabilities: Vec<Capability>,
    /// IPC 配额.
    pub ipc_quota: IpcQuota,
    /// CPU 配额.
    pub cpu_quota: CpuQuota,
    /// 沙箱是否活跃.
    pub active: bool,
}

impl PluginSandbox {
    /// 根据权限列表创建沙箱.
    ///
    /// 自动分配默认内存区域、IPC/CPU 配额，并将权限映射为能力集。
    pub fn new(permissions: &[Permission]) -> Result<Self, PluginError> {
        // 将权限映射为能力
        let mut capabilities = Vec::new();
        for perm in permissions {
            let caps = Capability::from_permission(perm);
            for cap in caps {
                if !capabilities.contains(&cap) {
                    capabilities.push(cap);
                }
            }
        }

        Ok(Self {
            memory_region: MemoryRegion::new(0, DEFAULT_SANDBOX_MEMORY),
            capabilities,
            ipc_quota: IpcQuota::new(DEFAULT_IPC_MAX_MSGS, DEFAULT_IPC_MAX_SIZE),
            cpu_quota: CpuQuota::new(DEFAULT_CPU_QUOTA_US),
            active: true,
        })
    }

    /// 使用自定义配额创建沙箱.
    pub fn with_quotas(
        permissions: &[Permission],
        memory_size: usize,
        ipc_quota: IpcQuota,
        cpu_quota: CpuQuota,
    ) -> Result<Self, PluginError> {
        let mut sandbox = Self::new(permissions)?;
        sandbox.memory_region = MemoryRegion::new(0, memory_size);
        sandbox.ipc_quota = ipc_quota;
        sandbox.cpu_quota = cpu_quota;
        Ok(sandbox)
    }

    /// 检查沙箱是否拥有指定能力.
    pub fn has_capability(&self, cap: &Capability) -> bool {
        self.capabilities.contains(cap)
    }

    /// 检查内存访问是否合法.
    pub fn check_memory_access(&self, addr: usize, len: usize) -> Result<(), PluginError> {
        if !self.active {
            return Err(PluginError::SandboxInactive);
        }
        if !self.memory_region.contains_range(addr, len) {
            return Err(PluginError::SandboxViolation);
        }
        Ok(())
    }

    /// 尝试执行 IPC 发送（消耗配额）.
    pub fn try_ipc_send(&mut self, msg_size: usize) -> Result<(), PluginError> {
        if !self.active {
            return Err(PluginError::SandboxInactive);
        }
        if !self.has_capability(&Capability::SendIpc) {
            return Err(PluginError::PermissionDenied);
        }
        self.ipc_quota.try_consume(msg_size)
    }

    /// 记录 CPU 时间消耗.
    pub fn consume_cpu(&mut self, us: u64) -> Result<(), PluginError> {
        if !self.active {
            return Err(PluginError::SandboxInactive);
        }
        self.cpu_quota.consume(us)
    }

    /// 停用沙箱（插件卸载/崩溃时调用）.
    pub fn deactivate(&mut self) {
        self.active = false;
    }

    /// 重置配额（新调度周期开始时调用）.
    pub fn reset_quotas(&mut self) {
        self.ipc_quota.reset_period();
        self.cpu_quota.reset_period();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_region_contains() {
        let region = MemoryRegion::new(0x1000, 0x1000);
        assert!(region.contains(0x1000));
        assert!(region.contains(0x1FFF));
        assert!(!region.contains(0x2000));
        assert!(!region.contains(0x0FFF));
    }

    #[test]
    fn test_memory_region_contains_range() {
        let region = MemoryRegion::new(0x1000, 0x1000);
        assert!(region.contains_range(0x1000, 0x100));
        assert!(region.contains_range(0x1F00, 0x100));
        assert!(!region.contains_range(0x1F00, 0x200)); // 越界
    }

    #[test]
    fn test_ipc_quota() {
        let mut quota = IpcQuota::new(2, 1024);
        assert!(quota.try_consume(512).is_ok());
        assert!(quota.try_consume(512).is_ok());
        assert_eq!(
            quota.try_consume(512).unwrap_err(),
            PluginError::QuotaExceeded
        );
        // 超大消息
        quota.reset_period();
        assert_eq!(
            quota.try_consume(2048).unwrap_err(),
            PluginError::SandboxViolation
        );
    }

    #[test]
    fn test_cpu_quota() {
        let mut quota = CpuQuota::new(10_000);
        assert!(quota.consume(5000).is_ok());
        assert_eq!(quota.remaining(), 5000);
        assert!(quota.consume(5000).is_ok());
        assert_eq!(quota.consume(1).unwrap_err(), PluginError::QuotaExceeded);
        quota.reset_period();
        assert_eq!(quota.remaining(), 10_000);
    }

    #[test]
    fn test_sandbox_creation() {
        let perms = alloc::vec![Permission::Ipc, Permission::Network];
        let sandbox = PluginSandbox::new(&perms).unwrap();
        assert!(sandbox.active);
        assert!(sandbox.has_capability(&Capability::SendIpc));
        assert!(sandbox.has_capability(&Capability::RecvIpc));
        assert!(sandbox.has_capability(&Capability::NetworkAccess));
        assert!(!sandbox.has_capability(&Capability::ReadFile));
    }

    #[test]
    fn test_sandbox_memory_violation() {
        let sandbox = PluginSandbox::new(&[]).unwrap();
        // 在默认 1MB 范围内
        assert!(sandbox.check_memory_access(0, 1024).is_ok());
        // 越界
        assert_eq!(
            sandbox
                .check_memory_access(DEFAULT_SANDBOX_MEMORY, 1)
                .unwrap_err(),
            PluginError::SandboxViolation
        );
    }

    #[test]
    fn test_sandbox_ipc_without_permission() {
        let mut sandbox = PluginSandbox::new(&[]).unwrap();
        // 无 IPC 权限
        assert_eq!(
            sandbox.try_ipc_send(64).unwrap_err(),
            PluginError::PermissionDenied
        );
    }

    #[test]
    fn test_sandbox_deactivation() {
        let mut sandbox = PluginSandbox::new(&[Permission::Ipc]).unwrap();
        sandbox.deactivate();
        assert!(!sandbox.active);
        assert_eq!(
            sandbox.check_memory_access(0, 1).unwrap_err(),
            PluginError::SandboxInactive
        );
    }

    #[test]
    fn test_capability_from_permission() {
        let caps = Capability::from_permission(&Permission::Ipc);
        assert_eq!(caps.len(), 2);
        assert!(caps.contains(&Capability::SendIpc));
        assert!(caps.contains(&Capability::RecvIpc));

        let caps = Capability::from_permission(&Permission::MemoryMap);
        assert!(caps.is_empty());
    }
}

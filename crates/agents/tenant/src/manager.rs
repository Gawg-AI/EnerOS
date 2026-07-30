//! 租户生命周期管理

use alloc::string::String;
use alloc::vec::Vec;

use crate::types::{QuotaUsage, Tenant, TenantQuota};
use crate::PlatformError;

/// 租户管理 trait
pub trait TenantManager {
    /// 创建租户
    fn create(&mut self, name: &str, quota: TenantQuota) -> Result<Tenant, PlatformError>;
    /// 删除租户（仍有 Agent 绑定时失败）
    fn delete(&mut self, tenant_id: u64) -> Result<(), PlatformError>;
    /// 列出所有租户
    fn list(&self) -> Vec<Tenant>;
    /// 将 Agent 分配给租户
    fn assign_agent(&mut self, tenant_id: u64, agent_id: u64) -> Result<(), PlatformError>;
    /// 查询租户当前资源使用
    fn quota_used(&self, tenant_id: u64) -> Result<QuotaUsage, PlatformError>;
}

/// 内存态租户存储
#[derive(Debug, Clone)]
pub struct TenantStore {
    /// 所有租户
    pub tenants: Vec<Tenant>,
    /// (agent_id, tenant_id) 分配关系
    pub assignments: Vec<(u64, u64)>,
    /// (tenant_id, QuotaUsage) 使用快照
    pub usage: Vec<(u64, QuotaUsage)>,
    /// 下一个可用租户 ID
    pub next_id: u64,
}

impl TenantStore {
    /// 创建空的租户存储
    pub fn new() -> Self {
        Self {
            tenants: Vec::new(),
            assignments: Vec::new(),
            usage: Vec::new(),
            next_id: 1,
        }
    }

    fn find_tenant(&self, tenant_id: u64) -> Option<&Tenant> {
        self.tenants.iter().find(|t| t.tenant_id == tenant_id)
    }

    fn agent_count(&self, tenant_id: u64) -> u32 {
        self.assignments
            .iter()
            .filter(|(_, tid)| *tid == tenant_id)
            .count() as u32
    }
}

impl Default for TenantStore {
    fn default() -> Self {
        Self::new()
    }
}

impl TenantManager for TenantStore {
    fn create(&mut self, name: &str, quota: TenantQuota) -> Result<Tenant, PlatformError> {
        if quota.max_cpu_pct > 100.0 || quota.max_agents == 0 {
            return Err(PlatformError::InvalidQuota);
        }
        let tenant = Tenant {
            tenant_id: self.next_id,
            name: String::from(name),
            created_at: 0,
            quota,
        };
        self.next_id += 1;
        self.tenants.push(tenant.clone());
        self.usage.push((
            tenant.tenant_id,
            QuotaUsage {
                cpu_pct: 0.0,
                mem_kb: 0,
                storage_kb: 0,
                agents: 0,
            },
        ));
        Ok(tenant)
    }

    fn delete(&mut self, tenant_id: u64) -> Result<(), PlatformError> {
        if self.find_tenant(tenant_id).is_none() {
            return Err(PlatformError::TenantNotFound);
        }
        if self.agent_count(tenant_id) > 0 {
            return Err(PlatformError::TenantBusy);
        }
        self.tenants.retain(|t| t.tenant_id != tenant_id);
        self.usage.retain(|(tid, _)| *tid != tenant_id);
        Ok(())
    }

    fn list(&self) -> Vec<Tenant> {
        self.tenants.clone()
    }

    fn assign_agent(&mut self, tenant_id: u64, agent_id: u64) -> Result<(), PlatformError> {
        let tenant = self
            .find_tenant(tenant_id)
            .ok_or(PlatformError::TenantNotFound)?;
        // 检查 agent 是否已分配
        if self.assignments.iter().any(|(aid, _)| *aid == agent_id) {
            return Err(PlatformError::AgentAlreadyAssigned);
        }
        // 检查配额
        let count = self.agent_count(tenant_id);
        if count >= tenant.quota.max_agents {
            return Err(PlatformError::QuotaExceeded);
        }
        self.assignments.push((agent_id, tenant_id));
        // 更新 usage
        if let Some((_, u)) = self.usage.iter_mut().find(|(tid, _)| *tid == tenant_id) {
            u.agents += 1;
        }
        Ok(())
    }

    fn quota_used(&self, tenant_id: u64) -> Result<QuotaUsage, PlatformError> {
        if self.find_tenant(tenant_id).is_none() {
            return Err(PlatformError::TenantNotFound);
        }
        self.usage
            .iter()
            .find(|(tid, _)| *tid == tenant_id)
            .map(|(_, u)| u.clone())
            .ok_or(PlatformError::TenantNotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_quota() -> TenantQuota {
        TenantQuota {
            max_cpu_pct: 50.0,
            max_mem_kb: 1024,
            max_storage_kb: 4096,
            max_agents: 2,
            max_bandwidth_kbps: 1000,
        }
    }

    #[test]
    fn test_create_tenant() {
        let mut store = TenantStore::new();
        let t = store.create("alpha", test_quota()).unwrap();
        assert_eq!(t.tenant_id, 1);
        assert_eq!(t.name, "alpha");
    }

    #[test]
    fn test_create_invalid_quota() {
        let mut store = TenantStore::new();
        let bad = TenantQuota {
            max_cpu_pct: 200.0,
            max_mem_kb: 1024,
            max_storage_kb: 4096,
            max_agents: 2,
            max_bandwidth_kbps: 1000,
        };
        assert_eq!(store.create("bad", bad), Err(PlatformError::InvalidQuota));
    }

    #[test]
    fn test_delete_tenant() {
        let mut store = TenantStore::new();
        let t = store.create("alpha", test_quota()).unwrap();
        assert!(store.delete(t.tenant_id).is_ok());
        assert!(store.list().is_empty());
    }

    #[test]
    fn test_delete_not_found() {
        let mut store = TenantStore::new();
        assert_eq!(store.delete(999), Err(PlatformError::TenantNotFound));
    }

    #[test]
    fn test_delete_busy() {
        let mut store = TenantStore::new();
        let t = store.create("alpha", test_quota()).unwrap();
        store.assign_agent(t.tenant_id, 100).unwrap();
        assert_eq!(store.delete(t.tenant_id), Err(PlatformError::TenantBusy));
    }

    #[test]
    fn test_list() {
        let mut store = TenantStore::new();
        store.create("a", test_quota()).unwrap();
        store.create("b", test_quota()).unwrap();
        assert_eq!(store.list().len(), 2);
    }

    #[test]
    fn test_assign_agent() {
        let mut store = TenantStore::new();
        let t = store.create("alpha", test_quota()).unwrap();
        assert!(store.assign_agent(t.tenant_id, 10).is_ok());
        let usage = store.quota_used(t.tenant_id).unwrap();
        assert_eq!(usage.agents, 1);
    }

    #[test]
    fn test_assign_quota_exceeded() {
        let mut store = TenantStore::new();
        let t = store.create("alpha", test_quota()).unwrap();
        store.assign_agent(t.tenant_id, 10).unwrap();
        store.assign_agent(t.tenant_id, 11).unwrap();
        // max_agents = 2, third should fail
        assert_eq!(
            store.assign_agent(t.tenant_id, 12),
            Err(PlatformError::QuotaExceeded)
        );
    }

    #[test]
    fn test_assign_already_assigned() {
        let mut store = TenantStore::new();
        let t = store.create("alpha", test_quota()).unwrap();
        store.assign_agent(t.tenant_id, 10).unwrap();
        assert_eq!(
            store.assign_agent(t.tenant_id, 10),
            Err(PlatformError::AgentAlreadyAssigned)
        );
    }

    #[test]
    fn test_quota_used_not_found() {
        let store = TenantStore::new();
        assert_eq!(store.quota_used(999), Err(PlatformError::TenantNotFound));
    }
}

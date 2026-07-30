//! 隔离边界强制执行

use crate::manager::TenantStore;
use crate::quota::check_resource;
use crate::types::ResourceReq;
use crate::PlatformError;

/// 隔离边界 trait — 在资源分配前强制执行租户隔离
pub trait IsolationBoundary {
    /// 校验指定租户是否允许此资源请求
    fn enforce(&self, tenant_id: u64, req: &ResourceReq) -> Result<(), PlatformError>;
}

/// 基于 TenantStore 的隔离实现
#[derive(Debug, Clone)]
pub struct TenantIsolation {
    /// 内部租户存储引用
    pub store: TenantStore,
}

impl TenantIsolation {
    /// 从已有 TenantStore 构建隔离层
    pub fn new(store: TenantStore) -> Self {
        Self { store }
    }
}

impl IsolationBoundary for TenantIsolation {
    fn enforce(&self, tenant_id: u64, req: &ResourceReq) -> Result<(), PlatformError> {
        let tenant = self
            .store
            .tenants
            .iter()
            .find(|t| t.tenant_id == tenant_id)
            .ok_or(PlatformError::TenantNotFound)?;
        check_resource(&tenant.quota, req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::TenantManager;
    use crate::types::TenantQuota;

    fn setup() -> TenantIsolation {
        let mut store = TenantStore::new();
        let quota = TenantQuota {
            max_cpu_pct: 60.0,
            max_mem_kb: 2048,
            max_storage_kb: 4096,
            max_agents: 3,
            max_bandwidth_kbps: 1000,
        };
        store.create("iso-tenant", quota).unwrap();
        TenantIsolation::new(store)
    }

    #[test]
    fn test_enforce_within_quota() {
        let iso = setup();
        let req = ResourceReq {
            cpu_pct: 30.0,
            mem_kb: 512,
        };
        assert!(iso.enforce(1, &req).is_ok());
    }

    #[test]
    fn test_enforce_over_quota_cpu() {
        let iso = setup();
        let req = ResourceReq {
            cpu_pct: 80.0,
            mem_kb: 100,
        };
        assert_eq!(iso.enforce(1, &req), Err(PlatformError::QuotaExceeded));
    }

    #[test]
    fn test_enforce_over_quota_mem() {
        let iso = setup();
        let req = ResourceReq {
            cpu_pct: 10.0,
            mem_kb: 9999,
        };
        assert_eq!(iso.enforce(1, &req), Err(PlatformError::QuotaExceeded));
    }

    #[test]
    fn test_enforce_tenant_not_found() {
        let iso = setup();
        let req = ResourceReq {
            cpu_pct: 1.0,
            mem_kb: 1,
        };
        assert_eq!(iso.enforce(999, &req), Err(PlatformError::TenantNotFound));
    }
}

//! 租户配额强制执行（v0.231.0，B3-08）。
//!
//! 隔离从咨询性升级为强制性：Agent spawn 前必须通过
//! [`TenantEnforcer`] 校验——超配直接拒绝（`QuotaExceeded`），
//! 通过后 usage 真实累计（agent 数量/内存/CPU），释放时扣减。
//!
//! seL4 资源挂钩（v0.231.0）：配额映射到 seL4 调度优先级/内存池
//! 由上层 `ResourceHooks` 注入；本模块保证账目一致（usage 与
//! 内核统计对齐的记账面）。

use crate::manager::TenantStore;
use crate::quota::check_resource;
use crate::types::{QuotaUsage, ResourceReq};
use crate::PlatformError;

/// Agent spawn 请求（含配额影响）。
#[derive(Debug, Clone)]
pub struct SpawnReq {
    /// 租户 ID。
    pub tenant_id: u64,
    /// 资源请求。
    pub resource: ResourceReq,
}

/// 租户隔离强制力抽象（蓝图/20 R2-3）：
/// spawn 前必须通过 [`Enforcer::check_spawn`]，超配拒绝；通过后 usage
/// 真实累计。实现方为 [`TenantEnforcer`]；seL4 真实 spawn 路径经此接口
/// 获得强制力（不再咨询性）。
pub trait Enforcer {
    /// 检查并记账一次 spawn（超配 → `QuotaExceeded` / `TenantNotFound`）。
    fn check_spawn(&mut self, req: &SpawnReq) -> Result<(), PlatformError>;

    /// 释放一次 spawn（失败/退出时扣减，幂等）。
    fn release_spawn(
        &mut self,
        tenant_id: u64,
        mem_kb: usize,
        cpu_pct: f32,
    ) -> Result<(), PlatformError>;

    /// 查询真实 usage。
    fn usage(&self, tenant_id: u64) -> Result<QuotaUsage, PlatformError>;
}

/// 配额强制执行器。
#[derive(Debug, Clone)]
pub struct TenantEnforcer {
    /// 租户存储（配额与 usage 事实源）。
    store: TenantStore,
}

impl Enforcer for TenantEnforcer {
    fn check_spawn(&mut self, req: &SpawnReq) -> Result<(), PlatformError> {
        self.try_spawn(req)
    }

    fn release_spawn(
        &mut self,
        tenant_id: u64,
        mem_kb: usize,
        cpu_pct: f32,
    ) -> Result<(), PlatformError> {
        self.release_agent(tenant_id, mem_kb, cpu_pct)
    }

    fn usage(&self, tenant_id: u64) -> Result<QuotaUsage, PlatformError> {
        TenantEnforcer::usage(self, tenant_id)
    }
}

impl TenantEnforcer {
    /// 创建执行器。
    pub fn new(store: TenantStore) -> Self {
        Self { store }
    }

    /// 访问底层存储（只读）。
    pub fn store(&self) -> &TenantStore {
        &self.store
    }

    /// 尝试 spawn：超配拒绝（强制性隔离）。
    ///
    /// 通过后累计 usage 并返回 `Ok(())`；调用方随后真正拉起 Agent。
    pub fn try_spawn(&mut self, req: &SpawnReq) -> Result<(), PlatformError> {
        // 1. 租户存在（只读收集配额）。
        let quota = self
            .store
            .tenants
            .iter()
            .find(|t| t.tenant_id == req.tenant_id)
            .map(|t| t.quota.clone())
            .ok_or(PlatformError::TenantNotFound)?;
        // 2. 配额校验（咨询性 check_resource 升级为强制）。
        check_resource(&quota, &req.resource)?;
        // 3. Agent 数量上限（spawn 拒绝）。
        let usage = self.usage_mut(req.tenant_id)?;
        if usage.agents >= quota.max_agents {
            return Err(PlatformError::QuotaExceeded);
        }
        // 4. 内存累计校验。
        let new_mem = usage.mem_kb.saturating_add(req.resource.mem_kb);
        if new_mem > quota.max_mem_kb {
            return Err(PlatformError::QuotaExceeded);
        }
        // 5. 真实累计（与内核统计对齐的记账面）。
        usage.agents += 1;
        usage.mem_kb = new_mem;
        usage.cpu_pct += req.resource.cpu_pct;
        Ok(())
    }

    /// 释放一个 Agent（spawn 失败/退出时扣减，幂等）。
    pub fn release_agent(
        &mut self,
        tenant_id: u64,
        mem_kb: usize,
        cpu_pct: f32,
    ) -> Result<(), PlatformError> {
        let usage = self.usage_mut(tenant_id)?;
        usage.agents = usage.agents.saturating_sub(1);
        usage.mem_kb = usage.mem_kb.saturating_sub(mem_kb);
        usage.cpu_pct = (usage.cpu_pct - cpu_pct).max(0.0);
        Ok(())
    }

    /// 查询租户 usage（真实累计值）。
    pub fn usage(&self, tenant_id: u64) -> Result<QuotaUsage, PlatformError> {
        self.store
            .usage
            .iter()
            .find(|(tid, _)| *tid == tenant_id)
            .map(|(_, u)| u.clone())
            .ok_or(PlatformError::TenantNotFound)
    }

    fn usage_mut(&mut self, tenant_id: u64) -> Result<&mut QuotaUsage, PlatformError> {
        self.store
            .usage
            .iter_mut()
            .find(|(tid, _)| *tid == tenant_id)
            .map(|(_, u)| u)
            .ok_or(PlatformError::TenantNotFound)
    }
}

// ============================================================================
// Tests（超配拒绝 + usage 真实累计 + 释放扣减）
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::TenantManager;
    use crate::types::TenantQuota;

    fn quota() -> TenantQuota {
        TenantQuota {
            max_cpu_pct: 100.0,
            max_mem_kb: 8192,
            max_storage_kb: 65536,
            max_agents: 2,
            max_bandwidth_kbps: 1000,
        }
    }

    fn make_enforcer() -> (TenantEnforcer, u64) {
        let mut store = TenantStore::new();
        let tenant = store.create("t1", quota()).expect("create");
        let tenant_id = tenant.tenant_id;
        (TenantEnforcer::new(store), tenant_id)
    }

    #[test]
    fn test_spawn_within_quota() {
        let (mut enf, tid) = make_enforcer();
        let req = SpawnReq {
            tenant_id: tid,
            resource: ResourceReq {
                cpu_pct: 20.0,
                mem_kb: 1024,
            },
        };
        enf.try_spawn(&req).expect("spawn 1");
        enf.try_spawn(&req).expect("spawn 2");
        // usage 真实累计。
        let u = enf.usage(tid).expect("usage");
        assert_eq!(u.agents, 2);
        assert_eq!(u.mem_kb, 2048);
        assert!((u.cpu_pct - 40.0).abs() < 1e-6);
    }

    #[test]
    fn test_spawn_over_agent_limit_rejected() {
        let (mut enf, tid) = make_enforcer();
        let req = SpawnReq {
            tenant_id: tid,
            resource: ResourceReq {
                cpu_pct: 10.0,
                mem_kb: 512,
            },
        };
        enf.try_spawn(&req).expect("spawn 1");
        enf.try_spawn(&req).expect("spawn 2");
        // 第三个超 max_agents=2 → 拒绝。
        assert_eq!(enf.try_spawn(&req), Err(PlatformError::QuotaExceeded));
        // usage 不变。
        assert_eq!(enf.usage(tid).unwrap().agents, 2);
    }

    #[test]
    fn test_spawn_over_memory_rejected() {
        let (mut enf, tid) = make_enforcer();
        let req = SpawnReq {
            tenant_id: tid,
            resource: ResourceReq {
                cpu_pct: 5.0,
                mem_kb: 6000,
            },
        };
        enf.try_spawn(&req).expect("spawn 1");
        // 第二个 6000KB 超 8192 上限 → 拒绝。
        assert_eq!(enf.try_spawn(&req), Err(PlatformError::QuotaExceeded));
    }

    #[test]
    fn test_spawn_unknown_tenant_rejected() {
        let (mut enf, _tid) = make_enforcer();
        let req = SpawnReq {
            tenant_id: 999,
            resource: ResourceReq {
                cpu_pct: 1.0,
                mem_kb: 1,
            },
        };
        assert_eq!(enf.try_spawn(&req), Err(PlatformError::TenantNotFound));
    }

    #[test]
    fn test_release_decrements_usage() {
        let (mut enf, tid) = make_enforcer();
        let req = SpawnReq {
            tenant_id: tid,
            resource: ResourceReq {
                cpu_pct: 30.0,
                mem_kb: 2048,
            },
        };
        enf.try_spawn(&req).expect("spawn");
        enf.release_agent(tid, 2048, 30.0).expect("release");
        let u = enf.usage(tid).expect("usage");
        assert_eq!(u.agents, 0);
        assert_eq!(u.mem_kb, 0);
        assert!((u.cpu_pct - 0.0).abs() < 1e-6);
        // 释放后可以再次 spawn（配额释放）。
        enf.try_spawn(&req).expect("spawn after release");
    }

    #[test]
    fn test_release_idempotent_no_underflow() {
        let (mut enf, tid) = make_enforcer();
        // 未 spawn 就释放 → 不 panic，usage 饱和到 0。
        enf.release_agent(tid, 100, 5.0).expect("release");
        let u = enf.usage(tid).expect("usage");
        assert_eq!(u.agents, 0);
        assert_eq!(u.mem_kb, 0);
    }
}

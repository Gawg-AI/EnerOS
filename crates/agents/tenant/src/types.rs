//! 核心数据结构

use alloc::string::String;

/// 租户资源配额
#[derive(Debug, Clone, PartialEq)]
pub struct TenantQuota {
    /// 最大 CPU 使用百分比 (0.0 ~ 100.0)
    pub max_cpu_pct: f32,
    /// 最大内存 (KB)
    pub max_mem_kb: usize,
    /// 最大存储 (KB)
    pub max_storage_kb: usize,
    /// 最大 Agent 数量
    pub max_agents: u32,
    /// 最大带宽 (kbps)
    pub max_bandwidth_kbps: u64,
}

/// 租户实体
#[derive(Debug, Clone, PartialEq)]
pub struct Tenant {
    /// 租户唯一标识
    pub tenant_id: u64,
    /// 租户名称
    pub name: String,
    /// 创建时间戳 (Unix epoch seconds)
    pub created_at: u64,
    /// 资源配额
    pub quota: TenantQuota,
}

/// 当前资源使用情况
#[derive(Debug, Clone, PartialEq)]
pub struct QuotaUsage {
    /// 当前 CPU 使用百分比
    pub cpu_pct: f32,
    /// 当前内存使用 (KB)
    pub mem_kb: usize,
    /// 当前存储使用 (KB)
    pub storage_kb: usize,
    /// 当前 Agent 数量
    pub agents: u32,
}

/// 资源分配请求
#[derive(Debug, Clone, PartialEq)]
pub struct ResourceReq {
    /// 请求 CPU 百分比
    pub cpu_pct: f32,
    /// 请求内存 (KB)
    pub mem_kb: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_quota() -> TenantQuota {
        TenantQuota {
            max_cpu_pct: 50.0,
            max_mem_kb: 1024,
            max_storage_kb: 4096,
            max_agents: 4,
            max_bandwidth_kbps: 1000,
        }
    }

    #[test]
    fn test_tenant_quota_creation() {
        let q = sample_quota();
        assert_eq!(q.max_cpu_pct, 50.0);
        assert_eq!(q.max_mem_kb, 1024);
        assert_eq!(q.max_storage_kb, 4096);
        assert_eq!(q.max_agents, 4);
        assert_eq!(q.max_bandwidth_kbps, 1000);
    }

    #[test]
    fn test_tenant_creation() {
        let t = Tenant {
            tenant_id: 1,
            name: String::from("tenant-a"),
            created_at: 1700000000,
            quota: sample_quota(),
        };
        assert_eq!(t.tenant_id, 1);
        assert_eq!(t.name, "tenant-a");
        assert_eq!(t.created_at, 1700000000);
    }

    #[test]
    fn test_quota_usage_default_zero() {
        let u = QuotaUsage {
            cpu_pct: 0.0,
            mem_kb: 0,
            storage_kb: 0,
            agents: 0,
        };
        assert_eq!(u.cpu_pct, 0.0);
        assert_eq!(u.agents, 0);
    }

    #[test]
    fn test_resource_req() {
        let r = ResourceReq {
            cpu_pct: 10.0,
            mem_kb: 256,
        };
        assert_eq!(r.cpu_pct, 10.0);
        assert_eq!(r.mem_kb, 256);
    }

    #[test]
    fn test_clone_equality() {
        let q = sample_quota();
        let q2 = q.clone();
        assert_eq!(q, q2);
    }
}

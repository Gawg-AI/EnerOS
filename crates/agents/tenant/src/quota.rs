//! 配额校验与剩余容量计算

use crate::types::{QuotaUsage, ResourceReq, TenantQuota};
use crate::PlatformError;

/// 校验当前使用是否在配额范围内
pub fn check_quota(quota: &TenantQuota, usage: &QuotaUsage) -> Result<(), PlatformError> {
    if usage.cpu_pct > quota.max_cpu_pct
        || usage.mem_kb > quota.max_mem_kb
        || usage.storage_kb > quota.max_storage_kb
        || usage.agents > quota.max_agents
    {
        return Err(PlatformError::QuotaExceeded);
    }
    Ok(())
}

/// 校验一个新的资源请求是否能被当前配额容纳
pub fn check_resource(quota: &TenantQuota, req: &ResourceReq) -> Result<(), PlatformError> {
    if req.cpu_pct > quota.max_cpu_pct || req.mem_kb > quota.max_mem_kb {
        return Err(PlatformError::QuotaExceeded);
    }
    Ok(())
}

/// 计算剩余容量
pub fn remaining(quota: &TenantQuota, usage: &QuotaUsage) -> QuotaUsage {
    QuotaUsage {
        cpu_pct: (quota.max_cpu_pct - usage.cpu_pct).max(0.0),
        mem_kb: quota.max_mem_kb.saturating_sub(usage.mem_kb),
        storage_kb: quota.max_storage_kb.saturating_sub(usage.storage_kb),
        agents: quota.max_agents.saturating_sub(usage.agents),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quota() -> TenantQuota {
        TenantQuota {
            max_cpu_pct: 80.0,
            max_mem_kb: 2048,
            max_storage_kb: 8192,
            max_agents: 4,
            max_bandwidth_kbps: 5000,
        }
    }

    #[test]
    fn test_check_quota_within() {
        let usage = QuotaUsage {
            cpu_pct: 40.0,
            mem_kb: 1024,
            storage_kb: 4096,
            agents: 2,
        };
        assert!(check_quota(&quota(), &usage).is_ok());
    }

    #[test]
    fn test_check_quota_exceeded_cpu() {
        let usage = QuotaUsage {
            cpu_pct: 90.0,
            mem_kb: 0,
            storage_kb: 0,
            agents: 0,
        };
        assert_eq!(
            check_quota(&quota(), &usage),
            Err(PlatformError::QuotaExceeded)
        );
    }

    #[test]
    fn test_check_quota_exceeded_agents() {
        let usage = QuotaUsage {
            cpu_pct: 0.0,
            mem_kb: 0,
            storage_kb: 0,
            agents: 5,
        };
        assert_eq!(
            check_quota(&quota(), &usage),
            Err(PlatformError::QuotaExceeded)
        );
    }

    #[test]
    fn test_check_resource_ok() {
        let req = ResourceReq {
            cpu_pct: 50.0,
            mem_kb: 1024,
        };
        assert!(check_resource(&quota(), &req).is_ok());
    }

    #[test]
    fn test_check_resource_exceeded() {
        let req = ResourceReq {
            cpu_pct: 90.0,
            mem_kb: 100,
        };
        assert_eq!(
            check_resource(&quota(), &req),
            Err(PlatformError::QuotaExceeded)
        );
    }

    #[test]
    fn test_remaining() {
        let usage = QuotaUsage {
            cpu_pct: 30.0,
            mem_kb: 512,
            storage_kb: 2048,
            agents: 1,
        };
        let rem = remaining(&quota(), &usage);
        assert_eq!(rem.cpu_pct, 50.0);
        assert_eq!(rem.mem_kb, 1536);
        assert_eq!(rem.storage_kb, 6144);
        assert_eq!(rem.agents, 3);
    }

    #[test]
    fn test_remaining_saturates_at_zero() {
        let usage = QuotaUsage {
            cpu_pct: 100.0,
            mem_kb: 9999,
            storage_kb: 9999,
            agents: 10,
        };
        let rem = remaining(&quota(), &usage);
        assert_eq!(rem.cpu_pct, 0.0);
        assert_eq!(rem.mem_kb, 0);
        assert_eq!(rem.storage_kb, 0);
        assert_eq!(rem.agents, 0);
    }
}

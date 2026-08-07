//! EnerOS 多租户支持 (workspace v0.192.0)
//!
//! 提供租户管理、资源配额与隔离能力，使一台 Edge Box 可同时服务多个主体。
//!
//! # 模块
//! - [`types`][]: 核心数据结构（Tenant / TenantQuota / QuotaUsage / ResourceReq）
//! - [`manager`][]: 租户生命周期管理（创建/删除/分配/查询）
//! - [`quota`][]: 配额校验与剩余容量计算
//! - [`isolation`][]: 隔离边界强制执行

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod enforcement;
pub mod isolation;
pub mod manager;
pub mod quota;
pub mod types;

pub use enforcement::SpawnReq;
pub use enforcement::{Enforcer, TenantEnforcer};
pub use isolation::{IsolationBoundary, TenantIsolation};
pub use manager::{TenantManager, TenantStore};
pub use quota::{check_quota, check_resource, remaining};
pub use types::{QuotaUsage, ResourceReq, Tenant, TenantQuota};

/// 平台错误类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformError {
    /// 租户不存在
    TenantNotFound,
    /// 配额超限
    QuotaExceeded,
    /// 租户仍有 Agent 绑定，无法删除
    TenantBusy,
    /// 配额参数非法
    InvalidQuota,
    /// Agent 已分配给其他租户
    AgentAlreadyAssigned,
}

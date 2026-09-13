//! EnerOS 蓝图 61 v0.314.35 —— ResourceRegistry 与 CapabilityEnvelope。
//!
//! 唯一不变量（[08] §4.1）：规划和控制只使用由当前 TrustedSnapshot、
//! catalog 和合同求交得到的实时 `CapabilityEnvelope`；离线、故障、维护、
//! 工艺或合同限制**只会收紧能力，绝不回退到铭牌最大值**。
//!
//! 外部只能调用 [`ResourceRegistry`]（建/查/增量更新）；[`CapabilityProvider`]
//! 与包络修改在模块内部（深模块）。
//!
//! 依赖纪律（[16] §3.5）：依赖 snapshot-service、semantic-catalog；
//! 禁止依赖 solver backend、writer。

#![no_std]
#![cfg_attr(not(test), allow(dead_code))]

extern crate alloc;

pub mod capability;
pub mod contract;
pub mod provider;
pub mod registry;

pub use capability::{Availability, CapabilityEnvelope, DeratingReason};
pub use contract::ControlContractView;
pub use provider::{CapabilityProvider, ResourceKind};
pub use registry::{
    registry_from_catalog, CatalogRegistryReport, ResilienceInputs, ResourceRegistry,
};

/// 资源统一种类（[08] §4.2 闭枚举；与 catalog asset kind 对齐）。
pub mod kinds {
    pub const BATTERY: u16 = 1;
    // 蓝图 70.8 WP-70.8.3：目录资产 kind（config-bundle `catalog_kinds::
    // ASSET_*`）与本闭枚举数值对齐——parity 由 `known()` 边界与
    // catalog 装配测试共同锁定。
    pub const PV: u16 = 2;
    pub const WIND: u16 = 3;
    pub const GENERATOR: u16 = 4;
    pub const FLEXIBLE_LOAD: u16 = 5;
    pub const FIXED_LOAD: u16 = 6;
    pub const EVSE: u16 = 7;
    pub const HVAC: u16 = 8;
    pub const PROCESS_UNIT: u16 = 9;
    pub const GRID_CONNECTION: u16 = 10;
    pub const AGGREGATE: u16 = 11;
    /// 已知资源种类（目录资产 kind → 本闭枚举；未知 = 不可控）。
    pub fn known(kind: u16) -> bool {
        matches!(
            kind,
            BATTERY
                | PV
                | WIND
                | GENERATOR
                | FLEXIBLE_LOAD
                | FIXED_LOAD
                | EVSE
                | HVAC
                | PROCESS_UNIT
                | GRID_CONNECTION
                | AGGREGATE
        )
    }
}

/// 资源描述（[02] §6 字段集）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceDescriptor {
    pub asset_id: u64,
    pub site_id: u32,
    /// [`kinds`] 闭枚举。
    pub resource_kind: u16,
    pub control_contract_id: u32,
    pub topology_node_id: u64,
    pub capability_model_id: u32,
    pub owner_tenant_id: u32,
}

/// 注册表构建/增量更新错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryError {
    pub kind: RegistryErrorKind,
    pub detail: alloc::string::String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryErrorKind {
    /// 资产未在 catalog 登记/已 tombstone。
    UnknownAsset,
    /// 无完整控制合同（可观测不可控）。
    NoControlContract,
    /// 未知控制合同 id。
    UnknownContract,
    /// 资源种类未实现 provider。
    NoProvider,
    /// 拓扑节点未知（悬空）。
    UnknownTopologyNode,
    /// 包络求交为空 → Unavailable（含原因）。
    EmptyIntersection,
    /// capacity 输入缺失（快照不完整）。
    MissingSnapshotInput,
}

impl core::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "registry:{:?}:{}", self.kind, self.detail)
    }
}

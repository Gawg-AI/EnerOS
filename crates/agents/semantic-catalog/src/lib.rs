//! EnerOS 蓝图 61 v0.314.33 —— SemanticCatalog 深模块。
//!
//! 把「schema、单位、资产、拓扑、点语义、反馈槽、安全覆盖、控制合同、
//! 安全约束、迁移别名、tombstone」编译进一个封闭二进制 bundle
//! （`configs/catalog.toml` → `config-bundle compile_catalog` → 签名
//! bundle "catalog" 条目），target 只读编译产物，绝不解析任意企业文档
//! （[08] §2.3）。
//!
//! 唯一不变量：每个进入控制的观测和资源都绑定同一版本组的 schema、单位、
//! 资产、拓扑、点映射和控制合同——不存在「靠点号猜语义」。本 crate 公开
//! [`SemanticCatalog`] 查询窄口；bundle 解码、交叉引用、版本激活、tombstone
//! 记忆全部隐藏（深模块：调用方只传 `(key, versions)` 或 id）。
//!
//! 依赖纪律（[16] §3.3）：禁止依赖 solver、LLM、protocol transport。

#![no_std]
#![cfg_attr(not(test), allow(dead_code))]

extern crate alloc;

pub mod decode;
pub mod model;
pub mod repository;
pub mod units;

pub use model::*;

/// 蓝图 61 [02] §4.1 全局点键：`(tenant_id, site_id, source_id, device_id,
/// point_id, mapping_version)`——站内 `PointId` 不能单独作为全局键。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ObservationKey {
    pub tenant_id: u32,
    pub site_id: u32,
    pub source_id: u32,
    pub device_id: u16,
    pub point_id: u32,
    pub mapping_version: u64,
}

impl ObservationKey {
    /// 从观察事件派生（事件自身字段即键；与 event_id 的域序无关）。
    pub fn from_event(ev: &eneros_telemetry_model::event::ObservationEvent) -> Self {
        Self {
            tenant_id: ev.tenant_id,
            site_id: ev.site_id,
            source_id: ev.source_id,
            device_id: ev.device_id,
            point_id: ev.point_id,
            mapping_version: ev.mapping_version,
        }
    }
}

/// 目录错误（fail-closed 结构件；`kind` 供程序分支，`detail` 供诊断）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogError {
    pub kind: CatalogErrorKind,
    pub detail: alloc::string::String,
}

impl CatalogError {
    pub fn new(kind: CatalogErrorKind, detail: alloc::string::String) -> Self {
        Self { kind, detail }
    }
}

impl core::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "catalog:{:?}:{}", self.kind, self.detail)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogErrorKind {
    /// bundle 解码歧义（截断/未知字段/重复/越界）。
    Decode,
    /// 版本组不匹配激活目录（catalog 切换后旧 VersionSet 失效）。
    VersionMismatch,
    /// 点不在点语义目录（未绑定资产/单位 → 不得进控制快照）。
    UnknownPoint,
    /// 资产未登记或已 tombstone。
    UnknownAsset,
    /// schema 未注册。
    UnknownSchema,
    /// 单位未登记（unit_code=0 或未在 UnitCatalog）。
    UnknownUnit,
    /// 资产已被 tombstone（ID 永不复用）。
    TombstonedAsset,
    /// 控制合同缺失/不存在。
    UnknownContract,
    /// 反馈槽不完整（5 槽缺一不可用）。
    IncompleteFeedback,
    /// 拓扑视图不可得（站点无拓扑或版本不匹配）。
    TopologyUnavailable,
    /// 拓扑引用悬空（解码期交叉引用不过）。
    InvalidTopology,
    /// 目录有效性窗口外（过期/未生效）。
    OutsideValidity,
    /// 激活被拒（tombstone ID 复用等）。
    ActivationRejected,
    /// 单位换算不存在或量纲不兼容。
    NoConversion,
}

impl From<eneros_config_bundle::ConfigError> for CatalogError {
    fn from(e: eneros_config_bundle::ConfigError) -> Self {
        CatalogError::new(CatalogErrorKind::Decode, alloc::format!("payload:{e:?}"))
    }
}

/// [08] §2.1 版本组：[`SemanticCatalog`] 查询必须携带同一版本组；目录
/// 切换（generation 递增）后旧版本组失效——旧快照/计划不得混用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct VersionSet {
    /// bundle generation（目录整体世代；生命周期内唯一递增）。
    pub generation: u64,
    /// 点映射/点表世代。
    pub mapping_version: u64,
    /// 拓扑版本。
    pub topology_version: u64,
    /// 资产目录版本。
    pub asset_version: u64,
    /// 控制合同目录版本。
    pub contract_version: u64,
    /// 安全约束目录版本。
    pub safety_version: u64,
}

impl VersionSet {
    pub const ZERO: VersionSet = VersionSet {
        generation: 0,
        mapping_version: 0,
        topology_version: 0,
        asset_version: 0,
        contract_version: 0,
        safety_version: 0,
    };
}

/// 观测语义（[08] §2.1「不存在靠点号猜语义」的解析结果）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservationSemantics {
    /// 全局资产 ID（≠0；tombstone 资产解析失败）。
    pub asset_id: u64,
    /// schema 注册 ID。
    pub schema_id: u32,
    /// schema 版本。
    pub schema_version: u16,
    /// UnitCatalog 单位码（≠0）。
    pub unit_code: u16,
    /// 点角色（measurement/control/switch-state）。
    pub role: u8,
    /// 控制点回读点（0 = 非控制点）。
    pub readback_point_id: u32,
}

impl ObservationSemantics {
    /// 控制资格：单位与资产齐全（事件侧由 SnapshotModule 再验
    /// quality/integrity/freshness）。
    pub fn control_eligible(&self) -> bool {
        self.asset_id != 0 && self.unit_code != 0 && self.schema_id != 0
    }
}

/// 蓝图 61 [08] §2.2 外部窄口。
pub trait SemanticCatalog {
    /// 当前激活版本组（快照/计划绑定用）。
    fn version_set(&self) -> VersionSet;

    /// 解析观测键语义；`versions` ≠ 激活版本组 → `VersionMismatch`。
    fn resolve_observation(
        &self,
        key: &ObservationKey,
        versions: VersionSet,
    ) -> Result<ObservationSemantics, CatalogError>;

    /// 资产描述（版本不符/未登记/tombstone → None 或错误）。
    fn asset(&self, asset_id: u64, version: u64) -> Option<&AssetDescriptor>;

    /// 站点拓扑视图（版本不符 → 错误；站点无拓扑 → `TopologyUnavailable`）。
    fn topology_view(&self, site_id: u32, version: u64) -> Result<TopologyView, CatalogError>;

    /// 控制合同（版本不符/不存在 → 错误）。
    fn control_contract(&self, id: u32, version: u64) -> Result<&ControlContract, CatalogError>;

    /// 当前绑定资产是否在激活目录中可控（供装配层生成可控池）。
    fn asset_control_contract(&self, asset_id: u64, version: u64) -> Option<&ControlContract>;

    /// 开关状态 → 带电节点集（[08] §2.4；None = 拓扑不可得/版本不符）。
    /// 默认实现 = None（不含拓扑的目录实现如实不可用）。
    fn energized_nodes(
        &self,
        _switch_states: &[(u32, bool)],
    ) -> Option<alloc::collections::BTreeSet<u64>> {
        None
    }

    /// 反馈槽（控制链五槽；不完整目录 → None）。
    fn feedback_slots(&self) -> Option<model::FeedbackSlots> {
        None
    }
}

/// 目录公开查询的保守错误（无 `std`；`Vec<u8>` 承载诊断是可打印的）。
pub type CatalogResult<T> = Result<T, CatalogError>;

/// 便捷 re-export（调用方最少符号数）。
pub use repository::CatalogRepository;
pub use repository::CatalogSet;
pub use units::UnitConverter;

/// 目录 bundle 原始字节 ≥ 0 的约束（防御性上界，[08] §2.3 封闭）。
pub const CATALOG_PAYLOAD_MAX: usize = 1664;
pub(crate) fn sanity_len(len: usize) -> bool {
    len <= CATALOG_PAYLOAD_MAX
}

/// 从配置 payload 解码并构建仓库（`decode` 与 `repository` 的装配入口；
/// svc-config ConfigGet("catalog") 返回字节即此输入）。
pub fn open(payload: &[u8]) -> Result<CatalogRepository, CatalogError> {
    let bundle = decode::decode_catalog(payload)?;
    repository::CatalogRepository::build(bundle)
}

/// 空目录（无 bundle 时装配期的 fail-closed 空态；查询全失败）。
pub fn empty() -> CatalogRepository {
    CatalogRepository::default()
}

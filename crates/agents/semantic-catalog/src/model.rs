//! 目录数据模型（纯类型；解码/索引在 `decode`/`repository`）。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::VersionSet;

/// 目录头（[`crate::schema` 布局：64B]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogHeader {
    pub versions: VersionSet,
    /// 有效期（0 = 不限）。
    pub valid_from_ns: u64,
    pub valid_until_ns: u64,
}

impl CatalogHeader {
    pub fn is_valid_at(&self, at_ns: u64) -> bool {
        let after_start = self.valid_from_ns == 0 || at_ns >= self.valid_from_ns;
        let before_end = self.valid_until_ns == 0 || at_ns <= self.valid_until_ns;
        after_start && before_end
    }
}

/// schema 注册项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchemaEntry {
    pub id: u32,
    pub version: u16,
    pub flags: u16,
}

/// 单位换算表项（量纲兼容已由编译/解码与 PhysicalUnit 双重强制）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UnitConversion {
    pub from: u16,
    pub to: u16,
    pub factor: f64,
}

/// 资产描述（ID 永不复用；删除走 tombstone）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssetDescriptor {
    pub asset_id: u64,
    pub site_id: u32,
    pub tenant_id: u32,
    /// [`catalog_kinds::ASSET_*`] 闭枚举。
    pub kind: u16,
    /// 站内协议设备短 ID（0 = 无）。
    pub device_id: u16,
    /// 拓扑节点绑定（0 = 未入拓扑）。
    pub topology_node_id: u64,
    /// [`catalog::ASSET_FLAG_TOMBSTONE`]。
    pub flags: u16,
}

impl AssetDescriptor {
    pub fn is_tombstone(&self) -> bool {
        self.flags & crate::decode::ASSET_FLAG_TOMBSTONE != 0
    }
}

/// 拓扑节点。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TopologyNode {
    pub id: u64,
    /// [`catalog_kinds::NODE_*`] 闭枚举。
    pub kind: u16,
    pub flags: u16,
}

/// 拓扑边（方向 from → to；容量/损耗/有效期）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TopologyEdge {
    pub from: u64,
    pub to: u64,
    /// 1=switch（必须带开关状态观测点）2=branch。
    pub kind: u16,
    /// 开关状态观测点（kind=switch 时 ≠0）。
    pub switch_point_id: u32,
    pub capacity_kw: f64,
    pub loss: f64,
    pub valid_from_ns: u64,
    pub valid_until_ns: u64,
    pub flags: u16,
}

/// 点语义目录项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointSemantics {
    pub point_id: u32,
    pub asset_id: u64,
    pub schema_id: u32,
    pub schema_version: u16,
    /// UnitCatalog 单位码（≠0）。
    pub unit_code: u16,
    /// [`catalog::ROLE_*`]。
    pub role: u8,
    /// 控制点回读点（0 = 无）。
    pub readback_point_id: u32,
    pub flags: u8,
}

impl PointSemantics {
    pub fn is_control(&self) -> bool {
        self.role == crate::decode::ROLE_CONTROL
    }
}

/// 反馈槽（槽序 = TrustedFeedbackBuilder 字段声明序；5 槽齐全）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeedbackSlots {
    pub grid_up: u32,
    pub soc: u32,
    pub dc_bus_v: u32,
    pub overcurrent: u32,
    pub overtemp: u32,
}

impl FeedbackSlots {
    /// 按槽码取点（[`catalog::FEEDBACK_SLOT_*`]）。
    pub fn point_of(&self, slot: u8) -> Option<u32> {
        match slot {
            crate::decode::FEEDBACK_SLOT_GRID_UP => Some(self.grid_up),
            crate::decode::FEEDBACK_SLOT_SOC => Some(self.soc),
            crate::decode::FEEDBACK_SLOT_DC_BUS_V => Some(self.dc_bus_v),
            crate::decode::FEEDBACK_SLOT_OVERCURRENT => Some(self.overcurrent),
            crate::decode::FEEDBACK_SLOT_OVERTEMP => Some(self.overtemp),
            _ => None,
        }
    }
}

/// 安全覆盖项（点量程/周期/单位）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoverageEntry {
    pub point_id: u32,
    pub min: f64,
    pub max: f64,
    pub period_ms: u32,
    pub unit_code: u16,
    pub flags: u16,
}

/// 控制合同（[08] §4.4 最小集；无完整合同 → 可观测不可控）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ControlContract {
    pub contract_id: u32,
    pub asset_id: u64,
    pub point_id: u32,
    pub unit_code: u16,
    /// [`catalog::CONTRACT_CLASS_*`]。
    pub class: u16,
    pub min: f64,
    pub max: f64,
    pub ramp_kw_per_s: f64,
    pub settle_ms: u32,
    pub min_on_ms: u64,
    pub min_off_ms: u64,
    pub max_starts: u16,
    pub readback_point_id: u32,
    pub tolerance_kw: f64,
    pub reversible: u8,
    pub compensation: u8,
    /// 1=local 2=remote 3=manual。
    pub priority: u8,
    /// 1=hold 2=zero 3=safe。
    pub safe_state: u8,
    /// 1=hold 2=zero。
    pub lost_link: u8,
    pub flags: u8,
}

/// 安全约束目录项。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SafetyConstraint {
    pub constraint_id: u32,
    pub asset_id: u64,
    /// [`catalog::SAFETY_KIND_*`]。
    pub kind: u16,
    pub point_id: u32,
    pub limit: f64,
    pub flags: u16,
}

/// 迁移别名（旧 market/VPP ID → asset_id）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AliasEntry {
    /// [`catalog::ALIAS_KIND_*`]。
    pub kind: u8,
    pub legacy_id: u64,
    pub asset_id: u64,
}

/// 物理参数铭牌（蓝图 70.8 WP-70.8.3：容量/效率/功率/爬坡的可追溯来源）。
///
/// 与目录同代（generation/contract_version）——包络求交、认证电池参数与
/// 计划模型都从这里取值；缺铭牌的资产**不可控**（fail-closed，不从 host
/// fixture 补值）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Nameplate {
    pub asset_id: u64,
    /// 额定容量（kWh）——SOC → 能量的换算基准。
    pub capacity_kwh: f64,
    /// 充电效率（0,1]。
    pub eta_c: f64,
    /// 放电效率（0,1]。
    pub eta_d: f64,
    /// 可调度能量上界（kWh；通常 ≤ capacity_kwh）。
    pub energy_max_kwh: f64,
    pub p_min_kw: f64,
    pub p_max_kw: f64,
    pub q_min_kvar: f64,
    pub q_max_kvar: f64,
    pub ramp_up_kw_s: f64,
    pub ramp_down_kw_s: f64,
    /// 可用 SOC 下限（0..1 分数）——认证 MinSoc 硬约束的物理来源。
    pub soc_min: f64,
    /// 可用 SOC 上限（0..1 分数）——认证 MaxSoc 硬约束的物理来源。
    pub soc_max: f64,
}

/// 解码后的封闭目录（`decode::decode_catalog` 产物）。
#[derive(Debug, Clone, Default)]
pub struct CatalogBundle {
    pub header: Option<CatalogHeader>,
    pub schemas: Vec<SchemaEntry>,
    pub conversions: Vec<UnitConversion>,
    pub assets: Vec<AssetDescriptor>,
    pub nodes: Vec<TopologyNode>,
    pub edges: Vec<TopologyEdge>,
    pub points: Vec<PointSemantics>,
    pub feedback: Vec<(u8, u32)>,
    pub coverage: Vec<CoverageEntry>,
    pub contracts: Vec<ControlContract>,
    pub safety: Vec<SafetyConstraint>,
    pub aliases: Vec<AliasEntry>,
    pub tombstones: Vec<u64>,
    /// 蓝图 70.8：物理参数铭牌（字段 14；缺省空表 = 无铭牌资产不可控）。
    pub nameplates: Vec<Nameplate>,
}

impl CatalogBundle {
    pub fn assets_index(&self) -> BTreeMap<u64, AssetDescriptor> {
        self.assets.iter().map(|a| (a.asset_id, *a)).collect()
    }
}

/// 站点拓扑视图（[08] §2.4；energized 由开关观测派生，见 `topology`）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TopologyView {
    pub site_id: u32,
    pub nodes: Vec<TopologyNode>,
    pub edges: Vec<TopologyEdge>,
}

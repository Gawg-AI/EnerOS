//! 目录仓库（深模块内部）：索引、版本激活、tombstone 记忆、energized 视图。
//!
//! 外部只经 [`crate::SemanticCatalog`] 窄口查询；`CatalogSet` 负责跨代
//! 激活/回滚与「ID 永不复用」记忆。

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

/// 解码期常量（与 config-bundle schema::catalog 同源 re-export）。
pub use eneros_config_bundle::schema::catalog::FEEDBACK_SLOT_COUNT;

use crate::decode::{
    FEEDBACK_SLOT_DC_BUS_V, FEEDBACK_SLOT_GRID_UP, FEEDBACK_SLOT_OVERCURRENT,
    FEEDBACK_SLOT_OVERTEMP, FEEDBACK_SLOT_SOC,
};
use crate::model::*;
use crate::{
    CatalogBundle, CatalogError, CatalogErrorKind, ObservationKey, ObservationSemantics,
    SemanticCatalog, VersionSet,
};

/// 单一目录世代（索引构建完成即不可变——查询免锁）。
#[derive(Debug, Clone, Default)]
pub struct CatalogRepository {
    pub(crate) header: Option<CatalogHeader>,
    schemas: BTreeMap<u32, SchemaEntry>,
    conversions: BTreeMap<(u16, u16), f64>,
    assets: BTreeMap<u64, AssetDescriptor>,
    nodes: BTreeMap<u64, TopologyNode>,
    edges: Vec<TopologyEdge>,
    points: BTreeMap<u32, PointSemantics>,
    feedback: Option<FeedbackSlots>,
    coverage: BTreeMap<u32, CoverageEntry>,
    contracts: BTreeMap<u32, ControlContract>,
    /// asset_id → 控制合同（唯一性在构建期校验）。
    contract_by_asset: BTreeMap<u64, u32>,
    safety: Vec<SafetyConstraint>,
    aliases: BTreeMap<(u8, u64), u64>,
    tombstones: BTreeSet<u64>,
    /// 目录整体摘要（配置 digest 派生；证据/审计用）。
    digest: [u8; 32],
}

impl CatalogRepository {
    /// 由解码 bundle 构建索引（交叉引用构建期最终校验）。
    pub fn build(bundle: CatalogBundle) -> Result<Self, CatalogError> {
        let header = bundle
            .header
            .ok_or_else(|| CatalogError::new(CatalogErrorKind::Decode, "missing-header".into()))?;
        let mut repo = Self {
            header: Some(header),
            ..Self::default()
        };
        for s in bundle.schemas {
            repo.schemas.insert(s.id, s);
        }
        for c in bundle.conversions {
            repo.conversions.insert((c.from, c.to), c.factor);
        }
        // 活资产排除 tombstone（编译期互斥已验；防御再查）；墓碑入
        // 跨代记忆集（ID 永不复用——[08] §2.3）。
        for a in bundle.assets {
            if a.is_tombstone() {
                repo.tombstones.insert(a.asset_id);
            } else {
                repo.assets.insert(a.asset_id, a);
            }
        }
        for n in bundle.nodes {
            repo.nodes.insert(n.id, n);
        }
        repo.edges = bundle.edges;
        for p in bundle.points {
            repo.points.insert(p.point_id, p);
        }
        if !bundle.feedback.is_empty() {
            let mut slots = FeedbackSlots {
                grid_up: 0,
                soc: 0,
                dc_bus_v: 0,
                overcurrent: 0,
                overtemp: 0,
            };
            for (slot, pid) in bundle.feedback.iter() {
                match *slot {
                    FEEDBACK_SLOT_GRID_UP => slots.grid_up = *pid,
                    FEEDBACK_SLOT_SOC => slots.soc = *pid,
                    FEEDBACK_SLOT_DC_BUS_V => slots.dc_bus_v = *pid,
                    FEEDBACK_SLOT_OVERCURRENT => slots.overcurrent = *pid,
                    FEEDBACK_SLOT_OVERTEMP => slots.overtemp = *pid,
                    _ => {}
                }
            }
            repo.feedback = Some(slots);
        }
        for c in bundle.coverage {
            repo.coverage.insert(c.point_id, c);
        }
        for c in bundle.contracts {
            if let Some(prev) = repo.contract_by_asset.insert(c.asset_id, c.contract_id) {
                return Err(CatalogError::new(
                    CatalogErrorKind::ActivationRejected,
                    alloc::format!("asset-multi-contract:{}/{}", c.asset_id, prev),
                ));
            }
            repo.contracts.insert(c.contract_id, c);
        }
        repo.safety = bundle.safety;
        for a in bundle.aliases {
            repo.aliases.insert((a.kind, a.legacy_id), a.asset_id);
        }
        for t in bundle.tombstones {
            repo.tombstones.insert(t);
        }
        // 目录摘要（generation + 各版本 + 资产/点计数——证据用，非安全锚）。
        {
            use sha2::Digest;
            let mut h = sha2::Sha256::new();
            h.update(b"eneros.catalog.v1");
            let vs = header.versions;
            for v in [
                vs.generation,
                vs.mapping_version,
                vs.topology_version,
                vs.asset_version,
                vs.contract_version,
                vs.safety_version,
            ] {
                h.update(v.to_le_bytes());
            }
            h.update((repo.assets.len() as u64).to_le_bytes());
            h.update((repo.points.len() as u64).to_le_bytes());
            h.update((repo.edges.len() as u64).to_le_bytes());
            repo.digest = h.finalize().into();
        }
        Ok(repo)
    }

    /// 目录整体摘要（generation/digest 证据）。
    pub fn digest(&self) -> [u8; 32] {
        if self.header.is_none() {
            return [0u8; 32];
        }
        self.digest
    }

    /// 有效性窗口（0=不限；查询期由调用方注入 now_ns）。
    pub fn is_valid_at(&self, at_ns: u64) -> bool {
        self.header.map(|h| h.is_valid_at(at_ns)).unwrap_or(false)
    }

    /// 反馈槽（5 槽不齐 → None，调用方 fail-closed）。
    pub fn feedback_slots(&self) -> Option<FeedbackSlots> {
        self.feedback
    }

    /// 安全覆盖表（按点）。
    pub fn coverage(&self) -> impl Iterator<Item = &CoverageEntry> {
        self.coverage.values()
    }

    /// 资产的全部安全约束。
    pub fn safety_constraints(&self, asset_id: u64) -> impl Iterator<Item = &SafetyConstraint> {
        self.safety.iter().filter(move |s| s.asset_id == asset_id)
    }

    /// 迁移别名解析（kind: 1=market-device 2=vpp-resource）。
    pub fn resolve_alias(&self, kind: u8, legacy_id: u64) -> Option<u64> {
        self.aliases.get(&(kind, legacy_id)).copied()
    }

    /// asset → 控制合同（无 → None；「没有完整控制合同的资源可观测、
    /// 可预测，但不能进入可控池」）。
    pub fn control_contract_for_asset(&self, asset_id: u64) -> Option<&ControlContract> {
        let id = self.contract_by_asset.get(&asset_id)?;
        self.contracts.get(id)
    }

    /// 全部控制合同（枚举序 = 合同 id 升序；WP-5.1 装配侧编译合同
    /// 视图消费——`PlanContractView` 装配单源）。
    pub fn control_contracts(&self) -> impl Iterator<Item = &ControlContract> {
        self.contracts.values()
    }

    /// 显式控制绑定（替代「首个 Control 点」）：资产 → 控制点。
    /// 设备短 ID 属协议映射（points.toml），由装配方从点表解析。
    pub fn control_binding_point(&self, asset_id: u64) -> Option<u32> {
        self.control_contract_for_asset(asset_id)
            .map(|c| c.point_id)
    }

    /// 站点拓扑视图（版本不符 → 错误）。
    pub fn topology_view_of(
        &self,
        site_id: u32,
        version: u64,
    ) -> Result<TopologyView, CatalogError> {
        if version
            != self
                .header
                .map(|h| h.versions.topology_version)
                .unwrap_or(0)
        {
            return Err(CatalogError::new(
                CatalogErrorKind::VersionMismatch,
                alloc::format!("topology:{version}"),
            ));
        }
        Ok(TopologyView {
            site_id,
            nodes: self.nodes.values().copied().collect(),
            edges: self.edges.clone(),
        })
    }

    /// 开关状态 → energized 节点集（[08] §2.4）。
    ///
    /// - 开关状态观测缺失/未知 → 相关边视为断开（岛不进 energized）；
    /// - 同开关点状态冲突（双值）→ 视为未知（最保守）；
    /// - 只有闭合开关与 branch 边可传递 energization。
    pub fn energized_view(&self, switch_states: &[(u32, bool)]) -> EnergizedView {
        let mut state: BTreeMap<u32, bool> = BTreeMap::new();
        let mut conflicted: Vec<u32> = Vec::new();
        for (pid, on) in switch_states {
            match state.get(pid) {
                None => {
                    state.insert(*pid, *on);
                }
                Some(prev) if *prev != *on => {
                    // 冲突：同一开关点双值 → 未知（最保守）。
                    state.remove(pid);
                    if !conflicted.contains(pid) {
                        conflicted.push(*pid);
                    }
                }
                _ => {}
            }
        }
        // 需要但未提供的开关点 = 未知（缺观测）。
        let mut unknown_switches: Vec<u32> = Vec::new();
        for e in self.edges.iter() {
            if e.kind == 1
                && e.switch_point_id != 0
                && !state.contains_key(&e.switch_point_id)
                && !unknown_switches.contains(&e.switch_point_id)
            {
                unknown_switches.push(e.switch_point_id);
            }
        }
        // BFS：grid 节点 + 闭合开关/分支边。
        let grid: Vec<u64> = self
            .nodes
            .iter()
            .filter(|(_, n)| n.kind == eneros_config_bundle::schema::catalog_kinds::NODE_GRID)
            .map(|(id, _)| *id)
            .collect();
        let mut energized: BTreeSet<u64> = BTreeSet::new();
        let mut stack: Vec<u64> = grid.clone();
        let mut closed: BTreeSet<(u64, u64)> = BTreeSet::new();
        for e in self.edges.iter() {
            let pass = if e.kind == 1 {
                state.get(&e.switch_point_id).copied().unwrap_or(false)
            } else {
                true
            };
            if pass {
                closed.insert((e.from, e.to));
                closed.insert((e.to, e.from));
            }
        }
        while let Some(n) = stack.pop() {
            if !energized.insert(n) {
                continue;
            }
            for e in self.edges.iter() {
                let other = if e.from == n {
                    Some(e.to)
                } else if e.to == n {
                    Some(e.from)
                } else {
                    None
                };
                if let Some(m) = other {
                    if closed.contains(&(n, m)) && !energized.contains(&m) {
                        stack.push(m);
                    }
                }
            }
        }
        EnergizedView {
            energized_nodes: energized,
            unknown_switches,
            conflicted_switches: conflicted,
        }
    }

    /// 按角色枚举点语义（装配层查询用：控制点枚举等）。
    pub fn points_with_role(&self, role: u8) -> impl Iterator<Item = (u32, PointSemantics)> + '_ {
        self.points
            .iter()
            .filter(move |(_, p)| p.role == role)
            .map(|(id, p)| (*id, *p))
    }

    /// 点 → 资产（点不在目录 → None）。
    pub fn asset_of_point(&self, point_id: u32) -> Option<u64> {
        self.points.get(&point_id).map(|p| p.asset_id)
    }
}

/// energized 视图（[08] §2.4：状态冲突或未知使相关岛/资源不可控）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EnergizedView {
    /// 带电节点集（含 grid 源）。
    pub energized_nodes: BTreeSet<u64>,
    /// 无观测的开关点（视为断开）。
    pub unknown_switches: Vec<u32>,
    /// 冲突开关点（双值 → 未知）。
    pub conflicted_switches: Vec<u32>,
}

impl EnergizedView {
    pub fn is_energized(&self, node_id: u64) -> bool {
        self.energized_nodes.contains(&node_id)
    }

    /// 资源可控性：节点带电且无未知/冲突开关。
    pub fn resource_controllable(&self, node_id: u64) -> bool {
        self.is_energized(node_id)
            && self.unknown_switches.is_empty()
            && self.conflicted_switches.is_empty()
    }
}

impl SemanticCatalog for CatalogRepository {
    fn version_set(&self) -> VersionSet {
        self.header.map(|h| h.versions).unwrap_or(VersionSet::ZERO)
    }

    fn resolve_observation(
        &self,
        key: &ObservationKey,
        versions: VersionSet,
    ) -> Result<ObservationSemantics, CatalogError> {
        let vs = self.version_set();
        if versions != vs {
            return Err(CatalogError::new(
                CatalogErrorKind::VersionMismatch,
                alloc::format!("want={versions:?} active={vs:?}"),
            ));
        }
        let p = self.points.get(&key.point_id).ok_or_else(|| {
            CatalogError::new(
                CatalogErrorKind::UnknownPoint,
                alloc::format!("point:{}", key.point_id),
            )
        })?;
        if p.asset_id == 0 {
            return Err(CatalogError::new(
                CatalogErrorKind::UnknownAsset,
                alloc::format!("point:{} unbound", key.point_id),
            ));
        }
        if self.tombstones.contains(&p.asset_id) {
            return Err(CatalogError::new(
                CatalogErrorKind::TombstonedAsset,
                alloc::format!("asset:{}", p.asset_id),
            ));
        }
        // 资产 tombstone（bundle 内活表已排除；跨代记忆在此防线）。
        let asset = self.assets.get(&p.asset_id).ok_or_else(|| {
            CatalogError::new(
                CatalogErrorKind::UnknownAsset,
                alloc::format!("asset:{}", p.asset_id),
            )
        })?;
        if asset.is_tombstone() {
            return Err(CatalogError::new(
                CatalogErrorKind::TombstonedAsset,
                alloc::format!("asset:{}", p.asset_id),
            ));
        }
        if p.schema_id != 0 && !self.schemas.contains_key(&p.schema_id) {
            return Err(CatalogError::new(
                CatalogErrorKind::UnknownSchema,
                alloc::format!("schema:{}", p.schema_id),
            ));
        }
        if p.unit_code == 0
            || eneros_telemetry_model::unit_catalog::from_code(p.unit_code).is_none()
        {
            return Err(CatalogError::new(
                CatalogErrorKind::UnknownUnit,
                alloc::format!("unit:{}", p.unit_code),
            ));
        }
        Ok(ObservationSemantics {
            asset_id: p.asset_id,
            schema_id: p.schema_id,
            schema_version: p.schema_version,
            unit_code: p.unit_code,
            role: p.role,
            readback_point_id: p.readback_point_id,
        })
    }

    fn asset(&self, asset_id: u64, version: u64) -> Option<&AssetDescriptor> {
        if version != self.version_set().asset_version {
            return None;
        }
        let a = self.assets.get(&asset_id)?;
        if a.is_tombstone() {
            return None;
        }
        Some(a)
    }

    fn topology_view(&self, site_id: u32, version: u64) -> Result<TopologyView, CatalogError> {
        self.topology_view_of(site_id, version)
    }

    fn control_contract(&self, id: u32, version: u64) -> Result<&ControlContract, CatalogError> {
        if version != self.version_set().contract_version {
            return Err(CatalogError::new(
                CatalogErrorKind::VersionMismatch,
                alloc::format!("contract:{version}"),
            ));
        }
        self.contracts.get(&id).ok_or_else(|| {
            CatalogError::new(
                CatalogErrorKind::UnknownContract,
                alloc::format!("contract:{id}"),
            )
        })
    }

    fn asset_control_contract(&self, asset_id: u64, version: u64) -> Option<&ControlContract> {
        if version != self.version_set().contract_version {
            return None;
        }
        self.control_contract_for_asset(asset_id)
    }

    fn energized_nodes(
        &self,
        switch_states: &[(u32, bool)],
    ) -> Option<alloc::collections::BTreeSet<u64>> {
        self.header.as_ref()?;
        Some(self.energized_view(switch_states).energized_nodes)
    }

    fn feedback_slots(&self) -> Option<crate::model::FeedbackSlots> {
        self.feedback
    }
}

/// 版本化目录集：激活/回滚 + 跨代 tombstone 记忆（[08] §2.3 原子切换）。
///
/// 激活纪律：
/// - 新目录 generation 必须 > 当前激活（回拨拒绝）；
/// - 新目录不得复用任何已知 tombstone 的资产 ID；
/// - 激活失败保持旧目录（不半应用）。
#[derive(Debug, Default)]
pub struct CatalogSet {
    active: Option<Box<CatalogRepository>>,
    previous: Option<Box<CatalogRepository>>,
    /// 跨代 tombstone 记忆（ID 永不复用）。
    tombstones: BTreeSet<u64>,
}

impl CatalogSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// 装载并原子激活；失败 → 状态不变（旧目录继续服务）。
    pub fn activate(&mut self, payload: &[u8]) -> Result<(), CatalogError> {
        let bundle = crate::decode::decode_catalog(payload)?;
        let next = CatalogRepository::build(bundle)?;
        let vs = next.version_set();
        let active_vs = self.active.as_ref().map(|a| a.version_set());
        if let Some(av) = active_vs {
            if vs.generation <= av.generation {
                return Err(CatalogError::new(
                    CatalogErrorKind::ActivationRejected,
                    alloc::format!(
                        "generation-not-increasing:{}<={}",
                        vs.generation,
                        av.generation
                    ),
                ));
            }
        }
        // tombstone 复用检查（跨代记忆；同代内编译期已互斥）。
        for t in self.tombstones.iter() {
            if next.asset(*t, vs.asset_version).is_some() {
                return Err(CatalogError::new(
                    CatalogErrorKind::ActivationRejected,
                    alloc::format!("tombstone-reuse:asset:{t}"),
                ));
            }
        }
        // 新目录中的墓碑并入记忆（跨代 ID 永不复用）。
        for t in next.tombstones.iter() {
            self.tombstones.insert(*t);
        }
        self.previous = self.active.take();
        self.active = Some(Box::new(next));
        Ok(())
    }

    /// 回滚到上一激活目录（[08] §2.7 回滚 gate）。
    pub fn rollback(&mut self) -> bool {
        if self.previous.is_none() {
            return false;
        }
        self.active = self.previous.take();
        true
    }

    pub fn active(&self) -> Option<&CatalogRepository> {
        self.active.as_deref()
    }
}

/// 控制点角色常量（供装配层使用）。
pub use crate::decode::ROLE_CONTROL as CATALOG_ROLE_CONTROL;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::ROLE_CONTROL;

    /// 手工编码一条最小目录 bundle（跳过 TOML 编译面；布局与编译面一致）。
    /// `revive_99` = 把 tombstone 99 换成活资产（跨代复用场景）。
    /// 返回可直接 decode 的 payload。
    pub(crate) fn minimal_payload_variant(gen: u64, revive_99: bool) -> alloc::vec::Vec<u8> {
        use eneros_config_bundle::payload::encode_payload;
        use eneros_config_bundle::schema::catalog as c;
        let mut header = alloc::vec::Vec::with_capacity(c::HEADER_LEN);
        for v in [gen, 0u64, 0u64, 1u64, 1u64, 1u64, 1u64, 1u64] {
            header.extend_from_slice(&v.to_le_bytes());
        }
        // schemas: id=1 v=1
        let mut schemas = alloc::vec::Vec::new();
        schemas.extend_from_slice(&1u32.to_le_bytes());
        schemas.extend_from_slice(&1u32.to_le_bytes());
        schemas.extend_from_slice(&1u16.to_le_bytes());
        schemas.extend_from_slice(&0u16.to_le_bytes());
        // conversions: kW→MW
        let mut conv = alloc::vec::Vec::new();
        conv.extend_from_slice(&1u32.to_le_bytes());
        conv.extend_from_slice(&6u16.to_le_bytes()); // kW
        conv.extend_from_slice(&7u16.to_le_bytes()); // MW
        conv.extend_from_slice(&0.001f64.to_le_bytes());
        // assets: 1 (battery, device 2), 10 (grid)
        let mut assets = alloc::vec::Vec::new();
        assets.extend_from_slice(&2u32.to_le_bytes());
        for (id, kind, dev, topo) in [(1u64, 1u16, 2u16, 2u64), (10u64, 10u16, 0u16, 1u64)] {
            assets.extend_from_slice(&id.to_le_bytes());
            assets.extend_from_slice(&1u32.to_le_bytes()); // site
            assets.extend_from_slice(&1u32.to_le_bytes()); // tenant
            assets.extend_from_slice(&kind.to_le_bytes());
            assets.extend_from_slice(&dev.to_le_bytes());
            assets.extend_from_slice(&topo.to_le_bytes());
            assets.extend_from_slice(&0u16.to_le_bytes()); // flags
            assets.extend_from_slice(&0u16.to_le_bytes()); // pad
        }
        // nodes: 1 grid, 2 der
        let mut nodes = alloc::vec::Vec::new();
        nodes.extend_from_slice(&2u32.to_le_bytes());
        nodes.extend_from_slice(&1u64.to_le_bytes());
        nodes.extend_from_slice(&10u16.to_le_bytes()); // grid
        nodes.extend_from_slice(&0u16.to_le_bytes());
        nodes.extend_from_slice(&2u64.to_le_bytes());
        nodes.extend_from_slice(&5u16.to_le_bytes()); // der
        nodes.extend_from_slice(&0u16.to_le_bytes());
        // edges: 1→2 switch @point 5
        let mut edges = alloc::vec::Vec::new();
        edges.extend_from_slice(&1u32.to_le_bytes());
        edges.extend_from_slice(&1u64.to_le_bytes());
        edges.extend_from_slice(&2u64.to_le_bytes());
        edges.extend_from_slice(&1u16.to_le_bytes()); // switch
        edges.extend_from_slice(&5u32.to_le_bytes());
        edges.extend_from_slice(&500.0f64.to_le_bytes());
        edges.extend_from_slice(&0.0f64.to_le_bytes());
        edges.extend_from_slice(&0u64.to_le_bytes());
        edges.extend_from_slice(&0u64.to_le_bytes());
        edges.extend_from_slice(&0u16.to_le_bytes());
        // points 1..=7, 10, 1001
        let mut points = alloc::vec::Vec::new();
        points.extend_from_slice(&9u32.to_le_bytes());
        for (pid, unit, role, readback) in [
            (1u32, 1u16, 1u8, 0u32),     // V
            (2u32, 3u16, 1u8, 0u32),     // A
            (3u32, 11u16, 1u8, 0u32),    // Hz
            (4u32, 18u16, 1u8, 0u32),    // ratio
            (5u32, 18u16, 3u8, 0u32),    // switch-state
            (6u32, 18u16, 1u8, 0u32),    // ratio
            (7u32, 18u16, 1u8, 0u32),    // ratio
            (10u32, 6u16, 2u8, 1001u32), // kW control
            (1001u32, 6u16, 1u8, 0u32),  // kW
        ] {
            points.extend_from_slice(&pid.to_le_bytes());
            points.extend_from_slice(&1u64.to_le_bytes()); // asset 1
            points.extend_from_slice(&1u32.to_le_bytes()); // schema 1
            points.extend_from_slice(&1u16.to_le_bytes()); // schema ver
            points.extend_from_slice(&unit.to_le_bytes());
            points.push(role);
            points.extend_from_slice(&readback.to_le_bytes());
            points.push(0u8); // flags
        }
        // feedback 5 slots: grid_up=5 soc=4 dc=1 oc=6 ot=7
        let mut fb = alloc::vec::Vec::new();
        fb.extend_from_slice(&5u32.to_le_bytes());
        for (slot, pid) in [(1u8, 5u32), (2, 4), (3, 1), (4, 6), (5, 7)] {
            fb.push(slot);
            fb.extend_from_slice(&pid.to_le_bytes());
            fb.push(0u8);
        }
        // coverage 1..=7
        let mut cov = alloc::vec::Vec::new();
        cov.extend_from_slice(&7u32.to_le_bytes());
        for (pid, unit) in [
            (1u32, 1u16),
            (2, 3),
            (3, 11),
            (4, 18),
            (5, 18),
            (6, 18),
            (7, 18),
        ] {
            cov.extend_from_slice(&pid.to_le_bytes());
            cov.extend_from_slice(&0.0f64.to_le_bytes());
            cov.extend_from_slice(&1.0f64.to_le_bytes());
            cov.extend_from_slice(&5000u32.to_le_bytes());
            cov.extend_from_slice(&unit.to_le_bytes());
            cov.extend_from_slice(&0u16.to_le_bytes());
        }
        // contract 1: asset 1 point 10 kW setpoint
        let mut contracts = alloc::vec::Vec::new();
        contracts.extend_from_slice(&1u32.to_le_bytes());
        contracts.extend_from_slice(&1u32.to_le_bytes()); // id
        contracts.extend_from_slice(&1u64.to_le_bytes()); // asset
        contracts.extend_from_slice(&10u32.to_le_bytes()); // point
        contracts.extend_from_slice(&6u16.to_le_bytes()); // kW
        contracts.extend_from_slice(&2u16.to_le_bytes()); // setpoint
        contracts.extend_from_slice(&(-500.0f64).to_le_bytes());
        contracts.extend_from_slice(&500.0f64.to_le_bytes());
        contracts.extend_from_slice(&3.3333f64.to_le_bytes());
        contracts.extend_from_slice(&30000u32.to_le_bytes());
        contracts.extend_from_slice(&0u64.to_le_bytes());
        contracts.extend_from_slice(&0u64.to_le_bytes());
        contracts.extend_from_slice(&1u16.to_le_bytes());
        contracts.extend_from_slice(&1001u32.to_le_bytes()); // readback
        contracts.extend_from_slice(&0.1f64.to_le_bytes()); // tolerance
        contracts.push(1u8); // reversible
        contracts.push(0u8); // compensation
        contracts.push(2u8); // priority remote
        contracts.push(1u8); // safe hold
        contracts.push(1u8); // lost hold
        contracts.push(0u8); // flags
        contracts.push(0u8); // pad×3
        contracts.push(0u8);
        contracts.push(0u8);
        // safety: overcurrent point 6 limit 1.0, overtemp point 7 limit 1.0
        let mut safety = alloc::vec::Vec::new();
        safety.extend_from_slice(&2u32.to_le_bytes());
        for (cid, kind, pid) in [(1u32, 1u16, 6u32), (2, 2, 7)] {
            safety.extend_from_slice(&cid.to_le_bytes());
            safety.extend_from_slice(&1u64.to_le_bytes());
            safety.extend_from_slice(&kind.to_le_bytes());
            safety.extend_from_slice(&pid.to_le_bytes());
            safety.extend_from_slice(&1.0f64.to_le_bytes());
            safety.extend_from_slice(&0u16.to_le_bytes());
            safety.extend_from_slice(&0u32.to_le_bytes());
        }
        // aliases: market-device 1 → 1; vpp 1 → 1
        let mut aliases = alloc::vec::Vec::new();
        aliases.extend_from_slice(&2u32.to_le_bytes());
        for (kind, legacy) in [(1u8, 1u64), (2, 1)] {
            aliases.push(kind);
            aliases.extend_from_slice(&legacy.to_le_bytes());
            aliases.extend_from_slice(&1u64.to_le_bytes());
            aliases.push(0u8);
        }
        // tombstones: 99（revive_99 时取消墓碑）。
        let mut tombs = alloc::vec::Vec::new();
        if revive_99 {
            tombs.extend_from_slice(&0u32.to_le_bytes());
        } else {
            tombs.extend_from_slice(&1u32.to_le_bytes());
            tombs.extend_from_slice(&99u64.to_le_bytes());
            tombs.extend_from_slice(&1u16.to_le_bytes());
            tombs.extend_from_slice(&0u16.to_le_bytes());
        }

        // revive_99：资产表追加 asset 99（battery，无拓扑绑定）并把 count
        // 补为 3（否则表尾多记录被解码面 trailing-bytes 拒绝——封装纪律）。
        if revive_99 {
            assets[0..4].copy_from_slice(&3u32.to_le_bytes());
            assets.extend_from_slice(&99u64.to_le_bytes());
            assets.extend_from_slice(&1u32.to_le_bytes()); // site
            assets.extend_from_slice(&1u32.to_le_bytes()); // tenant
            assets.extend_from_slice(&1u16.to_le_bytes()); // battery
            assets.extend_from_slice(&0u16.to_le_bytes()); // device
            assets.extend_from_slice(&0u64.to_le_bytes()); // no topo node
            assets.extend_from_slice(&0u16.to_le_bytes()); // flags
            assets.extend_from_slice(&0u16.to_le_bytes()); // pad
        }
        encode_payload([
            (c::FIELD_HEADER, header.as_slice()),
            (c::FIELD_SCHEMAS, schemas.as_slice()),
            (c::FIELD_UNIT_CONVERSIONS, conv.as_slice()),
            (c::FIELD_ASSETS, assets.as_slice()),
            (c::FIELD_TOPO_NODES, nodes.as_slice()),
            (c::FIELD_TOPO_EDGES, edges.as_slice()),
            (c::FIELD_POINT_SEMANTICS, points.as_slice()),
            (c::FIELD_FEEDBACK_SLOTS, fb.as_slice()),
            (c::FIELD_COVERAGE, cov.as_slice()),
            (c::FIELD_CONTROL_CONTRACTS, contracts.as_slice()),
            (c::FIELD_SAFETY_CONSTRAINTS, safety.as_slice()),
            (c::FIELD_ALIASES, aliases.as_slice()),
            (c::FIELD_TOMBSTONES, tombs.as_slice()),
        ])
    }

    #[test]
    fn build_and_resolve_observation() {
        let payload = minimal_payload_variant(1, false);
        let repo = crate::open(&payload).unwrap();
        let vs = repo.version_set();
        assert_eq!(vs.generation, 1);
        // 点 10 控制语义。
        let key = ObservationKey {
            tenant_id: 1,
            site_id: 1,
            source_id: 2,
            device_id: 1,
            point_id: 10,
            mapping_version: 1,
        };
        let sem = repo.resolve_observation(&key, vs).unwrap();
        assert!(sem.control_eligible());
        assert_eq!(sem.role, ROLE_CONTROL);
        assert_eq!(sem.readback_point_id, 1001);
        // 版本不符 → VersionMismatch；未知点 → UnknownPoint。
        let mut wrong = vs;
        wrong.mapping_version = 2;
        assert_eq!(
            repo.resolve_observation(&key, wrong).unwrap_err().kind,
            CatalogErrorKind::VersionMismatch
        );
        let unknown = ObservationKey {
            point_id: 7777,
            ..key
        };
        assert_eq!(
            repo.resolve_observation(&unknown, vs).unwrap_err().kind,
            CatalogErrorKind::UnknownPoint
        );
        // asset 查询版本不符 → None。
        assert!(repo.asset(1, vs.asset_version).is_some());
        assert!(repo.asset(1, 99).is_none());
        assert!(repo.asset(99, vs.asset_version).is_none()); // tombstone
                                                             // 控制合同。
        let cc = repo.control_contract(1, vs.contract_version).unwrap();
        assert_eq!(cc.point_id, 10);
        assert_eq!(cc.readback_point_id, 1001);
        assert_eq!(repo.control_binding_point(1), Some(10));
        // 别名。
        assert_eq!(repo.resolve_alias(1, 1), Some(1));
        assert_eq!(repo.resolve_alias(2, 1), Some(1));
        assert_eq!(repo.resolve_alias(1, 42), None);
    }

    #[test]
    fn feedback_and_coverage_present() {
        let repo = crate::open(&minimal_payload_variant(1, false)).unwrap();
        let fb = repo.feedback_slots().unwrap();
        assert_eq!(fb.grid_up, 5);
        assert_eq!(fb.soc, 4);
        assert_eq!(fb.dc_bus_v, 1);
        assert_eq!(fb.overcurrent, 6);
        assert_eq!(fb.overtemp, 7);
        assert_eq!(repo.coverage().count(), 7);
        assert_eq!(repo.safety_constraints(1).count(), 2);
    }

    #[test]
    fn energized_view_closed_and_open_and_unknown() {
        let repo = crate::open(&minimal_payload_variant(1, false)).unwrap();
        // 开关闭合：asset node 2 带电。
        let v = repo.energized_view(&[(5, true)]);
        assert!(v.resource_controllable(2));
        assert!(v.unknown_switches.is_empty());
        // 断开：node 2 失电。
        let v2 = repo.energized_view(&[(5, false)]);
        assert!(!v2.is_energized(2));
        // 无观测：node 2 失电 + unknown_switches=[5]。
        let v3 = repo.energized_view(&[]);
        assert!(!v3.is_energized(2));
        assert_eq!(v3.unknown_switches, alloc::vec![5]);
        assert!(!v3.resource_controllable(2));
        // 冲突（双值）→ 未知。
        let v4 = repo.energized_view(&[(5, true), (5, false)]);
        assert!(!v4.is_energized(2));
        assert_eq!(v4.conflicted_switches, alloc::vec![5]);
    }

    #[test]
    fn catalog_set_generation_rollback_and_tombstone_reuse() {
        let mut set = CatalogSet::new();
        set.activate(&minimal_payload_variant(1, false)).unwrap();
        assert_eq!(set.active().unwrap().version_set().generation, 1);
        // 同代/回拨拒绝。
        assert_eq!(
            set.activate(&minimal_payload_variant(1, false))
                .unwrap_err()
                .kind,
            CatalogErrorKind::ActivationRejected
        );
        // 新代复活 tombstone 99 → 激活拒绝（跨代 ID 永不复用记忆）。
        assert_eq!(
            set.activate(&minimal_payload_variant(3, true))
                .unwrap_err()
                .kind,
            CatalogErrorKind::ActivationRejected
        );
        // 正常换代 → 激活。
        set.activate(&minimal_payload_variant(3, false)).unwrap();
        assert_eq!(set.active().unwrap().version_set().generation, 3);
        // 回滚。
        assert!(set.rollback());
        assert_eq!(set.active().unwrap().version_set().generation, 1);
        assert!(!set.rollback()); // previous 已被消费（只能回滚一次）
    }

    #[test]
    fn catalog_expiry_and_double_read_determinism() {
        // 有效性：0=不限（生产 catalog 形态）。
        let repo = crate::open(&minimal_payload_variant(1, false)).unwrap();
        assert!(repo.is_valid_at(0));
        assert!(repo.is_valid_at(u64::MAX));
        // 双读确定性：同一 payload 解码两次 → 同 digest、同合同。
        let repo2 = crate::open(&minimal_payload_variant(1, false)).unwrap();
        assert_eq!(repo.digest(), repo2.digest());
        let vs = repo.version_set();
        let c1 = repo.control_contract(1, vs.contract_version).unwrap();
        let c2 = repo2.control_contract(1, vs.contract_version).unwrap();
        assert_eq!(c1.min, c2.min);
        assert_eq!(c1.readback_point_id, c2.readback_point_id);
    }

    /// 首个匹配子串原位等长替换（解码面负例：闭枚举越界注入）。
    fn patch_first(payload: &mut [u8], from: &[u8], to: &[u8]) -> bool {
        debug_assert_eq!(from.len(), to.len());
        if payload.len() < from.len() {
            return false;
        }
        for i in 0..=payload.len() - from.len() {
            if &payload[i..i + from.len()] == from {
                payload[i..i + to.len()].copy_from_slice(to);
                return true;
            }
        }
        false
    }

    /// 解码面闭枚举复核（与模块文档声明一致——编译代际可能不同，解码
    /// 面必须自查）：节点 kind / 边 kind / 合同 class / 约束 kind 越界
    /// → 拒绝装载；约束 id 重复/零 → 拒绝。
    #[test]
    fn decode_rejects_out_of_enum_and_dup_constraint_id() {
        // 节点 kind 闭枚举（1..=10）：der 节点（id=2, kind=5）kind → 77。
        let mut p = minimal_payload_variant(1, false);
        assert!(patch_first(
            &mut p,
            &[2, 0, 0, 0, 0, 0, 0, 0, 5, 0, 0, 0],
            &[2, 0, 0, 0, 0, 0, 0, 0, 77, 0, 0, 0],
        ));
        assert_eq!(crate::open(&p).unwrap_err().kind, CatalogErrorKind::Decode);
        // 边 kind 闭枚举（1=switch 2=branch）：唯一边 kind 1 → 9。
        let mut p = minimal_payload_variant(1, false);
        assert!(patch_first(
            &mut p,
            &[
                1, 0, 0, 0, 0, 0, 0, 0, // from=1
                2, 0, 0, 0, 0, 0, 0, 0, // to=2
                1, 0, // kind=switch
            ],
            &[
                1, 0, 0, 0, 0, 0, 0, 0, //
                2, 0, 0, 0, 0, 0, 0, 0, //
                9, 0, // kind=9（未知）
            ],
        ));
        assert_eq!(crate::open(&p).unwrap_err().kind, CatalogErrorKind::Decode);
        // 合同 class 闭枚举（1=switch 2=setpoint）：class 2 → 7。
        let mut p = minimal_payload_variant(1, false);
        assert!(patch_first(
            &mut p,
            &[
                1, 0, 0, 0, // contract_id=1
                1, 0, 0, 0, 0, 0, 0, 0, // asset=1
                10, 0, 0, 0, // point=10
                6, 0, // unit=kW
                2, 0, // class=setpoint
            ],
            &[
                1, 0, 0, 0, //
                1, 0, 0, 0, 0, 0, 0, 0, //
                10, 0, 0, 0, //
                6, 0, //
                7, 0, // class=7（未知）
            ],
        ));
        assert_eq!(crate::open(&p).unwrap_err().kind, CatalogErrorKind::Decode);
        // 约束 kind 闭枚举（1=overcurrent 2=overtemp）：kind 1 → 5。
        let mut p = minimal_payload_variant(1, false);
        assert!(patch_first(
            &mut p,
            &[
                1, 0, 0, 0, // constraint_id=1
                1, 0, 0, 0, 0, 0, 0, 0, // asset=1
                1, 0, // kind=overcurrent
                6, 0, 0, 0, // point=6
            ],
            &[
                1, 0, 0, 0, //
                1, 0, 0, 0, 0, 0, 0, 0, //
                5, 0, // kind=5（未知）
                6, 0, 0, 0, //
            ],
        ));
        assert_eq!(crate::open(&p).unwrap_err().kind, CatalogErrorKind::Decode);
        // 约束 id 重复：constraint 2 → 1（与首条撞 id）。
        let mut p = minimal_payload_variant(1, false);
        assert!(patch_first(
            &mut p,
            &[
                2, 0, 0, 0, // constraint_id=2
                1, 0, 0, 0, 0, 0, 0, 0, // asset=1
                2, 0, // kind=overtemp
            ],
            &[
                1, 0, 0, 0, // constraint_id=1（重复）
                1, 0, 0, 0, 0, 0, 0, 0, //
                2, 0, //
            ],
        ));
        assert_eq!(crate::open(&p).unwrap_err().kind, CatalogErrorKind::Decode);
    }
}

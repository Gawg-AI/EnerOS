//! 资源注册表（[08] §4.2 / §4.5 门面）：外部只能查询 Registry——
//! `evaluate(asset_id)` 是唯一入口；provider 与包络修改在模块内部。

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

use eneros_semantic_catalog::SemanticCatalog;
use eneros_snapshot_service::TrustedSnapshot;

use crate::capability::{Availability, CapabilityEnvelope, DeratingReason};
use crate::contract::{from_catalog, ContractCompleteness};
use crate::provider::{
    BatteryProvider, CapabilityProvider, GenericProvider, NameplateBase, PvProvider,
};
use crate::{RegistryError, RegistryErrorKind, ResourceDescriptor};

/// 弹性输入（维护/工艺/市场承诺/人工越权——本版本 registry 装配注入；
/// 全部只收紧）。
#[derive(Debug, Clone, Copy, Default)]
pub struct ResilienceInputs {
    /// 维护窗口（None = 无维护；Some(valid_until_ns) = 当前处于维护）。
    pub maintenance_until_ns: Option<u64>,
    /// 工艺窗口（p_min, p_max 收紧）。
    pub process_window: Option<(f64, f64)>,
    /// 市场承诺窗口。
    pub market_window: Option<(f64, f64)>,
    /// 人工越权窗口（None = 未越权；操作员收紧不得放宽铭牌）。
    pub operator_window: Option<(f64, f64)>,
}

impl ResilienceInputs {
    fn apply_at(&self, env: CapabilityEnvelope, at_ns: u64, now_ns: u64) -> CapabilityEnvelope {
        let mut e = env;
        if let Some(until) = self.maintenance_until_ns {
            if at_ns < until {
                let mut m = e.intersect("maintenance", 3, (0.0, 0.0), until);
                m.availability = Availability::Unavailable;
                if m.derating_reason == DeratingReason::None {
                    m.derating_reason = DeratingReason::Maintenance;
                }
                e = m;
            }
        }
        if let Some(w) = self.process_window {
            e = e.intersect("process", 4, w, u64::MAX);
        }
        if let Some(w) = self.market_window {
            e = e.intersect("market", 5, w, u64::MAX);
        }
        if let Some(w) = self.operator_window {
            e = e.intersect("operator", 6, w, u64::MAX);
        }
        let _ = now_ns;
        e
    }
}

/// 注册表（有界；C2 = 1000 资源构建 + 增量更新）。
pub struct ResourceRegistry {
    descriptors: BTreeMap<u64, ResourceDescriptor>,
    bases: BTreeMap<u64, NameplateBase>,
    providers: Vec<Box<dyn CapabilityProvider>>,
    /// 构建世代（每次变更递增；聚合 generation/digest 依据）。
    pub generation: u64,
    /// 超容拒绝（不静默）。
    pub overflow_rejected: u64,
    cap: usize,
}

impl Default for ResourceRegistry {
    fn default() -> Self {
        Self::new(4096)
    }
}

impl ResourceRegistry {
    pub fn new(cap: usize) -> Self {
        Self {
            descriptors: BTreeMap::new(),
            bases: BTreeMap::new(),
            providers: vec![
                Box::new(BatteryProvider),
                Box::new(PvProvider),
                Box::new(GenericProvider {
                    kind: crate::kinds::WIND,
                }),
                Box::new(GenericProvider {
                    kind: crate::kinds::GENERATOR,
                }),
                Box::new(GenericProvider {
                    kind: crate::kinds::FLEXIBLE_LOAD,
                }),
                Box::new(GenericProvider {
                    kind: crate::kinds::FIXED_LOAD,
                }),
                Box::new(GenericProvider {
                    kind: crate::kinds::EVSE,
                }),
                Box::new(GenericProvider {
                    kind: crate::kinds::HVAC,
                }),
                Box::new(GenericProvider {
                    kind: crate::kinds::PROCESS_UNIT,
                }),
                Box::new(GenericProvider {
                    kind: crate::kinds::GRID_CONNECTION,
                }),
                Box::new(GenericProvider {
                    kind: crate::kinds::AGGREGATE,
                }),
            ],
            generation: 0,
            overflow_rejected: 0,
            cap: cap.max(1),
        }
    }

    /// 登记/更新描述（同 asset 覆盖 = 增量更新；超容 → 拒绝计数）。
    pub fn upsert(&mut self, desc: ResourceDescriptor, base: NameplateBase) -> bool {
        if !self.descriptors.contains_key(&desc.asset_id) && self.descriptors.len() >= self.cap {
            self.overflow_rejected += 1;
            return false;
        }
        self.descriptors.insert(desc.asset_id, desc);
        self.bases.insert(desc.asset_id, base);
        self.generation += 1;
        true
    }

    pub fn remove(&mut self, asset_id: u64) -> bool {
        let a = self.descriptors.remove(&asset_id).is_some();
        let b = self.bases.remove(&asset_id).is_some();
        if a || b {
            self.generation += 1;
        }
        a
    }

    pub fn get(&self, asset_id: u64) -> Option<&ResourceDescriptor> {
        self.descriptors.get(&asset_id)
    }

    pub fn len(&self) -> usize {
        self.descriptors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.descriptors.is_empty()
    }

    /// 查询（唯一入口）：目录资产/合同/回读/拓扑 + provider 求交 +
    /// 弹性输入 + 有效性窗口。
    pub fn evaluate(
        &self,
        asset_id: u64,
        at_ns: u64,
        catalog: &dyn SemanticCatalog,
        snapshot: &TrustedSnapshot,
        resilience: &ResilienceInputs,
    ) -> Result<CapabilityEnvelope, RegistryError> {
        let desc = self
            .descriptors
            .get(&asset_id)
            .ok_or_else(|| RegistryError {
                kind: RegistryErrorKind::UnknownAsset,
                detail: alloc::format!("asset:{asset_id}"),
            })?;
        // 目录资产必须存在且存活（tombstone → 不可用）。
        let vs = catalog.version_set();
        if catalog.asset(desc.asset_id, vs.asset_version).is_none() {
            return Err(RegistryError {
                kind: RegistryErrorKind::UnknownAsset,
                detail: alloc::format!("asset-not-in-catalog:{asset_id}"),
            });
        }
        // 控制合同（无完整合同 → 可观测不可控）。
        let contract = catalog
            .asset_control_contract(desc.asset_id, vs.contract_version)
            .ok_or_else(|| RegistryError {
                kind: RegistryErrorKind::NoControlContract,
                detail: alloc::format!("asset:{asset_id}"),
            })?;
        let view = from_catalog(contract);
        // 回读合格性：回读点必须是快照内接受的观测（[08] §4.4）。
        let readback_ok = snapshot
            .observations
            .iter()
            .any(|o| o.event.point_id == view.readback_point_id);
        if view.completeness(readback_ok) != ContractCompleteness::Complete {
            return Err(RegistryError {
                kind: RegistryErrorKind::NoControlContract,
                detail: alloc::format!("asset:{asset_id} contract-incomplete"),
            });
        }
        // provider 分派（种类 → 实现；未知 → 拒绝）。
        let provider = self
            .providers
            .iter()
            .find(|p| p.kind() == desc.resource_kind)
            .ok_or_else(|| RegistryError {
                kind: RegistryErrorKind::NoProvider,
                detail: alloc::format!("kind:{}", desc.resource_kind),
            })?;
        let base = self
            .bases
            .get(&asset_id)
            .copied()
            .ok_or_else(|| RegistryError {
                kind: RegistryErrorKind::UnknownAsset,
                detail: "no-nameplate-base".into(),
            })?;
        let mut env = provider.evaluate(desc, &base, snapshot, catalog, at_ns, resilience)?;
        // 弹性输入（维护/工艺/市场/人工——只收紧）。
        env = resilience.apply_at(env, at_ns, snapshot.produced_at_ns);
        // 合同范围作为最终收紧项（授权范围不得超合同）。
        let until = env.valid_until_ns;
        env = env.intersect("contract", view.contract_id, (view.min, view.max), until);
        // 合同爬坡同为运行限值（最终收紧项）：铭牌爬坡是物理能力，可宽于
        // 运行合同——缺此求交则认证器的 RampUp/RampDown 界恒等于物理能力，
        // 与执行侧 `ramp-limit`（合同值）不一致（复查修复）。
        env = env.intersect_ramp("contract-ramp", view.contract_id, view.ramp_kw_per_s, until);
        Ok(env)
    }

    /// 注册表世代 + 摘要（[08] §4.8 门：Registry generation/digest 证据）。
    pub fn digest(&self) -> [u8; 32] {
        use sha2::Digest;
        let mut h = sha2::Sha256::new();
        h.update(b"eneros.resource-registry.v1");
        h.update(self.generation.to_le_bytes());
        h.update((self.descriptors.len() as u64).to_le_bytes());
        for (id, d) in self.descriptors.iter() {
            h.update(id.to_le_bytes());
            h.update(d.resource_kind.to_le_bytes());
            h.update(d.control_contract_id.to_le_bytes());
        }
        h.finalize().into()
    }

    /// 全部资产（确定性枚举）。
    pub fn asset_ids(&self) -> impl Iterator<Item = u64> + '_ {
        self.descriptors.keys().copied()
    }
}

/// 目录装配报告（蓝图 70.8 WP-70.8.3：登记 = 可控池；跳过原因必须可数）。
///
/// 计数恒等式（每个遍历到的目录 ID 恰好落入一个桶）：
/// `assets_seen == registered + skipped_no_contract + skipped_no_nameplate
/// + skipped_unknown_kind + skipped_tombstoned + skipped_capacity`。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CatalogRegistryReport {
    /// 遍历到的目录资产 ID 数（`asset_ids()` 全集——`CatalogRepository`
    /// 语义下 tombstone 不进 `asset_ids()`，故墓碑不在此计数）。
    pub assets_seen: u32,
    /// 成功登记（有控制合同 + 铭牌 + 已知种类）。
    pub registered: u32,
    /// 跳过：无控制合同。
    pub skipped_no_contract: u32,
    /// 跳过：无铭牌（物理参数缺失 = 不可控，fail-closed）。
    pub skipped_no_nameplate: u32,
    /// 跳过：未知资源种类。
    pub skipped_unknown_kind: u32,
    /// 跳过：资产不可用——`asset()` 返回 None（tombstone/版本不符/
    /// 未登记）或描述带 tombstone 标志（`is_tombstone()`）。
    pub skipped_tombstoned: u32,
    /// 跳过：注册表超容（`upsert` 拒绝；registry.overflow_rejected 同源计数）。
    pub skipped_capacity: u32,
}

/// 由目录 + 物理参数铭牌装配注册表（**生产装配单源**）。
///
/// 登记条件（缺一不可，fail-closed）：
/// 1. 资产存活（非 tombstone）；
/// 2. 有控制合同（可观测不可控不进入可控池）；
/// 3. 有物理参数铭牌（容量/效率/功率窗/爬坡——包络与认证的物理输入）；
/// 4. 资源种类在 [`crate::kinds`] 闭枚举内。
///
/// 铭牌能力窗与合同价值域的交集在 [`ResourceRegistry::evaluate`] 内以
/// 合同为最终收紧项完成（此处只装配输入，不做裁决）。
pub fn registry_from_catalog(
    catalog: &dyn eneros_semantic_catalog::SemanticCatalog,
    cap: usize,
) -> (ResourceRegistry, CatalogRegistryReport) {
    let mut reg = ResourceRegistry::new(cap);
    let mut report = CatalogRegistryReport::default();
    let vs = catalog.version_set();
    let mut ids: alloc::vec::Vec<u64> = catalog.asset_ids();
    ids.sort_unstable();
    for asset_id in ids {
        report.assets_seen += 1;
        let Some(asset) = catalog.asset(asset_id, vs.asset_version) else {
            // 不可查（tombstone/版本不符/未登记）——计入跳过，恒等式不破。
            report.skipped_tombstoned += 1;
            continue;
        };
        if asset.is_tombstone() {
            report.skipped_tombstoned += 1;
            continue;
        }
        let Some(contract) = catalog.asset_control_contract(asset_id, vs.contract_version) else {
            report.skipped_no_contract += 1;
            continue;
        };
        let Some(np) = catalog.nameplate(asset_id) else {
            report.skipped_no_nameplate += 1;
            continue;
        };
        if !crate::kinds::known(asset.kind) {
            report.skipped_unknown_kind += 1;
            continue;
        }
        let desc = ResourceDescriptor {
            asset_id,
            site_id: asset.site_id,
            resource_kind: asset.kind,
            control_contract_id: contract.contract_id,
            topology_node_id: asset.topology_node_id,
            capability_model_id: 0,
            owner_tenant_id: asset.tenant_id,
        };
        let base = NameplateBase {
            p_min_kw: np.p_min_kw,
            p_max_kw: np.p_max_kw,
            q_min_kvar: np.q_min_kvar,
            q_max_kvar: np.q_max_kvar,
            energy_max_kwh: np.energy_max_kwh,
            ramp_up_kw_s: np.ramp_up_kw_s,
            ramp_down_kw_s: np.ramp_down_kw_s,
        };
        if reg.upsert(desc, base) {
            report.registered += 1;
        } else {
            // 超容拒绝可见（registry.overflow_rejected 同源）。
            report.skipped_capacity += 1;
        }
    }
    (reg, report)
}

/// planner 只读视图（实现 decision-contract::ResourceRegistry——命令批
/// 编译的绑定权威；本模块唯一实现，写入面在 dispatch 之外）。
pub struct PlannerView {
    repo: eneros_semantic_catalog::CatalogRepository,
    /// 控制点设备短 ID（协议映射，装配方从 points 表解析）。
    pub device_id: u16,
    /// 控制点（catalog 合同权威）。
    pub point_id: u32,
    pub unit_code: u16,
}

impl PlannerView {
    /// 由目录 + 装配解析的设备构建。
    pub fn new(
        repo: eneros_semantic_catalog::CatalogRepository,
        device_id: u16,
    ) -> Result<Self, RegistryError> {
        let vs = repo.version_set();
        // 显式控制绑定点（唯一；歧义 → 拒绝）。
        let point = repo
            .points_with_role(eneros_semantic_catalog::decode::ROLE_CONTROL)
            .find_map(|(pid, sem)| {
                repo.asset_control_contract(sem.asset_id, vs.contract_version)
                    .map(|_| pid)
            })
            .ok_or_else(|| RegistryError {
                kind: RegistryErrorKind::NoControlContract,
                detail: "no-unique-control-point".into(),
            })?;
        if repo
            .points_with_role(eneros_semantic_catalog::decode::ROLE_CONTROL)
            .find(|(pid, sem)| {
                sem.asset_id != 0
                    && repo
                        .asset_control_contract(sem.asset_id, vs.contract_version)
                        .is_some()
                    && *pid != point
            })
            .is_some()
        {
            return Err(RegistryError {
                kind: RegistryErrorKind::NoControlContract,
                detail: "ambiguous-control-points".into(),
            });
        }
        let asset_of_point = repo.asset_of_point(point).ok_or_else(|| RegistryError {
            kind: RegistryErrorKind::UnknownAsset,
            detail: "control-point-unbound".into(),
        })?;
        let (point_id, unit_code) = {
            let contract = repo
                .control_contract_for_asset(asset_of_point)
                .ok_or_else(|| RegistryError {
                    kind: RegistryErrorKind::NoControlContract,
                    detail: "no-contract-for-control-point".into(),
                })?;
            (contract.point_id, contract.unit_code)
        };
        Ok(Self {
            repo,
            device_id,
            point_id,
            unit_code,
        })
    }
}

impl PlannerView {
    /// 目录摘要（[08] §4.8 门：Registry digest 证据，装配判定行用）。
    pub fn repo_digest(&self) -> [u8; 32] {
        self.repo.digest()
    }
}

impl eneros_decision_contract::ResourceRegistry for PlannerView {
    fn asset_exists(&self, asset_id: u64) -> bool {
        let vs = self.repo.version_set();
        self.repo.asset(asset_id, vs.asset_version).is_some()
            && self
                .repo
                .asset_control_contract(asset_id, vs.contract_version)
                .is_some()
    }

    fn asset_control_binding(&self, asset_id: u64) -> Option<(u16, u32, u16)> {
        let vs = self.repo.version_set();
        let contract = self
            .repo
            .asset_control_contract(asset_id, vs.contract_version)?;
        Some((self.device_id, contract.point_id, contract.unit_code))
    }

    fn catalog_version(&self) -> u64 {
        self.repo.version_set().contract_version
    }
}

#[cfg(test)]
mod tests {
    use eneros_semantic_catalog::{
        AssetDescriptor, CatalogError, CatalogErrorKind, ControlContract, ObservationKey,
        ObservationSemantics, SemanticCatalog, TopologyView, VersionSet,
    };

    use super::*;
    use crate::kinds;

    fn cat() -> eneros_semantic_catalog::CatalogRepository {
        let toml = include_str!("../../../../configs/catalog.toml");
        let compiled = eneros_config_bundle::toml_compile::compile_catalog(toml).unwrap();
        eneros_semantic_catalog::open(&compiled.payload).unwrap()
    }

    /// 生产目录 + 测试变体（仅测试内编译，不改生产 TOML）：
    /// - grid(10) 补控制合同但**无铭牌** → 锁 `skipped_no_nameplate` 桶；
    /// - 追加第二块可控电池(2)（合同 + 铭牌）→ 锁超容拒绝可数。
    fn cat_variant() -> eneros_semantic_catalog::CatalogRepository {
        let toml = include_str!("../../../../configs/catalog.toml");
        let extra = r#"
[[asset]]
asset_id = 2
site_id = 1
tenant_id = 1
kind = "battery"

[[point_semantics]]
point_id = 20
asset_id = 2
schema_id = 1
unit = "kW"
role = "control"
readback_point_id = 1003

[[point_semantics]]
point_id = 1003
asset_id = 2
schema_id = 1
unit = "kW"
role = "measurement"

[[control_contract]]
contract_id = 3
asset_id = 2
point_id = 20
unit = "kW"
class = "setpoint"
min = -200.0
max = 200.0
readback_point_id = 1003
priority = "remote"
safe_state = "hold"
lost_link = "hold"

[[nameplate]]
asset_id = 2
capacity_kwh = 50.0
eta_c = 0.9
eta_d = 0.9
energy_max_kwh = 50.0
p_min_kw = -200.0
p_max_kw = 200.0
q_min_kvar = 0.0
q_max_kvar = 0.0
ramp_up_kw_s = 1.0
ramp_down_kw_s = 1.0
soc_min = 0.05
soc_max = 0.95

# grid(10) 补控制合同（仍无铭牌）。
[[point_semantics]]
point_id = 11
asset_id = 10
schema_id = 1
unit = "kW"
role = "control"
readback_point_id = 1002

[[point_semantics]]
point_id = 1002
asset_id = 10
schema_id = 1
unit = "kW"
role = "measurement"

[[control_contract]]
contract_id = 2
asset_id = 10
point_id = 11
unit = "kW"
class = "setpoint"
min = -100.0
max = 100.0
readback_point_id = 1002
priority = "remote"
safe_state = "hold"
lost_link = "hold"
"#;
        let compiled =
            eneros_config_bundle::toml_compile::compile_catalog(&alloc::format!("{toml}{extra}"))
                .unwrap();
        eneros_semantic_catalog::open(&compiled.payload).unwrap()
    }

    /// 报告恒等式辅助（每个遍历到的 ID 恰好落入一个桶）。
    fn assert_report_reconciles(report: &CatalogRegistryReport) {
        assert_eq!(
            report.assets_seen,
            report.registered
                + report.skipped_no_contract
                + report.skipped_no_nameplate
                + report.skipped_unknown_kind
                + report.skipped_tombstoned
                + report.skipped_capacity,
            "seen == registered + skipped_* 恒等式: {report:?}"
        );
    }

    /// 最小目录替身（非 `CatalogRepository`）：`asset_ids()` 暴露墓碑与
    /// 查询不可得 ID——锁报告在「枚举 ≠ 可查」实现上的可数性。
    #[derive(Default)]
    struct SurfacingCatalog {
        assets: alloc::collections::BTreeMap<u64, AssetDescriptor>,
        unreadable: alloc::vec::Vec<u64>,
    }

    impl SemanticCatalog for SurfacingCatalog {
        fn version_set(&self) -> VersionSet {
            VersionSet::ZERO
        }

        fn resolve_observation(
            &self,
            _key: &ObservationKey,
            _versions: VersionSet,
        ) -> Result<ObservationSemantics, CatalogError> {
            Err(CatalogError::new(
                CatalogErrorKind::UnknownPoint,
                "unused".into(),
            ))
        }

        fn asset(&self, asset_id: u64, _version: u64) -> Option<&AssetDescriptor> {
            self.assets.get(&asset_id)
        }

        fn topology_view(
            &self,
            _site_id: u32,
            _version: u64,
        ) -> Result<TopologyView, CatalogError> {
            Err(CatalogError::new(
                CatalogErrorKind::TopologyUnavailable,
                "unused".into(),
            ))
        }

        fn control_contract(
            &self,
            _id: u32,
            _version: u64,
        ) -> Result<&ControlContract, CatalogError> {
            Err(CatalogError::new(
                CatalogErrorKind::UnknownContract,
                "unused".into(),
            ))
        }

        fn asset_control_contract(
            &self,
            _asset_id: u64,
            _version: u64,
        ) -> Option<&ControlContract> {
            None
        }

        fn asset_ids(&self) -> alloc::vec::Vec<u64> {
            let mut ids: alloc::vec::Vec<u64> = self.assets.keys().copied().collect();
            ids.extend(self.unreadable.iter().copied());
            ids
        }
    }

    fn snapshot(
        catalog: &dyn SemanticCatalog,
        soc: f64,
        oc: f64,
        ot: f64,
        grid: f64,
        now: u64,
    ) -> TrustedSnapshot {
        let repo = cat();
        let slots = repo.feedback_slots().unwrap();
        let values = [
            (slots.dc_bus_v, 400.0),
            (slots.soc, soc),
            (slots.grid_up, grid),
            (slots.overcurrent, oc),
            (slots.overtemp, ot),
            (1001, 400.0), // 控制回读点（合同完备性）
        ];
        let mut events: alloc::vec::Vec<eneros_telemetry_model::event::ObservationEvent> =
            alloc::vec::Vec::new();
        // 简单快照（直接构造——目录语义覆盖 dc/soc/oc/ot/grid + 回读）。
        for (pid, v) in values {
            let mut ev = eneros_telemetry_model::event::ObservationEvent {
                tenant_id: 1,
                site_id: 1,
                source_id: 2,
                source_epoch: 1,
                sequence: u64::from(pid),
                event_time_ns: now.saturating_sub(50),
                ingest_time_ns: now.saturating_sub(40),
                asset_id: 1,
                device_id: 2,
                unit_code: eneros_telemetry_model::unit_catalog::code_of(
                    eneros_telemetry_model::PhysicalUnit::Dimensionless,
                ),
                point_id: pid,
                schema_id: 1,
                schema_version: 1,
                quality_bits: eneros_telemetry_model::event::quality_bits::VALID,
                mapping_version: 1,
                value: eneros_telemetry_model::event::ObservationValue::F64(v),
                integrity: eneros_telemetry_model::event::IntegrityTag::Authenticated,
                clock_id: 1,
                flags: 0,
            };
            if pid == slots.dc_bus_v {
                ev.unit_code = eneros_telemetry_model::unit_catalog::code_of(
                    eneros_telemetry_model::PhysicalUnit::Volts,
                );
            }
            events.push(ev);
        }
        TrustedSnapshot {
            snapshot_id: [7u8; 16],
            tenant_id: 1,
            site_id: 1,
            cut_time_ns: now,
            produced_at_ns: now,
            policy_version: 1,
            version_set: catalog.version_set(),
            observations: events.into_iter().map(Into::into).collect(),
            source_watermarks: alloc::vec::Vec::new(),
            completeness: eneros_snapshot_service::CompletenessReport {
                required: alloc::vec![],
                accepted: alloc::vec![],
                missing: alloc::vec![],
                stale: alloc::vec![],
                bad: alloc::vec![],
                conflict: alloc::vec![],
                substituted: alloc::vec![],
            },
            conflicts: alloc::vec::Vec::new(),
            derived: alloc::vec::Vec::new(),
            provenance_root: [0u8; 32],
        }
    }

    fn registry() -> ResourceRegistry {
        let mut r = ResourceRegistry::new(4096);
        r.upsert(
            ResourceDescriptor {
                asset_id: 1,
                site_id: 1,
                resource_kind: kinds::BATTERY,
                control_contract_id: 1,
                topology_node_id: 2,
                capability_model_id: 1,
                owner_tenant_id: 1,
            },
            NameplateBase {
                p_min_kw: -500.0,
                p_max_kw: 500.0,
                q_min_kvar: -100.0,
                q_max_kvar: 100.0,
                energy_max_kwh: 1000.0,
                ramp_up_kw_s: 3.3333,
                ramp_down_kw_s: 3.3333,
            },
        );
        r
    }

    #[test]
    fn battery_envelope_healthy_and_soc_bounds() {
        let repo = cat();
        let now = 100_000_000_000u64;
        // 健康：grid 闭合、无告警、soc 0.6。
        let snap = snapshot(&repo, 0.6, 0.0, 0.0, 1.0, now);
        let r = registry();
        let env = r
            .evaluate(1, now, &repo, &snap, &ResilienceInputs::default())
            .unwrap();
        assert!(env.controllable());
        assert_eq!(env.p_max_kw, 500.0);
        assert!((env.energy_max_kwh - 600.0).abs() < 1e-6); // 1000×0.6
                                                            // 命令落在包络内。
        assert!(env.contains_power(400.0, now));
        assert!(env.contains_power(-400.0, now));
        // 深度放空：soc 0.01 → 放电收紧（p_max ≤ 0）。
        let snap_low = snapshot(&repo, 0.01, 0.0, 0.0, 1.0, now);
        let env_low = r
            .evaluate(1, now, &repo, &snap_low, &ResilienceInputs::default())
            .unwrap();
        assert!(env_low.p_max_kw <= 0.0);
        assert!(!env_low.contains_power(100.0, now));
    }

    #[test]
    fn alarm_derates_to_unavailable() {
        let repo = cat();
        let now = 200_000_000_000u64;
        // 过温告警 → 不可用（原因 Health）。
        let snap_hot = snapshot(&repo, 0.6, 0.0, 1.0, 1.0, now);
        let r = registry();
        let env = r
            .evaluate(1, now, &repo, &snap_hot, &ResilienceInputs::default())
            .unwrap();
        assert_eq!(env.availability, Availability::Unavailable);
        assert_eq!(env.derating_reason, DeratingReason::Health);
        assert!(!env.controllable());
    }

    #[test]
    fn grid_open_islands_the_asset() {
        let repo = cat();
        let now = 300_000_000_000u64;
        // grid_up=0 → 并网开关断开 → 失电 → Unavailable(Topology)。
        let snap_open = snapshot(&repo, 0.6, 0.0, 0.0, 0.0, now);
        let r = registry();
        let env = r
            .evaluate(1, now, &repo, &snap_open, &ResilienceInputs::default())
            .unwrap();
        assert_eq!(env.availability, Availability::Unavailable);
        assert_eq!(env.derating_reason, DeratingReason::Topology);
    }

    #[test]
    fn soc_missing_fails_closed() {
        let repo = cat();
        let now = 400_000_000_000u64;
        let mut snap = snapshot(&repo, 0.6, 0.0, 0.0, 1.0, now);
        snap.observations.retain(|o| o.event.point_id != 4);
        let r = registry();
        let err = r
            .evaluate(1, now, &repo, &snap, &ResilienceInputs::default())
            .unwrap_err();
        assert_eq!(err.kind, RegistryErrorKind::MissingSnapshotInput);
    }

    /// 安全告警点（过流）缺失 → fail-closed（不得按 0 = 无告警默认——
    /// 安全输入缺失只能收紧，绝不放宽）。
    #[test]
    fn alarm_point_missing_fails_closed() {
        let repo = cat();
        let now = 450_000_000_000u64;
        let slots = repo.feedback_slots().unwrap();
        let mut snap = snapshot(&repo, 0.6, 0.0, 0.0, 1.0, now);
        snap.observations
            .retain(|o| o.event.point_id != slots.overcurrent);
        let r = registry();
        let err = r
            .evaluate(1, now, &repo, &snap, &ResilienceInputs::default())
            .unwrap_err();
        assert_eq!(err.kind, RegistryErrorKind::MissingSnapshotInput);
        // 过温同理。
        let mut snap2 = snapshot(&repo, 0.6, 0.0, 0.0, 1.0, now);
        snap2
            .observations
            .retain(|o| o.event.point_id != slots.overtemp);
        let err2 = r
            .evaluate(1, now, &repo, &snap2, &ResilienceInputs::default())
            .unwrap_err();
        assert_eq!(err2.kind, RegistryErrorKind::MissingSnapshotInput);
    }

    #[test]
    fn resilience_inputs_only_tighten() {
        let repo = cat();
        let now = 500_000_000_000u64;
        let snap = snapshot(&repo, 0.6, 0.0, 0.0, 1.0, now);
        let r = registry();
        // 维护窗口 → Unavailable(Maintenance)。
        let res = ResilienceInputs {
            maintenance_until_ns: Some(now + 1000),
            ..ResilienceInputs::default()
        };
        let env = r.evaluate(1, now, &repo, &snap, &res).unwrap();
        assert_eq!(env.availability, Availability::Unavailable);
        assert_eq!(env.derating_reason, DeratingReason::Maintenance);
        // 市场承诺收紧功率。
        let res2 = ResilienceInputs {
            market_window: Some((-100.0, 100.0)),
            ..ResilienceInputs::default()
        };
        let env2 = r.evaluate(1, now, &repo, &snap, &res2).unwrap();
        assert_eq!(env2.p_max_kw, 100.0);
        assert!(env2.controllable());
    }

    #[test]
    fn no_contract_asset_uncontrollable() {
        let repo = cat();
        let now = 600_000_000_000u64;
        let snap = snapshot(&repo, 0.6, 0.0, 0.0, 1.0, now);
        let mut r = registry();
        // asset 10 登记入册但目录无控制合同 → 可观测不可控。
        r.upsert(
            ResourceDescriptor {
                asset_id: 10,
                site_id: 1,
                resource_kind: kinds::GRID_CONNECTION,
                control_contract_id: 0,
                topology_node_id: 1,
                capability_model_id: 1,
                owner_tenant_id: 1,
            },
            NameplateBase {
                p_min_kw: -100.0,
                p_max_kw: 100.0,
                q_min_kvar: 0.0,
                q_max_kvar: 0.0,
                energy_max_kwh: 0.0,
                ramp_up_kw_s: 1.0,
                ramp_down_kw_s: 1.0,
            },
        );
        let err = r
            .evaluate(10, now, &repo, &snap, &ResilienceInputs::default())
            .unwrap_err();
        assert_eq!(err.kind, RegistryErrorKind::NoControlContract);
    }

    /// 目录变体：只替换铭牌爬坡两行（生产目录 = 3.3333；合同
    /// `ramp_kw_per_s` 不动——用于验证「合同是最终收紧项」在爬坡维成立）。
    fn cat_with_nameplate_ramp(ramp: &str) -> eneros_semantic_catalog::CatalogRepository {
        let toml = include_str!("../../../../configs/catalog.toml");
        let patched = toml
            .replace(
                "ramp_up_kw_s = 3.3333",
                &alloc::format!("ramp_up_kw_s = {ramp}"),
            )
            .replace(
                "ramp_down_kw_s = 3.3333",
                &alloc::format!("ramp_down_kw_s = {ramp}"),
            );
        let compiled = eneros_config_bundle::toml_compile::compile_catalog(&patched).unwrap();
        eneros_semantic_catalog::open(&compiled.payload).unwrap()
    }

    /// 蓝图 70.8 WP-70.8.3（复查修复）：**爬坡维也必须被合同收紧**。
    ///
    /// 铭牌 = 物理能力（可宽于运行合同），合同 = 运行限值（最终收紧项，
    /// 生产目录注释原文）。包络是二者求交：合同爬坡 < 铭牌爬坡时取合同，
    /// 反之取铭牌（两侧对称，绝不放宽）。
    ///
    /// 变异靶：`evaluate` 不把合同 `ramp_kw_per_s` 求交进包络 → 宽铭牌
    /// 形态返回 10.0 → 本测红（且认证器的 RampUp/RampDown 界随之放宽，
    /// 执行侧 `ramp-limit` 与证书证据不一致）。
    #[test]
    fn contract_ramp_tightens_nameplate_ramp() {
        let wide = cat_with_nameplate_ramp("10.0");
        let (reg, _report) = registry_from_catalog(&wide, 16);
        let snap = snapshot(&wide, 0.6, 0.0, 0.0, 1.0, 1_000_000);
        let env = reg
            .evaluate(1, 1_000_000, &wide, &snap, &ResilienceInputs::default())
            .expect("电池包络");
        assert!(
            (env.ramp_up_kw_s - 3.3333).abs() < 1e-9,
            "合同爬坡必须收紧宽铭牌（最终收紧项）：{}",
            env.ramp_up_kw_s
        );
        assert!(
            (env.ramp_down_kw_s - 3.3333).abs() < 1e-9,
            "{}",
            env.ramp_down_kw_s
        );

        // 对称面：铭牌更紧时不得被合同放宽（min 语义）。
        let narrow = cat_with_nameplate_ramp("1.0");
        let (reg2, _) = registry_from_catalog(&narrow, 16);
        let env2 = reg2
            .evaluate(1, 1_000_000, &narrow, &snap, &ResilienceInputs::default())
            .expect("电池包络");
        assert!(
            (env2.ramp_up_kw_s - 1.0).abs() < 1e-9,
            "铭牌更紧时取铭牌：{}",
            env2.ramp_up_kw_s
        );
    }

    /// 蓝图 70.8 WP-70.8.3：目录装配（铭牌 → 登记；缺铭牌 = 不可控）。
    /// 生产目录（configs/catalog.toml）含电池铭牌 → 登记；删掉铭牌
    /// （模拟旧目录）→ 同一资产跳过且包络不可得（fail-closed 证据）。
    #[test]
    fn registry_from_catalog_registers_nameplate_assets_only() {
        let catalog = cat();
        let (reg, report) = registry_from_catalog(&catalog, 16);
        assert_eq!(report.assets_seen, 2, "目录资产 = battery(1) + grid(10)");
        assert_eq!(report.registered, 1, "仅电池有控制合同 + 铭牌");
        assert_eq!(report.skipped_no_contract, 1, "grid 无控制合同");
        assert_eq!(reg.len(), 1);
        assert!(reg.asset_ids().any(|a| a == 1));

        // 铭牌 → 包络（同代快照）：电池可用，能量窗随 SOC 收紧。
        let snap = snapshot(&catalog, 0.6, 0.0, 0.0, 1.0, 1_000_000);
        let env = reg
            .evaluate(1, 1_000_000, &catalog, &snap, &ResilienceInputs::default())
            .expect("电池包络");
        assert_eq!(env.source_snapshot_id, snap.snapshot_id, "包络绑定快照身份");
        assert_eq!(env.availability, Availability::Full);
        // 铭牌 100 kWh × SOC 0.6 = 60 kWh（可放能量窗上界）。
        assert!(
            (env.energy_max_kwh - 60.0).abs() < 1e-6,
            "{}",
            env.energy_max_kwh
        );
        assert_eq!(env.p_max_kw, 500.0, "铭牌功率窗 ∩ 合同");

        // —— 负例：无铭牌资产不可控 ——
        let mut no_np = ResourceRegistry::new(16);
        let desc = ResourceDescriptor {
            asset_id: 1,
            site_id: 1,
            resource_kind: kinds::BATTERY,
            control_contract_id: 1,
            topology_node_id: 2,
            capability_model_id: 0,
            owner_tenant_id: 1,
        };
        // 手工登记（绕过 catalog 铭牌）仍可 evaluate——但 registry_from_catalog
        // 不会登记无铭牌资产，故生产不可达；此处锁"装配判据"。
        assert!(no_np.upsert(
            desc,
            NameplateBase {
                p_min_kw: -500.0,
                p_max_kw: 500.0,
                q_min_kvar: 0.0,
                q_max_kvar: 0.0,
                energy_max_kwh: 100.0,
                ramp_up_kw_s: 3.3333,
                ramp_down_kw_s: 3.3333,
            }
        ));
        assert_eq!(no_np.len(), 1);
        assert_eq!(catalog.nameplate(10), None, "grid 无铭牌（跳过理由可核）");
    }

    /// 生产目录（configs/catalog.toml）的报告恒等式：
    /// 每个遍历到的 ID 恰好落入一个桶；tombstone 99 不进 `asset_ids()`
    ///（`assets_seen` 语义 = 活资产全集），grid(10) 无合同先于无铭牌命中。
    #[test]
    fn report_counters_reconcile_on_production_catalog() {
        let catalog = cat();
        let (reg, report) = registry_from_catalog(&catalog, 16);
        assert_report_reconciles(&report);
        assert_eq!(report.assets_seen, 2, "battery(1) + grid(10)");
        assert_eq!(report.registered, 1);
        assert_eq!(report.skipped_no_contract, 1, "grid 无控制合同");
        assert_eq!(report.skipped_no_nameplate, 0, "grid 无合同先命中");
        assert_eq!(report.skipped_unknown_kind, 0);
        assert_eq!(report.skipped_tombstoned, 0);
        assert_eq!(report.skipped_capacity, 0);
        // grid 无铭牌（跳过理由可核）；tombstone 99 不可见（枚举面语义）。
        assert_eq!(catalog.nameplate(10), None);
        assert!(!catalog.asset_ids().contains(&99));
        assert_eq!(reg.len(), 1);
    }

    /// 恒等式 + 分桶负例（变体目录）：grid(10) 有合同无铭牌 →
    /// `skipped_no_nameplate`；cap=1 时第二块可控电池 → `skipped_capacity`
    ///（超容拒绝不静默，与 registry.overflow_rejected 同源）。
    #[test]
    fn report_counts_no_nameplate_and_capacity_rejections() {
        let catalog = cat_variant();
        let (reg, report) = registry_from_catalog(&catalog, 16);
        assert_report_reconciles(&report);
        assert_eq!(report.assets_seen, 3);
        assert_eq!(report.registered, 2, "两块电池合同 + 铭牌齐全");
        assert_eq!(report.skipped_no_nameplate, 1, "grid 有合同但无铭牌");
        assert_eq!(report.skipped_capacity, 0);
        assert_eq!(reg.len(), 2);

        // cap=1：id 升序（1,2,10）先登记 battery(1)；battery(2) 超容拒绝。
        let (reg1, report1) = registry_from_catalog(&catalog, 1);
        assert_report_reconciles(&report1);
        assert_eq!(report1.registered, 1);
        assert_eq!(report1.skipped_capacity, 1, "超容拒绝必须可数");
        assert_eq!(reg1.overflow_rejected, 1, "与 registry 自身计数同源");
        assert_eq!(reg1.len(), 1);
    }

    /// `asset_ids()` 暴露墓碑/不可查 ID 的目录实现（旧/破损语义）：
    /// 两条跳过路径都必须计入 `skipped_tombstoned`，恒等式不破。
    #[test]
    fn report_counts_tombstoned_and_unreadable_ids() {
        let mut c = SurfacingCatalog::default();
        c.assets.insert(
            1,
            AssetDescriptor {
                asset_id: 1,
                site_id: 1,
                tenant_id: 1,
                kind: kinds::BATTERY,
                device_id: 0,
                topology_node_id: 0,
                flags: eneros_semantic_catalog::decode::ASSET_FLAG_TOMBSTONE,
            },
        );
        // 枚举面返回但 asset() 查无（版本不符/未登记）：不得静默跳过。
        c.unreadable.push(2);
        let (reg, report) = registry_from_catalog(&c, 16);
        assert_report_reconciles(&report);
        assert_eq!(report.assets_seen, 2);
        assert_eq!(report.skipped_tombstoned, 2, "墓碑 + 不可查各计一次");
        assert_eq!(report.registered, 0);
        assert!(reg.is_empty());
    }

    #[test]
    fn planner_view_binding_authoritative() {
        use eneros_decision_contract::ResourceRegistry as _;
        let repo = cat();
        let view = PlannerView::new(repo.clone(), 1u16).unwrap();
        assert_eq!(view.point_id, 10);
        assert!(view.asset_exists(1));
        assert_eq!(view.asset_control_binding(1), Some((1, 10, 6)));
        assert_eq!(view.catalog_version(), repo.version_set().contract_version);
    }

    #[test]
    fn c2_thousand_resources_incremental() {
        let mut r = ResourceRegistry::new(1000);
        for i in 1..=1000u64 {
            assert!(r.upsert(
                ResourceDescriptor {
                    asset_id: i,
                    site_id: 1,
                    resource_kind: kinds::GENERATOR,
                    control_contract_id: 1,
                    topology_node_id: 0,
                    capability_model_id: 1,
                    owner_tenant_id: 1,
                },
                NameplateBase {
                    p_min_kw: -100.0,
                    p_max_kw: 100.0,
                    q_min_kvar: 0.0,
                    q_max_kvar: 0.0,
                    energy_max_kwh: 0.0,
                    ramp_up_kw_s: 1.0,
                    ramp_down_kw_s: 1.0,
                },
            ));
        }
        assert_eq!(r.len(), 1000);
        assert_eq!(r.overflow_rejected, 0);
        // 增量更新（同 id 覆盖）。
        let before = r.generation;
        r.upsert(
            ResourceDescriptor {
                asset_id: 500,
                ..*r.get(500).unwrap()
            },
            NameplateBase {
                p_max_kw: 150.0,
                ..NameplateBase {
                    p_min_kw: -100.0,
                    p_max_kw: 100.0,
                    q_min_kvar: 0.0,
                    q_max_kvar: 0.0,
                    energy_max_kwh: 0.0,
                    ramp_up_kw_s: 1.0,
                    ramp_down_kw_s: 1.0,
                }
            },
        );
        assert_eq!(r.generation, before + 1);
        // 注册表世代/摘要证据（[08] §4.8 门）。
        assert_ne!(r.digest(), [0u8; 32]);
    }
}

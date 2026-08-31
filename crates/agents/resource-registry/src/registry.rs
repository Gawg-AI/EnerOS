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
    use eneros_semantic_catalog::SemanticCatalog;

    use super::*;
    use crate::kinds;

    fn cat() -> eneros_semantic_catalog::CatalogRepository {
        let toml = include_str!("../../../../configs/catalog.toml");
        let compiled = eneros_config_bundle::toml_compile::compile_catalog(toml).unwrap();
        eneros_semantic_catalog::open(&compiled.payload).unwrap()
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

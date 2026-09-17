//! 能力提供器（[08] §4.2 内部）：每资源种类的 `CapabilityProvider`；
//! 外部只能调 Registry，不能直接调 provider 或修改 envelope。

use eneros_semantic_catalog::SemanticCatalog;
use eneros_snapshot_service::TrustedSnapshot;

use crate::capability::{Availability, CapabilityEnvelope, DeratingReason};
use crate::registry::ResilienceInputs;
use crate::{RegistryError, RegistryErrorKind, ResourceDescriptor};

/// 资源种类码（[`crate::kinds`] 闭枚举；与 catalog asset kind 对齐）。
pub type ResourceKind = u16;

/// 快照输入提取（provider 从快照取本资源相关状态；缺输入 → 按规则收紧）。
pub struct SnapshotSlice<'a> {
    pub snapshot: &'a TrustedSnapshot,
}

impl<'a> SnapshotSlice<'a> {
    /// 取点值（最新合格观测；非 F64/非有限 → None）。
    pub fn point_value(&self, point_id: u32) -> Option<f64> {
        self.snapshot
            .observations
            .iter()
            .find(|o| o.event.point_id == point_id)
            .and_then(|o| match o.event.value {
                eneros_telemetry_model::event::ObservationValue::F64(v) if v.is_finite() => Some(v),
                _ => None,
            })
    }

    /// 开关状态表（快照全部有效 F64 点 → >0.5 = 闭合）。
    pub fn switch_states(&self) -> alloc::vec::Vec<(u32, bool)> {
        self.snapshot
            .observations
            .iter()
            .filter(|o| {
                o.event.quality_bits & eneros_telemetry_model::event::quality_bits::VALID != 0
            })
            .filter_map(|o| match o.event.value {
                eneros_telemetry_model::event::ObservationValue::F64(v) if v.is_finite() => {
                    Some((o.event.point_id, v > 0.5))
                }
                _ => None,
            })
            .collect()
    }
}

/// 铭牌基座（registry 装配期注入；来自 import DTO 或 catalog 合同）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NameplateBase {
    pub p_min_kw: f64,
    pub p_max_kw: f64,
    pub q_min_kvar: f64,
    pub q_max_kvar: f64,
    pub energy_max_kwh: f64,
    pub ramp_up_kw_s: f64,
    pub ramp_down_kw_s: f64,
}

/// 能力提供器（内部 trait；Registry 唯一调用方）。
pub trait CapabilityProvider {
    fn kind(&self) -> ResourceKind;

    /// 给定资源/快照/目录，产出**当前能力窗口**（只收紧，绝不放大）。
    fn evaluate(
        &self,
        resource: &ResourceDescriptor,
        base: &NameplateBase,
        snapshot: &TrustedSnapshot,
        catalog: &dyn SemanticCatalog,
        at_ns: u64,
        resilience: &ResilienceInputs,
    ) -> Result<CapabilityEnvelope, RegistryError>;
}

/// 拓扑带电收紧（registry 内部；资源节点失电/未知 → Unavailable）。
pub fn apply_topology(
    env: CapabilityEnvelope,
    node_id: u64,
    snapshot: &TrustedSnapshot,
    catalog: &dyn SemanticCatalog,
    at_ns: u64,
) -> CapabilityEnvelope {
    let slice = SnapshotSlice { snapshot };
    let states = slice.switch_states();
    match catalog.energized_nodes(&states) {
        Some(nodes) if node_id != 0 && nodes.contains(&node_id) => env,
        _ => {
            // 未知/失电拓扑 → 收紧到不可用（[08] §4.3 拓扑项；
            // 状态冲突或未知使资源不可控——[08] §2.4）。
            let mut e = env;
            e.availability = Availability::Unavailable;
            if e.derating_reason == DeratingReason::None {
                e.derating_reason = DeratingReason::Topology;
            }
            let _ = at_ns;
            e
        }
    }
}

/// 电池储能提供器：铭牌 ∩ 电气告警（oc/ot 置位 → 不可用）∩ SOC/能量
/// 边界（soc 缺 → fail-closed MissingSnapshotInput；SOC ≤ 0 → 能量窗归零，
/// 放电收紧）。
pub struct BatteryProvider;

impl CapabilityProvider for BatteryProvider {
    fn kind(&self) -> ResourceKind {
        crate::kinds::BATTERY
    }

    fn evaluate(
        &self,
        resource: &ResourceDescriptor,
        base: &NameplateBase,
        snapshot: &TrustedSnapshot,
        catalog: &dyn SemanticCatalog,
        at_ns: u64,
        _resilience: &ResilienceInputs,
    ) -> Result<CapabilityEnvelope, RegistryError> {
        let slice = SnapshotSlice { snapshot };
        let slots = catalog.feedback_slots().ok_or_else(|| RegistryError {
            kind: RegistryErrorKind::MissingSnapshotInput,
            detail: "catalog-no-feedback-slots".into(),
        })?;
        // 电气告警：过流/过温置位 → 不可用（安全告警绝不放宽）；告警点
        // 缺失 → fail-closed（安全输入缺失 ≠ 无告警，不得按 0 默认）。
        let oc = slice
            .point_value(slots.overcurrent)
            .ok_or_else(|| RegistryError {
                kind: RegistryErrorKind::MissingSnapshotInput,
                detail: alloc::format!("overcurrent-point:{}", slots.overcurrent),
            })?;
        let ot = slice
            .point_value(slots.overtemp)
            .ok_or_else(|| RegistryError {
                kind: RegistryErrorKind::MissingSnapshotInput,
                detail: alloc::format!("overtemp-point:{}", slots.overtemp),
            })?;
        let mut env = base_env(base, resource, snapshot, at_ns);
        if oc > 0.5 || ot > 0.5 {
            env.availability = Availability::Unavailable;
            env.derating_reason = DeratingReason::Health;
            return Ok(env);
        }
        // SOC/能量边界（[08] §4.7 SOC 边界）：soc 缺 → fail-closed。
        let soc = snapshot
            .asset_soc(catalog, resource.asset_id)
            .ok_or_else(|| RegistryError {
                kind: RegistryErrorKind::MissingSnapshotInput,
                detail: alloc::format!("asset-soc:{}", resource.asset_id),
            })?;
        // 能量上限随 SOC 收紧（剩余可放能量 = 总量 × SOC；可充能量 =
        // 总量 × (1-SOC)）。
        env.energy_max_kwh = base.energy_max_kwh * soc;
        env.q_min_kvar *= soc;
        env.q_max_kvar *= soc;
        let until = env.valid_until_ns;
        if soc <= 0.05 {
            // 深度放空：放电方向不可用（p_max 收紧到 0）。
            let mut e = env.intersect("energy", 1, (f64::NEG_INFINITY, 0.0), until);
            // 交集不空时语义仍成立；若原本放电受限 → 保持。
            if e.p_max_kw < 0.0 {
                e.availability = Availability::Unavailable;
                e.derating_reason = DeratingReason::EnergyBound;
            }
            env = e;
        }
        if soc >= 0.95 {
            // 满充：充电方向不可用。
            let e = env.intersect("energy", 2, (0.0, f64::INFINITY), until);
            env = if e.p_max_kw > 0.0 {
                e
            } else {
                let mut e2 = e;
                e2.availability = Availability::Unavailable;
                e2.derating_reason = DeratingReason::EnergyBound;
                e2
            };
        }
        Ok(apply_topology(
            env,
            resource.topology_node_id,
            snapshot,
            catalog,
            at_ns,
        ))
    }
}

/// 光伏提供器：铭牌 ∩ 拓扑带电。
pub struct PvProvider;

impl CapabilityProvider for PvProvider {
    fn kind(&self) -> ResourceKind {
        crate::kinds::PV
    }

    fn evaluate(
        &self,
        resource: &ResourceDescriptor,
        base: &NameplateBase,
        snapshot: &TrustedSnapshot,
        catalog: &dyn SemanticCatalog,
        at_ns: u64,
        _resilience: &ResilienceInputs,
    ) -> Result<CapabilityEnvelope, RegistryError> {
        let env = base_env(base, resource, snapshot, at_ns);
        Ok(apply_topology(
            env,
            resource.topology_node_id,
            snapshot,
            catalog,
            at_ns,
        ))
    }
}

/// 通用提供器（Wind/Generator/Load/EVSE/HVAC/GridConnection/其他）：
/// 铭牌 ∩ 拓扑带电。
pub struct GenericProvider {
    pub kind: ResourceKind,
}

impl CapabilityProvider for GenericProvider {
    fn kind(&self) -> ResourceKind {
        self.kind
    }

    fn evaluate(
        &self,
        resource: &ResourceDescriptor,
        base: &NameplateBase,
        snapshot: &TrustedSnapshot,
        catalog: &dyn SemanticCatalog,
        at_ns: u64,
        _resilience: &ResilienceInputs,
    ) -> Result<CapabilityEnvelope, RegistryError> {
        let env = base_env(base, resource, snapshot, at_ns);
        Ok(apply_topology(
            env,
            resource.topology_node_id,
            snapshot,
            catalog,
            at_ns,
        ))
    }
}

/// 基座 → 初始包络（有效性 = 快照生命期；0 值成员按铭牌计算）。
/// 有效期单源 = `CONTROL_HORIZON_MS`（DEC-70-13 ①：包络生命周期 ≥
/// 计划时域 4×15s=60s——此前本地 +30s 双胞胎常量使多时段计划第 3 步
/// 起被认证器 `Envelope` 拒，2026-09-17 R10 门实弹捕获）。
fn base_env(
    base: &NameplateBase,
    resource: &ResourceDescriptor,
    snapshot: &TrustedSnapshot,
    _at_ns: u64,
) -> CapabilityEnvelope {
    let mut env = CapabilityEnvelope {
        asset_id: resource.asset_id,
        valid_from_ns: snapshot.produced_at_ns,
        valid_until_ns: snapshot
            .produced_at_ns
            .saturating_add(eneros_decision_contract::CONTROL_HORIZON_MS * 1_000_000),
        p_min_kw: base.p_min_kw,
        p_max_kw: base.p_max_kw,
        q_min_kvar: base.q_min_kvar,
        q_max_kvar: base.q_max_kvar,
        energy_min_kwh: 0.0,
        energy_max_kwh: base.energy_max_kwh,
        ramp_up_kw_s: base.ramp_up_kw_s,
        ramp_down_kw_s: base.ramp_down_kw_s,
        min_on_ns: 0,
        min_off_ns: 0,
        availability: Availability::Full,
        derating_reason: DeratingReason::None,
        source_snapshot_id: snapshot.snapshot_id,
        tightenings: alloc::vec::Vec::new(),
        generation: snapshot.version_set.generation,
    };
    env = env.sanitize();
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap_at(produced_ns: u64) -> TrustedSnapshot {
        TrustedSnapshot {
            snapshot_id: [7u8; 16],
            tenant_id: 1,
            site_id: 1,
            cut_time_ns: produced_ns,
            produced_at_ns: produced_ns,
            policy_version: 1,
            version_set: eneros_semantic_catalog::VersionSet {
                generation: 1,
                mapping_version: 1,
                topology_version: 1,
                asset_version: 1,
                contract_version: 1,
                safety_version: 1,
            },
            observations: alloc::vec::Vec::new(),
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

    fn base() -> NameplateBase {
        NameplateBase {
            p_min_kw: -5.0,
            p_max_kw: 5.0,
            q_min_kvar: -1.0,
            q_max_kvar: 1.0,
            energy_max_kwh: 10.0,
            ramp_up_kw_s: 3.33,
            ramp_down_kw_s: 3.33,
        }
    }

    fn descriptor() -> ResourceDescriptor {
        ResourceDescriptor {
            asset_id: 1,
            site_id: 1,
            resource_kind: crate::kinds::BATTERY,
            control_contract_id: 1,
            topology_node_id: 0,
            capability_model_id: 0,
            owner_tenant_id: 1,
        }
    }

    /// DEC-70-13 ① 红线：基座包络有效期必须覆盖控制窗（= 计划时域
    /// 4×15s=60s）。红 = 旧本地 +30s 双胞胎常量：第 3 步（+45s）起超窗，
    /// 认证器 `Envelope { asset_id }` 拒——R10 多时段结构性不可达
    /// （2026-09-17 R10 门实弹捕获，`TRUSTED-ERR … Envelope { asset_id: 1 }`）。
    #[test]
    fn base_env_validity_covers_control_horizon() {
        let produced = 1_700_000_000_000_000_000u64;
        let env = base_env(&base(), &descriptor(), &snap_at(produced), produced);
        let horizon_ns = eneros_decision_contract::CONTROL_HORIZON_MS * 1_000_000;
        assert!(
            env.valid_until_ns >= produced + horizon_ns,
            "包络有效期 {:?}ns < 产刻+控制窗 {}ns（DEC-70-13 ① 违约）",
            env.valid_until_ns.saturating_sub(produced),
            horizon_ns
        );
        // 末步起始（+45s）必须在窗内——认证器按步起始判覆盖。
        let last_step_start = produced + 45 * 1_000_000_000;
        assert!(last_step_start >= env.valid_from_ns && last_step_start <= env.valid_until_ns);
    }
}

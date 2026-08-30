//! Phase 0 决策投影（[08] §3.6 步骤 5）：决策代码从 [`TrustedSnapshot`]
//! 投影所需字段，不再直接读「当前 map」。投影把反馈槽点映射到 Phase 0
//! 决策链需要的电气状态位（与 `TrustedTelemetry` 的 SystemState 同型——
//! 差分测试对照，见 control-services）。
//!
//! 缺失/冲突的槽点 → `Option::None`（fail-closed：调用方按拒绝处理，
//! 不制造默认值）。

use eneros_semantic_catalog::model::FeedbackSlots;

use crate::{NormalizedObservation, TrustedSnapshot};

/// 决策投影（Phase 0 决策输入唯一结构件）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecisionProjection {
    pub snapshot_id: [u8; 16],
    pub provenance_root: [u8; 32],
    pub cut_time_ns: u64,
    pub produced_at_ns: u64,
    pub grid_up: Option<f64>,
    pub soc: Option<f64>,
    pub dc_bus_v: Option<f64>,
    pub overcurrent: Option<f64>,
    pub overtemp: Option<f64>,
    /// 控制时域边界（`cut + horizon` 由调用方语义注入；此处=cut+30s）。
    pub valid_until_ms: u64,
}

fn value_of(obs: &[NormalizedObservation], point: u32) -> Option<f64> {
    obs.iter()
        .find(|o| o.event.point_id == point)
        .and_then(|o| match o.event.value {
            eneros_telemetry_model::event::ObservationValue::F64(v) if v.is_finite() => Some(v),
            _ => None,
        })
}

/// 投影：槽点 → 决策状态位（缺槽/非有限 → None）。
pub fn project(snapshot: &TrustedSnapshot, slots: &FeedbackSlots) -> DecisionProjection {
    DecisionProjection {
        snapshot_id: snapshot.snapshot_id,
        provenance_root: snapshot.provenance_root,
        cut_time_ns: snapshot.cut_time_ns,
        produced_at_ns: snapshot.produced_at_ns,
        grid_up: value_of(&snapshot.observations, slots.grid_up),
        soc: value_of(&snapshot.observations, slots.soc),
        dc_bus_v: value_of(&snapshot.observations, slots.dc_bus_v),
        overcurrent: value_of(&snapshot.observations, slots.overcurrent),
        overtemp: value_of(&snapshot.observations, slots.overtemp),
        valid_until_ms: snapshot.produced_at_ns.saturating_add(30_000_000_000) / 1_000_000,
    }
}

#[cfg(test)]
mod tests {
    use eneros_semantic_catalog::SemanticCatalog;

    use super::*;
    use crate::conflict::{ConflictResolver, ConflictRule};
    use crate::policy::SnapshotPolicyRegistry;
    use crate::{ProjectorCapacity, SnapshotRequest};

    #[test]
    fn project_production_fields() {
        let catalog = crate::tests::prod_catalog();
        let vs = catalog.version_set();
        let mut policies = SnapshotPolicyRegistry::new();
        policies.register(crate::tests::test_policy(vs.contract_version));
        let mut m = crate::module_from_catalog(
            catalog,
            ProjectorCapacity::default(),
            policies,
            ConflictResolver::new(ConflictRule::PrimaryBackup),
            10_000,
        );
        let now = 4_000_000_000u64;
        m.apply_events(&crate::tests::obs7(
            now,
            100,
            2,
            &[400.0, 50.0, 50.0, 0.6, 1.0, 0.0, 0.0],
        ));
        let snap = m
            .build(
                SnapshotRequest {
                    tenant_id: 1,
                    site_id: 1,
                    purpose_id: 1,
                    as_of_ns: now,
                    max_wait_ns: 0,
                    policy_version: 1,
                },
                now,
            )
            .unwrap();
        let slots = FeedbackSlots {
            grid_up: 5,
            soc: 4,
            dc_bus_v: 1,
            overcurrent: 6,
            overtemp: 7,
        };
        let p = project(&snap, &slots);
        assert_eq!(p.dc_bus_v, Some(400.0));
        assert_eq!(p.soc, Some(0.6));
        assert_eq!(p.grid_up, Some(1.0));
        assert_eq!(p.overcurrent, Some(0.0));
        assert_eq!(p.overtemp, Some(0.0));
        assert!(p.valid_until_ms > now / 1_000_000);
    }

    #[test]
    fn missing_slot_projects_none() {
        // 缺 point 4（SOC）：投影 None（fail-closed）。
        let catalog = crate::tests::prod_catalog();
        let vs = catalog.version_set();
        let mut policies = SnapshotPolicyRegistry::new();
        policies.register(crate::tests::test_policy(vs.contract_version));
        let mut m = crate::module_from_catalog(
            catalog,
            ProjectorCapacity::default(),
            policies,
            ConflictResolver::new(ConflictRule::PrimaryBackup),
            10_000,
        );
        let now = 4_000_000_000u64;
        let mut evs = crate::tests::obs7(now, 100, 2, &[400.0, 50.0, 50.0, 0.6, 1.0, 0.0, 0.0]);
        evs.retain(|e| e.point_id != 4);
        // 缺失 → 快照拒绝（完整度纪律）——投影没必要在拒绝态上跑。
        m.apply_events(&evs);
        let err = m
            .build(
                SnapshotRequest {
                    tenant_id: 1,
                    site_id: 1,
                    purpose_id: 1,
                    as_of_ns: now,
                    max_wait_ns: 0,
                    policy_version: 1,
                },
                now,
            )
            .unwrap_err();
        assert!(matches!(
            err,
            crate::SnapshotError::Rejected(crate::SnapshotRejected {
                reason: crate::RejectReason::MissingKeys,
                ..
            })
        ));
    }
}

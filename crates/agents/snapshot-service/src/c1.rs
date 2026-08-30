//! C1 负载（[08] §3.6 步骤 6）：10k 观测、100 来源、并发 purpose。
//! host 载体（目标级长跑 = 存储句柄移交后登记，Phase 2 先例）。

use alloc::vec::Vec;

use eneros_telemetry_model::event::{
    quality_bits, IntegrityTag, ObservationEvent, ObservationValue,
};

/// 生产形状 7 点单位码（点 1-7：V/A/Hz/ratio×4）。
const PROD_UNITS: [u16; 7] = [
    eneros_telemetry_model::unit_catalog::code_of(eneros_telemetry_model::PhysicalUnit::Volts),
    eneros_telemetry_model::unit_catalog::code_of(eneros_telemetry_model::PhysicalUnit::Amperes),
    eneros_telemetry_model::unit_catalog::code_of(eneros_telemetry_model::PhysicalUnit::Hertz),
    eneros_telemetry_model::unit_catalog::code_of(
        eneros_telemetry_model::PhysicalUnit::Dimensionless,
    ),
    eneros_telemetry_model::unit_catalog::code_of(
        eneros_telemetry_model::PhysicalUnit::Dimensionless,
    ),
    eneros_telemetry_model::unit_catalog::code_of(
        eneros_telemetry_model::PhysicalUnit::Dimensionless,
    ),
    eneros_telemetry_model::unit_catalog::code_of(
        eneros_telemetry_model::PhysicalUnit::Dimensionless,
    ),
];

/// 造 100 来源 × 100 点 = 10k 事件（确定性种子化）。点 1-7 = 生产语义
/// （目录可解析，进快照必需集）；点 1001..1093 = 填充观测（目录外，
/// 只能投影，不能进快照——[08] §2.7：单位和资产缺失事件不能进入控制）。
pub fn c1_events(now_ns: u64, seed: u64) -> Vec<ObservationEvent> {
    let mut out = Vec::with_capacity(10_000);
    for source in 1..=100u32 {
        for point in 1..=100u32 {
            let seq = seed + u64::from(point);
            let v = (source as f64) * 0.5 + (point as f64) * 0.25;
            let unit_code = if point <= 7 {
                PROD_UNITS[(point - 1) as usize]
            } else {
                eneros_telemetry_model::unit_catalog::code_of(
                    eneros_telemetry_model::PhysicalUnit::Dimensionless,
                )
            };
            out.push(ObservationEvent {
                tenant_id: 1,
                site_id: 1,
                source_id: source,
                source_epoch: 1,
                sequence: seq,
                event_time_ns: now_ns.saturating_sub(1000) + u64::from(point),
                ingest_time_ns: now_ns.saturating_sub(900) + u64::from(point),
                asset_id: 1,
                device_id: 2,
                unit_code,
                point_id: if point <= 7 { point } else { 1000 + point },
                schema_id: 1,
                schema_version: 1,
                quality_bits: quality_bits::VALID,
                mapping_version: 1,
                value: ObservationValue::F64(v),
                integrity: IntegrityTag::Authenticated,
                clock_id: 1,
                flags: 0,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use eneros_semantic_catalog::SemanticCatalog;

    use super::*;
    use crate::conflict::{ConflictResolver, ConflictRule};
    use crate::policy::{SnapshotPolicy, SnapshotPolicyRegistry};
    use crate::{ProjectorCapacity, SnapshotRequest};

    /// C1：10k 观测 / 100 来源 / 并发 purpose（16 个）——
    /// 全部构建成功且 provenance 确定性；投影/水位容量守恒。
    #[test]
    fn c1_10k_obs_100_sources_concurrent_purposes() {
        let catalog = crate::tests::prod_catalog();
        let vs = catalog.version_set();
        let mut policies = SnapshotPolicyRegistry::new();
        // 16 个并发 purpose：每个 required = 生产 7 点（点 1-7）。
        for purpose in 0..16u32 {
            policies.register(SnapshotPolicy {
                purpose_id: purpose + 1,
                policy_version: 1,
                required: alloc::vec![1u32, 2, 3, 4, 5, 6, 7],
                optional: Vec::new(),
                allowed_lateness_ns: 10_000,
                max_age_ns: 1_000_000,
                min_quality_bits_ok: true,
                require_integrity: true,
                degraded_ok: false,
                substitution: None,
                allowed_sources: None,
                catalog_version: vs.contract_version,
            });
        }
        let mut module = crate::module_from_catalog(
            catalog,
            ProjectorCapacity {
                max_points: 10_000,
                max_events_per_point: 4,
            },
            policies,
            ConflictResolver::new(ConflictRule::PrimaryBackup),
            1_000,
        );
        let now = 5_000_000_000u64;
        module.apply_events(&c1_events(now, 100));
        assert_eq!(module.watermark_count(), 100);
        assert_eq!(module.projected_points(), 100);
        let mut roots: Vec<[u8; 32]> = Vec::new();
        for purpose in 1..=16u32 {
            let snap = module
                .build(
                    SnapshotRequest {
                        tenant_id: 1,
                        site_id: 1,
                        purpose_id: purpose,
                        as_of_ns: now,
                        max_wait_ns: 0,
                        policy_version: 1,
                    },
                    now,
                )
                .unwrap();
            assert_eq!(snap.observations.len(), 7);
            assert!(snap.completeness.is_complete());
            roots.push(snap.provenance_root);
        }
        // 并发目的相互独立且确定性：重复构建同 root。
        let snap_again = module
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
        assert_eq!(snap_again.provenance_root, roots[0]);
    }
}

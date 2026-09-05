//! 蓝图 61 v0.314.34：TrustedTelemetry evaluator ↔ TrustedSnapshot 差分
//! 测试（[08] §3.6 步骤 4「迁移现有 TrustedTelemetry evaluator 的安全检查
//! 到 policy/normalizer，保留差分测试」）。
//!
//! 同一样本集合两条路径独立裁决；健康用例两者接受且投影状态逐位相等，
//! 拒绝用例两者拒绝且根因类别一致（fail-closed 双向）。生产 `allowed_
//! sources` 与 `mapping_version` 取自 config（points [[source]] +
//! bundle generation），两侧同源注入。

#![cfg(test)]
mod snapshot_diff_tests {
    #![allow(clippy::disallowed_macros)]

    use eneros_safety_validator::{
        EvaluatorConfig, TelemetrySample, TelemetryTrustEvaluator, TrustRejection,
    };
    use eneros_semantic_catalog::model::FeedbackSlots;
    use eneros_semantic_catalog::SemanticCatalog;
    use eneros_snapshot_service::{
        conflict::{ConflictResolver, ConflictRule},
        policy::{SnapshotPolicy, SnapshotPolicyRegistry},
        projection::project,
        ProjectorCapacity, RejectReason, SnapshotError, SnapshotModule, SnapshotRequest,
    };
    use eneros_telemetry_model::event::{
        quality_bits, IntegrityTag, ObservationEvent, ObservationValue,
    };

    /// 生产 7 点事件（与 [svc-telemetry] OBS 轨同型：点 1-7、epoch=1、
    /// 事件时间 = now-50ns；单位码按点 1-7 = V/A/Hz/ratio×4）。
    fn events(now_ns: u64, values: [f64; 7], seq0: u64, source: u32) -> Vec<ObservationEvent> {
        use eneros_telemetry_model::unit_catalog;
        use eneros_telemetry_model::PhysicalUnit;
        let units = [
            unit_catalog::code_of(PhysicalUnit::Volts),
            unit_catalog::code_of(PhysicalUnit::Amperes),
            unit_catalog::code_of(PhysicalUnit::Hertz),
            unit_catalog::code_of(PhysicalUnit::Dimensionless),
            unit_catalog::code_of(PhysicalUnit::Dimensionless),
            unit_catalog::code_of(PhysicalUnit::Dimensionless),
            unit_catalog::code_of(PhysicalUnit::Dimensionless),
        ];
        (1..=7)
            .zip(values.iter().copied())
            .map(|(point, v)| ObservationEvent {
                tenant_id: 1,
                site_id: 1,
                source_id: source,
                source_epoch: 1,
                sequence: seq0 + u64::from(point),
                event_time_ns: now_ns.saturating_sub(50),
                ingest_time_ns: now_ns.saturating_sub(40),
                asset_id: 1,
                device_id: 2,
                unit_code: units[(point - 1) as usize],
                point_id: point,
                schema_id: 1,
                schema_version: 1,
                quality_bits: quality_bits::VALID,
                mapping_version: 1,
                value: ObservationValue::F64(v),
                integrity: IntegrityTag::Authenticated,
                clock_id: 1,
                flags: 0,
            })
            .collect()
    }

    /// 事件 → 旧视图样本（obs_shadow 影子对照同函数）。
    fn samples(evs: &[ObservationEvent]) -> Vec<TelemetrySample> {
        evs.iter()
            .filter_map(crate::obs_shadow::convert_to_legacy)
            .collect()
    }

    fn evaluator_raw() -> TelemetryTrustEvaluator {
        let cfg = EvaluatorConfig {
            coverage: eneros_control_dispatch::feedback::safety_coverage(),
            bindings: eneros_safety_validator::StateBindings {
                voltage: 1,
                current: 2,
                frequency: 3,
                soc: 4,
            },
            mapping_version: 1,
            max_skew_ms: 1000,
            allowed_sources: alloc::vec![2],
            require_integrity: true,
            control_horizon_ms: 30_000,
        };
        TelemetryTrustEvaluator::new(cfg)
    }

    fn evaluator() -> TelemetryTrustEvaluator {
        let mut ev = evaluator_raw();
        // 68.2 per-source 种子：事件 source=2、epoch=1（registry 分配形态）。
        ev.restore(&[(2, 1)], 0, alloc::collections::BTreeMap::new());
        ev
    }

    fn compile_catalog_payload() -> alloc::vec::Vec<u8> {
        let toml = include_str!("../../../../configs/catalog.toml");
        eneros_config_bundle::toml_compile::compile_catalog(toml)
            .expect("compile catalog")
            .payload
    }

    fn snapshot_module() -> SnapshotModule {
        let catalog = eneros_semantic_catalog::open(&compile_catalog_payload()).expect("catalog");
        let vs = catalog.version_set();
        let mut policies = SnapshotPolicyRegistry::new();
        policies.register(SnapshotPolicy {
            purpose_id: 1,
            policy_version: 1,
            required: alloc::vec![1u32, 2, 3, 4, 5, 6, 7],
            optional: alloc::vec::Vec::new(),
            allowed_lateness_ns: 10_000,
            max_age_ns: 500_000,
            min_quality_bits_ok: true,
            require_integrity: true,
            degraded_ok: false,
            substitution: None,
            allowed_sources: Some(alloc::vec![2]),
            catalog_version: vs.contract_version,
        });
        SnapshotModule::new(
            alloc::boxed::Box::new(catalog),
            ProjectorCapacity::default(),
            policies,
            ConflictResolver::new(ConflictRule::PrimaryBackup),
            1_000,
        )
    }

    fn slots() -> FeedbackSlots {
        FeedbackSlots {
            grid_up: 5,
            soc: 4,
            dc_bus_v: 1,
            overcurrent: 6,
            overtemp: 7,
        }
    }

    fn build(
        module: &mut SnapshotModule,
        now_ns: u64,
    ) -> Result<eneros_snapshot_service::TrustedSnapshot, SnapshotError> {
        module.build(
            SnapshotRequest {
                tenant_id: 1,
                site_id: 1,
                purpose_id: 1,
                as_of_ns: now_ns,
                max_wait_ns: 0,
                policy_version: 1,
            },
            now_ns,
        )
    }

    /// 健康：评估器接受 + 快照接受 + 电气状态逐位相等（E4 基线值）。
    #[test]
    fn healthy_paths_agree() {
        let now = 6_000_000_000u64;
        let evs = events(now, [400.0, 50.0, 50.0, 0.6, 1.0, 0.0, 0.0], 100, 2);
        let mut ev = evaluator();
        let trusted = ev
            .evaluate(&samples(&evs), now / 1_000_000)
            .expect("evaluator accepted");
        let mut m = snapshot_module();
        m.apply_events(&evs);
        let snap = build(&mut m, now).expect("snapshot accepted");
        let p = project(&snap, &slots());
        // 重叠字段逐位对照（SystemState 含电压/电流/频率/SOC；grid/oc/ot
        // 在 FeedbackBundle 层——差分只锁两侧共同语义）。
        assert_eq!(p.dc_bus_v, Some(trusted.state.voltage_v));
        assert_eq!(p.soc, Some(trusted.state.soc_pct));
        assert!(snap.completeness.is_complete());
    }

    /// 缺 SOC（点 4）：双方拒绝且根因同族（缺失）。
    #[test]
    fn missing_key_paths_reject() {
        let now = 7_000_000_000u64;
        let mut evs = events(now, [400.0, 50.0, 50.0, 0.6, 1.0, 0.0, 0.0], 100, 2);
        evs.retain(|e| e.point_id != 4);
        let mut ev = evaluator();
        assert!(matches!(
            ev.evaluate(&samples(&evs), now / 1_000_000),
            Err(TrustRejection::MissingPoints { .. })
        ));
        let mut m = snapshot_module();
        m.apply_events(&evs);
        let err = build(&mut m, now).unwrap_err();
        match err {
            SnapshotError::Rejected(r) => assert_eq!(r.reason, RejectReason::MissingKeys),
            other => panic!("unexpected: {other:?}"),
        }
    }

    /// 坏品质（点 5 置 INVALID）：双方拒绝。
    #[test]
    fn bad_quality_paths_reject() {
        let now = 8_000_000_000u64;
        let mut evs = events(now, [400.0, 50.0, 50.0, 0.6, 1.0, 0.0, 0.0], 100, 2);
        let mut evil = evs[4];
        evil.quality_bits = quality_bits::INVALID;
        evs[4] = evil;
        let mut ev = evaluator();
        assert!(ev.evaluate(&samples(&evs), now / 1_000_000).is_err());
        let mut m = snapshot_module();
        m.apply_events(&evs);
        let err = build(&mut m, now).unwrap_err();
        match err {
            SnapshotError::Rejected(r) => assert_eq!(r.reason, RejectReason::BadQuality),
            other => panic!("unexpected: {other:?}"),
        }
    }

    /// 未注册来源（source=9 不在白名单）：双方拒绝。
    #[test]
    fn unknown_source_paths_reject() {
        let now = 9_000_000_000u64;
        let evs = events(now, [400.0, 50.0, 50.0, 0.6, 1.0, 0.0, 0.0], 100, 9);
        let mut ev = evaluator();
        assert!(matches!(
            ev.evaluate(&samples(&evs), now / 1_000_000),
            Err(TrustRejection::UnknownSource { .. })
        ));
        let mut m = snapshot_module();
        m.apply_events(&evs);
        let err = build(&mut m, now).unwrap_err();
        match err {
            SnapshotError::Rejected(r) => assert_eq!(r.reason, RejectReason::UnknownSource),
            other => panic!("unexpected: {other:?}"),
        }
    }

    /// 重放帧：评估器拒绝（Replay）；快照水位拒绝计数（重复帧不入
    /// 投影，快照仍一致——接入门已去重，水位拒绝可观测）。
    #[test]
    fn replay_observable_both_paths() {
        let now = 10_000_000_000u64;
        let evs = events(now, [400.0, 50.0, 50.0, 0.6, 1.0, 0.0, 0.0], 100, 2);
        let dup = evs[3];
        let mut m = snapshot_module();
        m.apply_events(&evs);
        // 快照在先（重放前一致性成立）。
        let snap_pre = build(&mut m, now).expect("snapshot accepted");
        assert!(snap_pre.completeness.is_complete());
        m.apply_event(dup);
        assert!(m.watermark_rejected() >= 1);
        // 重放后快照仍一致（重复帧未污染投影）。
        let snap_post = build(&mut m, now).expect("snapshot accepted");
        assert_eq!(snap_pre.provenance_root, snap_post.provenance_root);
        // 评估器同帧 → Replay。
        let mut ev = evaluator();
        let mut with_dup = samples(&evs);
        with_dup.push(samples(&[dup])[0]);
        assert!(matches!(
            ev.evaluate(&with_dup, now / 1_000_000),
            Err(TrustRejection::Replay { .. })
        ));
    }

    /// 版本混搭（mapping_version=2 事件混入）：快照拒绝（版本不匹配）。
    #[test]
    fn version_mix_rejected() {
        let now = 11_000_000_000u64;
        let mut evs = events(now, [400.0, 50.0, 50.0, 0.6, 1.0, 0.0, 0.0], 100, 2);
        let mut evil = evs[0];
        evil.mapping_version = 2;
        evs[0] = evil;
        let mut m = snapshot_module();
        m.apply_events(&evs);
        let err = build(&mut m, now).unwrap_err();
        assert!(matches!(err, SnapshotError::Rejected(_)));
    }

    /// Phase 0 决策投影迁移产物：TrustedSnapshot → 决策输入结构件
    /// （`DecisionProjection`），不再读「当前 map」。
    #[test]
    fn phase0_decision_input_is_projection() {
        let now = 12_000_000_000u64;
        let evs = events(now, [400.0, 50.0, 50.0, 0.6, 1.0, 0.0, 0.0], 100, 2);
        let mut m = snapshot_module();
        m.apply_events(&evs);
        let snap = build(&mut m, now).expect("snapshot");
        let p = project(&snap, &slots());
        // 决策输入 = 快照投影（携带 provenance，可追溯）。
        assert_eq!(p.snapshot_id, snap.snapshot_id);
        assert_eq!(p.provenance_root, snap.provenance_root);
        assert!(p.valid_until_ms > 0);
    }
}

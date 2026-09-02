//! 蓝图 61 Phase 6 §5.7 step 4：外部事实 → FactSet → ObjectiveModule
//! shadow——核对冲突。三类事实只能**收紧/提供经济输入**；不能覆盖
//! 安全/工艺硬约束（[11] §5.5 事实优先级）。

use eneros_business_services::external_facts::{
    EnterpriseFact, FactAdapter, FactEnvelope, FactHeader, FactPayload, FactReject, MarketFact,
    MemFactEventSink, ProcessFact,
};
use eneros_decision_contract::objective::ConstraintKind;
use eneros_objective_engine::compiler::{CompileError, CompileInputs, ObjectiveCompiler};
use eneros_objective_engine::spec::FactSet;
use eneros_snapshot_service::{NormalizedObservation, TrustedSnapshot};
use eneros_telemetry_model::event::{ObservationEvent, ObservationValue};

// ---------------------------------------------------------------------------
// 构造件
// ---------------------------------------------------------------------------

fn snapshot(cut_time_ns: u64) -> TrustedSnapshot {
    let ev = ObservationEvent {
        tenant_id: 1,
        site_id: 1,
        source_id: 5,
        source_epoch: 1,
        sequence: 1,
        event_time_ns: cut_time_ns,
        ingest_time_ns: cut_time_ns,
        asset_id: 100,
        device_id: 0,
        unit_code: 30,
        point_id: 1,
        schema_id: 1,
        schema_version: 1,
        quality_bits: eneros_telemetry_model::event::quality_bits::VALID,
        mapping_version: 1,
        value: ObservationValue::F64(50.0),
        integrity: eneros_telemetry_model::event::IntegrityTag::Authenticated,
        clock_id: 3,
        flags: 0,
    };
    TrustedSnapshot {
        snapshot_id: [9; 16],
        tenant_id: 1,
        site_id: 1,
        cut_time_ns,
        produced_at_ns: cut_time_ns + 1,
        policy_version: 1,
        version_set: eneros_semantic_catalog::VersionSet {
            generation: 1,
            mapping_version: 1,
            topology_version: 1,
            asset_version: 1,
            contract_version: 1,
            safety_version: 1,
        },
        observations: vec![NormalizedObservation { event: ev }],
        source_watermarks: vec![],
        completeness: eneros_snapshot_service::CompletenessReport {
            required: vec![],
            accepted: vec![],
            missing: vec![],
            stale: vec![],
            bad: vec![],
            conflict: vec![],
            substituted: vec![],
        },
        conflicts: vec![],
        derived: vec![],
        provenance_root: [0; 32],
    }
}

/// 最小签名 bundle（一条 DEVICE_SAFETY 层 MinSoc 硬约束）。
fn safety_bundle() -> eneros_objective_engine::bundle::RuleBundle {
    use eneros_objective_engine::bundle::RuleBundle;
    use eneros_objective_engine::layer;
    use eneros_objective_engine::normalization;
    use eneros_objective_engine::spec::{ConstraintSpec, HorizonPlan, ScopeRef};
    use eneros_objective_engine::term_kind;
    use eneros_telemetry_model::{unit_catalog, PhysicalUnit};
    let soc_min = ConstraintSpec::new(
        1,
        ConstraintKind::MinSoc.code(),
        0,
        layer::DEVICE_SAFETY,
        ScopeRef::SITE,
        unit_catalog::code_of(PhysicalUnit::Dimensionless),
        0.1,
        0.9,
        0,
        u64::MAX,
        [1; 16],
    )
    .unwrap();
    let soc_max = ConstraintSpec::new(
        2,
        ConstraintKind::MaxSoc.code(),
        0,
        layer::DEVICE_SAFETY,
        ScopeRef::SITE,
        unit_catalog::code_of(PhysicalUnit::Dimensionless),
        0.1,
        0.9,
        0,
        u64::MAX,
        [1; 16],
    )
    .unwrap();
    // 经济项（价格缩放；编译需要可映射 objective term）。
    let econ = eneros_objective_engine::spec::ObjectiveTerm::new(
        1,
        term_kind::ECONOMIC,
        1.0,
        unit_catalog::code_of(PhysicalUnit::Dimensionless),
        normalization::PRICE_SCALED,
        layer::OPERATIONAL,
        [1; 16],
    )
    .unwrap();
    RuleBundle::builder([7; 16], 3)
        .validity_window(0, u64::MAX)
        .horizon(HorizonPlan::new(900_000_000_000, 96).unwrap())
        .site_rating_kw(100.0)
        .add_constraint(soc_min)
        .add_constraint(soc_max)
        .add_term(econ)
        .build()
        .unwrap()
}

fn compiler() -> ObjectiveCompiler {
    ObjectiveCompiler::new(3_600_000_000_000, 1, false)
}

/// 已认证市场事实（两时段价格）。
fn market_envelopes() -> Vec<FactEnvelope<MarketFact>> {
    let (sk, pk) = eneros_business_services::external_facts::test_keypair();
    let mut out = Vec::new();
    // 96 步价格（与 bundle horizon step_count 对齐——编译器要求
    // prices ∈ {0, 1, step_count}）。
    for i in 0..96u32 {
        let mut env = FactEnvelope {
            header: FactHeader {
                external_id: [7; 16],
                revision: i + 1,
                source_id: 9,
                classification: 1,
                published_at_ms: 1000,
                effective_from_ms: 1000,
                expires_at_ms: u64::MAX / 2,
            },
            payload: MarketFact {
                product: 100,
                region: 1,
                delivery_start_ms: 1_000_000_000,
                duration_ns: 900_000_000_000,
                price: 400.0 + i as f64,
                volume: 10.0,
                currency_code: 156,
                tz_offset_min: 480,
            },
            key_id: *b"testkey1",
            signature: [0; 64],
            payload_digest: [0; 32],
        };
        eneros_business_services::external_facts::seal(&mut env, &sk, &pk);
        out.push(env);
    }
    out
}

fn fact_adapter() -> FactAdapter {
    let (_sk, pk) = eneros_business_services::external_facts::test_keypair();
    let mut keys = eneros_signer_keyring::SignerKeyring::new();
    keys.register(*b"testkey1", &pk);
    FactAdapter::new(keys)
}

/// 认证→事件面（返回事件面条目）。
fn ingest_market_facts() -> (FactSet, u64) {
    let mut a = fact_adapter();
    let mut sink = MemFactEventSink::default();
    let mut facts = FactSet::default();
    let mut prices = Vec::new();
    for env in market_envelopes() {
        a.accept(&mut sink, &env, 5000).unwrap();
        prices.push(env.payload.price);
    }
    facts.as_of_ns = 5000;
    facts.market.prices_per_kwh = prices;
    // 事实来源 = 事件面最后一条（revision 可追溯）。
    let (_, _, _, _, offset) = sink.entries.last().unwrap();
    facts.market.source_event_id = crate_event_id(*offset);
    (facts, *offset)
}

fn crate_event_id(offset: u64) -> [u8; 16] {
    let mut id = [0u8; 16];
    id[..8].copy_from_slice(&offset.to_le_bytes());
    id
}

// ---------------------------------------------------------------------------
// shadow 核对
// ---------------------------------------------------------------------------

#[test]
fn market_facts_feed_objective_shadow() {
    let (facts, _) = ingest_market_facts();
    let snap = snapshot(1_000_000_000);
    let compiled = compiler()
        .compile(CompileInputs {
            snapshot: &snap,
            registry_generation: 1,
            bundle: &safety_bundle(),
            facts: &facts,
            intent: None,
            verify: None,
        })
        .expect("市场事实 + 安全 bundle 必须可编译");
    // 经济输入进入（价格向量非空）；safety 硬约束保留（外部事实不能
    // 覆盖/删除安全约束）。
    assert_eq!(compiled.prices_per_kwh.len(), 96);
    assert!(
        compiled
            .spec
            .hard()
            .iter()
            .any(|c| c.kind == ConstraintKind::MinSoc && c.bound == 0.1),
        "DEVICE_SAFETY MinSoc 下界必须保留"
    );
}

#[test]
fn maintenance_window_conflicts_rejected() {
    // 工艺维护窗与地平线相交 → 编译拒绝（资源在窗内不可控，保守不规划）。
    let mut facts = FactSet {
        as_of_ns: 5000,
        market: eneros_objective_engine::spec::MarketFacts {
            prices_per_kwh: vec![420.0], // 广播（step_count 等长）
            source_event_id: [1; 16],
        },
        ..Default::default()
    };
    // 窗口 [0, 10_000s) 覆盖 cut=1e9ns 的地平线。
    facts.enterprise.maintenance_windows_ns = vec![(0, 10_000_000_000_000)];
    let snap = snapshot(1_000_000_000);
    let err = compiler()
        .compile(CompileInputs {
            snapshot: &snap,
            registry_generation: 1,
            bundle: &safety_bundle(),
            facts: &facts,
            intent: None,
            verify: None,
        })
        .unwrap_err();
    assert_eq!(err, CompileError::WindowNotCovered("maintenance"));
    // 窗在地平线之后 → 允许（不影响本次规划）。
    let mut facts = FactSet {
        as_of_ns: 5000,
        market: eneros_objective_engine::spec::MarketFacts {
            prices_per_kwh: vec![420.0],
            source_event_id: [1; 16],
        },
        ..Default::default()
    };
    facts.enterprise.maintenance_windows_ns = vec![(u64::MAX / 2, u64::MAX - 1)];
    assert!(compiler()
        .compile(CompileInputs {
            snapshot: &snap,
            registry_generation: 1,
            bundle: &safety_bundle(),
            facts: &facts,
            intent: None,
            verify: None,
        })
        .is_ok());
}

#[test]
fn external_facts_cannot_override_safety() {
    // 1. 类型面：工厂事实没有约束字段——市场/工艺/企业 payload 只有
    //    经济/边界输入；不存在「外部目标放宽 safety」的 wire 形状。
    let m = MarketFact {
        product: 100,
        region: 1,
        delivery_start_ms: 1_000_000_000,
        duration_ns: 900_000_000_000,
        price: 400.0,
        volume: 10.0,
        currency_code: 156,
        tz_offset_min: 480,
    };
    assert_eq!(m.class(), 1);
    // 2. 语义面：非法企业事实（未知 kind/审批标志）在 adapter 拒——
    //    进不了事实集。
    let mut e = EnterpriseFact {
        kind: 99,
        subject_id: 1,
        priority: 1,
        window_from_ms: 1,
        window_to_ms: 2,
        value: 0.0,
        approval_required: 0,
    };
    assert_eq!(e.validate_semantics(), Err(FactReject::BadKind));
    e.kind = 4;
    let (sk, pk) = eneros_business_services::external_facts::test_keypair();
    let mut env = FactEnvelope {
        header: FactHeader {
            external_id: [8; 16],
            revision: 1,
            source_id: 9,
            classification: 2,
            published_at_ms: 1000,
            effective_from_ms: 1000,
            expires_at_ms: u64::MAX / 2,
        },
        payload: e,
        key_id: *b"testkey1",
        signature: [0; 64],
        payload_digest: [0; 32],
    };
    eneros_business_services::external_facts::seal(&mut env, &sk, &pk);
    // 3. 编译面：即使用户在事实集里试图塞越权 override 域外值，编译
    //    仍以 bundle 硬约束为唯一 safety 权威（编译输出只是经济项）。
    // 域外 override（比率 0..1 之外）——事实集只能表达经济输入；越权
    // 值在编译面被拒。
    let facts = FactSet {
        as_of_ns: 5000,
        enterprise: eneros_objective_engine::spec::EnterpriseFacts {
            operator_override_soc: Some((1.5, 0.2)),
            ..Default::default()
        },
        ..Default::default()
    };
    let snap = snapshot(1_000_000_000);
    // 编译器对域外 override 直接拒绝（fail-closed——比保留更严）。
    let err = compiler()
        .compile(CompileInputs {
            snapshot: &snap,
            registry_generation: 1,
            bundle: &safety_bundle(),
            facts: &facts,
            intent: None,
            verify: None,
        })
        .unwrap_err();
    assert_eq!(err, CompileError::InvalidRule("operator_override_soc"));
    // 域内 override 仍以 bundle 硬约束为唯一 safety 权威（safety 约束
    // 不可能被外部事实删除/放宽——编译输出保留全部硬约束）。
    let mut facts_ok = FactSet {
        as_of_ns: 5000,
        market: eneros_objective_engine::spec::MarketFacts {
            prices_per_kwh: vec![420.0],
            source_event_id: [1; 16],
        },
        ..Default::default()
    };
    facts_ok.enterprise.operator_override_soc = Some((0.2, 0.9));
    let compiled = compiler()
        .compile(CompileInputs {
            snapshot: &snap,
            registry_generation: 1,
            bundle: &safety_bundle(),
            facts: &facts_ok,
            intent: None,
            verify: None,
        })
        .expect("域内 override 可编译");
    // 企业事实只能**收紧**：MinSoc 从 0.1 提到 0.2（override 生效），
    // 安全约束整体保留——不存在「外部目标放宽 safety」路径。
    assert!(
        compiled
            .spec
            .hard()
            .iter()
            .any(|c| c.kind == ConstraintKind::MinSoc && c.bound == 0.2),
        "override 只能收紧 MinSoc"
    );
    assert!(
        compiled
            .spec
            .hard()
            .iter()
            .any(|c| c.kind == ConstraintKind::MaxSoc && c.bound == 0.9),
        "MaxSoc 全量保留"
    );
}

#[test]
fn disconnected_external_system_keeps_station_safe() {
    // [11] §5.8：断开三类外部系统进入 D2，站内安全持续——拒绝路径
    // 不改变任何规划输入；有效事实仍可独立编译。
    let mut a = fact_adapter();
    let mut sink = MemFactEventSink::default();
    // 恶意/损坏事实（坏签名）→ 拒；计数可见。
    let (sk, pk) = eneros_business_services::external_facts::test_keypair();
    let mut bad = FactEnvelope {
        header: FactHeader {
            external_id: [1; 16],
            revision: 1,
            source_id: 9,
            classification: 1,
            published_at_ms: 1000,
            effective_from_ms: 1000,
            expires_at_ms: u64::MAX / 2,
        },
        payload: ProcessFact {
            process_unit: 7,
            batch: 1,
            recipe: 2,
            recipe_version: 1,
            phase: 4,
            interlock_mask: 0,
            mode: 2,
            rate_min: 0.0,
            rate_max: 10.0,
            temp_min: 0.0,
            temp_max: 50.0,
            pressure_min: 0.0,
            pressure_max: 1.0,
            flow_min: 0.0,
            flow_max: 10.0,
            quality_min: 0.0,
            maintenance: None,
        },
        key_id: *b"testkey1",
        signature: [0; 64],
        payload_digest: [0; 32],
    };
    eneros_business_services::external_facts::seal(&mut bad, &sk, &pk);
    bad.signature[3] ^= 0xFF;
    assert_eq!(
        a.accept(&mut sink, &bad, 5000),
        Err(FactReject::BadSignature)
    );
    assert_eq!(a.rejected_total, 1);
    assert_eq!(sink.entries.len(), 0);
    // 站内安全路径不受影响：合法的市场事实照常编译进 shadow。
    let (facts, _) = ingest_market_facts();
    let snap = snapshot(1_000_000_000);
    let compiled = compiler()
        .compile(CompileInputs {
            snapshot: &snap,
            registry_generation: 1,
            bundle: &safety_bundle(),
            facts: &facts,
            intent: None,
            verify: None,
        })
        .expect("D2 下站内规划照常");
    assert_eq!(compiled.prices_per_kwh.len(), 96);
}

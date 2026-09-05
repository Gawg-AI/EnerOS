//! 蓝图 68 卷 68.2（WP-68.2.1 t4）：双源满流下快照零 `SourceEpochMismatch`。
//!
//! F13 根因模型（每服务 `open()` 的 floor 各自取 boot 单调时刻 → epoch 必
//! 不相等；消费侧单全局 epoch = "最后钉扎者赢"）：modbus/iec104 双源（floor
//! 不同）交错上行，快照反复评估。行为锁本体 = `SNAPSHOT rejected:
//! SourceEpochMismatch` 计数恒 0（QEMU gate 新增判据的服务级对照——现无
//! gate 显式锁零，本测试即其 host 形态）。

#![allow(clippy::disallowed_macros)] // 测试断言 panic!（ai_worker/snapshot_diff 先例）

extern crate alloc;

use alloc::vec::Vec;

use eneros_control_services::telemetry::TelemetryService;
use eneros_ingestion_core::{MemEpochStore, RawObservation, SourceDescriptor, SourceRegistry};
use eneros_safety_validator::{
    EvaluatorConfig, PointUnit, RequiredPoint, RequiredPointCoverage, StateBindings, TrustRejection,
};
use eneros_telemetry_model::event::{quality_bits, IntegrityTag, ObservationValue};

/// 每源点数：源 1 → 点 1..=4，源 2 → 点 5..=8（互不重叠 = 无 SourceConflict）。
const POINTS_PER_SOURCE: u32 = 4;

fn descriptor(source_id: u32) -> SourceDescriptor {
    SourceDescriptor {
        source_id,
        tenant_id: 1,
        site_id: 1,
        allow_control: true,
        require_integrity: true,
        point_range: (
            (source_id - 1) * POINTS_PER_SOURCE + 1,
            source_id * POINTS_PER_SOURCE,
        ),
        max_events_per_s: 1000,
        allowed_schemas: alloc::vec![1],
        clock_id: 1,
        enabled: true,
    }
}

fn eval_config() -> EvaluatorConfig {
    let total = POINTS_PER_SOURCE * 2;
    EvaluatorConfig {
        coverage: RequiredPointCoverage {
            points: (1u32..=total)
                .map(|p| RequiredPoint {
                    point: p,
                    min: 0.0,
                    max: 1000.0,
                    max_age_ms: 5000,
                    unit: PointUnit::Kilowatt,
                })
                .collect(),
        },
        bindings: StateBindings {
            voltage: 1,
            current: 2,
            frequency: 3,
            soc: 4,
        },
        mapping_version: 7, // = bundle generation（配置事实注入）
        max_skew_ms: 1000,
        allowed_sources: alloc::vec![1, 2],
        require_integrity: true,
        control_horizon_ms: 30_000,
    }
}

/// 单源一轮批次（4 帧 128B wire；epoch = registry `open_source` 分配值，
/// sequence 显式递增跨轮单调）。
fn frames_for(reg: &mut SourceRegistry, source_id: u32, seq_base: u64, now_ms: u64) -> Vec<u8> {
    let mut wire = Vec::new();
    for i in 0..POINTS_PER_SOURCE {
        let p = (source_id - 1) * POINTS_PER_SOURCE + i + 1;
        let raw = RawObservation {
            point_id: p,
            value: ObservationValue::F64(100.0 + f64::from(p)),
            event_time_ns: Some((now_ms - 50) * 1_000_000),
            quality_bits: quality_bits::VALID,
            integrity: IntegrityTag::LocalTrustedEdge,
            flags: 0,
            schema_id: 1,
            schema_version: 1,
            unit_code: 6, // kW
            asset_id: 1,
            mapping_version: 7,
            device_id: source_id as u16,
        };
        let mut ev = reg
            .verify(source_id, &raw, now_ms * 1_000_000)
            .expect("verify");
        ev.sequence = seq_base + u64::from(i) + 1;
        wire.extend_from_slice(&ev.encode_v1());
    }
    wire
}

/// t4（WP-68.2.1）：双源满流下快照零 SourceEpochMismatch（行为锁）。
#[test]
fn t4_snapshot_source_epoch_mismatch_count_is_zero() {
    // 装配：来源注册表一次开源两源（floor 各取"boot now_ms"→ 必不相等，
    // F13 ③ 前半）；服务 + 影子同配置。
    let mut reg = SourceRegistry::new();
    let mut store = MemEpochStore::default();
    reg.register(descriptor(1), 0);
    reg.register(descriptor(2), 0);
    let epoch_a = reg.open_source(1, 10_000, &mut store).expect("open A");
    let epoch_b = reg.open_source(2, 20_000, &mut store).expect("open B");
    assert_ne!(
        epoch_a, epoch_b,
        "每服务 floor 各取 boot now_ms → epoch 必不相等（F13 模型前提）"
    );

    let mut svc = TelemetryService::new(eval_config());
    svc.enable_obs_shadow(alloc::vec![descriptor(1), descriptor(2)], eval_config());

    // 交错上行 5 轮（B 先到 = 调查窗毒化方向），每轮后快照反复评估。
    for round in 0..5u64 {
        let now = 30_000 + round * 1_000;
        let seq = round * 100;
        let wa = frames_for(&mut reg, 1, seq, now);
        let wb = frames_for(&mut reg, 2, seq, now);
        // B 先到（毒化方向不随序翻转的对照半边由 t2 锁）。
        let resp_b = svc.obs_ingest(2, &wb, now);
        let resp_a = svc.obs_ingest(1, &wa, now);
        // 响应 [OK][accepted u16][rejected u16]：双源 4 帧全收。
        assert_eq!(resp_b[0], 0, "B 批 ack OK");
        assert_eq!(u16::from_le_bytes([resp_b[1], resp_b[2]]), 4, "B 全收");
        assert_eq!(resp_a[0], 0, "A 批 ack OK");
        assert_eq!(u16::from_le_bytes([resp_a[1], resp_a[2]]), 4, "A 全收");
        // 行为锁本体：双源满流后快照零 SourceEpochMismatch（其余拒绝同样
        // 零容忍——满流 + 全覆盖配置下任何结构化拒绝都是回归）。三连快照
        // 兼锁幂等重评（68.2 实弹订正二：快照 = 缓冲只读投影，等值重替身
        // 非重放；首轮实弹中该形态被打成 Replay 死锁 agent 链）。
        for snap in 0..3 {
            match svc.snapshot(now) {
                Ok(_) => {}
                Err(TrustRejection::SourceEpochMismatch { .. }) => {
                    panic!("SNAPSHOT rejected: SourceEpochMismatch（毒化复发，round {round} snap {snap}）")
                }
                Err(e) => panic!("非本卷判据的快照拒绝（round {round} snap {snap}）：{e:?}"),
            }
        }
    }
}

/// t8（68.2 实弹订正锁）：部分批（单源点位、非全量覆盖）跳过影子评估
/// ——零影子拒绝、uncovered 计数递增、无影子结论；全量批照常评估。
/// QEMU 实弹对照：修复前 iec104 部分批每批 `OBS-SHADOW eval reject:
/// MissingPoints`（窗内 11 次，全为 MissingPoints、零 epoch 类）。
#[test]
fn t8_shadow_partial_batch_skipped_not_rejected() {
    let mut reg = SourceRegistry::new();
    let mut store = MemEpochStore::default();
    reg.register(descriptor(1), 0);
    reg.register(descriptor(2), 0);
    let _ = reg.open_source(1, 10_000, &mut store).expect("open A");
    let _ = reg.open_source(2, 20_000, &mut store).expect("open B");

    let mut svc = TelemetryService::new(eval_config());
    svc.enable_obs_shadow(alloc::vec![descriptor(1), descriptor(2)], eval_config());

    let now = 30_000;
    // 仅源 1（点 1-4）→ 非全量覆盖 → 影子跳过（不产拒绝/结论）。
    let wa = frames_for(&mut reg, 1, 0, now);
    let resp = svc.obs_ingest(1, &wa, now);
    assert_eq!(resp[0], 0, "批次本身 ack OK");
    {
        let obs = svc.obs.as_ref().expect("影子已装配");
        assert_eq!(obs.shadow_uncovered, 1, "部分批跳过计数");
        assert_eq!(obs.rejected, 0, "零影子拒绝（对照修复前 MissingPoints）");
        assert_eq!(obs.last_shadow_ok(), None, "无影子结论（对照面不产分歧）");
    }

    // 补齐源 2（点 5-8）后单批仍非全量（影子逐批对照 = 单批语义）→ 跳过。
    let wb = frames_for(&mut reg, 2, 0, now + 1_000);
    let resp = svc.obs_ingest(2, &wb, now + 1_000);
    assert_eq!(resp[0], 0);
    {
        let obs = svc.obs.as_ref().expect("影子已装配");
        assert_eq!(obs.shadow_uncovered, 2);
        assert_eq!(obs.rejected, 0);
    }
}

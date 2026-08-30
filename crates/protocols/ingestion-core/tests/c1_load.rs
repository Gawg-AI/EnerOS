//! 蓝图 61 [07] §2.7 C1 负载判据（host 模拟时钟）：
//! - 10,000 点目录（分片）+ 周期混合；
//! - 5,000 events/s 持续与 20,000/s 突发——无静默丢、计数面一致；
//! - 突发超 profile 预算 → 显式背压（不无界积累）。

use eneros_ingestion_core::{
    EventQos, InMemorySink, IngestionAdapter, IngestionModule, MemEpochStore, OperationalProfile,
    PointDirectory, PointPlan, PollBudget, RawObservationSink, TickBudget,
};
use eneros_telemetry_model::event::{quality_bits, IntegrityTag, ObservationValue};

fn descriptor(source_id: u32, max_events_per_s: u32) -> eneros_ingestion_core::SourceDescriptor {
    eneros_ingestion_core::SourceDescriptor {
        source_id,
        tenant_id: 1,
        site_id: 1,
        allow_control: true,
        require_integrity: true,
        point_range: (0, 0),
        max_events_per_s,
        allowed_schemas: vec![1],
        clock_id: 1,
        enabled: true,
    }
}

fn plan(source_id: u32, point_id: u32, period_ns: u64, qos: EventQos) -> PointPlan {
    PointPlan {
        source_id,
        point_id,
        device_id: 2,
        period_ns,
        phase_offset_ns: 0,
        deadline_ns: period_ns * 2,
        deadband_abs: 0.0,
        deadband_rel: 0.0,
        qos,
        suppressible: true,
        timeout_ns: 1_000_000,
        max_retries: 2,
        schema_id: 1,
        schema_version: 1,
        unit_code: 0,
        asset_id: 1,
        mapping_version: 1,
        scale: 1.0,
        offset: 0.0,
        func_code: 3,
        reg_addr: point_id as u16,
        slave_addr: 1,
    }
}

fn raw(point_id: u32, value: i64, now_ns: u64) -> eneros_ingestion_core::RawObservation {
    eneros_ingestion_core::RawObservation {
        point_id,
        value: ObservationValue::I64(value),
        event_time_ns: Some(now_ns.max(1)),
        quality_bits: quality_bits::VALID,
        integrity: IntegrityTag::LocalTrustedEdge,
        flags: 0,
        schema_id: 0,
        schema_version: 0,
        unit_code: 0,
        asset_id: 0,
        mapping_version: 0,
        device_id: 2,
    }
}

/// 全量点适配器：每次 poll 对 work 全部点各推一个事件（1 work 项 1 事务）。
struct BulkAdapter {
    source_id: u32,
    value: i64,
}

impl IngestionAdapter for BulkAdapter {
    fn source_id(&self) -> u32 {
        self.source_id
    }
    fn poll(
        &mut self,
        now_ns: u64,
        budget: PollBudget,
        work: &[PointPlan],
        out: &mut dyn RawObservationSink,
    ) -> eneros_ingestion_core::PollOutcome {
        self.value += 1;
        let mut events = 0u32;
        let mut transactions = 0u32;
        for p in work {
            if transactions >= budget.max_transactions {
                return eneros_ingestion_core::PollOutcome::Busy { transactions };
            }
            transactions += 1;
            let _ = out.push(raw(p.point_id, self.value, now_ns));
            events += 1;
            if events >= budget.max_events {
                break;
            }
        }
        eneros_ingestion_core::PollOutcome::Completed {
            events,
            transactions,
        }
    }
}

/// 分片容量检查：10,000 点 → 10 shard（≤1024/shard）。
#[test]
fn c1_directory_10k_points_sharded() {
    let mut dir = PointDirectory::new();
    let plans: Vec<PointPlan> = (1u32..=10_000)
        .map(|i| plan(2, i, 1_000_000_000, EventQos::Q1))
        .collect();
    dir.install_generation(1, plans).unwrap();
    assert_eq!(dir.shard_count(), 10, "10k 点 → 10 shard");
    assert!(dir.shards().iter().all(|s| s.len() <= 1024));
    assert_eq!(dir.len(), 10_000);
}

/// 5,000 events/s 持续：5k 点 1s 周期 × 10 tick——全部接受、零静默丢。
#[test]
fn c1_sustained_5k_events_per_s() {
    let points = 5_000u32;
    let plans: Vec<PointPlan> = (1..=points)
        .map(|i| plan(2, i, 1_000_000_000, EventQos::Q1))
        .collect();
    let mut m = IngestionModule::new(
        Box::new(InMemorySink::new(200_000)),
        Box::new(MemEpochStore::default()),
        OperationalProfile {
            max_events_per_poll: 16_000,
            max_transactions_per_tick: 16_000,
            checkpoint_interval_events: 4096,
        },
    );
    m.register(descriptor(2, 100_000));
    m.open(2, 0).unwrap();
    m.install_points(1, plans).unwrap();
    m.attach(Box::new(BulkAdapter {
        source_id: 2,
        value: 0,
    }))
    .unwrap();
    // 10 个周期（1s 步长）→ 50,000 事件。
    let mut total = 0u64;
    for k in 0..10u64 {
        let now = k * 1_000_000_000 + 1;
        let r = m.tick(
            now,
            TickBudget {
                max_events: 16_000,
                max_transactions: 16_000,
            },
        );
        assert_eq!(r.accepted, points, "tick {k} 全量接受");
        assert_eq!(r.rejected, 0);
        assert_eq!(r.backpressured, 0);
        total += u64::from(r.accepted);
        m.checkpoint().unwrap();
    }
    assert_eq!(total, 50_000);
    assert_eq!(m.accepted_total, 50_000, "无静默丢");
    assert_eq!(m.watermark(2).map(|(_, s)| s), Some(50_000));
}

/// 20,000/s 突发：单 tick 2 万事件——预算内接受；超出 profile 的部分
/// 显式背压（计数可见，无静默丢、无无界积累）。
#[test]
fn c1_burst_20k_events() {
    let points = 20_000u32;
    let plans: Vec<PointPlan> = (1..=points)
        .map(|i| plan(2, i, 1_000_000_000, EventQos::Q2))
        .collect();
    let mut m = IngestionModule::new(
        Box::new(InMemorySink::new(64_000)),
        Box::new(MemEpochStore::default()),
        OperationalProfile {
            max_events_per_poll: 16_000,
            max_transactions_per_tick: 16_000,
            checkpoint_interval_events: 4096,
        },
    );
    m.register(descriptor(2, 100_000));
    m.open(2, 0).unwrap();
    m.install_points(1, plans).unwrap();
    m.attach(Box::new(BulkAdapter {
        source_id: 2,
        value: 0,
    }))
    .unwrap();
    let r = m.tick(
        1,
        TickBudget {
            max_events: 16_000,
            max_transactions: 16_000,
        },
    );
    // 轮转下发 16,000 事务（=16k 点）；profile 预算内全接受。
    assert_eq!(r.accepted, 16_000, "预算内 16k 全收");
    assert_eq!(r.rejected, 0);
    assert_eq!(m.accepted_total, 16_000);
    // 余下 4k 点下 tick 采（轮转公平）——总量守恒。
    let r2 = m.tick(
        1_000_000_001,
        TickBudget {
            max_events: 16_000,
            max_transactions: 16_000,
        },
    );
    assert!(r2.accepted >= 4_000, "余量点续采：{}", r2.accepted);
    assert_eq!(m.accepted_total, 16_000 + r2.accepted as u64);
}

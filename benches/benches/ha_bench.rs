//! HA 同步基准测试 (T030-06)
//!
//! 测量高可用热点路径的延迟：
//! - heartbeat：心跳包序列化（HeartbeatPacket JSON 编码，send_heartbeat 核心路径）
//! - sync：状态同步批量编码（SyncBatch bincode 编码，状态同步核心路径）
//! - fault_detection：故障检测（FailoverEngine::on_node_state_change 状态机转换）
//!
//! 注意：非 Linux 平台心跳/同步的网络 I/O 不可用，基准测试聚焦纯逻辑路径
//! （序列化、状态机转换），这些是跨平台可用的核心热点。
//!
//! 运行：cargo bench -p eneros-benches --bench ha_bench

use std::hint::black_box;
use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use eneros_os::ha::{
    ConflictResolution, FailoverConfig, FailoverEngine, HaConfig, HeartbeatPacket, NodeRole,
    NodeState, NodeStateChange, SharedStore, StorageQuota, SyncBatch, SyncMessage,
};

/// 构建基准测试用 HaConfig（非生产环境，允许 fencing_strategy = none）
fn build_ha_config() -> HaConfig {
    HaConfig::load_from_str(
        r#"
node_id = "bench-node"
role = "primary"
heartbeat_interval_ms = 100
heartbeat_suspect_ms = 100
heartbeat_dead_ms = 300
multicast_addr = "239.0.0.1"
heartbeat_port = 5400
sync_port = 5401
priority = 100
fencing_strategy = "none"
is_production = false
"#,
    )
    .expect("parse HaConfig")
}

/// 构建基准测试用 HeartbeatPacket
fn build_heartbeat_packet() -> HeartbeatPacket {
    HeartbeatPacket {
        node_id: "bench-node".to_string(),
        role: NodeRole::Primary,
        timestamp: 1700000000000,
        seq: 42,
        priority: 100,
        hmac: [0u8; 32],
        epoch: 1,
    }
}

/// 构建基准测试用 SyncMessage 列表（模拟 SCADA 数据同步）
fn build_sync_messages(count: usize) -> Vec<SyncMessage> {
    (0..count)
        .map(|i| SyncMessage::ScadaData {
            key: format!("bus_{}_voltage_pu", i),
            value: serde_json::json!(1.02),
            timestamp: 1700000000000,
            seq: i as u64,
        })
        .collect()
}

/// heartbeat 基准测试：心跳包序列化延迟
///
/// 测量 HeartbeatPacket → JSON 字节的序列化延迟。
/// 这是 send_heartbeat() 的核心路径（Linux 上序列化后通过 UDP 多播发送）。
fn bench_ha_heartbeat(c: &mut Criterion) {
    let packet = build_heartbeat_packet();

    let mut group = c.benchmark_group("ha_heartbeat");
    group.sample_size(500);
    group.measurement_time(std::time::Duration::from_secs(5));

    group.bench_function("packet_serialize", |b| {
        b.iter(|| {
            black_box(
                serde_json::to_vec(black_box(&packet)).unwrap(),
            );
        });
    });

    group.finish();
}

/// sync 基准测试：状态同步批量编码延迟
///
/// 测量 SyncBatch::encode() 的延迟——bincode varint 编码。
/// 这是状态同步的核心路径：累积消息 → 批量打包 → bincode 编码 → TCP 发送。
fn bench_ha_sync(c: &mut Criterion) {
    let messages = build_sync_messages(100);
    let batch = SyncBatch::new(messages, 1);

    let mut group = c.benchmark_group("ha_sync");
    group.sample_size(500);
    group.measurement_time(std::time::Duration::from_secs(5));

    group.bench_function("batch_encode_100msgs", |b| {
        b.iter(|| {
            black_box(black_box(&batch).encode().unwrap());
        });
    });

    group.finish();
}

/// fault_detection 基准测试：故障检测状态机转换延迟
///
/// 测量 FailoverEngine::on_node_state_change() 的延迟——
/// 当对端节点状态变更（如 Alive → Dead）时，failover 状态机的判定与转换路径。
fn bench_ha_fault_detection(c: &mut Criterion) {
    let config = build_ha_config();
    let store = Arc::new(SharedStore::new(
        "bench-node",
        NodeRole::Primary,
        ConflictResolution::default(),
        StorageQuota::default(),
    ));
    let engine = FailoverEngine::new(config, store, FailoverConfig::default());

    let change = NodeStateChange {
        node_id: "peer-node".to_string(),
        old_state: NodeState::Alive,
        new_state: NodeState::Suspect,
        timestamp: 1700000000000,
    };

    let mut group = c.benchmark_group("ha_fault_detection");
    group.sample_size(500);
    group.measurement_time(std::time::Duration::from_secs(5));

    group.bench_function("on_node_state_change", |b| {
        b.iter(|| {
            engine.on_node_state_change(black_box(&change));
            black_box(());
        });
    });

    group.finish();
}

criterion_group!(benches, bench_ha_heartbeat, bench_ha_sync, bench_ha_fault_detection);
criterion_main!(benches);

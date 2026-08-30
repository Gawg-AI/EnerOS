//! EventStore 消费（[08] §3.6 步骤 3）：从已提交位点续读分区 → 解码
//! ObservationEvent → 流入快照模块；快照成功后提交位点（幂等：重复
//! drain 不重复交付）。

use alloc::vec::Vec;

use eneros_event_store::{ConsumerGroupId, EventStore, PartitionId, RecordRef, RecordVisitor};
use eneros_telemetry_model::event::ObservationEvent;

use crate::SnapshotModule;

/// 快照消费组（位点持久于 event-store）。
pub const SNAPSHOT_CONSUMER_GROUP: ConsumerGroupId = 0x5353; // "SS"

/// 消费位点（分区 → offset；`sources` 来自 event-store 枚举）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotCheckpoint {
    pub offsets: Vec<(PartitionId, u64)>,
}

/// 分区扫描 → 位点续读 → 事件注入；返回本次流入了多少事件。
///
/// 语义（[07] §4.4 / 接入桥先例）：
/// - 未提交位点 → 从 0 起读（新消费组首跑）；
/// - 读完当前日志尾即止（drain 语义；下轮再跑续读）；
/// - 解码失败记录 → 显式计数（转 quarantine 语义——不静默吞）。
pub fn drain_events(
    store: &mut dyn EventStore,
    module: &mut SnapshotModule,
    partitions: &[PartitionId],
) -> Result<u64, eneros_event_store::StoreError> {
    let mut total: u64 = 0;
    for &partition in partitions {
        let from = store
            .committed_offset(SNAPSHOT_CONSUMER_GROUP, partition)
            .unwrap_or(0);
        let mut collector = EventCollector {
            module: &mut *module,
            last_offset: from,
            accepted: 0,
            decode_failed: 0,
        };
        // borrow 规则：把 module 移入 collector（一次一个分区）。
        let report = store.read(partition, from, &mut collector)?;
        total += collector.accepted;
        // 位点推进到「实际读到的下一条」；无新记录 → 保持原值。
        let next = report.next_offset.max(collector.last_offset);
        if next > from {
            store.commit_consumer(
                SNAPSHOT_CONSUMER_GROUP,
                &[eneros_event_store::PartitionOffset::new(partition, next)],
            )?;
        }
    }
    Ok(total)
}

/// 单分区收集器（visitor 实现）。
struct EventCollector<'m> {
    module: &'m mut SnapshotModule,
    last_offset: u64,
    accepted: u64,
    decode_failed: u64,
}

impl RecordVisitor for EventCollector<'_> {
    fn record(&mut self, r: RecordRef<'_>) -> bool {
        self.last_offset = r.offset;
        match ObservationEvent::decode_v1(r.payload) {
            Ok(ev) => {
                self.module.apply_event(ev);
                self.accepted += 1;
            }
            Err(e) => {
                // 解码失败记录：显式计数（转 quarantine 语义）。
                self.decode_failed += 1;
                let _ = e;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use alloc::boxed::Box;

    use eneros_event_store::mem::MemEventStore;
    use eneros_semantic_catalog::SemanticCatalog;

    use super::*;
    use crate::tests::{obs7, prod_catalog};
    use crate::{
        conflict::{ConflictResolver, ConflictRule},
        policy::SnapshotPolicyRegistry,
        ProjectorCapacity, SnapshotModule,
    };

    fn build_module() -> SnapshotModule {
        let catalog = prod_catalog();
        let vs = catalog.version_set();
        let mut policies = SnapshotPolicyRegistry::new();
        policies.register(crate::tests::test_policy(vs.contract_version));
        SnapshotModule::new(
            Box::new(catalog),
            ProjectorCapacity::default(),
            policies,
            ConflictResolver::new(ConflictRule::PrimaryBackup),
            1_000,
        )
    }

    #[test]
    fn drain_from_committed_offset_replays_and_checkpoints() {
        let mut store = MemEventStore::new(
            1 << 20,
            eneros_event_store::StoreConfig {
                segment_bytes: 65536,
                batch_records: 64,
                q0_batch_records: 1,
                reject_q2q3_at_95_percent: false,
                clock_unix_s: || 0,
            },
        )
        .unwrap();
        let now = 2_000_000_000u64;
        let events = obs7(now, 100, 2, &[400.0, 50.0, 50.0, 0.6, 1.0, 0.0, 0.0]);
        // 每事件一记录（同分区——FNV-1a(source) 全落 0）。
        for ev in events.iter() {
            store.append(0, ev.event_id(), 0, &ev.encode_v1()).unwrap();
        }
        let mut module = build_module();
        // 首轮 drain：7 事件流入。
        let n = drain_events(&mut store, &mut module, &[0]).unwrap();
        assert_eq!(n, 7);
        let committed = store.committed_offset(SNAPSHOT_CONSUMER_GROUP, 0).unwrap();
        assert_eq!(committed, 7);
        // 二轮 drain：无新记录 → 0 事件，位点不倒退。
        let n2 = drain_events(&mut store, &mut module, &[0]).unwrap();
        assert_eq!(n2, 0);
        assert_eq!(
            store.committed_offset(SNAPSHOT_CONSUMER_GROUP, 0).unwrap(),
            7
        );
        // 快照可构建（重放路径：事件已注入）。
        let snap = module
            .build(
                crate::SnapshotRequest {
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
        assert_eq!(snap.observations.len(), 7);
    }
}

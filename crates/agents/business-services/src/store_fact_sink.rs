//! # 机制状态（蓝图 68 卷 68.4 WP-68.4.1 解冻翻转——原总册 ID SFS-deferred）
//!
//! **StoreFactEventSink 已接生产装配**（svc-market bin 启动期
//! `fact_face::open_fact_face` → DATA_FACT_* 区段 → Q0 写透 RPO=0；
//! 授予边 = svc-market→svc-storage 既有边）。上游事实 transport
//! （market peer / sealed FactEnvelope 通道）未部署 ⇒ sink 生产在位、
//! 零消费——跨 bin 接线（market_ingest/external_facts）登记后续卷
//! （68.4 执行卡 WP-68.4.1 改动面 3）。解冻条件="68.3 块面 + 68.4 接线
//! 完成"（Z8），证据 = evidence/68/vol68-4/ 双窗 QEMU 日志。
//!
//! 蓝图 61 [11] §5.1/§5.9 + [07] §4.6.3（v0.314.32）：外部事实的
//! **持久化**事件面 sink——把已认证事实（class/external_id/revision/
//! canonical/now_ns）写入分区事件日志，RPO=0、掉电/重启后可有序重放，
//! 重放不重复、不丢。
//!
//! # 不变量
//!
//! - **ACK 即 durable**：本 sink 用 Q0（写透，`flags=0`）追加——`append`
//!   返回前记录已落盘（RPO=0，[07] §4.6）。
//! - **同源同分区保序**：分区键 = `external_id` 的确定性散列（[07] §4.3
//!   分区键语义的 external 面）——同一外部事实源的修订按 revision 序
//!   落同分区，重放保序。
//! - **重放去重**：`event_id` 由 (class, external_id, revision) 确定性
//!   派生；消费侧以 event_id/位点幂等关联（[07] §4.4）。
//! - **水位重建**：`recover()` 重放全部分区，重建 `external_id → 已见
//!   最高 revision`（跨实例/重启后新修订仍单调；[11] §5.5 append 新
//!   revision 纪律）。
//!
//! 本模块实现 [`crate::external_facts::FactEventSink`]；target 装配把
//! 认证后的事实经本 sink 持久化（此前仅 [`MemFactEventSink`] 内存形态，
//! 掉电即失——本模块补上持久面）。

use alloc::vec::Vec;

use eneros_event_store::{
    EventStore, LogOffset, PartitionId, PartitionOffset, RecordRef, RecordVisitor,
};
use eneros_signer_keyring::SignerKeyring;

use crate::external_facts::{FactEventEntry, FactEventSink};

/// 本 sink 的消费组（位点双槽持久；与 Snapshot 消费组隔离）。
pub const FACT_SINK_GROUP: eneros_event_store::ConsumerGroupId = 0x4641; // "FA"

/// 分区键取模范围（与 event-store 接入桥同域：低 24 位）。
const PARTITION_MOD: u32 = 1 << 24;

/// 事实记录 magic/版本。
const FACT_MAGIC: [u8; 4] = *b"FAC1";
const FACT_VERSION: u8 = 1;
/// 记录头长度：magic(4) + version(1) + class(1) + external_id(16) +
/// revision(4) + now_ns(8) + canonical_len(4)。
const FACT_HEADER_LEN: usize = 4 + 1 + 1 + 16 + 4 + 8 + 4;

/// 事件日志的事实记录编码。
fn encode_fact(
    class: u8,
    external_id: &[u8; 16],
    revision: u32,
    canonical: &[u8],
    now_ns: u64,
) -> Result<Vec<u8>, &'static str> {
    let total = FACT_HEADER_LEN
        .checked_add(canonical.len())
        .ok_or("fact record too large")?;
    if total > 1152 {
        return Err("fact record exceeds 1152B limit");
    }
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&FACT_MAGIC);
    out.push(FACT_VERSION);
    out.push(class);
    out.extend_from_slice(external_id);
    out.extend_from_slice(&revision.to_le_bytes());
    out.extend_from_slice(&now_ns.to_le_bytes());
    out.extend_from_slice(&(canonical.len() as u32).to_le_bytes());
    out.extend_from_slice(canonical);
    Ok(out)
}

/// 事件日志的事实记录解码（fail-closed：坏记录返回 None，调用方计数）。
fn decode_fact(payload: &[u8]) -> Option<FactEventEntry> {
    if payload.len() < FACT_HEADER_LEN || payload[..4] != FACT_MAGIC || payload[4] != FACT_VERSION {
        return None;
    }
    let class = payload[5];
    let mut external_id = [0u8; 16];
    external_id.copy_from_slice(&payload[6..22]);
    let revision = u32::from_le_bytes(payload[22..26].try_into().ok()?);
    let now_ns = u64::from_le_bytes(payload[26..34].try_into().ok()?);
    let canonical_len = u32::from_le_bytes(payload[34..38].try_into().ok()?) as usize;
    let end = 38usize.checked_add(canonical_len)?;
    if end > payload.len() {
        return None;
    }
    Some((
        class,
        external_id,
        revision,
        payload[38..end].to_vec(),
        now_ns,
    ))
}

/// 分区键：external_id 的 FNV-1a 散列（确定性、同源同分区保序）。
fn partition_of(external_id: &[u8; 16]) -> PartitionId {
    let mut h: u32 = 0x811C_9DC5;
    for b in external_id {
        h ^= u32::from(*b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h.wrapping_rem(PARTITION_MOD)
}

/// event_id：由 (class, external_id, revision) 确定性派生（重放去重键）。
fn event_id_of(class: u8, external_id: &[u8; 16], revision: u32) -> [u8; 16] {
    let mut seed = Vec::with_capacity(1 + 16 + 4);
    seed.push(class);
    seed.extend_from_slice(external_id);
    seed.extend_from_slice(&revision.to_le_bytes());
    let h = eneros_crypto::sm3::hash(&seed);
    let mut id = [0u8; 16];
    id.copy_from_slice(&h[..16]);
    // event-store 拒绝全零 event_id（与擦除态歧义）；确定性种子几乎
    // 不可能全零，防御性置位。
    if id == [0; 16] {
        id[0] = 1;
    }
    id
}

/// 事实消费者（重放 visitor）。
struct FactCollector {
    entries: Vec<FactEventEntry>,
    decode_failures: u64,
}

impl RecordVisitor for FactCollector {
    fn record(&mut self, r: RecordRef<'_>) -> bool {
        match decode_fact(r.payload) {
            Some((class, external_id, revision, canonical, now_ns)) => {
                self.entries
                    .push((class, external_id, revision, canonical, now_ns));
            }
            None => self.decode_failures += 1,
        }
        true
    }
}

/// 事件日志事实 sink：把已认证外部事实持久化进分区事件日志。
pub struct StoreFactEventSink<S: EventStore> {
    store: S,
    /// external_id → 已见最高 revision（重放重建，跨实例单调）。
    /// 修订水位（SignerKeyring 承载——判据单源 63.3 WP-63.3.1；原本地
    /// BTreeMap + 手写回退比较删除）。
    revisions: SignerKeyring,
    /// 追加失败计数（可观测，不吞）。
    pub append_failures: u64,
    /// 解码失败计数（重放时损坏记录，可观测）。
    pub decode_failures: u64,
}

impl<S: EventStore> StoreFactEventSink<S> {
    pub fn new(store: S) -> Self {
        Self {
            store,
            revisions: SignerKeyring::new(),
            append_failures: 0,
            decode_failures: 0,
        }
    }

    pub fn into_inner(self) -> S {
        self.store
    }

    /// 重放全部分区的事实（位点续读 + 提交；重复调用不重复交付）。
    pub fn collect_facts(&mut self) -> Result<Vec<FactEventEntry>, &'static str> {
        let mut entries = Vec::new();
        let mut all: Vec<PartitionId> = self.store.partitions().into_iter().collect();
        // 追加失败后记录的分区仍应可枚举（store.partitions 已含）。
        all.sort_unstable();
        for partition in all {
            let from = self
                .store
                .committed_offset(FACT_SINK_GROUP, partition)
                .unwrap_or(0);
            let mut collector = FactCollector {
                entries: Vec::new(),
                decode_failures: 0,
            };
            let next = self
                .store
                .read(partition, from, &mut collector)
                .map_err(|_| "event-store read failed")?
                .next_offset;
            self.decode_failures += collector.decode_failures;
            if next > from {
                self.store
                    .commit_consumer(FACT_SINK_GROUP, &[PartitionOffset::new(partition, next)])
                    .map_err(|_| "event-store commit failed")?;
            }
            entries.extend(collector.entries);
        }
        Ok(entries)
    }

    /// 重启恢复：重放**全量**日志重建 revision 水位（跨实例新修订仍
    /// 单调）。与交付位点无关——水位是"已 durable 追加"的性质，必须
    /// 从 0 起读，不能从消费组已提交位点读（位点已推进时水位会漏建）。
    pub fn recover(&mut self) -> Result<(), &'static str> {
        let mut all: Vec<PartitionId> = self.store.partitions().into_iter().collect();
        all.sort_unstable();
        for partition in all {
            let mut collector = FactCollector {
                entries: Vec::new(),
                decode_failures: 0,
            };
            self.store
                .read(partition, 0, &mut collector)
                .map_err(|_| "event-store read failed")?;
            self.decode_failures += collector.decode_failures;
            for (_class, external_id, revision, _canonical, _now) in collector.entries {
                // max-merge：commit_revision 只升不降——等值/回退即跳过
                //（Err 忽略 = 与原 `if revision > *e` 语义逐点一致）。
                let _ = self.revisions.commit_revision(&external_id, revision);
            }
        }
        Ok(())
    }
}

impl<S: EventStore> FactEventSink for StoreFactEventSink<S> {
    fn append(
        &mut self,
        class: u8,
        external_id: &[u8; 16],
        revision: u32,
        canonical: &[u8],
        now_ns: u64,
    ) -> Result<u64, &'static str> {
        // 修订单调（含重启后重建的水位；[11] §5.5 不原地改值）——判据
        // 由 keyring check_revision 单源把守（63.3 WP-63.3.1）。
        if self
            .revisions
            .check_revision(external_id, revision)
            .is_err()
        {
            return Err("stale revision");
        }
        let payload = encode_fact(class, external_id, revision, canonical, now_ns)?;
        let partition = partition_of(external_id);
        let event_id = event_id_of(class, external_id, revision);
        // Q0 写透：ACK 即 durable（RPO=0）。
        let offset: LogOffset = self
            .store
            .append(partition, event_id, 0, &payload)
            .map_err(|_| {
                self.append_failures += 1;
                "event-store append failed"
            })?;
        // 判据已由 check_revision 把守——commit 仅防御（不可达臂与
        // market/facts 两消费方同款）。
        let _ = self.revisions.commit_revision(external_id, revision);
        Ok(offset)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use eneros_event_store::block::BlockEventStore;
    use eneros_event_store::format::BLOCK_SIZE;
    use eneros_event_store::memdev::MemBlockDevice;
    use eneros_event_store::StoreConfig;

    use super::*;
    use crate::external_facts::{seal, test_keypair, FactEnvelope, FactHeader, MarketFact};

    fn config() -> StoreConfig {
        StoreConfig {
            segment_bytes: 8192,
            ..StoreConfig::default()
        }
    }

    fn header(external_id: [u8; 16], revision: u32) -> FactHeader {
        FactHeader {
            external_id,
            revision,
            source_id: 9,
            classification: 1,
            published_at_ms: 1000,
            effective_from_ms: 1000,
            expires_at_ms: 1_000_000,
        }
    }

    fn market_fact() -> MarketFact {
        MarketFact {
            product: 100,
            region: 1,
            delivery_start_ms: 1_000_000_000,
            duration_ns: 900_000_000_000,
            price: 412.5,
            volume: 10.0,
            currency_code: 156,
            tz_offset_min: 480,
        }
    }

    fn sealed(external_id: [u8; 16], revision: u32) -> FactEnvelope<MarketFact> {
        let mut env = FactEnvelope {
            header: header(external_id, revision),
            payload: market_fact(),
            key_id: *b"testkey1",
            signature: [0; 64],
            payload_digest: [0; 32],
        };
        let (sk, pk) = test_keypair();
        seal(&mut env, &sk, &pk);
        env
    }

    #[test]
    fn durable_fact_sink_rpo0_across_power_cut() {
        // 真实块介质 + 真实 EventStore：掉电 = 丢弃 sink/store、介质保留，
        // 重开介质必须完整恢复全部已 ACK 事实（RPO=0，[07] §4.7 门 2）。
        let dev = MemBlockDevice::new(3 + 64 * 2, BLOCK_SIZE);
        let store = BlockEventStore::format(dev, config()).unwrap();
        let mut sink = StoreFactEventSink::new(store);

        // 写入 3 条事实（同 external_id 不同 revision + 另一 external_id）。
        let a = sealed([7; 16], 1);
        sink.append(1, &[7; 16], 1, &a.canonical_bytes(), 2_000)
            .unwrap();
        let a2 = sealed([7; 16], 2);
        sink.append(1, &[7; 16], 2, &a2.canonical_bytes(), 3_000)
            .unwrap();
        let b = sealed([8; 16], 1);
        sink.append(2, &[8; 16], 1, &b.canonical_bytes(), 4_000)
            .unwrap();

        // 掉电：丢弃 sink+store，介质保留。
        let dev = sink.into_inner().into_inner();
        let store = BlockEventStore::open(dev, config()).unwrap();
        let mut sink2 = StoreFactEventSink::new(store);
        let recovered = sink2.collect_facts().unwrap();
        assert_eq!(recovered.len(), 3, "RPO=0：已 ACK 事实全恢复");
        // 顺序（分区内 offset 序）与 canonical 原样。
        assert_eq!(recovered[0].1, [7; 16]);
        assert_eq!(recovered[0].2, 1);
        assert_eq!(recovered[0].3, a.canonical_bytes());
        assert_eq!(recovered[2].1, [8; 16]);
        // 幂等：再 drain 零重复（位点双槽持久）。
        assert!(sink2.collect_facts().unwrap().is_empty());
        assert_eq!(sink2.decode_failures, 0);

        // 重启后 revision 水位重建：旧修订被拒，新修订通过。
        sink2.recover().unwrap();
        let stale = sealed([7; 16], 1);
        assert_eq!(
            sink2.append(1, &[7; 16], 1, &stale.canonical_bytes(), 5_000),
            Err("stale revision")
        );
        let a3 = sealed([7; 16], 3);
        sink2
            .append(1, &[7; 16], 3, &a3.canonical_bytes(), 6_000)
            .unwrap();
    }

    #[test]
    fn durable_sink_wires_through_fact_adapter() {
        // 目标装配面：FactAdapter.accept → StoreFactEventSink → event-store。
        // 认证 + 修订水位 + 时效门走 adapter；持久化走 durable sink；掉电
        // 重开介质后已接受事实可追溯恢复（[11] §5.9）。
        use crate::external_facts::FactAdapter;

        let dev = MemBlockDevice::new(3 + 64 * 2, BLOCK_SIZE);
        let store = BlockEventStore::format(dev, config()).unwrap();
        let mut sink = StoreFactEventSink::new(store);
        let (_sk, pk) = test_keypair();
        let mut keys = eneros_signer_keyring::SignerKeyring::new();
        keys.register(*b"testkey1", &pk);
        let mut a = FactAdapter::new(keys);

        let e1 = sealed([9; 16], 1);
        let off = a.accept(&mut sink, &e1, 5000).unwrap();
        assert_eq!(off, 0, "adapter 接受后 offset 返回");
        assert_eq!(a.revision_of(&[9; 16]), Some(1));

        // 掉电重启：新 adapter（内存水位丢失）+ 新 durable sink（从日志
        // 重建水位）——同一外部事实不得重复入账，新修订可接受。
        let dev = sink.into_inner().into_inner();
        let store = BlockEventStore::open(dev, config()).unwrap();
        let mut sink2 = StoreFactEventSink::new(store);
        sink2.recover().unwrap();
        let recovered = sink2.collect_facts().unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].2, 1, "revision 可追溯");
        assert_eq!(
            recovered[0].3,
            e1.canonical_bytes(),
            "canonical（含签名）可追溯"
        );

        let (_sk2, pk2) = test_keypair();
        let mut keys2 = eneros_signer_keyring::SignerKeyring::new();
        keys2.register(*b"testkey1", &pk2);
        let mut a2 = FactAdapter::new(keys2);
        // 旧修订重放 → adapter 水位重建后拒（水位来自 durable sink 的
        // recover——adapter 自身内存水位在重启后为空，此处验证 durable
        // sink 水位拦下）。
        let stale = sealed([9; 16], 1);
        // adapter 内存水位空 → 认证/水位通过 → 落到 durable sink → sink
        // 水位（recover 重建）拒。
        let res = a2.accept(&mut sink2, &stale, 5000);
        assert!(
            matches!(res, Err(crate::external_facts::FactReject::Sink(_))),
            "durable sink 水位必须拦下重启后重放的旧修订，got {res:?}"
        );
        // 新修订 → 通过并落盘；再次 drain 只交付新记录（旧修订已被首个
        // drain 消费并提交位点——幂等交付，[07] §4.4）。
        let e2 = sealed([9; 16], 2);
        a2.accept(&mut sink2, &e2, 5000).unwrap();
        let delivered = sink2.collect_facts().unwrap();
        assert_eq!(delivered.len(), 1, "幂等交付：只交付新修订");
        assert_eq!(delivered[0].2, 2);
    }
}

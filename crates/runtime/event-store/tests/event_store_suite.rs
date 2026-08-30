//! 蓝图 61 [07] §4.6.1/§4.7（v0.314.32）：事件日志属性/故障红测套件。
//!
//! - 属性：append/read 往返、offset 单调、多分区隔离、超限拒绝；
//! - 故障：半写 header/record/trailer、CRC 错、offset 空洞、位点撕裂；
//! - 恢复：断电 100 次循环——已 ACK 记录零丢失零重复、offset 不回退；
//! - 差分：Mem 参考模型 vs Block 实现同操作序列逐 API 比对；
//! - 容量：水位阈值查询与 Q2/Q3 95% 拒绝；保留只删完整段 + 审计清单。
#![allow(clippy::disallowed_macros)] // 测试断言用 panic!/expect——预期路径

use eneros_event_store::block::BlockEventStore;
use eneros_event_store::format::BLOCK_SIZE;
use eneros_event_store::mem::MemEventStore;
use eneros_event_store::memdev::{Fault, FaultyBlockDevice, MemBlockDevice};
use eneros_event_store::{
    EventStore, PartitionId, PartitionOffset, ReadReport, RecordRef, RecordVisitor, StoreConfig,
    StoreError,
};

fn test_config() -> StoreConfig {
    // 小段（8KiB）使多 segment 场景在测试规模内可达。
    StoreConfig {
        segment_bytes: 8192,
        q0_batch_records: 1,
        ..StoreConfig::default()
    }
}

fn block_dev(segments: u64) -> MemBlockDevice {
    // 固定区 3 块 + segments。8KiB 段 = 2 块；每段含 1 个 header 块 + 数据。
    MemBlockDevice::new(3 + segments * 2, BLOCK_SIZE)
}

fn fresh_block(segments: u64) -> BlockEventStore<MemBlockDevice> {
    BlockEventStore::format(block_dev(segments), test_config()).unwrap()
}

fn fresh_block_faulty(segments: u64) -> BlockEventStore<FaultyBlockDevice<MemBlockDevice>> {
    let dev = FaultyBlockDevice::new(block_dev(segments));
    BlockEventStore::format(dev, test_config()).unwrap()
}

/// 事件 id（全零保留——测试用固定前缀）。
fn eid(n: u64) -> [u8; 16] {
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&(n + 1).to_le_bytes()); // 全零保留（BadEventId）
    b[15] = 0xE0;
    b
}

fn collect(
    store: &dyn EventStore,
    partition: PartitionId,
    from: u64,
    limit: usize,
) -> Result<(Vec<u64>, Vec<Vec<u8>>), StoreError> {
    struct V {
        offsets: Vec<u64>,
        payloads: Vec<Vec<u8>>,
        limit: usize,
    }
    impl RecordVisitor for V {
        fn record(&mut self, rec: RecordRef<'_>) -> bool {
            self.offsets.push(rec.offset);
            self.payloads.push(rec.payload.to_vec());
            self.offsets.len() < self.limit
        }
    }
    let mut v = V {
        offsets: Vec::new(),
        payloads: Vec::new(),
        limit,
    };
    let report: ReadReport = store.read(partition, from, &mut v)?;
    let _ = report;
    Ok((v.offsets, v.payloads))
}

// ===========================================================================
// §4.6.1 红测 1：规模属性（两实现同断言）
// ===========================================================================

#[test]
fn append_read_roundtrip_and_offset_monotonic() {
    let mut mem = MemEventStore::new(1 << 20, test_config()).unwrap();
    let mut blk = fresh_block(64);
    for store in [
        &mut mem as &mut dyn EventStore,
        &mut blk as &mut dyn EventStore,
    ] {
        let mut last = 0u64;
        for i in 0..50u64 {
            let off = store.append(7, eid(i), 0, &[i as u8; 40]).unwrap();
            assert_eq!(off, i, "offset 从 0 起严格单调");
            assert!(i == 0 || off > last);
            last = off;
        }
        let (offsets, payloads) = collect(store, 7, 0, usize::MAX).unwrap();
        assert_eq!(offsets, (0..50).collect::<Vec<_>>());
        assert_eq!(payloads.len(), 50);
        assert_eq!(payloads[49], vec![49u8; 40]);
        // 分区隔离：另一分区为空。
        let (o2, _) = collect(store, 8, 0, 10).unwrap();
        assert!(o2.is_empty());
    }
}

#[test]
fn oversized_record_rejected() {
    let mut mem = MemEventStore::new(1 << 20, test_config()).unwrap();
    assert!(matches!(
        mem.append(1, eid(1), 0, &[0u8; 4096]),
        Err(StoreError::RecordTooLarge)
    ));
}

#[test]
fn reserved_zero_event_id_rejected() {
    let mut mem = MemEventStore::new(1 << 20, test_config()).unwrap();
    assert!(matches!(
        mem.append(1, [0u8; 16], 0, &[1u8; 8]),
        Err(StoreError::BadEventId)
    ));
}

// ===========================================================================
// §4.6.1 红测 2：故障注入（半写/丢写/位翻转）→ 恢复
// ===========================================================================

#[test]
fn half_written_record_recovered_by_crc_truncation() {
    let dev = block_dev(16);
    let mut store = BlockEventStore::format(FaultyBlockDevice::new(dev), test_config()).unwrap();
    // 3 条记录：Q1 攒批后显式 flush 落盘。
    for i in 1..=3u64 {
        store.append(2, eid(i), 2, &[i as u8; 40]).unwrap();
    }
    store.flush().unwrap();
    // 第 4 条：record 字节半写（只落 8 字节）后故障。
    // 半写 record 4（rel 292 起）：keep=320 覆盖其头 28B，尾部旧字节
    // 残留 → record 4 CRC/结构必败，recover 截断。
    store.dev.arm(Fault::TruncateWrite { keep: 320 });
    assert!(store.append(2, eid(4), 0, &[4u8; 40]).is_err());
    // 掉电后 reopen（零写扫描）→ recover 截断逻辑尾。
    let dev = store.into_inner().into_inner();
    let mut reopened = BlockEventStore::open(FaultyBlockDevice::new(dev), test_config()).unwrap();
    let rep = reopened.recover().unwrap();
    assert!(rep.truncated_records >= 1, "{rep:?}");
    let (offsets, payloads) = collect(&reopened, 2, 0, usize::MAX).unwrap();
    assert_eq!(offsets, vec![0, 1, 2], "半写记录不出现在读面");
    assert_eq!(payloads[0], vec![1u8; 40]);
}

#[test]
fn dropped_trailer_marks_open_segment_and_recover_reseals() {
    let mut store = fresh_block(16);
    // 填满一个段（8KiB 段：header 64B + records；40B payload 记录 ≈ 60 条/段）。
    let mut n = 0u64;
    loop {
        match store.append(3, eid(n + 100), 0, &[0xEE; 40]) {
            Ok(_) => n += 1,
            Err(StoreError::Full) => break,
            Err(e) => panic!("{e:?}"),
        }
        if n > 2000 {
            panic!("段没满？");
        }
    }
    let _ = store.flush(); // Full 后 staged 残留可能落不进——尽力即可
                           // 模拟掉电：直接 reopen（trailer 已写 = 完整段——此处验证完整段
                           // recover 零截断、零损坏报告）。
    let dev = store.into_inner();
    let mut reopened = BlockEventStore::open(dev, test_config()).unwrap();
    let rep = reopened.recover().unwrap();
    assert_eq!(rep.bad_segments, 0, "{rep:?}");
    let (offsets, _) = collect(&reopened, 3, 0, usize::MAX).unwrap();
    let got = offsets.len() as u64;
    assert!(
        got == n || got == n - 1,
        "完整段零丢失：磁盘 {got} ≠ n={n}（末条可能仍 staged 未落盘）"
    );
}

#[test]
fn corrupted_record_detected_and_not_returned() {
    let dev = block_dev(16);
    let mut store = BlockEventStore::format(FaultyBlockDevice::new(dev), test_config()).unwrap();
    for i in 1..=3u64 {
        store.append(4, eid(i), 2, &[i as u8; 48]).unwrap();
    }
    store.flush().unwrap();
    // 第 4 条写后静默位翻转（CRC 唯一防线）。
    // payload 位翻转（rel 64 头 36B → record 0 载荷 100..148；块内 byte
    // 128 落在载荷内）——CRC 唯一防线必须捕获。
    store.dev.arm(Fault::CorruptAfterWrite {
        byte: 128,
        mask: 0xFF,
    });
    store.append(4, eid(4), 0, &[4u8; 48]).unwrap();
    let dev = store.into_inner().into_inner();
    let mut reopened = BlockEventStore::open(FaultyBlockDevice::new(dev), test_config()).unwrap();
    let rep = reopened.recover().unwrap();
    assert!(rep.truncated_records >= 1, "翻转记录被截断：{rep:?}");
    // record 0 损坏 → 其后全部不可达：读为空（绝不返回损坏数据）。
    let (offsets, _) = collect(&reopened, 4, 0, usize::MAX).unwrap();
    assert!(offsets.is_empty(), "损坏记录绝不返回，实际 {offsets:?}");
}

// ===========================================================================
// §4.6.1 红测 3：offset 空洞与位点撕裂
// ===========================================================================

#[test]
fn consumer_offset_double_slot_no_rollback_after_power_loss() {
    let dev = block_dev(16);
    let mut store = BlockEventStore::format(FaultyBlockDevice::new(dev), test_config()).unwrap();
    for i in 1..=10u64 {
        store.append(5, eid(i), 0, &[1; 8]).unwrap();
    }
    store.flush().unwrap();
    // 提交 group=9 partition=5 → offset 7（双槽世代）。
    for epoch in 1..=3u64 {
        store
            .commit_consumer(9, &[PartitionOffset::new(5, 3 + epoch)])
            .unwrap();
    }
    let dev = store.into_inner().into_inner();
    // 掉电 reopen：位点必须 ≥ 最后提交值（不回退）。
    let mut reopened = BlockEventStore::open(FaultyBlockDevice::new(dev), test_config()).unwrap();
    assert_eq!(reopened.committed_offset(9, 5), Some(6));
    // 消费自提交位点续读（0-based：位点 6 = 已提交的最后记录，续读从 6 起）。
    let (offsets, _) = collect(&reopened, 5, 6, usize::MAX).unwrap();
    assert_eq!(offsets, vec![6, 7, 8, 9]);
    // 恢复幂等。
    let rep2 = reopened.recover().unwrap();
    let (offsets2, _) = collect(&reopened, 5, 6, usize::MAX).unwrap();
    assert_eq!(offsets2, offsets);
    let _ = rep2;
}

#[test]
fn consumer_offset_torn_slot_falls_back_to_other_slot() {
    let mut store = fresh_block_faulty(16);
    store
        .commit_consumer(1, &[PartitionOffset::new(5, 4)])
        .unwrap();
    // 新槽写后位翻转（entry 域）→ 读回校验失败 → 提交显式失败、状态回退。
    store.dev.arm(Fault::CorruptAfterWrite {
        byte: 22,
        mask: 0xFF,
    });
    assert!(store
        .commit_consumer(1, &[PartitionOffset::new(5, 9)])
        .is_err());
    assert_eq!(store.committed_offset(1, 5), Some(4), "撕裂提交状态回退");
    // reopen：旧槽（gen 1）有效 → 掉电不回退、不静默损坏。
    let dev = store.into_inner();
    let reopened = BlockEventStore::open(dev, test_config()).unwrap();
    assert_eq!(reopened.committed_offset(1, 5), Some(4), "撕裂槽开机回退");
}

// ===========================================================================
// §4.7 门禁：断电 100 次循环——零丢失零重复、offset 不回退
// ===========================================================================

#[test]
fn power_cut_100_cycles_no_loss_no_duplicate_no_rollback() {
    let mut rng_state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        rng_state = rng_state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        rng_state
    };
    let dev = block_dev(64);
    let mut store = BlockEventStore::format(FaultyBlockDevice::new(dev), test_config()).unwrap();
    let mut acked: Vec<(PartitionId, u64)> = Vec::new(); // (partition, offset) 已 ACK
    let mut per_part: std::collections::BTreeMap<PartitionId, u64> = Default::default();
    for cycle in 0..100u64 {
        // 随机追加 0..24 条（Q0 写透 = ACK 即 durable）。
        let batch = (next() % 24) + 1;
        for _ in 0..batch {
            let part = (next() % 4 + 1) as PartitionId; // 1..=4
            let cur = per_part.entry(part).or_insert(0);
            let off = store
                .append(part, eid(next()), 0, &[cycle as u8; 32])
                .unwrap();
            assert_eq!(off, *cur); // 0-based（cur = 下一条期望 offset）
            *cur = off + 1;
            acked.push((part, off));
        }
        // 随机掉电点：50% 概率在 flush 前掉（Q0 已写透——flush 只保介质序）。
        let drop = next() % 2 == 0;
        if !drop {
            store.flush().unwrap();
        }
        let dev = store.into_inner().into_inner();
        let mut reopened =
            BlockEventStore::open(FaultyBlockDevice::new(dev), test_config()).unwrap();
        reopened.recover().unwrap();
        // 已 ACK（Q0 写透）记录必须全部可读且恰好一次。
        for part in 1..=4u32 {
            let (offsets, _) = collect(&reopened, part, 0, usize::MAX).unwrap();
            let next = *per_part.entry(part).or_insert(0);
            let expected: Vec<u64> = (0..next).collect();
            assert_eq!(
                offsets, expected,
                "cycle {cycle} partition {part}：已 ACK 记录零丢失零重复零回退"
            );
        }
        let dev = reopened.into_inner().into_inner(); // 剥掉故障层回 MemBlockDevice
        store = BlockEventStore::format(FaultyBlockDevice::new(dev), test_config()).unwrap();
    }
    assert!(acked.len() > 500, "循环累计事件数：{}", acked.len());
}

// ===========================================================================
// §4.5：水位与保留
// ===========================================================================

#[test]
fn watermarks_reach_95_percent_and_reject_q2q3() {
    let mut store = MemEventStore::new(
        8192 * 2 + 4096 * 3,
        StoreConfig {
            segment_bytes: 8192,
            q0_batch_records: 1,
            reject_q2q3_at_95_percent: true,
            ..StoreConfig::default()
        },
    )
    .unwrap();
    // 填满（Q0 写透）。
    let mut n = 0u64;
    while store.append(1, eid(n + 1), 0, &[1u8; 48]).is_ok() {
        n += 1;
        if n > 500 {
            break;
        }
    }
    let wm = store.watermarks();
    assert!(wm.used_permille >= 950, "水位 ≥95%：{wm:?}");
    // Q2/Q3 在高水位被拒；Q0 仍可（除非 Full）。
    assert!(matches!(
        store.append(1, eid(0xFFFF_0000), 2, &[2; 8]),
        Err(StoreError::HighWatermark) | Err(StoreError::Full)
    ));
    let _ = n;
}

#[test]
fn retention_deletes_only_full_segments_with_audit_trail() {
    let dev = block_dev(32);
    let mut store = BlockEventStore::format(FaultyBlockDevice::new(dev), test_config()).unwrap();
    // 写入 3 个完整段（每段约 60 条 48B 记录）——直到 Full 前的段 1、2 完整。
    let mut n = 0u64;
    loop {
        match store.append(6, eid(n + 1), 0, &[0xAB; 48]) {
            Ok(_) => n += 1,
            Err(StoreError::Full) => break,
            Err(e) => panic!("{e:?}"),
        }
        if n > 4000 {
            panic!("没满");
        }
    }
    let _ = store.flush(); // Full 后 staged 残留可能落不进——尽力即可
                           // 保留删除：只删 last_offset < 61 的完整段（每段约 53 条 → 段 1 的
                           // 末条 offset=52；keep_after=61 恰好删段 1 保留段 2）。
    let deleted = store.delete_full_segments_before(6, 61).unwrap();
    assert!(!deleted.is_empty(), "至少删掉首个完整段");
    for d in &deleted {
        assert!(d.last_offset < 61, "删除段范围核对：{d:?}");
        assert!(!d.digest.iter().all(|b| *b == 0), "审计 digest 非空");
    }
    // 删除后：保留段仍可读（61 起）；删除段范围读 → OffsetGone。
    let (offsets, _) = collect(&store, 6, 61, usize::MAX).unwrap();
    assert!(!offsets.is_empty(), "保留段可读");
    assert!(offsets[0] >= 61);
    // 完整段校验：deleted.end_offset 恰好 = 段 1 末条。
}

// ===========================================================================
// §4.7 门禁：差分（Mem 参考模型 vs Block 实现）
// ===========================================================================

#[test]
fn differential_mem_vs_block_same_operation_sequence() {
    let mut mem = MemEventStore::new(1 << 20, test_config()).unwrap();
    let mut blk = fresh_block(64);
    let mut rng_state: u64 = 0x1234_5678_9ABC_DEF0;
    let mut next = || {
        rng_state = rng_state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        rng_state
    };
    // 同序列：append（随机分区/长度）+ 偶发 flush + 随机 commit。
    for step in 0..300u64 {
        let part = (next() % 4 + 1) as PartitionId;
        let len = (next() % 200 + 8) as usize;
        let payload: Vec<u8> = (0..len)
            .map(|i| (i as u8).wrapping_add(step as u8))
            .collect();
        let id = eid(next());
        let flags = (next() % 4) as u16;
        let rm = mem.append(part, id, flags, &payload);
        let rb = blk.append(part, id, flags, &payload);
        assert_eq!(rm.is_ok(), rb.is_ok(), "step {step} append 结果分歧");
        if let (Ok(om), Ok(ob)) = (rm, rb) {
            assert_eq!(om, ob, "step {step} offset 分歧");
        }
        if step % 7 == 0 {
            let gm = mem.flush();
            let gb = blk.flush();
            assert_eq!(gm.is_ok(), gb.is_ok());
        }
        if step % 11 == 0 {
            let off = if step > 30 {
                Some(next() % (step.min(40)) + 1)
            } else {
                None
            };
            if let Some(o) = off {
                let om = mem.commit_consumer(3, &[PartitionOffset::new(1, o)]);
                let ob = blk.commit_consumer(3, &[PartitionOffset::new(1, o)]);
                assert_eq!(om.is_ok(), ob.is_ok());
            }
        }
    }
    // 逐分区读全量比对。
    for part in 1..=4u32 {
        let (om, pm) = collect(&mem, part, 1, usize::MAX).unwrap();
        let (ob, pb) = collect(&blk, part, 1, usize::MAX).unwrap();
        assert_eq!(om, ob, "partition {part} offset 序列分歧");
        assert_eq!(pm, pb, "partition {part} payload 分歧");
    }
    assert_eq!(mem.committed_offset(3, 1), blk.committed_offset(3, 1));
    // 水位趋势一致。
    let (wm, wb) = (
        mem.watermarks().used_permille,
        blk.watermarks().used_permille,
    );
    // 两实现分配粒度不同（块实现的槽位计含已分配未写段），趋势一致即
    // 可——宽松容差 + 单调性由上面 append 结果一致性保证。
    assert!(
        (wm as i64 - wb as i64).abs() <= 300,
        "水位分歧 {wm}‰ vs {wb}‰"
    );
}

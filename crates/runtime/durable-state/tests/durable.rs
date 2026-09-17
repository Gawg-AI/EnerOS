//! durable-state 语义测试（蓝图 41 WP-41.3 二次验证四象限）。
//!
//! 覆盖：
//! - 正例：RegionStateStore CAS 推进 / AppendOutbox enqueue→sent→acked；
//! - 输入负例：坏 CRC（撕裂）、旧 generation、坏长度、未知 magic；
//! - 依赖故障：写失败（fail_writes）、flush 失败、容量不足；
//! - 恢复：掉电撕裂（尾部槽位丢弃、已提交记录保留）、重启恢复 pending、
//!   幂等去重、重复 ACK no-op。

extern crate alloc;
use std::cell::RefCell;
use std::rc::Rc;

use eneros_durable_state::{
    AppendOutbox, Decode, DurableOutbox, DurableStateStore, Encode, Generation, IdempotencyKey,
    MemIo, MemOutbox, MemStateStore, MessageState, OutboxId, PeerReceipt, Pending, PersistIo,
    RegionStateStore, StoreError, UnavailableOutbox, UnavailableStateStore,
};

/// 共享内存后端（测试）：多个实例共享同一介质——模拟掉电/重启后
/// 持久化字节仍在（区别于 MemIo 的 per-instance 克隆）。
#[derive(Clone)]
struct SharedMem(Rc<RefCell<Vec<u8>>>);

impl SharedMem {
    fn new(capacity: u64) -> Self {
        SharedMem(Rc::new(RefCell::new(vec![0u8; capacity as usize])))
    }
    fn tear(&self, at: u64) {
        let mut b = self.0.borrow_mut();
        for i in (at as usize)..b.len() {
            b[i] = 0xFF;
        }
    }
}

impl PersistIo for SharedMem {
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), StoreError> {
        let b = self.0.borrow();
        let start = offset as usize;
        if start + buf.len() > b.len() {
            return Err(StoreError::OutOfSpace);
        }
        buf.copy_from_slice(&b[start..start + buf.len()]);
        Ok(())
    }
    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), StoreError> {
        let mut b = self.0.borrow_mut();
        let start = offset as usize;
        if start + data.len() > b.len() {
            return Err(StoreError::OutOfSpace);
        }
        b[start..start + data.len()].copy_from_slice(data);
        Ok(())
    }
    fn flush(&mut self) -> Result<(), StoreError> {
        Ok(())
    }
    fn capacity(&self) -> u64 {
        self.0.borrow().len() as u64
    }
}

/// 测试消息（定长编码）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Msg {
    kind: u8,
    seq: u32,
    body: [u8; 16],
}

impl Encode for Msg {
    fn encode(&self) -> Result<alloc::vec::Vec<u8>, StoreError> {
        let mut out = alloc::vec::Vec::with_capacity(21);
        out.push(self.kind);
        out.extend_from_slice(&self.seq.to_le_bytes());
        out.extend_from_slice(&self.body);
        Ok(out)
    }
}

impl Decode for Msg {
    fn decode(bytes: &[u8]) -> Result<(Generation, Self), StoreError> {
        if bytes.len() != 21 {
            return Err(StoreError::Corrupt);
        }
        let mut body = [0u8; 16];
        body.copy_from_slice(&bytes[5..21]);
        Ok((
            Generation(0),
            Msg {
                kind: bytes[0],
                seq: u32::from_le_bytes(bytes[1..5].try_into().unwrap()),
                body,
            },
        ))
    }
}

/// 测试状态。
#[derive(Debug, Clone, PartialEq, Eq)]
struct St {
    value: u64,
    label: [u8; 8],
}

impl Encode for St {
    fn encode(&self) -> Result<alloc::vec::Vec<u8>, StoreError> {
        let mut out = alloc::vec::Vec::with_capacity(16);
        out.extend_from_slice(&self.value.to_le_bytes());
        out.extend_from_slice(&self.label);
        Ok(out)
    }
}

impl Decode for St {
    fn decode(bytes: &[u8]) -> Result<(Generation, Self), StoreError> {
        // payload = generation(8)‖state(16)。
        if bytes.len() != 24 {
            return Err(StoreError::Corrupt);
        }
        let mut label = [0u8; 8];
        label.copy_from_slice(&bytes[16..24]);
        Ok((
            Generation(u64::from_le_bytes(bytes[0..8].try_into().unwrap())),
            St {
                value: u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
                label,
            },
        ))
    }
}

fn key(n: u8) -> IdempotencyKey {
    IdempotencyKey([n; 16])
}

// ---------------------------------------------------------------------------
// RegionStateStore
// ---------------------------------------------------------------------------

#[test]
fn state_cas_positive_and_stale() {
    let mut store: RegionStateStore<St, MemIo> = RegionStateStore::new(MemIo::new(4096), 0, 1024);
    assert_eq!(store.load().unwrap(), None);
    // 首次写入：expected=None。
    let g1 = store
        .compare_and_swap(
            None,
            &St {
                value: 1,
                label: [1; 8],
            },
        )
        .unwrap();
    assert_eq!(g1, Generation(1));
    // 再次 load 得到当前值。
    let v = store.load().unwrap().unwrap();
    assert_eq!(v.generation, Generation(1));
    assert_eq!(v.state.value, 1);
    // 旧 expected → StaleGeneration。
    let err = store
        .compare_and_swap(
            Some(Generation(0)),
            &St {
                value: 9,
                label: [9; 8],
            },
        )
        .unwrap_err();
    assert!(matches!(err, StoreError::StaleGeneration { .. }));
    // 正确 expected 推进。
    let g2 = store
        .compare_and_swap(
            Some(Generation(1)),
            &St {
                value: 2,
                label: [2; 8],
            },
        )
        .unwrap();
    assert_eq!(g2, Generation(2));
}

#[test]
fn state_power_loss_recovery() {
    // 写入后掉电：重新打开（共享介质——模拟持久存储掉电后仍在）能恢复
    // 到完整 generation。
    let mem = SharedMem::new(4096);
    {
        let mut store: RegionStateStore<St, SharedMem> =
            RegionStateStore::new(mem.clone(), 0, 1024);
        store
            .compare_and_swap(
                None,
                &St {
                    value: 42,
                    label: [7; 8],
                },
            )
            .unwrap();
    }
    {
        let mut store: RegionStateStore<St, SharedMem> = RegionStateStore::new(mem, 0, 1024);
        let v = store.load().unwrap().unwrap();
        assert_eq!(v.generation, Generation(1));
        assert_eq!(v.state.value, 42);
        assert_eq!(v.state.label, [7; 8]);
    }
}

#[test]
fn state_corruption_rejected() {
    let mem = SharedMem::new(4096);
    {
        let mut store: RegionStateStore<St, SharedMem> =
            RegionStateStore::new(mem.clone(), 0, 1024);
        store
            .compare_and_swap(
                None,
                &St {
                    value: 1,
                    label: [1; 8],
                },
            )
            .unwrap();
    }
    // 撕裂一个字节 → CRC 失败 → Corrupt（不静默 default）。
    mem.0.borrow_mut()[20] ^= 0xFF;
    let mut store: RegionStateStore<St, SharedMem> = RegionStateStore::new(mem, 0, 1024);
    assert!(matches!(store.load(), Err(StoreError::Corrupt)));
}

#[test]
fn state_torn_latest_write_recovers_previous_generation() {
    // E 续作（win4 实测）：双槽代次切换——硬杀撕裂**最新**写入时，重启
    // 必须恢复到上一完整 generation（单槽实现在此场景 Corrupt ⇒ 消费方
    // fail-closed；盘上实证 payload 完好而 CRC 撕裂）。
    let mem = SharedMem::new(4096);
    {
        let mut store: RegionStateStore<St, SharedMem> =
            RegionStateStore::new(mem.clone(), 0, 1024);
        store
            .compare_and_swap(
                None,
                &St {
                    value: 1,
                    label: [1; 8],
                },
            )
            .unwrap();
        store
            .compare_and_swap(
                Some(Generation(1)),
                &St {
                    value: 2,
                    label: [2; 8],
                },
            )
            .unwrap();
        // 模拟硬杀撕裂 gen2（slot1，slot_len=512 起）写窗口。
        mem.0.borrow_mut()[512 + 20] ^= 0xFF;
    }
    let mut store: RegionStateStore<St, SharedMem> = RegionStateStore::new(mem, 0, 1024);
    let v = store.load().unwrap().unwrap();
    assert_eq!(v.generation, Generation(1));
    assert_eq!(v.state.value, 1);
    // 恢复后仍可推进（写入落回撕过的槽）。
    let g = store
        .compare_and_swap(
            Some(Generation(1)),
            &St {
                value: 3,
                label: [3; 8],
            },
        )
        .unwrap();
    assert_eq!(g, Generation(2));
}

#[test]
fn state_write_failure_no_fake_success() {
    let mut mem = MemIo::new(4096);
    mem.fail_writes = true;
    let mut store: RegionStateStore<St, MemIo> = RegionStateStore::new(mem, 0, 1024);
    let err = store
        .compare_and_swap(
            None,
            &St {
                value: 1,
                label: [1; 8],
            },
        )
        .unwrap_err();
    assert_eq!(err, StoreError::WriteFailed);
    // 失败后状态未变。
    assert_eq!(store.load().unwrap(), None);
}

// ---------------------------------------------------------------------------
// AppendOutbox
// ---------------------------------------------------------------------------

const SLOT_LEN: usize = 128;
const MAX_SLOTS: usize = 8;

fn new_outbox(mem: MemIo) -> AppendOutbox<Msg, MemIo> {
    let mut ob = AppendOutbox::new(mem, 0, SLOT_LEN, MAX_SLOTS);
    ob.open().unwrap();
    ob
}

#[test]
fn outbox_lifecycle_queued_sent_acked() {
    let mut ob = new_outbox(MemIo::new(64 + 8 * SLOT_LEN as u64 + 1024));
    let id = ob
        .enqueue(
            &Msg {
                kind: 1,
                seq: 7,
                body: [3; 16],
            },
            key(1),
        )
        .unwrap();
    assert_eq!(id, OutboxId(0));
    // 幂等：同键再入队返回同 id。
    let id2 = ob
        .enqueue(
            &Msg {
                kind: 1,
                seq: 7,
                body: [3; 16],
            },
            key(1),
        )
        .unwrap();
    assert_eq!(id2, id);
    // 不同键新消息。
    let id3 = ob
        .enqueue(
            &Msg {
                kind: 2,
                seq: 8,
                body: [4; 16],
            },
            key(2),
        )
        .unwrap();
    assert_eq!(id3, OutboxId(1));

    ob.mark_sent(id, 1000).unwrap();
    let pending = ob.recover_pending().unwrap();
    assert_eq!(pending.len(), 2);
    let p0 = pending.iter().find(|p| p.id == id).unwrap();
    assert_eq!(p0.attempts, 1);
    assert_eq!(p0.last_sent_at_ms, 1000);
    assert_eq!(p0.state, MessageState::Sent);

    ob.mark_acked(
        id,
        &PeerReceipt {
            peer: "peer-a".into(),
            receipt_id: 99,
            at_ms: 2000,
        },
    )
    .unwrap();
    // ACK 后不再 pending；另一条仍在。
    let pending = ob.recover_pending().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, id3);
    // 重复 ACK no-op。
    ob.mark_acked(
        id,
        &PeerReceipt {
            peer: "peer-a".into(),
            receipt_id: 99,
            at_ms: 3000,
        },
    )
    .unwrap();
}

#[test]
fn outbox_restart_recovers_pending() {
    let mem = SharedMem::new(64 + 8 * SLOT_LEN as u64 + 1024);
    {
        let mut ob: AppendOutbox<Msg, SharedMem> =
            AppendOutbox::new(mem.clone(), 0, SLOT_LEN, MAX_SLOTS);
        ob.open().unwrap();
        ob.enqueue(
            &Msg {
                kind: 1,
                seq: 1,
                body: [1; 16],
            },
            key(1),
        )
        .unwrap();
        ob.enqueue(
            &Msg {
                kind: 2,
                seq: 2,
                body: [2; 16],
            },
            key(2),
        )
        .unwrap();
        ob.mark_sent(OutboxId(0), 500).unwrap();
    }
    // 重启：新实例同一介质。
    {
        let mut ob: AppendOutbox<Msg, SharedMem> = AppendOutbox::new(mem, 0, SLOT_LEN, MAX_SLOTS);
        ob.open().unwrap();
        let pending = ob.recover_pending().unwrap();
        assert_eq!(pending.len(), 2);
        let p: Vec<Pending<Msg>> = pending;
        assert_eq!(p[0].id, OutboxId(0));
        assert_eq!(p[0].attempts, 1);
        assert_eq!(p[0].message.seq, 1);
        assert_eq!(p[1].id, OutboxId(1));
        assert_eq!(p[1].message.seq, 2);
    }
}

#[test]
fn outbox_acked_slots_remain_reusable_after_restart() {
    // 回归：ACK 后重启，旧实现会把已 ACK 槽位重新占住，导致容量随
    // 重启次数递减，最终 OutOfSpace。已 ACK 槽位必须保持可复用。
    let mem = SharedMem::new(64 + 8 * SLOT_LEN as u64 + 1024);
    {
        let mut ob: AppendOutbox<Msg, SharedMem> =
            AppendOutbox::new(mem.clone(), 0, SLOT_LEN, MAX_SLOTS);
        ob.open().unwrap();
        // 占满所有槽位。
        for i in 0..MAX_SLOTS {
            ob.enqueue(
                &Msg {
                    kind: 1,
                    seq: i as u32,
                    body: [1; 16],
                },
                key(i as u8),
            )
            .unwrap();
        }
        // ACK 全部，释放所有槽位。
        for i in 0..MAX_SLOTS {
            ob.mark_acked(
                OutboxId(i as u64),
                &PeerReceipt {
                    peer: "p".into(),
                    receipt_id: i as u64,
                    at_ms: 1,
                },
            )
            .unwrap();
        }
    }
    // 重启后，ACK 槽位不应占住容量。
    {
        let mut ob: AppendOutbox<Msg, SharedMem> =
            AppendOutbox::new(mem.clone(), 0, SLOT_LEN, MAX_SLOTS);
        ob.open().unwrap();
        assert!(ob.recover_pending().unwrap().is_empty());
        for i in 0..MAX_SLOTS {
            ob.enqueue(
                &Msg {
                    kind: 2,
                    seq: i as u32,
                    body: [2; 16],
                },
                key(100 + i as u8),
            )
            .unwrap();
        }
        // 再次 ACK 并重启后仍可继续复用。
        for i in 0..MAX_SLOTS {
            ob.mark_acked(
                OutboxId(MAX_SLOTS as u64 + i as u64),
                &PeerReceipt {
                    peer: "p".into(),
                    receipt_id: 200 + i as u64,
                    at_ms: 1,
                },
            )
            .unwrap();
        }
    }
    {
        let mut ob: AppendOutbox<Msg, SharedMem> = AppendOutbox::new(mem, 0, SLOT_LEN, MAX_SLOTS);
        ob.open().unwrap();
        for i in 0..MAX_SLOTS {
            ob.enqueue(
                &Msg {
                    kind: 3,
                    seq: i as u32,
                    body: [3; 16],
                },
                key(200 + i as u8),
            )
            .unwrap();
        }
    }
}

#[test]
fn outbox_power_loss_tear_drops_only_tail() {
    let mem = SharedMem::new(64 + 8 * SLOT_LEN as u64 + 1024);
    let mut ob: AppendOutbox<Msg, SharedMem> =
        AppendOutbox::new(mem.clone(), 0, SLOT_LEN, MAX_SLOTS);
    ob.open().unwrap();
    ob.enqueue(
        &Msg {
            kind: 1,
            seq: 1,
            body: [1; 16],
        },
        key(1),
    )
    .unwrap();
    ob.enqueue(
        &Msg {
            kind: 2,
            seq: 2,
            body: [2; 16],
        },
        key(2),
    )
    .unwrap();
    ob.mark_acked(
        OutboxId(0),
        &PeerReceipt {
            peer: "p".into(),
            receipt_id: 1,
            at_ms: 1,
        },
    )
    .unwrap();
    // 掉电撕裂：tail 之后槽位区后半段置 FF（模拟第二次入队写了一半掉电）。
    let tear = 64 + SLOT_LEN as u64 + 40;
    mem.tear(tear);
    // 重新打开：尾部槽位损坏被丢弃（视为未提交），已 ACK 的不恢复，
    // 未损坏的 pending 保留。
    let mut ob2: AppendOutbox<Msg, SharedMem> = AppendOutbox::new(mem, 0, SLOT_LEN, MAX_SLOTS);
    ob2.open().unwrap();
    let pending = ob2.recover_pending().unwrap();
    // id=1（第二条）可能因撕裂被丢弃；id=0 已 ACK 不得恢复。
    assert!(!pending.iter().any(|p| p.id == OutboxId(0)));
    assert!(pending.len() <= 1);
}

#[test]
fn outbox_full_rejects_and_idempotent() {
    let mut ob = new_outbox(MemIo::new(64 + 8 * SLOT_LEN as u64 + 1024));
    for i in 0..MAX_SLOTS {
        ob.enqueue(
            &Msg {
                kind: 1,
                seq: i as u32,
                body: [1; 16],
            },
            key(i as u8),
        )
        .unwrap();
    }
    // 满 → OutOfSpace（显式背压，不静默丢）。
    let err = ob
        .enqueue(
            &Msg {
                kind: 9,
                seq: 9,
                body: [9; 16],
            },
            key(9),
        )
        .unwrap_err();
    assert_eq!(err, StoreError::OutOfSpace);
    // ACK 释放槽位后新消息可入队（槽位复用；上限仍为 MAX_SLOTS）。
    ob.mark_acked(
        OutboxId(0),
        &PeerReceipt {
            peer: "p".into(),
            receipt_id: 1,
            at_ms: 1,
        },
    )
    .unwrap();
    let id9 = ob
        .enqueue(
            &Msg {
                kind: 9,
                seq: 9,
                body: [9; 16],
            },
            key(9),
        )
        .unwrap();
    assert_eq!(id9, OutboxId(8));
    // 再次填满后仍拒绝。
    let err = ob
        .enqueue(
            &Msg {
                kind: 10,
                seq: 10,
                body: [10; 16],
            },
            key(10),
        )
        .unwrap_err();
    assert_eq!(err, StoreError::OutOfSpace);
}

#[test]
fn outbox_write_failure_no_fake_success() {
    let mut mem = MemIo::new(64 + 8 * SLOT_LEN as u64 + 1024);
    mem.fail_writes = true;
    let mut ob = new_outbox(mem);
    let err = ob
        .enqueue(
            &Msg {
                kind: 1,
                seq: 1,
                body: [1; 16],
            },
            key(1),
        )
        .unwrap_err();
    assert_eq!(err, StoreError::WriteFailed);
    assert!(ob.recover_pending().unwrap().is_empty());
}

#[test]
fn outbox_corrupt_tail_still_recovers_committed_slots() {
    // 回归：tail 撕裂不等于“无副作用”——已完整写入的槽位必须仍能恢复，
    // 且 next_id 要按已见槽位推进，避免复用旧 OutboxId。
    let mem = SharedMem::new(64 + 8 * SLOT_LEN as u64 + 1024);
    {
        let mut ob: AppendOutbox<Msg, SharedMem> =
            AppendOutbox::new(mem.clone(), 0, SLOT_LEN, MAX_SLOTS);
        ob.open().unwrap();
        ob.enqueue(
            &Msg {
                kind: 1,
                seq: 1,
                body: [1; 16],
            },
            key(1),
        )
        .unwrap();
        ob.enqueue(
            &Msg {
                kind: 2,
                seq: 2,
                body: [2; 16],
            },
            key(2),
        )
        .unwrap();
    }
    // 只破坏 tail（64B），槽位区保持完整。
    {
        let mut b = mem.0.borrow_mut();
        for x in b.iter_mut().take(64) {
            *x = 0xFF;
        }
    }
    let mut ob: AppendOutbox<Msg, SharedMem> = AppendOutbox::new(mem, 0, SLOT_LEN, MAX_SLOTS);
    ob.open().unwrap();
    let pending = ob.recover_pending().unwrap();
    assert_eq!(pending.len(), 2, "tail 撕裂不应丢失已提交槽位");
    // 新入队 id 必须从已见最大 id+1 继续，不能复用 0/1。
    let id = ob
        .enqueue(
            &Msg {
                kind: 3,
                seq: 3,
                body: [3; 16],
            },
            key(3),
        )
        .unwrap();
    assert_eq!(id, OutboxId(2));
}

#[test]
fn outbox_corrupt_tail_starts_clean() {
    // 首写掉电（tail 撕裂）→ 从零开始，无副作用被错误恢复。
    let mem = SharedMem::new(64 + 8 * SLOT_LEN as u64 + 1024);
    mem.tear(0);
    let mut ob: AppendOutbox<Msg, SharedMem> = AppendOutbox::new(mem, 0, SLOT_LEN, MAX_SLOTS);
    ob.open().unwrap();
    assert!(ob.recover_pending().unwrap().is_empty());
    // 且可继续正常入队。
    ob.enqueue(
        &Msg {
            kind: 1,
            seq: 1,
            body: [1; 16],
        },
        key(1),
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// 内存实现（单元级）
// ---------------------------------------------------------------------------

#[test]
fn mem_state_store_cas() {
    let mut s = MemStateStore::<St>::new();
    let g1 = s
        .compare_and_swap(
            None,
            &St {
                value: 5,
                label: [0; 8],
            },
        )
        .unwrap();
    assert_eq!(g1, Generation(1));
    assert!(s
        .compare_and_swap(
            Some(Generation(1)),
            &St {
                value: 6,
                label: [0; 8]
            }
        )
        .is_ok());
    assert!(s
        .compare_and_swap(
            Some(Generation(1)),
            &St {
                value: 7,
                label: [0; 8]
            }
        )
        .is_err());
}

#[test]
fn mem_outbox_dedup_and_ack() {
    let mut ob = MemOutbox::<Msg>::default();
    let id = ob
        .enqueue(
            &Msg {
                kind: 1,
                seq: 1,
                body: [0; 16],
            },
            key(1),
        )
        .unwrap();
    let id2 = ob
        .enqueue(
            &Msg {
                kind: 1,
                seq: 1,
                body: [0; 16],
            },
            key(1),
        )
        .unwrap();
    assert_eq!(id, id2);
    ob.mark_acked(
        id,
        &PeerReceipt {
            peer: "p".into(),
            receipt_id: 1,
            at_ms: 1,
        },
    )
    .unwrap();
    assert!(ob.recover_pending().unwrap().is_empty());
}

#[test]
fn unavailable_outbox_fails_closed() {
    let mut ob = UnavailableOutbox::<Msg>::new();
    assert_eq!(
        ob.enqueue(
            &Msg {
                kind: 1,
                seq: 1,
                body: [0; 16]
            },
            key(1)
        ),
        Err(StoreError::Unavailable)
    );
    assert_eq!(ob.mark_sent(OutboxId(1), 1), Err(StoreError::Unavailable));
    assert_eq!(
        ob.mark_acked(
            OutboxId(1),
            &PeerReceipt {
                peer: "p".into(),
                receipt_id: 1,
                at_ms: 1
            }
        ),
        Err(StoreError::Unavailable)
    );
    assert_eq!(ob.recover_pending(), Err(StoreError::Unavailable));
}

#[test]
fn unavailable_state_store_fails_closed() {
    let mut s = UnavailableStateStore::<St>::new();
    assert_eq!(s.load(), Err(StoreError::Unavailable));
    assert_eq!(
        s.compare_and_swap(
            None,
            &St {
                value: 1,
                label: [0; 8]
            }
        ),
        Err(StoreError::Unavailable)
    );
}

#[test]
fn region_rejects_complete_future_schema_instead_of_loading_older_slot() {
    let mut media = MemIo::new(1024);
    let mut payload = 1u64.to_le_bytes().to_vec();
    payload.extend_from_slice(
        &St {
            value: 7,
            label: [0; 8],
        }
        .encode()
        .unwrap(),
    );
    let old = eneros_durable_state::encode_record(&eneros_durable_state::MAGIC_STATE, 1, &payload);
    media.bytes[..old.len()].copy_from_slice(&old);
    payload[..8].copy_from_slice(&2u64.to_le_bytes());
    let future =
        eneros_durable_state::encode_record(&eneros_durable_state::MAGIC_STATE, 2, &payload);
    media.bytes[512..512 + future.len()].copy_from_slice(&future);
    let mut store = RegionStateStore::<St, _>::new(media, 0, 1024);
    assert!(
        matches!(
            store.load(),
            Err(StoreError::UnsupportedVersion { schema: 2, .. })
        ),
        "valid newer schema must block rollback, not silently restore stale ownership"
    );
}

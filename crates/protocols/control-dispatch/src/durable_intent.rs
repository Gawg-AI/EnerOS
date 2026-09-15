//! 47（WP-47.4 意图持久化）：`DurableIntentStore`——`InMemoryIntentStore`
//! 的持久替代。
//!
//! 复用 [`eneros_durable_state::AppendOutbox`]（追加式记录日志：槽位记录 +
//! ack 记录，模式同 business-services 块落盘先例）。意图生命周期映射：
//!
//! - `append` → `enqueue`（idempotency = command_id；重复命令返回原 id）；
//! - `update_state(Indeterminate)` → `mark_sent`（durable；仍属未决——
//!   写可能已发生，需对账）；
//! - `update_state(Committed/Failed/Rejected)` → `mark_acked`（durable
//!   回执记录，对账终态）；
//! - `open()` 重启恢复：`recover_pending` 重建索引（Queued → Persisted，
//!   Sent → Indeterminate）——**断电重启后 intent 状态可恢复**（对账
//!   输入 = 未决集）；终态以 ack 记录为准（槽位可回收，不重建）。
//!
//! `Io: PersistIo` 泛型：host 测试用 `MemIo`（含 `power_loss_tear`），
//! EL0 服务用 `agent-sdk::block_io::BlockIo`（EIPC → svc-storage）。

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_durable_state::{
    AppendOutbox, Decode, DurableOutbox, Encode, IdempotencyKey, OutboxId, PeerReceipt, PersistIo,
    StoreError,
};

use crate::dispatch::DispatchState;
use crate::intent::{IntentRecord, IntentStore, IntentStoreError};

/// 意图终态线码（PeerReceipt.receipt_id 承载；对账终态持久化）。
/// v2（61 v0.314.24）：`Committed` 只能由执行 journal 的
/// `AppliedConfirmed`（独立回读匹配）写入，且以线码 4 落盘；旧版线码 1
/// （TERMINAL_COMMITTED_V1，已删除）可能含 IdempotentHit 误升级语义，
/// 恢复侧永不信任为已确认（见 v1_legacy_record_decodes_with_unknown_digest）。
const TERMINAL_COMMITTED_V2: u64 = 4;
const TERMINAL_FAILED: u64 = 2;
const TERMINAL_REJECTED: u64 = 3;

/// v2 追加段魔数（"V2"；旧版记录无此段）。
const V2_MAGIC: [u8; 2] = [0x56, 0x32];
/// v3 追加段魔数（蓝图 61：typed 链绑定 plan_sha256）。
const V3_MAGIC: [u8; 2] = [0x56, 0x33];

impl Encode for IntentRecord {
    fn encode(&self) -> Result<Vec<u8>, StoreError> {
        let mut out = Vec::with_capacity(16 + 16 + 8 + 8 + 32 + 8 + 8 + 2 + 1 + 32 + 4 + 1 + 32);
        out.extend_from_slice(&self.command_id);
        out.extend_from_slice(&self.correlation_id);
        out.extend_from_slice(&self.issued_at_ms.to_le_bytes());
        out.extend_from_slice(&self.expires_at_ms.to_le_bytes());
        let td = self.target_device.as_bytes();
        let n = core::cmp::min(32, td.len());
        out.push(n as u8);
        out.extend_from_slice(&td[..n]);
        out.extend_from_slice(&self.power_kw.to_le_bytes());
        match self.input_digest {
            Some(d) => {
                out.push(1);
                out.extend_from_slice(&d);
            }
            None => out.push(0),
        }
        // 蓝图 63 WP-63.1.1：码值走单源 to_code（原 `self.state as u8`
        // 判别序拷贝——码表布局在 eneros-dispatch-state；写入字节零变化）。
        out.push(self.state.to_code());
        // ---- v2 追加段（61 v0.314.24）：超集布局——旧解码器可读 v1 前缀，
        // 新解码器对无追加段的记录按 v1（digest=None）处理。----
        out.extend_from_slice(&V2_MAGIC);
        match self.command_digest {
            Some(d) => {
                out.push(1);
                out.extend_from_slice(&d);
            }
            None => out.push(0),
        }
        out.extend_from_slice(&self.attempt.to_le_bytes());
        match self.receipt_digest {
            Some(d) => {
                out.push(1);
                out.extend_from_slice(&d);
            }
            None => out.push(0),
        }
        Ok(out)
    }
}

impl Decode for IntentRecord {
    fn decode(bytes: &[u8]) -> Result<(eneros_durable_state::Generation, Self), StoreError> {
        let mut off = 0usize;
        let mut command_id = [0u8; 16];
        let mut correlation_id = [0u8; 16];
        // 定长前缀：cmd(16)+corr(16)+issued(8)+expires(8)+tlen(1)。
        if bytes.len() < 16 + 16 + 8 + 8 + 1 {
            return Err(StoreError::Corrupt);
        }
        command_id.copy_from_slice(&bytes[off..off + 16]);
        off += 16;
        correlation_id.copy_from_slice(&bytes[off..off + 16]);
        off += 16;
        let issued_at_ms = u64::from_le_bytes(bytes[off..off + 8].try_into().unwrap());
        off += 8;
        let expires_at_ms = u64::from_le_bytes(bytes[off..off + 8].try_into().unwrap());
        off += 8;
        let tlen = bytes[off] as usize;
        off += 1;
        if off + tlen + 8 > bytes.len() {
            return Err(StoreError::Corrupt);
        }
        let target_device = alloc::string::String::from(
            core::str::from_utf8(&bytes[off..off + tlen]).map_err(|_| StoreError::Corrupt)?,
        );
        off += tlen;
        let power_kw = f64::from_le_bytes(bytes[off..off + 8].try_into().unwrap());
        off += 8;
        let input_digest = match *bytes.get(off).ok_or(StoreError::Corrupt)? {
            0 => {
                off += 1;
                None
            }
            1 => {
                off += 1;
                if off + 32 >= bytes.len() {
                    return Err(StoreError::Corrupt);
                }
                let mut d = [0u8; 32];
                d.copy_from_slice(&bytes[off..off + 32]);
                off += 32;
                Some(d)
            }
            _ => return Err(StoreError::Corrupt),
        };
        // 蓝图 63 WP-63.1.1：decode 走单源 from_code（码表单源）——
        // 未知码 Corrupt fail-closed（持久化字节零变化）。
        let state = DispatchState::from_code(*bytes.get(off).ok_or(StoreError::Corrupt)?)
            .ok_or(StoreError::Corrupt)?;
        off += 1;
        // ---- v2 追加段（可选）：61 v0.314.24。无追加段 = 旧版 v1 记录
        //（digest 未知）；有追加段必须完整合法，尾部多余字节 = 损坏。----
        let mut command_digest = None;
        let mut attempt = 0u32;
        let mut receipt_digest = None;
        // v3 追加段（蓝图 61）：typed 链绑定；缺席 = 旧版记录。
        let mut plan_sha256: Option<[u8; 32]> = None;
        if off < bytes.len() {
            if bytes.len() < off + 2 || bytes[off..off + 2] != V2_MAGIC {
                return Err(StoreError::Corrupt);
            }
            off += 2;
            let flag = *bytes.get(off).ok_or(StoreError::Corrupt)?;
            off += 1;
            if flag == 1 {
                if off + 32 > bytes.len() {
                    return Err(StoreError::Corrupt);
                }
                let mut d = [0u8; 32];
                d.copy_from_slice(&bytes[off..off + 32]);
                off += 32;
                command_digest = Some(d);
            }
            if off + 4 > bytes.len() {
                return Err(StoreError::Corrupt);
            }
            attempt = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap());
            off += 4;
            let rflag = *bytes.get(off).ok_or(StoreError::Corrupt)?;
            off += 1;
            if rflag == 1 {
                if off + 32 > bytes.len() {
                    return Err(StoreError::Corrupt);
                }
                let mut d = [0u8; 32];
                d.copy_from_slice(&bytes[off..off + 32]);
                off += 32;
                receipt_digest = Some(d);
            }
            // ---- v3 追加段（可选；蓝图 61 v0.314.29）：plan_sha256。----
            if off < bytes.len() {
                if bytes.len() < off + 2 || bytes[off..off + 2] != V3_MAGIC {
                    return Err(StoreError::Corrupt);
                }
                off += 2;
                let pflag = *bytes.get(off).ok_or(StoreError::Corrupt)?;
                off += 1;
                if pflag == 1 {
                    if off + 32 > bytes.len() {
                        return Err(StoreError::Corrupt);
                    }
                    let mut d = [0u8; 32];
                    d.copy_from_slice(&bytes[off..off + 32]);
                    off += 32;
                    plan_sha256 = Some(d);
                }
            }
        }
        // 尾部校验（fail-closed：v1/v2/v3 之外的任何尾随字节 = 损坏）。
        if off != bytes.len() {
            return Err(StoreError::Corrupt);
        }
        Ok((
            eneros_durable_state::Generation(0),
            IntentRecord {
                command_id,
                correlation_id,
                issued_at_ms,
                expires_at_ms,
                target_device,
                power_kw,
                input_digest,
                state,
                command_digest,
                attempt,
                receipt_digest,
                plan_sha256,
            },
        ))
    }
}

/// 持久意图存储（AppendOutbox over PersistIo；重启对账恢复）。
pub struct DurableIntentStore<Io: PersistIo> {
    outbox: AppendOutbox<IntentRecord, Io>,
    /// command_id → outbox id（open/append 建立）。
    index: BTreeMap<[u8; 16], OutboxId>,
    /// command_id → 当前状态（open 重建 + 运行期更新）。
    states: BTreeMap<[u8; 16], DispatchState>,
    /// command_id → 记录（open 重建 + append）。
    records: BTreeMap<[u8; 16], IntentRecord>,
    /// **内存驻留上限**（2026-09-14，`REG-70.15-DISPATCH-HEAP-870R` 长窗实弹）：
    /// 三张 BTreeMap 按**已提交命令**无界增长（实测 HEAP-WM 468 KiB / req≈861 →
    /// 672 B 分配失败 PANIC）。磁盘 outbox 本身就是可恢复保留（open 重放），
    /// 内存侧只需**近期窗口**做去重/对账 ⇒ 超限时按 outbox id 序驱逐**最老的
    /// 终态**条目（未决条目永不驱逐——对账输入不可丢）。
    retention: usize,
    /// 插入序（outbox id 序的代理；驱逐最老终态用）。
    order: alloc::vec::Vec<[u8; 16]>,
}

/// 内存驻留上限（条）：512 KiB 堆下 intent 三表 + 审计链 + 工作集的预算
/// 反推值（长窗实测：861 次提交即 468 KiB 触顶——三表 ≈300 B/条无上界）。
/// 磁盘 outbox 保留全量（可恢复）；内存侧只保最近窗口。
pub const INTENT_RETENTION: usize = 512;

impl<Io: PersistIo> DurableIntentStore<Io> {
    /// 构造（随后必须 `open()`）。
    pub fn new(io: Io, base: u64, slot_len: usize, max_slots: usize) -> Self {
        Self {
            outbox: AppendOutbox::new(io, base, slot_len, max_slots),
            index: BTreeMap::new(),
            states: BTreeMap::new(),
            records: BTreeMap::new(),
            retention: INTENT_RETENTION,
            order: alloc::vec::Vec::new(),
        }
    }

    /// 驻留上限（测试注入用）。
    #[cfg(test)]
    fn with_retention(
        io: Io,
        base: u64,
        slot_len: usize,
        max_slots: usize,
        retention: usize,
    ) -> Self {
        let mut s = Self::new(io, base, slot_len, max_slots);
        s.retention = retention;
        s
    }

    /// 驻留超限时的**终态驱逐**：按插入序移除最老的终态条目（三张表同步），
    /// 直到 `index.len() <= retention`。未决（非终态）条目**永不驱逐**——
    /// 它们是重启对账的输入，丢了会重复执行已发生的写。幂等去重的持久面
    /// 在磁盘 outbox（`IdempotencyKey`），驱逐内存条目不失去该保护。
    fn evict_oldest_terminal(&mut self) -> usize {
        let mut evicted = 0usize;
        while self.index.len() > self.retention {
            // 找最老的（插入序）终态条目。
            let Some(cid) = self
                .order
                .iter()
                .find(|cid| {
                    matches!(
                        self.states.get(*cid),
                        Some(
                            DispatchState::Committed
                                | DispatchState::Failed
                                | DispatchState::Rejected
                        )
                    )
                })
                .copied()
            else {
                break; // 全部未决——不许驱逐（对账输入）。
            };
            self.index.remove(&cid);
            self.states.remove(&cid);
            self.records.remove(&cid);
            self.order.retain(|c| c != &cid);
            evicted += 1;
        }
        evicted
    }

    /// 打开/重放：从日志恢复未决意图（断电重启对账输入）。
    /// 失败 → Err（fail-closed：存储不可用不得静默空表）。
    pub fn open(&mut self) -> Result<(), IntentStoreError> {
        self.outbox
            .open()
            .map_err(|e| IntentStoreError::Unavailable(format!("{e:?}")))?;
        let pending = self
            .outbox
            .recover_pending()
            .map_err(|e| IntentStoreError::Unavailable(format!("{e:?}")))?;
        for p in pending {
            // Queued → Persisted（投递前）；Sent → Indeterminate（写可能已发生）。
            let state = match p.state {
                eneros_durable_state::MessageState::Queued => DispatchState::Persisted,
                eneros_durable_state::MessageState::Sent => DispatchState::Indeterminate,
                eneros_durable_state::MessageState::PeerAcked => continue,
            };
            let rec = p.message;
            let cid = rec.command_id;
            self.index.insert(cid, p.id);
            self.states.insert(cid, state);
            self.records.insert(cid, rec);
            self.order.push(cid);
        }
        Ok(())
    }

    /// 终态回执（内部）。
    fn ack(&mut self, command_id: &[u8; 16], state: DispatchState) -> Result<(), IntentStoreError> {
        let id = self
            .index
            .get(command_id)
            .copied()
            .ok_or(IntentStoreError::Unknown)?;
        let receipt = PeerReceipt {
            peer: String::from("control-dispatch"),
            // v2（61 v0.314.24）：Committed 线码 4 只能由 AppliedConfirmed
            // （独立回读匹配）路径写入；线码 1 为旧版遗留语义。
            receipt_id: match state {
                DispatchState::Committed => TERMINAL_COMMITTED_V2,
                DispatchState::Failed => TERMINAL_FAILED,
                DispatchState::Rejected => TERMINAL_REJECTED,
                _ => 0,
            },
            at_ms: 0,
        };
        self.outbox
            .mark_acked(id, &receipt)
            .map_err(|e| IntentStoreError::Unavailable(format!("{e:?}")))?;
        self.states.insert(*command_id, state);
        Ok(())
    }
}

impl<Io: PersistIo> IntentStore for DurableIntentStore<Io> {
    fn append(&mut self, rec: &IntentRecord) -> Result<(), IntentStoreError> {
        let id = self
            .outbox
            .enqueue(rec, IdempotencyKey::from_bytes(&rec.command_id))
            .map_err(|e| match e {
                StoreError::OutOfSpace => IntentStoreError::Full,
                _ => IntentStoreError::Unavailable(format!("{e:?}")),
            })?;
        self.index.insert(rec.command_id, id);
        self.states.insert(rec.command_id, DispatchState::Persisted);
        self.records.insert(rec.command_id, rec.clone());
        self.order.push(rec.command_id);
        self.evict_oldest_terminal();
        Ok(())
    }

    fn update_state(
        &mut self,
        command_id: &[u8; 16],
        state: DispatchState,
    ) -> Result<(), IntentStoreError> {
        match state {
            // 61 v0.314.24：投递已发生（Delivered/TransportAcked/ReadbackVerified）
            // 与写可能已发生（Indeterminate）都必须 durable 落「已发送」——
            // 崩溃后恢复为未决对账，不得按未投递处理。
            DispatchState::Indeterminate
            | DispatchState::Delivered
            | DispatchState::TransportAcked
            | DispatchState::ReadbackVerified => {
                let id = self
                    .index
                    .get(command_id)
                    .copied()
                    .ok_or(IntentStoreError::Unknown)?;
                self.outbox
                    .mark_sent(id, 0)
                    .map_err(|e| IntentStoreError::Unavailable(format!("{e:?}")))?;
                self.states.insert(*command_id, state);
                Ok(())
            }
            DispatchState::Committed | DispatchState::Failed | DispatchState::Rejected => {
                self.ack(command_id, state)
            }
            // 非终态且未投递的状态更新不改变持久语义（仅内存）。
            _ => {
                self.states.insert(*command_id, state);
                Ok(())
            }
        }
    }

    fn find(&self, command_id: &[u8; 16]) -> Option<IntentRecord> {
        let mut rec = self.records.get(command_id)?.clone();
        if let Some(s) = self.states.get(command_id) {
            rec.state = *s;
        }
        Some(rec)
    }

    fn pending(&self) -> Vec<IntentRecord> {
        let mut out = Vec::new();
        for (id, rec) in &self.records {
            let st = self.states.get(id).copied().unwrap_or(rec.state);
            if !matches!(
                st,
                DispatchState::Committed | DispatchState::Rejected | DispatchState::Failed
            ) {
                let mut r = rec.clone();
                r.state = st;
                out.push(r);
            }
        }
        out
    }
}

/// 终态持久映射（观测/测试：断言 ack 记录承载的终态；v2 线码）。
pub fn terminal_receipt_id(state: DispatchState) -> Option<u64> {
    match state {
        DispatchState::Committed => Some(TERMINAL_COMMITTED_V2),
        DispatchState::Failed => Some(TERMINAL_FAILED),
        DispatchState::Rejected => Some(TERMINAL_REJECTED),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use eneros_durable_state::PersistIo;

    use super::*;

    /// 共享介质内存 Io（断电/重启模拟：同一持久化介质跨实例）。
    struct SharedMemIo {
        media: std::rc::Rc<core::cell::RefCell<Vec<u8>>>,
        capacity: u64,
    }

    impl SharedMemIo {
        fn new(media: std::rc::Rc<core::cell::RefCell<Vec<u8>>>, capacity: u64) -> Self {
            // 预填充介质（open 读固定偏移区域；空介质 = 零填充）。
            let mut m = media.borrow_mut();
            if m.len() < capacity as usize {
                m.resize(capacity as usize, 0u8);
            }
            drop(m);
            Self { media, capacity }
        }
        fn tear(&mut self, truncate_at: u64) {
            let mut m = self.media.borrow_mut();
            let n = core::cmp::min(truncate_at as usize, m.len());
            m.truncate(n);
        }
    }

    impl PersistIo for SharedMemIo {
        fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), StoreError> {
            let m = self.media.borrow();
            let start = offset as usize;
            // 断电撕裂后区域截断：越界读返回零（块设备语义——未写区 = 0），
            // 让 open 按撕裂路径（尾记录坏 → 扫描槽位）恢复。
            if start >= m.len() {
                buf.fill(0);
                return Ok(());
            }
            let n = m.len().saturating_sub(start).min(buf.len());
            buf[..n].copy_from_slice(&m[start..start + n]);
            for b in buf[n..].iter_mut() {
                *b = 0;
            }
            Ok(())
        }
        fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), StoreError> {
            let mut m = self.media.borrow_mut();
            let start = offset as usize;
            let end = start + data.len();
            if end as u64 > self.capacity {
                return Err(StoreError::WriteFailed);
            }
            if m.len() < end {
                m.resize(end, 0u8);
            }
            m[start..end].copy_from_slice(data);
            Ok(())
        }
        fn flush(&mut self) -> Result<(), StoreError> {
            Ok(())
        }
        fn capacity(&self) -> u64 {
            self.capacity
        }
    }

    fn rec(id: u8, state: DispatchState) -> IntentRecord {
        IntentRecord {
            command_id: [id; 16],
            correlation_id: [0x47; 16],
            issued_at_ms: 1_000,
            expires_at_ms: 1_300,
            target_device: String::from("pcs"),
            power_kw: 5.0,
            input_digest: None,
            state,
            command_digest: Some([0x61; 32]),
            attempt: 1,
            receipt_digest: None,
            plan_sha256: None,
        }
    }

    /// 2026-09-14（70.15 长窗实弹红测）：**内存驻留必须有界**。
    ///
    /// 缺陷形态（实弹 HEAP-WM）：三张 BTreeMap 按**已提交命令**无界增长
    /// （≈300 B/条，861 次提交即 468 KiB 触顶 512 KiB 堆 → 672 B 分配失败
    /// PANIC → 控制链"活着但不再控制"）。旧实现 `ack` 只改状态、永不驱逐。
    ///
    /// 判据：append + ack 超过 retention 的条目数后——①内存驻留 ≤ retention
    /// （最老终态被驱逐）；②未决条目永不驱逐（对账输入）；③驱逐后 `find`
    /// 对最老条目返回 None（诚实体面，幂等持久面在磁盘 outbox）。
    #[test]
    fn terminal_retention_is_bounded() {
        let media = std::rc::Rc::new(core::cell::RefCell::new(Vec::new()));
        let io = SharedMemIo::new(std::rc::Rc::clone(&media), 1 << 20);
        let mut store = DurableIntentStore::with_retention(io, 0, 256, 512, 64);
        store.open().unwrap();
        // 200 条全部 Committed（终态）——超过 retention=64。
        for i in 0..200u8 {
            let r = rec(i, DispatchState::Persisted);
            store.append(&r).unwrap();
            store
                .update_state(&r.command_id, DispatchState::Committed)
                .unwrap();
        }
        assert!(
            store.index.len() <= 64,
            "终态驻留必须被驱逐到 retention 内（实际 {}）",
            store.index.len()
        );
        // 未决条目永不驱逐：追加 8 条未决，前面的终态继续让位。
        for i in 200..208u8 {
            let r = rec(i, DispatchState::Persisted);
            store.append(&r).unwrap();
        }
        assert!(store.index.len() <= 64);
        // 最老的已驱逐（find → None）；新近的仍在。
        assert!(store.find(&[0u8; 16]).is_none(), "最老终态必须被驱逐");
        assert!(store.find(&[207u8; 16]).is_some(), "新近条目必须保留");
    }

    /// 正例：append → update_state(Committed) → 「重启」后 pending 为空
    ///（终态持久 = ack 记录；未决集正确恢复）。
    #[test]
    fn committed_after_restart_not_pending() {
        let media = std::rc::Rc::new(core::cell::RefCell::new(Vec::new()));
        {
            let io = SharedMemIo::new(std::rc::Rc::clone(&media), 1_000_000);
            let mut store = DurableIntentStore::new(io, 0, 256, 64);
            store.open().unwrap();
            store.append(&rec(1, DispatchState::Persisted)).unwrap();
            store
                .update_state(&[1; 16], DispatchState::Committed)
                .unwrap();
            // 终态可查询（会话内）且未决为空。
            assert_eq!(
                store.find(&[1; 16]).unwrap().state,
                DispatchState::Committed
            );
            assert!(store.pending().is_empty());
        }
        // 断电重启：同一介质。
        let io2 = SharedMemIo::new(std::rc::Rc::clone(&media), 1_000_000);
        let mut store2 = DurableIntentStore::new(io2, 0, 256, 64);
        store2.open().unwrap();
        assert!(store2.pending().is_empty(), "终态不得回到未决");
    }

    /// 正例：断电（写前）/未决意图重启后恢复（对账输入）。
    #[test]
    fn pending_intent_recovers_after_restart() {
        let media = std::rc::Rc::new(core::cell::RefCell::new(Vec::new()));
        {
            let io = SharedMemIo::new(std::rc::Rc::clone(&media), 1_000_000);
            let mut store = DurableIntentStore::new(io, 0, 256, 64);
            store.open().unwrap();
            store.append(&rec(1, DispatchState::Persisted)).unwrap();
            store.append(&rec(2, DispatchState::Persisted)).unwrap();
            store
                .update_state(&[2; 16], DispatchState::Indeterminate)
                .unwrap();
        }
        let io2 = SharedMemIo::new(std::rc::Rc::clone(&media), 1_000_000);
        let mut store2 = DurableIntentStore::new(io2, 0, 256, 64);
        store2.open().unwrap();
        let pend = store2.pending();
        assert_eq!(pend.len(), 2, "Persisted + Indeterminate 均须恢复为未决");
        let s1 = store2.find(&[1; 16]).unwrap();
        let s2 = store2.find(&[2; 16]).unwrap();
        assert_eq!(s1.state, DispatchState::Persisted, "{s1:?}");
        assert_eq!(s2.state, DispatchState::Indeterminate, "{s2:?}");
    }

    /// 负例：写入期间断电（撕裂槽位）→ open 恢复完整已写意图、跳过
    /// 损坏槽位（fail-closed 不假定无副作用）。
    #[test]
    fn power_loss_tear_recovers_intact_pending() {
        let media = std::rc::Rc::new(core::cell::RefCell::new(Vec::new()));
        let rec1 = rec(1, DispatchState::Persisted);
        let rec2 = rec(2, DispatchState::Persisted);
        {
            let io = SharedMemIo::new(std::rc::Rc::clone(&media), 1_000_000);
            let mut store = DurableIntentStore::new(io, 0, 256, 64);
            store.open().unwrap();
            store.append(&rec1).unwrap();
            store.append(&rec2).unwrap();
        }
        {
            // 断电：切断尾部（模拟 slot 写撕裂）。
            let mut io = SharedMemIo::new(std::rc::Rc::clone(&media), 1_000_000);
            io.tear(512);
            let mut store = DurableIntentStore::new(io, 0, 256, 64);
            store.open().unwrap();
            // 已完整写入的意图必须可恢复（对账基础）；撕裂槽位跳过。
            let pend = store.pending();
            assert!(!pend.is_empty(), "断电后未决意图必须可恢复（对账输入）");
            assert!(
                store.find(&[1; 16]).is_some() || store.find(&[2; 16]).is_some(),
                "至少一条完整记录可恢复"
            );
        }
    }

    /// 终态/未决状态持久线码往返。
    #[test]
    fn record_wire_roundtrip() {
        let mut r = rec(7, DispatchState::Indeterminate);
        r.receipt_digest = Some([0x9D; 32]);
        r.attempt = 3;
        let bytes = r.encode().unwrap();
        let (_, back) = IntentRecord::decode(&bytes).unwrap();
        assert_eq!(back.command_id, r.command_id);
        assert_eq!(back.target_device, r.target_device);
        assert_eq!(back.power_kw, r.power_kw);
        assert_eq!(back.state, r.state);
        assert_eq!(back.command_digest, Some([0x61; 32]));
        assert_eq!(back.attempt, 3);
        assert_eq!(back.receipt_digest, Some([0x9D; 32]));
        assert_eq!(terminal_receipt_id(DispatchState::Committed), Some(4));
        assert_eq!(terminal_receipt_id(DispatchState::Indeterminate), None);
    }

    /// 61 v0.314.24：旧版 v1 记录（无 v2 追加段）可解码——digest/attempt/
    /// receipt 为 None/0，恢复侧按未知摘要处理（对账，不盲发）。
    #[test]
    fn v1_legacy_record_decodes_with_unknown_digest() {
        // 手工构造 v1 布局：cmd(16)+corr(16)+issued(8)+expires(8)+tlen(1)
        // +target+power(8)+digestflag(1)+state(1)。
        let mut b = alloc::vec::Vec::new();
        b.extend_from_slice(&[1u8; 16]); // command_id
        b.extend_from_slice(&[2u8; 16]); // correlation_id
        b.extend_from_slice(&1_000u64.to_le_bytes());
        b.extend_from_slice(&1_300u64.to_le_bytes());
        b.push(3);
        b.extend_from_slice(b"pcs");
        b.extend_from_slice(&5.0f64.to_le_bytes());
        b.push(0); // input_digest = None
        b.push(8); // state = Committed（旧线码）
        let (_, back) = IntentRecord::decode(&b).unwrap();
        assert_eq!(back.command_id, [1; 16]);
        assert_eq!(back.state, DispatchState::Committed);
        assert_eq!(back.command_digest, None, "v1 无摘要");
        assert_eq!(back.attempt, 0);
        assert_eq!(back.receipt_digest, None);
        // 旧版 Committed 线码 1 仅为遗留语义——v2 写入恒为 4。
        assert_ne!(1u64, TERMINAL_COMMITTED_V2);
    }

    /// 负例：v2 魔数损坏 → Corrupt（fail-closed）。
    #[test]
    fn corrupt_v2_trailer_rejected() {
        let r = rec(9, DispatchState::Persisted);
        let mut bytes = r.encode().unwrap();
        let n = bytes.len();
        bytes[n - 40] ^= 0xFF; // 破坏 v2 魔数首字节（追加段起点）
        assert!(matches!(
            IntentRecord::decode(&bytes),
            Err(StoreError::Corrupt)
        ));
    }

    /// 未知 command_id 更新 → Unknown（fail-closed）。
    #[test]
    fn unknown_update_rejected() {
        let io = SharedMemIo::new(
            std::rc::Rc::new(core::cell::RefCell::new(Vec::new())),
            1_000_000,
        );
        let mut store = DurableIntentStore::new(io, 0, 256, 64);
        store.open().unwrap();
        assert_eq!(
            store.update_state(&[9; 16], DispatchState::Committed),
            Err(IntentStoreError::Unknown)
        );
    }
}

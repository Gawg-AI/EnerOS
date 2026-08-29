//! EnerOS DurableStateStore / DurableOutbox（蓝图 41 WP-41.3）——
//! 跨重启副作用唯一模式（35-01 §6.2 深接口）。
//!
//! 语义（蓝图 §3）：
//! - 所有“先改内存再发网络”的业务迁移到 outbox/state transaction 模式；
//! - 序列化含 schema、generation、CRC/hash、record length；掉电只允许
//!   恢复到上一个或下一个**完整** generation（撕裂记录必须可检测并拒绝）；
//! - 通用成功语义：`Queued`（只进 durable outbox）/ `Sent`（网络发送完成）/
//!   `PeerAcked`（对端按协议确认）/ `Reconciled`（本地与对端一致）——
//!   只有领域定义要求的终态才返回业务成功；
//! - 闭包返回 Ok、Vec push、计数增加不再是 “real”。
//!
//! 实现分层：
//! - [`PersistIo`]：极简持久化 seam（read/write/flush + 容量），target 侧
//!   由存储句柄提供（svc-storage 未授予时显式 Unavailable，fail-closed）；
//! - [`RegionStateStore`]：固定区域快照（generation + CRC），
//!   compare_and_swap 语义；
//! - [`AppendOutbox`]：追加式记录日志（槽位记录 + ack 记录），
//!   掉电只丢未完成尾部记录；
//! - [`MemIo`]（测试）：内存后端 + 掉电模拟（截断/撕裂字节）。

#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(not(test), allow(dead_code))]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;
use core::marker::PhantomData;

// ---------------------------------------------------------------------------
// 基础类型
// ---------------------------------------------------------------------------

/// 状态代次（单调；掉电恢复只接受 ≥ 已提交代次）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Generation(pub u64);

/// 版本化状态快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Versioned<S> {
    pub generation: Generation,
    pub state: S,
}

/// 幂等键（调用方保证同键同语义；重放去重）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IdempotencyKey(pub [u8; 16]);

impl IdempotencyKey {
    pub fn from_bytes(bytes: &[u8; 16]) -> Self {
        IdempotencyKey(*bytes)
    }
    pub fn is_zero(&self) -> bool {
        self.0.iter().all(|b| *b == 0)
    }
}

/// Outbox 消息 id（追加序单调）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct OutboxId(pub u64);

/// 对端回执（mark_acked 输入；`receipt_id` 为对端协议回执标识）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerReceipt {
    pub peer: alloc::string::String,
    pub receipt_id: u64,
    pub at_ms: u64,
}

/// 待恢复的 pending 消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending<M> {
    pub id: OutboxId,
    pub idempotency: IdempotencyKey,
    pub message: M,
    /// 已发送次数（Sent 后未 ACK 重试计数）。
    pub attempts: u32,
    /// 最近一次发送时间（重试退避依据）。
    pub last_sent_at_ms: u64,
    /// 消息状态（Queued/Sent/PeerAcked）。
    pub state: MessageState,
}

/// 消息状态（Queued/Sent/PeerAcked 分层成功语义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageState {
    Queued,
    Sent,
    PeerAcked,
}

/// 显式不可用的 [`DurableOutbox`]——存储能力未授予时使用，绝不退回内存
/// 假队列。任何操作返回 [`StoreError::Unavailable`]（fail-closed）。
#[derive(Debug, Clone, Default)]
pub struct UnavailableOutbox<M>(PhantomData<M>);

impl<M> UnavailableOutbox<M> {
    pub fn new() -> Self {
        Self(PhantomData)
    }
}

impl<M> DurableOutbox<M> for UnavailableOutbox<M> {
    fn enqueue(&mut self, _msg: &M, _idempotency: IdempotencyKey) -> Result<OutboxId, StoreError> {
        Err(StoreError::Unavailable)
    }
    fn mark_acked(&mut self, _id: OutboxId, _peer_receipt: &PeerReceipt) -> Result<(), StoreError> {
        Err(StoreError::Unavailable)
    }
    fn mark_sent(&mut self, _id: OutboxId, _at_ms: u64) -> Result<(), StoreError> {
        Err(StoreError::Unavailable)
    }
    fn recover_pending(&mut self) -> Result<Vec<Pending<M>>, StoreError> {
        Err(StoreError::Unavailable)
    }
}

/// 显式不可用的 [`DurableStateStore`]——存储能力未授予时使用，绝不退回
/// 内存假状态。任何操作返回 [`StoreError::Unavailable`]（fail-closed）。
#[derive(Debug, Clone, Default)]
pub struct UnavailableStateStore<S>(PhantomData<S>);

impl<S> UnavailableStateStore<S> {
    pub fn new() -> Self {
        Self(PhantomData)
    }
}

impl<S> DurableStateStore<S> for UnavailableStateStore<S> {
    fn load(&mut self) -> Result<Option<Versioned<S>>, StoreError> {
        Err(StoreError::Unavailable)
    }
    fn compare_and_swap(
        &mut self,
        _expected: Option<Generation>,
        _next: &S,
    ) -> Result<Generation, StoreError> {
        Err(StoreError::Unavailable)
    }
}

/// 存储错误——fail-closed：任何 IO 错误不得转为成功。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// 后端不可用（存储 capability 未授予/掉线）。
    Unavailable,
    /// 写入失败。
    WriteFailed,
    /// 记录损坏（CRC/长度/schema 不符——撕裂检测）。
    Corrupt,
    /// 代次倒挂（旧 generation 覆盖新值被拒）。
    StaleGeneration {
        current: Generation,
        attempted: Generation,
    },
    /// 编码/解码失败。
    Encode(&'static str),
    /// 越界/容量不足。
    OutOfSpace,
    /// 其他。
    Other(&'static str),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Unavailable => f.write_str("store-unavailable"),
            StoreError::WriteFailed => f.write_str("write-failed"),
            StoreError::Corrupt => f.write_str("record-corrupt"),
            StoreError::StaleGeneration { current, attempted } => write!(
                f,
                "stale-generation:current={} attempted={}",
                current.0, attempted.0
            ),
            StoreError::Encode(m) => write!(f, "encode:{m}"),
            StoreError::OutOfSpace => f.write_str("out-of-space"),
            StoreError::Other(m) => write!(f, "other:{m}"),
        }
    }
}

// ---------------------------------------------------------------------------
// PersistIo：持久化 seam
// ---------------------------------------------------------------------------

/// 持久化字节 I/O seam。**不**要求随机访问整介质；区域式/追加式后端均可。
/// 掉电语义由实现保证：`write` 之后 `flush` 成功才算持久；撕裂由
/// 上层记录格式（CRC + length + magic）检测。
pub trait PersistIo {
    /// 读取 `offset` 起 `buf.len()` 字节；越界/不可用 → Err。
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), StoreError>;
    /// 写入 `offset` 起 `data`；越界/不可用 → Err（不得部分成功伪装）。
    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), StoreError>;
    /// 冲刷（持久化屏障）；不可用 → Err。
    fn flush(&mut self) -> Result<(), StoreError>;
    /// 容量（字节）；0 = 未知/不可用。
    fn capacity(&self) -> u64;
}

/// blanket：`&mut T` 也是 PersistIo（借用持有者时复用同一后端——41
/// 落盘路径：RegionStateStore 持 `&mut BlockIo`，每次命令重建 store）。
impl<T: PersistIo + ?Sized> PersistIo for &mut T {
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), StoreError> {
        (**self).read(offset, buf)
    }
    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), StoreError> {
        (**self).write(offset, data)
    }
    fn flush(&mut self) -> Result<(), StoreError> {
        (**self).flush()
    }
    fn capacity(&self) -> u64 {
        (**self).capacity()
    }
}

/// 内存后端（host 测试/内存模式）。`fail_writes` 注入写失败；
/// `truncate_at` 模拟掉电撕裂（截断点之后的字节变 0xFF）。
#[derive(Debug, Clone)]
pub struct MemIo {
    pub bytes: Vec<u8>,
    pub fail_writes: bool,
    pub flush_ok: bool,
    pub capacity: u64,
}

impl MemIo {
    pub fn new(capacity: u64) -> Self {
        MemIo {
            bytes: vec![0u8; capacity as usize],
            fail_writes: false,
            flush_ok: true,
            capacity,
        }
    }

    /// 模拟掉电：`truncate_at` 之后字节置 0xFF（撕裂）。
    pub fn power_loss_tear(&mut self, truncate_at: u64) {
        let n = self.bytes.len();
        for i in (truncate_at as usize)..n {
            self.bytes[i] = 0xFF;
        }
    }
}

impl PersistIo for MemIo {
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), StoreError> {
        let start = offset as usize;
        if start + buf.len() > self.bytes.len() {
            return Err(StoreError::OutOfSpace);
        }
        buf.copy_from_slice(&self.bytes[start..start + buf.len()]);
        Ok(())
    }
    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), StoreError> {
        if self.fail_writes {
            return Err(StoreError::WriteFailed);
        }
        let start = offset as usize;
        if start + data.len() > self.bytes.len() {
            return Err(StoreError::OutOfSpace);
        }
        self.bytes[start..start + data.len()].copy_from_slice(data);
        Ok(())
    }
    fn flush(&mut self) -> Result<(), StoreError> {
        if !self.flush_ok {
            return Err(StoreError::WriteFailed);
        }
        Ok(())
    }
    fn capacity(&self) -> u64 {
        self.capacity
    }
}

// ---------------------------------------------------------------------------
// 记录格式基元（schema + generation + length + CRC32）
// ---------------------------------------------------------------------------

/// 记录 magic：状态快照 / outbox 槽位 / ack 记录 / 尾指针。
pub const MAGIC_STATE: [u8; 4] = *b"DSTA";
pub const MAGIC_SLOT: [u8; 4] = *b"DOBX";
pub const MAGIC_ACK: [u8; 4] = *b"DOBA";
pub const MAGIC_TAIL: [u8; 4] = *b"DOBT";
pub const RECORD_SCHEMA: u8 = 1;

// HYG-001（SPEC-B/C 2026-08-28 收口 10/10）：CRC32 唯一实现 = abi/ipc-wire
//（IEEE 802.3 0xEDB88320；本模块曾经的本地实现已删，调用点直引单源）。

/// 组装带长度 + CRC 的记录（magic‖schema‖len(4)‖payload‖crc32(4)）。
pub fn encode_record(magic: &[u8; 4], schema: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(13 + payload.len());
    out.extend_from_slice(magic);
    out.push(schema);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    let crc = eneros_ipc_wire::crc32(&out);
    out.extend_from_slice(&crc.to_le_bytes());
    out
}

/// 校验并解码记录：返回 (magic, payload)。损坏/长度越界/未知 magic → Corrupt。
pub fn decode_record(bytes: &[u8]) -> Result<([u8; 4], &[u8]), StoreError> {
    if bytes.len() < 13 {
        return Err(StoreError::Corrupt);
    }
    let magic = [bytes[0], bytes[1], bytes[2], bytes[3]];
    let schema = bytes[4];
    if schema != RECORD_SCHEMA {
        return Err(StoreError::Corrupt);
    }
    let len = u32::from_le_bytes([bytes[5], bytes[6], bytes[7], bytes[8]]) as usize;
    if len + 13 > bytes.len() {
        return Err(StoreError::Corrupt);
    }
    let payload = &bytes[9..9 + len];
    let expected = u32::from_le_bytes([
        bytes[9 + len],
        bytes[10 + len],
        bytes[11 + len],
        bytes[12 + len],
    ]);
    if eneros_ipc_wire::crc32(&bytes[..9 + len]) != expected {
        return Err(StoreError::Corrupt);
    }
    Ok((magic, payload))
}

// ---------------------------------------------------------------------------
// DurableStateStore
// ---------------------------------------------------------------------------

/// 跨重启状态存储（蓝图 §3 trait）。
pub trait DurableStateStore<S> {
    /// 加载当前状态；无记录 → Ok(None)（首次）。
    fn load(&mut self) -> Result<Option<Versioned<S>>, StoreError>;
    /// 原子 CAS：仅当 `expected == 当前代次` 时写入 `next`（新代次 =
    /// expected+1），否则 StaleGeneration。`expected = None` 表示首次写入。
    fn compare_and_swap(
        &mut self,
        expected: Option<Generation>,
        next: &S,
    ) -> Result<Generation, StoreError>;
}

/// 固定区域状态快照（RegionIo 语义：读/写/冲刷）。
///
/// E 续作订正（win4 实弹，2026-08-28）：单槽原地覆写在硬杀落进写窗口时
/// 撕裂唯一记录 ⇒ `Corrupt` ⇒ 消费方（svc-ai-worker 学习器）fail-closed
/// 关闭——违背本模块头"掉电恢复到上一个**完整** generation"承诺（盘上
/// 实证：payload 逐字段完好、CRC 撕裂）。改**双槽代次切换**：region 对半
/// 分槽，写入恒落非最新槽；`open` 双槽扫描取 CRC 合法且代次最大的记录，
/// 撕裂槽天然被忽略（另一槽保持上一完整代次）；双槽皆坏且区域非空 ⇒
/// `Corrupt`（fail-closed，不伪造恢复）。
pub struct RegionStateStore<S, Io: PersistIo> {
    io: Io,
    region_offset: u64,
    region_len: u64,
    current: Option<Versioned<S>>,
    /// 当前生效记录所在槽（0/1；None = 区域从未写入）。
    latest_slot: Option<usize>,
    _marker: core::marker::PhantomData<S>,
}

impl<S, Io: PersistIo> RegionStateStore<S, Io> {
    pub fn new(io: Io, region_offset: u64, region_len: u64) -> Self {
        RegionStateStore {
            io,
            region_offset,
            region_len,
            current: None,
            latest_slot: None,
            _marker: core::marker::PhantomData,
        }
    }

    /// 首次打开：双槽扫描取合法且代次最大的快照（撕裂槽忽略；双槽皆坏
    /// 且区域非空 → Corrupt；区域从未写入 → Ok(()) 且 current=None）。
    pub fn open(&mut self) -> Result<(), StoreError>
    where
        S: Encode + Decode,
    {
        if self.region_len < 128 {
            return Err(StoreError::Other("region-too-small"));
        }
        let slot_len = (self.region_len / 2) as usize;
        let mut buf = vec![0u8; self.region_len as usize];
        self.io
            .read(self.region_offset, &mut buf)
            .map_err(|_| StoreError::Unavailable)?;
        let mut best: Option<(Generation, S, usize)> = None;
        let mut any_bytes = false;
        for slot in 0..2usize {
            let s = &buf[slot * slot_len..(slot + 1) * slot_len];
            if s.iter().all(|&b| b == 0) || s.iter().all(|&b| b == 0xFF) {
                continue; // 从未写入的槽
            }
            any_bytes = true;
            // 撕裂/坏 CRC/坏 payload 的槽 → 忽略（另一槽保有上一完整代次）。
            let payload = match decode_record(s) {
                Ok((magic, payload)) if magic == MAGIC_STATE => payload.to_vec(),
                _ => continue,
            };
            let (generation, state) = match S::decode(&payload) {
                Ok(x) => x,
                Err(_) => continue,
            };
            if best
                .as_ref()
                .map(|(g, _, _)| generation.0 > g.0)
                .unwrap_or(true)
            {
                best = Some((generation, state, slot));
            }
        }
        match best {
            Some((generation, state, slot)) => {
                self.current = Some(Versioned { generation, state });
                self.latest_slot = Some(slot);
            }
            None if any_bytes => return Err(StoreError::Corrupt),
            None => self.current = None,
        }
        Ok(())
    }

    fn write_snapshot(&mut self, generation: Generation, state: &S) -> Result<(), StoreError>
    where
        S: Encode,
    {
        let slot_len = (self.region_len / 2) as usize;
        // 写入恒落**非最新槽**——硬杀撕裂本槽时，另一槽保有上一完整代次。
        let slot = match self.latest_slot {
            Some(l) => 1 - l,
            None => 0,
        };
        let mut payload = Vec::new();
        payload.extend_from_slice(&generation.0.to_le_bytes());
        payload.extend_from_slice(&state.encode()?);
        let record = encode_record(&MAGIC_STATE, RECORD_SCHEMA, &payload);
        if record.len() > slot_len {
            return Err(StoreError::OutOfSpace);
        }
        let off = self.region_offset + (slot as u64) * slot_len as u64;
        self.io
            .write(off, &record)
            .map_err(|_| StoreError::WriteFailed)?;
        self.io.flush().map_err(|_| StoreError::WriteFailed)?;
        self.latest_slot = Some(slot);
        Ok(())
    }
}

impl<S: Encode + Decode + Clone, Io: PersistIo> DurableStateStore<S> for RegionStateStore<S, Io> {
    fn load(&mut self) -> Result<Option<Versioned<S>>, StoreError> {
        if self.current.is_none() {
            self.open()?;
        }
        Ok(self.current.clone())
    }

    fn compare_and_swap(
        &mut self,
        expected: Option<Generation>,
        next: &S,
    ) -> Result<Generation, StoreError> {
        let current_gen = self
            .current
            .as_ref()
            .map(|v| v.generation)
            .unwrap_or(Generation(0));
        let expected_gen = expected.unwrap_or(Generation(0));
        if expected_gen != current_gen {
            return Err(StoreError::StaleGeneration {
                current: current_gen,
                attempted: expected_gen,
            });
        }
        let next_gen = Generation(current_gen.0 + 1);
        self.write_snapshot(next_gen, next)?;
        self.current = Some(Versioned {
            generation: next_gen,
            state: next.clone(),
        });
        Ok(next_gen)
    }
}

// ---------------------------------------------------------------------------
// DurableOutbox
// ---------------------------------------------------------------------------

/// 跨重启 outbox（蓝图 §3 trait）。
pub trait DurableOutbox<M> {
    /// 入队（持久化成功后返回 OutboxId）。重复 `idempotency` 返回同一 id
    /// （幂等去重——不产生重复副作用）。
    fn enqueue(&mut self, msg: &M, idempotency: IdempotencyKey) -> Result<OutboxId, StoreError>;
    /// 标记对端 ACK（幂等；未知 id 允许 no-op——ACK 可能重复到达）。
    fn mark_acked(&mut self, id: OutboxId, peer_receipt: &PeerReceipt) -> Result<(), StoreError>;
    /// 标记已发送（Sent 分层；更新 attempts/时间）。
    fn mark_sent(&mut self, id: OutboxId, at_ms: u64) -> Result<(), StoreError>;
    /// 重启后恢复 pending（未 ACK 消息，按入队序）。
    fn recover_pending(&mut self) -> Result<Vec<Pending<M>>, StoreError>;
}

/// 追加式 outbox 记录布局（每槽位定长）：
/// `[tail(64B)][slot_0][slot_1]...`，tail = MAGIC_TAIL‖schema‖len‖
/// next_id(8)‖head_index(4)‖slot_count(4)‖crc。
/// 每个 slot = MAGIC_SLOT‖schema‖len‖payload‖crc（payload = id‖idempotency‖
/// attempts‖last_sent‖state‖msg）。
pub struct AppendOutbox<M, Io: PersistIo> {
    io: Io,
    base: u64,
    slot_len: usize,
    max_slots: usize,
    next_id: u64,
    /// 活跃槽位索引（复用表：已 ACK 槽位可回收）。
    slots: BTreeMap<OutboxId, u32>,
    pending: BTreeMap<OutboxId, Pending<M>>,
    _marker: core::marker::PhantomData<M>,
}

const TAIL_LEN: u64 = 64;

impl<M: Encode + Decode + Clone, Io: PersistIo> AppendOutbox<M, Io> {
    pub fn new(io: Io, base: u64, slot_len: usize, max_slots: usize) -> Self {
        AppendOutbox {
            io,
            base,
            slot_len: slot_len.max(64),
            max_slots: max_slots.max(1),
            next_id: 0,
            slots: BTreeMap::new(),
            pending: BTreeMap::new(),
            _marker: core::marker::PhantomData,
        }
    }

    fn tail_offset(&self) -> u64 {
        self.base
    }
    fn slot_offset(&self, index: u32) -> u64 {
        self.base + TAIL_LEN + (index as u64) * (self.slot_len as u64)
    }

    /// 打开：读 tail（撕裂 → next_id 从 0 开始并继续扫描槽位，不假定
    /// 无副作用）；扫描全部槽位恢复 pending（损坏槽位 = 未提交，跳过；
    /// PeerAcked 槽位不恢复）。
    pub fn open(&mut self) -> Result<(), StoreError> {
        let mut buf = [0u8; 64];
        self.io
            .read(self.tail_offset(), &mut buf)
            .map_err(|_| StoreError::Unavailable)?;
        let tail = match decode_record(&buf) {
            Ok((magic, payload)) => {
                if magic != MAGIC_TAIL {
                    return Err(StoreError::Corrupt);
                }
                Some(payload)
            }
            Err(StoreError::Corrupt) => {
                // 尾记录撕裂：不能假定无副作用，仍继续扫描槽位，恢复
                // 已完整写入的 pending；next_id 由已见槽位推进。
                None
            }
            Err(e) => return Err(e),
        };
        if let Some(record) = tail {
            if record.len() < 8 {
                return Err(StoreError::Corrupt);
            }
            self.next_id = u64::from_le_bytes(record[0..8].try_into().unwrap());
        } else {
            self.next_id = 0;
        }
        // 全槽扫描（固定范围，槽位可复用）。
        let mut max_seen_id = None;
        for i in 0..self.max_slots {
            let mut slot = vec![0u8; self.slot_len];
            let off = self.slot_offset(i as u32);
            self.io
                .read(off, &mut slot)
                .map_err(|_| StoreError::Unavailable)?;
            let record = match decode_record(&slot) {
                Ok((magic, payload)) => {
                    if magic != MAGIC_SLOT {
                        continue; // 空槽
                    }
                    payload
                }
                Err(StoreError::Corrupt) => continue, // 撕裂槽位 → 视为未提交
                Err(e) => return Err(e),
            };
            if let Ok(Some(p)) = decode_slot_payload::<M>(record) {
                max_seen_id = Some(max_seen_id.map_or(p.id.0, |m: u64| m.max(p.id.0)));
                // PeerAcked 槽位在正常运行时已从 `slots` 释放，可被复用；
                // 重启后也必须保持可复用，不能把已 ACK 槽位重新占住。
                if p.state == MessageState::PeerAcked {
                    continue;
                }
                self.slots.insert(p.id, i as u32);
                self.pending.insert(p.id, p);
            }
        }
        // 若 tail 因掉电/写失败落后于已写入槽位，按已见最大 id 推进，
        // 防止新消息复用旧 OutboxId（旧 ACK 串到新消息）。
        if let Some(max_id) = max_seen_id {
            if max_id >= self.next_id {
                self.next_id = max_id + 1;
            }
        }
        Ok(())
    }

    fn write_tail(&mut self) -> Result<(), StoreError> {
        let mut payload = Vec::with_capacity(8);
        payload.extend_from_slice(&self.next_id.to_le_bytes());
        let record = encode_record(&MAGIC_TAIL, RECORD_SCHEMA, &payload);
        let mut buf = [0u8; 64];
        buf[..record.len()].copy_from_slice(&record);
        self.io
            .write(self.tail_offset(), &buf)
            .map_err(|_| StoreError::WriteFailed)?;
        self.io.flush().map_err(|_| StoreError::WriteFailed)
    }

    /// 分配一个空闲槽位（未被活跃消息占用）。
    fn alloc_slot(&self) -> Result<u32, StoreError> {
        for i in 0..self.max_slots as u32 {
            if !self.slots.values().any(|v| *v == i) {
                return Ok(i);
            }
        }
        Err(StoreError::OutOfSpace)
    }

    fn write_slot(&mut self, index: u32, p: &Pending<M>) -> Result<(), StoreError> {
        let payload = encode_slot_payload(p)?;
        let record = encode_record(&MAGIC_SLOT, RECORD_SCHEMA, &payload);
        if record.len() > self.slot_len {
            return Err(StoreError::OutOfSpace);
        }
        let mut buf = vec![0u8; self.slot_len];
        buf[..record.len()].copy_from_slice(&record);
        self.io
            .write(self.slot_offset(index), &buf)
            .map_err(|_| StoreError::WriteFailed)?;
        self.io.flush().map_err(|_| StoreError::WriteFailed)
    }
}

impl<M: Encode + Decode + Clone, Io: PersistIo> DurableOutbox<M> for AppendOutbox<M, Io> {
    fn enqueue(&mut self, msg: &M, idempotency: IdempotencyKey) -> Result<OutboxId, StoreError> {
        // 幂等：同键已存在 → 返回原 id。
        for (id, p) in &self.pending {
            if p.idempotency == idempotency {
                return Ok(*id);
            }
        }
        if self.slots.len() >= self.max_slots {
            return Err(StoreError::OutOfSpace);
        }
        let id = OutboxId(self.next_id);
        let pending = Pending {
            id,
            idempotency,
            message: msg.clone(),
            attempts: 0,
            last_sent_at_ms: 0,
            state: MessageState::Queued,
        };
        let slot_index = self.alloc_slot()?;
        self.write_slot(slot_index, &pending)?;
        self.slots.insert(id, slot_index);
        self.pending.insert(id, pending);
        self.next_id += 1;
        self.write_tail()?;
        Ok(id)
    }

    fn mark_sent(&mut self, id: OutboxId, at_ms: u64) -> Result<(), StoreError> {
        let index = *self
            .slots
            .get(&id)
            .ok_or(StoreError::Other("unknown-outbox-id"))?;
        let p = self
            .pending
            .get_mut(&id)
            .ok_or(StoreError::Other("unknown-outbox-id"))?;
        p.attempts += 1;
        p.last_sent_at_ms = at_ms;
        p.state = MessageState::Sent;
        let snapshot = p.clone();
        self.write_slot(index, &snapshot)?;
        Ok(())
    }

    fn mark_acked(&mut self, id: OutboxId, peer_receipt: &PeerReceipt) -> Result<(), StoreError> {
        let Some(mut p) = self.pending.remove(&id) else {
            return Ok(()); // 重复 ACK / 未知 id：幂等 no-op
        };
        let _ = peer_receipt;
        p.state = MessageState::PeerAcked;
        let index = self.slots[&id];
        // 槽位标记 acked（保留审计直至槽位复用；恢复时跳过）。
        self.write_slot(index, &p)?;
        self.slots.remove(&id);
        self.write_tail()?;
        Ok(())
    }

    fn recover_pending(&mut self) -> Result<Vec<Pending<M>>, StoreError> {
        Ok(self.pending.values().cloned().collect())
    }
}

// ---------------------------------------------------------------------------
// Encode/Decode
// ---------------------------------------------------------------------------

/// 可编码（记录 payload）。
pub trait Encode {
    fn encode(&self) -> Result<Vec<u8>, StoreError>;
}

/// 可解码。
pub trait Decode: Sized {
    fn decode(bytes: &[u8]) -> Result<(Generation, Self), StoreError>;
}

fn encode_slot_payload<M: Encode>(p: &Pending<M>) -> Result<Vec<u8>, StoreError> {
    let mut out = Vec::new();
    out.extend_from_slice(&p.id.0.to_le_bytes());
    out.extend_from_slice(&p.idempotency.0);
    out.extend_from_slice(&p.attempts.to_le_bytes());
    out.extend_from_slice(&p.last_sent_at_ms.to_le_bytes());
    out.push(p.state as u8);
    out.extend_from_slice(&p.message.encode()?);
    Ok(out)
}

fn decode_slot_payload<M: Decode>(bytes: &[u8]) -> Result<Option<Pending<M>>, StoreError> {
    if bytes.len() < 8 + 16 + 4 + 8 + 1 {
        return Err(StoreError::Corrupt);
    }
    let id = OutboxId(u64::from_le_bytes(bytes[0..8].try_into().unwrap()));
    let mut key = [0u8; 16];
    key.copy_from_slice(&bytes[8..24]);
    let attempts = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
    let last_sent = u64::from_le_bytes(bytes[28..36].try_into().unwrap());
    let state = match bytes[36] {
        0 => MessageState::Queued,
        1 => MessageState::Sent,
        2 => MessageState::PeerAcked,
        _ => return Err(StoreError::Corrupt),
    };
    let (gen, message) = M::decode(&bytes[37..])?;
    let _ = gen;
    Ok(Some(Pending {
        id,
        idempotency: IdempotencyKey(key),
        attempts,
        last_sent_at_ms: last_sent,
        message,
        state,
    }))
}

// ---------------------------------------------------------------------------
// 内存实现（host 测试 / 显式非持久模式）
// ---------------------------------------------------------------------------

/// 内存状态存储（测试）。
pub struct MemStateStore<S> {
    current: Option<Versioned<S>>,
}

impl<S> MemStateStore<S> {
    pub fn new() -> Self {
        MemStateStore { current: None }
    }
}

impl<S> Default for MemStateStore<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: Clone> DurableStateStore<S> for MemStateStore<S> {
    fn load(&mut self) -> Result<Option<Versioned<S>>, StoreError> {
        Ok(self.current.clone())
    }
    fn compare_and_swap(
        &mut self,
        expected: Option<Generation>,
        next: &S,
    ) -> Result<Generation, StoreError> {
        let current_gen = self
            .current
            .as_ref()
            .map(|v| v.generation)
            .unwrap_or(Generation(0));
        if expected.unwrap_or(Generation(0)) != current_gen {
            return Err(StoreError::StaleGeneration {
                current: current_gen,
                attempted: expected.unwrap_or(Generation(0)),
            });
        }
        let next_gen = Generation(current_gen.0 + 1);
        self.current = Some(Versioned {
            generation: next_gen,
            state: next.clone(),
        });
        Ok(next_gen)
    }
}

/// 内存 outbox（测试）。
pub struct MemOutbox<M> {
    pending: BTreeMap<OutboxId, Pending<M>>,
    next_id: u64,
}

impl<M> Default for MemOutbox<M> {
    fn default() -> Self {
        MemOutbox {
            pending: BTreeMap::new(),
            next_id: 0,
        }
    }
}

impl<M: Clone> DurableOutbox<M> for MemOutbox<M> {
    fn enqueue(&mut self, msg: &M, idempotency: IdempotencyKey) -> Result<OutboxId, StoreError> {
        for (id, p) in &self.pending {
            if p.idempotency == idempotency {
                return Ok(*id);
            }
        }
        let id = OutboxId(self.next_id);
        self.next_id += 1;
        self.pending.insert(
            id,
            Pending {
                id,
                idempotency,
                message: msg.clone(),
                attempts: 0,
                last_sent_at_ms: 0,
                state: MessageState::Queued,
            },
        );
        Ok(id)
    }
    fn mark_acked(&mut self, id: OutboxId, _peer_receipt: &PeerReceipt) -> Result<(), StoreError> {
        self.pending.remove(&id);
        Ok(())
    }
    fn mark_sent(&mut self, id: OutboxId, at_ms: u64) -> Result<(), StoreError> {
        let p = self
            .pending
            .get_mut(&id)
            .ok_or(StoreError::Other("unknown-outbox-id"))?;
        p.attempts += 1;
        p.last_sent_at_ms = at_ms;
        Ok(())
    }
    fn recover_pending(&mut self) -> Result<Vec<Pending<M>>, StoreError> {
        Ok(self.pending.values().cloned().collect())
    }
}

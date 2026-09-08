//! 审计存储抽象与内存实现.
//!
//! [`AuditStore`] 定义链上条目的持久化接口（追加/范围读/尾部读/计数），
//! 上层日志器与校验器仅依赖该 trait；[`MemStore`] 为易失性内存实现
//! （`Vec` backed），供单元测试与无盘场景使用（偏差 D4 新增）。

use alloc::vec::Vec;

use crate::events::ChainEntry;
use crate::AuditError;

/// 审计存储抽象（追加只增不改，读取按序号过滤）.
pub trait AuditStore {
    /// 追加一条链上条目（序号严格递增由调用方保证）.
    fn append(&mut self, entry: &ChainEntry) -> Result<(), AuditError>;

    /// 读取序号区间 `[start_seq, end_seq]`（含两端点）内的条目，按序号升序.
    fn read_range(&self, start_seq: u64, end_seq: u64) -> Result<Vec<ChainEntry>, AuditError>;

    /// 读取链尾最近 `n` 条，按序号升序；`n` 超过存量时返回全部.
    fn read_last_n(&self, n: usize) -> Result<Vec<ChainEntry>, AuditError>;

    /// 当前条目总数.
    fn count(&self) -> Result<u64, AuditError>;
}

/// 蓝图 70.1 WP-70.1.2：字节预算化的内存存储。
///
/// 与 [`MemStore`]（无界 `Vec`，扩容翻倍）不同，本实现**构造期一次性预分配**
/// 固定容量，此后 `append` 零分配、永不扩容：
///
/// - 预算以字节声明（`entries × size_of::<ChainEntry>() ≤ budget`），
///   数量上限不再独立于进程堆配额存在（全局契约 I4：容量上限必须换算为
///   实际字节并映射到每进程预算）；
/// - 链满时丢弃新记录并计数，返回 `AuditError::BufferFull`——满载是
///   可观测的正常拒绝，绝不通过 OOM 表达；
/// - 已落盘条目按追加序连续保留（丢新不丢旧），链条在容量窗内始终
///   完整可校验（`verify()` 语义与 MemStore 一致）。
///
/// 69.5 窗 OOM 归因（WP-70.1.1）：`MemStore` 的 `Vec` 在第 513 条追加时
/// 触发 512→1024 容量翻倍，申请 `1024 × size_of::<ChainEntry>()` =
/// 114 688 B 连续内存失败（evidence/69/vol69.5/qemu-serial.log:203189，
/// svc-control-dispatch PANIC）。预分配消除该扩容峰值。
#[derive(Debug, Clone)]
pub struct BoundedMemStore {
    /// 已追加条目（构造期预分配到容量，此后永不重分配）.
    entries: Vec<ChainEntry>,
    /// 因预算到顶被拒绝的追加次数.
    dropped: u64,
}

impl BoundedMemStore {
    /// 按条目容量创建：构造期一次预分配 `cap × size_of::<ChainEntry>()` 字节.
    ///
    /// `cap == 0` 合法（全拒存储，预算为 0 的显式表达）.
    pub fn with_capacity_entries(cap: usize) -> Self {
        BoundedMemStore {
            entries: Vec::with_capacity(cap),
            dropped: 0,
        }
    }

    /// 按字节预算创建：`cap = budget / size_of::<ChainEntry>()`（向下取整）.
    ///
    /// `budget < size_of::<ChainEntry>()` → 容量 0（全部拒绝）.
    pub fn with_byte_budget(budget_bytes: usize) -> Self {
        let entry = core::mem::size_of::<ChainEntry>();
        Self::with_capacity_entries(budget_bytes / entry)
    }

    /// 条目容量（构造期确定，恒定不变）.
    pub fn capacity_entries(&self) -> usize {
        self.entries.capacity()
    }

    /// 字节预算占用峰值 = `capacity_entries() × size_of::<ChainEntry>()`.
    pub fn byte_budget(&self) -> usize {
        self.entries.capacity() * core::mem::size_of::<ChainEntry>()
    }

    /// 因预算到顶被拒绝的追加次数.
    pub fn dropped_count(&self) -> u64 {
        self.dropped
    }

    /// 当前条目数（usize 视图，供测试断言）.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否为空存储.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl AuditStore for BoundedMemStore {
    /// 追加条目：预算到顶丢弃新记录并计数（不分配、不扩容、链状态不变）.
    fn append(&mut self, entry: &ChainEntry) -> Result<(), AuditError> {
        if self.entries.len() >= self.entries.capacity() {
            self.dropped += 1;
            return Err(AuditError::BufferFull);
        }
        self.entries.push(entry.clone());
        Ok(())
    }

    /// 范围读取：按 `seq ∈ [start_seq, end_seq]` 过滤，保持升序.
    fn read_range(&self, start_seq: u64, end_seq: u64) -> Result<Vec<ChainEntry>, AuditError> {
        let out = self
            .entries
            .iter()
            .filter(|e| e.seq >= start_seq && e.seq <= end_seq)
            .cloned()
            .collect();
        Ok(out)
    }

    /// 尾部读取：取最后 `n` 条；`n == 0` 返回空，`n` 超存量返回全部.
    fn read_last_n(&self, n: usize) -> Result<Vec<ChainEntry>, AuditError> {
        let total = self.entries.len();
        let skip = total.saturating_sub(n);
        let out = self.entries.iter().skip(skip).cloned().collect();
        Ok(out)
    }

    /// 条目总数.
    fn count(&self) -> Result<u64, AuditError> {
        Ok(self.entries.len() as u64)
    }
}

/// 内存存储（易失；条目按追加序保序，等价序号升序）.
#[derive(Debug, Clone, Default)]
pub struct MemStore {
    /// 已追加条目（私有，仅经 trait 方法访问）.
    entries: Vec<ChainEntry>,
}

impl MemStore {
    /// 创建空内存存储.
    pub fn new() -> Self {
        MemStore {
            entries: Vec::new(),
        }
    }

    /// 当前条目数（usize 视图，供测试断言）.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否为空存储.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl AuditStore for MemStore {
    /// 追加条目到内存尾部（克隆存入）.
    fn append(&mut self, entry: &ChainEntry) -> Result<(), AuditError> {
        self.entries.push(entry.clone());
        Ok(())
    }

    /// 范围读取：按 `seq ∈ [start_seq, end_seq]` 过滤，保持升序.
    fn read_range(&self, start_seq: u64, end_seq: u64) -> Result<Vec<ChainEntry>, AuditError> {
        let out = self
            .entries
            .iter()
            .filter(|e| e.seq >= start_seq && e.seq <= end_seq)
            .cloned()
            .collect();
        Ok(out)
    }

    /// 尾部读取：取最后 `n` 条；`n == 0` 返回空，`n` 超存量返回全部.
    fn read_last_n(&self, n: usize) -> Result<Vec<ChainEntry>, AuditError> {
        let total = self.entries.len();
        let skip = total.saturating_sub(n);
        let out = self.entries.iter().skip(skip).cloned().collect();
        Ok(out)
    }

    /// 条目总数.
    fn count(&self) -> Result<u64, AuditError> {
        Ok(self.entries.len() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造测试条目：仅 `seq` 有意义，哈希字段填充派生值便于区分.
    fn make_entry(seq: u64) -> ChainEntry {
        ChainEntry {
            seq,
            prev_hash: [seq as u8; 32],
            event_hash: [(seq as u8).wrapping_add(1); 32],
            combined_hash: [(seq as u8).wrapping_add(2); 32],
            timestamp_ms: seq * 1000,
        }
    }

    /// STO3：追加 3 条后 count == 3，read_range(1, 3) 含端点返回 3 条.
    #[test]
    fn sto3_append_then_count_and_range_inclusive() {
        let mut store = MemStore::new();
        for seq in 1..=3u64 {
            assert!(store.append(&make_entry(seq)).is_ok());
        }
        assert_eq!(store.count(), Ok(3));
        let got = store.read_range(1, 3).expect("范围读取应成功");
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].seq, 1);
        assert_eq!(got[2].seq, 3);
    }

    /// STO4：read_range(2, 2) 单点区间返回恰好 1 条（边界）.
    #[test]
    fn sto4_read_range_single_point_boundary() {
        let mut store = MemStore::new();
        for seq in 1..=3u64 {
            let _ = store.append(&make_entry(seq));
        }
        let got = store.read_range(2, 2).expect("单点区间读取应成功");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].seq, 2);
    }

    /// STO5：read_last_n(10) 超存量返回全部 3 条；read_last_n(2) 返回末 2 条且升序.
    #[test]
    fn sto5_read_last_n_overflow_and_order() {
        let mut store = MemStore::new();
        for seq in 1..=3u64 {
            let _ = store.append(&make_entry(seq));
        }
        // n 超存量 → 返回全部
        let all = store.read_last_n(10).expect("尾部读取应成功");
        assert_eq!(all.len(), 3);
        // n 小于存量 → 返回最后 n 条且 seq 升序
        let last2 = store.read_last_n(2).expect("尾部读取应成功");
        assert_eq!(last2.len(), 2);
        assert_eq!(last2[0].seq, 2);
        assert_eq!(last2[1].seq, 3);
        assert!(last2[0].seq < last2[1].seq);
    }

    /// 蓝图 70.1 WP-70.1.1 归因锚：`size_of::<ChainEntry>() == 112` 且
    /// `1024 × 112 == 114 688`——与 69.5 窗 svc-control-dispatch OOM 申请
    /// 字节数逐位一致（evidence/69/vol69.5/qemu-serial.log:203189），
    /// 对应 `MemStore` Vec 第 512→1024 容量翻倍的扩容申请。
    /// 字段增删导致条目尺寸漂移时本断言红，归因数字必须随源码重核。
    #[test]
    fn bnd0_chain_entry_size_matches_oom_attribution() {
        assert_eq!(core::mem::size_of::<ChainEntry>(), 112);
        assert_eq!(1024 * core::mem::size_of::<ChainEntry>(), 114_688);
    }

    /// 蓝图 70.1 WP-70.1.2：字节预算构造——容量向下取整且
    /// `capacity × size_of ≤ budget`；预算不足一条 → 容量 0.
    #[test]
    fn bnd1_byte_budget_floors_and_zero_budget_rejects_all() {
        let s = BoundedMemStore::with_byte_budget(64 * 1024);
        assert_eq!(s.capacity_entries(), 64 * 1024 / 112);
        assert!(s.byte_budget() <= 64 * 1024);
        assert!(s.is_empty());

        let mut zero = BoundedMemStore::with_byte_budget(64);
        assert_eq!(zero.capacity_entries(), 0);
        assert!(zero.append(&make_entry(1)).is_err());
        assert_eq!(zero.dropped_count(), 1);
        assert_eq!(zero.len(), 0);
    }

    /// 蓝图 70.1 WP-70.1.2：预分配不变量——容量在构造期锁定，追加到顶
    /// 及到顶后拒绝均不改变 capacity（永不重分配 → 扩容峰值 = 0），
    /// 满��拒绝返回 `BufferFull` 且链内既有条目完整可读.
    #[test]
    fn bnd2_capacity_locked_no_realloc_full_rejects_observably() {
        let mut s = BoundedMemStore::with_capacity_entries(4);
        let cap0 = s.capacity_entries();
        assert_eq!(cap0, 4);
        for seq in 1..=4u64 {
            s.append(&make_entry(seq)).expect("预算内追加应成功");
        }
        assert_eq!(s.len(), 4);
        assert_eq!(s.capacity_entries(), cap0, "到顶时容量不得变化");
        // 到顶后拒绝：BufferFull + 计数，既有条目不受影响
        for _ in 0..3 {
            assert!(matches!(
                s.append(&make_entry(9)),
                Err(AuditError::BufferFull)
            ));
        }
        assert_eq!(s.dropped_count(), 3);
        assert_eq!(s.capacity_entries(), cap0, "拒绝路径不得触发扩容");
        assert_eq!(s.count(), Ok(4));
        let range = s.read_range(1, 4).expect("读取应成功");
        assert_eq!(range.len(), 4);
        assert_eq!(range[0].seq, 1);
        assert_eq!(range[3].seq, 4);
    }

    /// 蓝图 70.1 WP-70.1.2：与链日志器组合——预算到顶时 `log` 返回
    /// `BufferFull` 且链头状态不推进（失败不前进，后续恢复语义正确）.
    #[test]
    fn bnd3_logger_at_capacity_fails_without_advancing_chain() {
        use crate::chain::{AuditLogger, AuditLoggerImpl, ChainVerifier, ChainVerifierImpl};
        use crate::events::{AuditEvent, AuditEventType};
        let store = BoundedMemStore::with_capacity_entries(2);
        let mut logger = AuditLoggerImpl::new(store, [0x5A; 32]);
        for i in 0..2u64 {
            let mut ev = AuditEvent::new(AuditEventType::AiDecision, "a", "t", "d");
            ev.timestamp_ms = 1000 + i;
            logger.log(ev).expect("预算内入链应成功");
        }
        let seq_before = logger.chain_state().last_seq;
        let count_before = logger.chain_state().entry_count;
        let ev = AuditEvent::new(AuditEventType::AiDecision, "a", "t", "d");
        assert!(matches!(logger.log(ev), Err(AuditError::BufferFull)));
        assert_eq!(
            logger.chain_state().last_seq,
            seq_before,
            "失败不得推进链头"
        );
        assert_eq!(logger.chain_state().entry_count, count_before);
        // 全链可校验：容量窗内链条完整
        let store = logger.store();
        let entries = store.read_range(1, 2).expect("读取应成功");
        let report = ChainVerifierImpl
            .verify_chain(&entries)
            .expect("校验应成功");
        assert!(report.valid);
        assert_eq!(report.total, 2);
    }
}

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
}

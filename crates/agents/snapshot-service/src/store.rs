//! 快照目录（[08] §3.2）：按 snapshot_id 查元数据与 BlobRef/偏移范围；
//! 重放确定性（同输入同 root）由 builder 保证，本目录只做索引与校验。

use alloc::collections::BTreeMap;
use alloc::string::String;

/// 快照元数据（blob 偏移范围 = 持久化介质上的段；None = 纯内存）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotMeta {
    pub snapshot_id: [u8; 16],
    pub provenance_root: [u8; 32],
    pub purpose_id: u32,
    pub produced_at_ns: u64,
    pub cut_time_ns: u64,
    /// 版本组（重放必须同版本组 + 同 catalog bundle）。
    pub version_generation: u64,
    pub offset_begin: u64,
    pub offset_end: u64,
}

/// 快照目录（有界；容量满 → 拒绝登记并计数——不静默丢）。
pub struct SnapshotCatalog {
    by_id: BTreeMap<[u8; 16], SnapshotMeta>,
    cap: usize,
    /// 满后拒绝登记数（观测）。
    pub rejected: u64,
}

impl Default for SnapshotCatalog {
    fn default() -> Self {
        Self {
            by_id: BTreeMap::new(),
            cap: 4096,
            rejected: 0,
        }
    }
}

impl SnapshotCatalog {
    pub fn new(cap: usize) -> Self {
        Self {
            by_id: BTreeMap::new(),
            cap: cap.max(1),
            rejected: 0,
        }
    }

    /// 登记（同 id 覆盖——确定性重放产生同 id 同元数据）。
    pub fn register(&mut self, meta: SnapshotMeta) -> bool {
        if self.by_id.contains_key(&meta.snapshot_id) {
            // 重复 id：元数据必须一致（重放确定性校验）。
            let prev = &self.by_id[&meta.snapshot_id];
            return prev == &meta;
        }
        if self.by_id.len() >= self.cap {
            self.rejected += 1;
            return false;
        }
        self.by_id.insert(meta.snapshot_id, meta);
        true
    }

    pub fn meta(&self, snapshot_id: &[u8; 16]) -> Option<&SnapshotMeta> {
        self.by_id.get(snapshot_id)
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

/// 持久化错误（blob 写入面 trait 语义简化：二元成败）。
#[allow(clippy::result_unit_err)]
pub trait SnapshotBlobStore {
    fn write(
        &mut self,
        snapshot_id: &[u8; 16],
        payload: &[u8],
        meta: &SnapshotMeta,
    ) -> Result<(), String>;
    fn read(&self, snapshot_id: &[u8; 16]) -> Option<alloc::vec::Vec<u8>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_replay_consistency() {
        let mut c = SnapshotCatalog::new(4);
        let meta = SnapshotMeta {
            snapshot_id: [1u8; 16],
            provenance_root: [2u8; 32],
            purpose_id: 1,
            produced_at_ns: 100,
            cut_time_ns: 99,
            version_generation: 1,
            offset_begin: 0,
            offset_end: 789,
        };
        assert!(c.register(meta.clone()));
        // 重放：同 id 同元数据 → 一致（true）；不同元数据 → false。
        assert!(c.register(meta.clone()));
        let mut drifted = meta.clone();
        drifted.offset_end = 790;
        assert!(!c.register(drifted));
        assert_eq!(c.meta(&[1u8; 16]).unwrap().offset_end, 789);
    }

    #[test]
    fn bounded_and_counted() {
        let mut c = SnapshotCatalog::new(2);
        for i in 0u8..4 {
            let mut id = [0u8; 16];
            id[0] = i;
            c.register(SnapshotMeta {
                snapshot_id: id,
                provenance_root: [0u8; 32],
                purpose_id: 1,
                produced_at_ns: 0,
                cut_time_ns: 0,
                version_generation: 1,
                offset_begin: 0,
                offset_end: 0,
            });
        }
        assert_eq!(c.len(), 2);
        assert_eq!(c.rejected, 2);
    }
}

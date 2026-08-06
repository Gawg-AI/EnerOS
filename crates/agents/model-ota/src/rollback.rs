//! 回滚保护与快照管理 (workspace v0.192.0).
//!
//! 更新前创建快照，任何阶段失败自动回滚。

use alloc::string::String;
use alloc::vec::Vec;

use crate::OtaError;

/// 快照 ID 类型.
pub type SnapshotId = u64;

// ============================================================
// Snapshot
// ============================================================

/// 模型快照.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// 快照 ID
    pub id: SnapshotId,
    /// 版本号
    pub version: String,
    /// 模型数据
    pub data: Vec<u8>,
    /// 创建时间
    pub created_at: u64,
}

// ============================================================
// RollbackProtection Trait
// ============================================================

/// 回滚保护接口.
pub trait RollbackProtection {
    /// 创建快照.
    fn create_snapshot(
        &mut self,
        version: &str,
        data: &[u8],
        now: u64,
    ) -> Result<SnapshotId, OtaError>;

    /// 回滚到快照.
    fn rollback(&mut self, snapshot_id: SnapshotId) -> Result<Vec<u8>, OtaError>;

    /// 清理旧快照（保留最近 keep 个）.
    fn cleanup_snapshots(&mut self, keep: usize) -> Result<(), OtaError>;

    /// 获取快照列表.
    fn list_snapshots(&self) -> &[Snapshot];

    /// 快照数量.
    fn snapshot_count(&self) -> usize;
}

// ============================================================
// SnapshotManager
// ============================================================

/// 快照管理器.
///
/// 内存快照存储（生产环境应持久化到文件系统）。
pub struct SnapshotManager {
    /// 快照列表
    snapshots: Vec<Snapshot>,
    /// 下一个快照 ID
    next_id: SnapshotId,
    /// 最大快照数
    max_snapshots: usize,
}

impl SnapshotManager {
    /// 创建快照管理器.
    pub fn new(max_snapshots: usize) -> Self {
        Self {
            snapshots: Vec::new(),
            next_id: 1,
            max_snapshots,
        }
    }

    /// 获取最新快照.
    pub fn latest(&self) -> Option<&Snapshot> {
        self.snapshots.last()
    }

    /// 按 ID 查找快照.
    pub fn get(&self, id: SnapshotId) -> Option<&Snapshot> {
        self.snapshots.iter().find(|s| s.id == id)
    }
}

impl RollbackProtection for SnapshotManager {
    fn create_snapshot(
        &mut self,
        version: &str,
        data: &[u8],
        now: u64,
    ) -> Result<SnapshotId, OtaError> {
        let id = self.next_id;
        self.next_id += 1;

        self.snapshots.push(Snapshot {
            id,
            version: String::from(version),
            data: data.to_vec(),
            created_at: now,
        });

        // 自动清理超出限制的快照
        if self.snapshots.len() > self.max_snapshots {
            let excess = self.snapshots.len() - self.max_snapshots;
            self.snapshots.drain(..excess);
        }

        Ok(id)
    }

    fn rollback(&mut self, snapshot_id: SnapshotId) -> Result<Vec<u8>, OtaError> {
        let snapshot = self
            .snapshots
            .iter()
            .find(|s| s.id == snapshot_id)
            .ok_or(OtaError::NoPreviousVersion)?;
        Ok(snapshot.data.clone())
    }

    fn cleanup_snapshots(&mut self, keep: usize) -> Result<(), OtaError> {
        if self.snapshots.len() > keep {
            let excess = self.snapshots.len() - keep;
            self.snapshots.drain(..excess);
        }
        Ok(())
    }

    fn list_snapshots(&self) -> &[Snapshot] {
        &self.snapshots
    }

    fn snapshot_count(&self) -> usize {
        self.snapshots.len()
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_snapshot() {
        let mut mgr = SnapshotManager::new(5);
        let id = mgr.create_snapshot("1.0.0", b"model data", 1000).unwrap();
        assert_eq!(id, 1);
        assert_eq!(mgr.snapshot_count(), 1);
        assert_eq!(mgr.latest().unwrap().version, "1.0.0");
    }

    #[test]
    fn test_rollback() {
        let mut mgr = SnapshotManager::new(5);
        let id1 = mgr.create_snapshot("1.0.0", b"v1 data", 1000).unwrap();
        let _id2 = mgr.create_snapshot("2.0.0", b"v2 data", 2000).unwrap();

        let data = mgr.rollback(id1).unwrap();
        assert_eq!(data, b"v1 data");
    }

    #[test]
    fn test_rollback_not_found() {
        let mut mgr = SnapshotManager::new(5);
        let result = mgr.rollback(999);
        assert_eq!(result, Err(OtaError::NoPreviousVersion));
    }

    #[test]
    fn test_cleanup_snapshots() {
        let mut mgr = SnapshotManager::new(10);
        for i in 0..5 {
            mgr.create_snapshot("v", b"data", i).unwrap();
        }
        assert_eq!(mgr.snapshot_count(), 5);

        mgr.cleanup_snapshots(2).unwrap();
        assert_eq!(mgr.snapshot_count(), 2);
    }

    #[test]
    fn test_max_snapshots_auto_cleanup() {
        let mut mgr = SnapshotManager::new(3);
        for i in 0..5 {
            mgr.create_snapshot("v", b"data", i).unwrap();
        }
        // 最多保留 3 个
        assert_eq!(mgr.snapshot_count(), 3);
        // 最早的快照已被清理
        assert!(mgr.get(1).is_none());
        assert!(mgr.get(2).is_none());
        assert!(mgr.get(3).is_some());
    }

    #[test]
    fn test_list_snapshots() {
        let mut mgr = SnapshotManager::new(5);
        mgr.create_snapshot("1.0", b"a", 100).unwrap();
        mgr.create_snapshot("2.0", b"b", 200).unwrap();

        let list = mgr.list_snapshots();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].version, "1.0");
        assert_eq!(list[1].version, "2.0");
    }
}

//! OTA 管理器 (v0.161.0).
//!
//! 编排差分更新全流程：快照 → 差分应用 → 验证 → 提交/回滚。

use alloc::string::String;
use alloc::vec::Vec;

use eneros_crypto::sm3;

use crate::diff::{DiffPatch, DiffUpdater};
use crate::rollback::{RollbackProtection, SnapshotId};
use crate::OtaError;

// ============================================================
// OTA State Machine
// ============================================================

/// OTA 会话状态.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtaState {
    /// 空闲
    Idle,
    /// 下载差分包中
    Downloading,
    /// 验证补丁中
    Verifying,
    /// 应用差分中
    Applying,
    /// 健康检查中
    Testing,
    /// 提交更新中
    Committing,
    /// 已回滚
    RolledBack,
    /// 完成
    Complete,
}

// ============================================================
// Model Version
// ============================================================

/// 模型版本信息.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelVersion {
    /// 版本号
    pub version: String,
    /// SM3 哈希
    pub hash: [u8; 32],
    /// 模型大小
    pub size: usize,
    /// 创建时间
    pub created_at: u64,
}

// ============================================================
// OTA Manager
// ============================================================

/// OTA 差分更新管理器.
///
/// 编排完整的差分更新流程：
/// 1. 创建快照（回滚点）
/// 2. 验证差分包
/// 3. 应用差分
/// 4. 验证新版本哈希
/// 5. 提交或回滚
pub struct OtaManager<'a> {
    /// 差分算法
    differ: &'a dyn DiffUpdater,
    /// 回滚保护
    snapshots: &'a mut dyn RollbackProtection,
    /// 当前模型
    current: ModelVersion,
    /// 当前模型数据
    current_data: Vec<u8>,
    /// 当前状态
    state: OtaState,
    /// 更新统计
    pub total_updates: u32,
    /// 回滚统计
    pub total_rollbacks: u32,
}

impl<'a> OtaManager<'a> {
    /// 创建 OTA 管理器.
    pub fn new(
        differ: &'a dyn DiffUpdater,
        snapshots: &'a mut dyn RollbackProtection,
        current: ModelVersion,
        current_data: Vec<u8>,
    ) -> Self {
        Self {
            differ,
            snapshots,
            current,
            current_data,
            state: OtaState::Idle,
            total_updates: 0,
            total_rollbacks: 0,
        }
    }

    /// 获取当前模型版本.
    pub fn current_version(&self) -> &ModelVersion {
        &self.current
    }

    /// 获取当前状态.
    pub fn state(&self) -> OtaState {
        self.state
    }

    /// 执行差分更新.
    ///
    /// 完整流程：快照 → 验证 → 应用 → 提交/回滚
    pub fn update(&mut self, patch: &DiffPatch, now: u64) -> Result<(), OtaError> {
        // 1. 创建快照
        self.state = OtaState::Downloading;
        let snapshot_id =
            self.snapshots
                .create_snapshot(&self.current.version, &self.current_data, now)?;

        // 2. 验证补丁
        self.state = OtaState::Verifying;
        if !self.differ.verify_patch(patch)? {
            self.state = OtaState::RolledBack;
            return Err(OtaError::InvalidManifest);
        }

        // 3. 应用差分
        self.state = OtaState::Applying;
        let new_data = match self.differ.apply_patch(&self.current_data, patch) {
            Ok(data) => data,
            Err(e) => {
                // 应用失败，回滚
                self.rollback(snapshot_id)?;
                return Err(e);
            }
        };

        // 4. 验证新版本哈希
        self.state = OtaState::Testing;
        let new_hash = sm3::hash(&new_data);
        if new_hash != patch.new_hash {
            self.rollback(snapshot_id)?;
            return Err(OtaError::HashMismatch);
        }

        // 5. 提交更新
        self.state = OtaState::Committing;
        self.current = ModelVersion {
            version: patch.new_version.clone(),
            hash: new_hash,
            size: new_data.len(),
            created_at: now,
        };
        self.current_data = new_data;
        self.total_updates += 1;
        self.state = OtaState::Complete;

        Ok(())
    }

    /// 回滚到指定快照.
    fn rollback(&mut self, snapshot_id: SnapshotId) -> Result<(), OtaError> {
        let data = self.snapshots.rollback(snapshot_id)?;
        let hash = sm3::hash(&data);
        // 恢复版本信息（从快照中无法获取完整版本信息，使用哈希标识）
        self.current = ModelVersion {
            version: String::from("rolled_back"),
            hash,
            size: data.len(),
            created_at: 0,
        };
        self.current_data = data;
        self.total_rollbacks += 1;
        self.state = OtaState::RolledBack;
        Ok(())
    }

    /// 手动回滚到最近快照.
    pub fn rollback_to_latest(&mut self) -> Result<(), OtaError> {
        let latest = self
            .snapshots
            .list_snapshots()
            .last()
            .ok_or(OtaError::NoPreviousVersion)?;
        let id = latest.id;
        self.rollback(id)
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::SimpleDiff;
    use crate::rollback::SnapshotManager;

    fn make_manager<'a>(
        differ: &'a SimpleDiff,
        snapshots: &'a mut SnapshotManager,
        data: &[u8],
        version: &str,
    ) -> OtaManager<'a> {
        let current = ModelVersion {
            version: String::from(version),
            hash: sm3::hash(data),
            size: data.len(),
            created_at: 0,
        };
        OtaManager::new(differ, snapshots, current, data.to_vec())
    }

    #[test]
    fn test_successful_update() {
        let differ = SimpleDiff::new();
        let mut snapshots = SnapshotManager::new(5);

        let old_data = b"model version 1 data here";
        let new_data = b"model version 2 data here";

        let mut mgr = make_manager(&differ, &mut snapshots, old_data, "1.0.0");

        let patch = differ
            .create_diff(old_data, new_data, "1.0.0", "2.0.0")
            .unwrap();

        mgr.update(&patch, 1000).unwrap();

        assert_eq!(mgr.current_version().version, "2.0.0");
        assert_eq!(mgr.state(), OtaState::Complete);
        assert_eq!(mgr.total_updates, 1);
        assert_eq!(mgr.total_rollbacks, 0);
    }

    #[test]
    fn test_update_creates_snapshot() {
        let differ = SimpleDiff::new();
        let mut snapshots = SnapshotManager::new(5);

        let old_data = b"original model";
        let new_data = b"updated model";

        let mut mgr = make_manager(&differ, &mut snapshots, old_data, "1.0");
        let patch = differ
            .create_diff(old_data, new_data, "1.0", "2.0")
            .unwrap();
        mgr.update(&patch, 100).unwrap();

        // 快照应已创建
        assert_eq!(snapshots.snapshot_count(), 1);
        assert_eq!(snapshots.latest().unwrap().version, "1.0");
    }

    #[test]
    fn test_rollback_on_hash_mismatch() {
        let differ = SimpleDiff::new();
        let mut snapshots = SnapshotManager::new(5);

        let old_data = b"original model data";
        let new_data = b"updated model data";

        let mut mgr = make_manager(&differ, &mut snapshots, old_data, "1.0");

        // 创建补丁但篡改 new_hash
        let mut patch = differ
            .create_diff(old_data, new_data, "1.0", "2.0")
            .unwrap();
        patch.new_hash = [0xFF; 32]; // 篡改

        let result = mgr.update(&patch, 100);
        assert_eq!(result, Err(OtaError::HashMismatch));
        assert_eq!(mgr.state(), OtaState::RolledBack);
        assert_eq!(mgr.total_rollbacks, 1);
    }

    #[test]
    fn test_rollback_to_latest() {
        let differ = SimpleDiff::new();
        let mut snapshots = SnapshotManager::new(5);

        let old_data = b"version 1";
        let new_data = b"version 2";

        let mut mgr = make_manager(&differ, &mut snapshots, old_data, "1.0");
        let patch = differ
            .create_diff(old_data, new_data, "1.0", "2.0")
            .unwrap();
        mgr.update(&patch, 100).unwrap();
        assert_eq!(mgr.current_version().version, "2.0");

        // 手动回滚
        mgr.rollback_to_latest().unwrap();
        assert_eq!(mgr.state(), OtaState::RolledBack);
        assert_eq!(mgr.total_rollbacks, 1);
    }

    #[test]
    fn test_state_transitions() {
        let differ = SimpleDiff::new();
        let mut snapshots = SnapshotManager::new(5);

        let data = b"model";
        let mut mgr = make_manager(&differ, &mut snapshots, data, "1.0");
        assert_eq!(mgr.state(), OtaState::Idle);

        let new_data = b"model updated";
        let patch = differ.create_diff(data, new_data, "1.0", "2.0").unwrap();
        mgr.update(&patch, 100).unwrap();
        assert_eq!(mgr.state(), OtaState::Complete);
    }
}

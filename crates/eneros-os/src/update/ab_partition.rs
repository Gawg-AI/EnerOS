//! A/B 分区管理（槽位切换、持久化、boot count、health 状态）
//!
//! v0.22.0 扩展：支持槽位状态持久化到 JSON 文件、boot count 计数、
//! Trying/Good/Failed 健康状态机，用于 OTA 更新后的安全启动与自动回滚。
//!
//! 状态机：`Trying`（首次启动新槽位）→ `Good`（启动成功确认）或
//! `Failed`（启动失败，回滚到 last_good_slot）。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::update::error::UpdateError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Slot {
    A,
    B,
}

impl Slot {
    pub fn other(&self) -> Self {
        match self {
            Slot::A => Slot::B,
            Slot::B => Slot::A,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SlotStatus {
    Active,
    Inactive,
    Trying,
    Good,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbPartition {
    pub active_slot: Slot,
    pub slot_a_status: SlotStatus,
    pub slot_b_status: SlotStatus,
    pub boot_count_a: u32,
    pub boot_count_b: u32,
    pub last_boot: Option<DateTime<Utc>>,
    pub last_update: Option<DateTime<Utc>>,
    /// 持久化路径缓存（不序列化）。设置后，switch_slot/mark_* 会自动持久化。
    #[serde(skip)]
    state_file: Option<PathBuf>,
}

impl Default for AbPartition {
    fn default() -> Self {
        Self {
            active_slot: Slot::A,
            slot_a_status: SlotStatus::Active,
            slot_b_status: SlotStatus::Inactive,
            boot_count_a: 0,
            boot_count_b: 0,
            last_boot: None,
            last_update: None,
            state_file: None,
        }
    }
}

impl AbPartition {
    pub fn inactive_slot(&self) -> Slot {
        self.active_slot.other()
    }

    /// 切换活跃槽位（用于回滚）。
    ///
    /// 旧槽位设为 Good（保留为回滚目标），新槽位设为 Active。
    /// 若设置了 `state_file`，则同时持久化。
    pub fn switch_slot(&mut self) {
        let new_active = self.inactive_slot();
        self.set_slot_status(self.active_slot, SlotStatus::Good);
        self.set_slot_status(new_active, SlotStatus::Active);
        self.active_slot = new_active;
        self.persist();
    }

    /// 切换到新槽位并标记为 Trying（用于 OTA 更新）。
    ///
    /// 旧槽位设为 Good（作为回滚目标），新槽位设为 Trying（尚未确认启动成功）。
    /// 若设置了 `state_file`，则同时持久化。
    pub fn switch_to_trying(&mut self) {
        let new_active = self.inactive_slot();
        self.set_slot_status(self.active_slot, SlotStatus::Good);
        self.set_slot_status(new_active, SlotStatus::Trying);
        self.active_slot = new_active;
        self.persist();
    }

    /// 内部辅助：设置指定槽位的状态
    fn set_slot_status(&mut self, slot: Slot, status: SlotStatus) {
        match slot {
            Slot::A => self.slot_a_status = status,
            Slot::B => self.slot_b_status = status,
        }
    }

    /// 标记槽位为 `Trying`，对应 boot_count +1，更新 last_boot。
    /// 若设置了 `state_file`，则同时持久化。
    pub fn mark_trying(&mut self, slot: Slot) {
        match slot {
            Slot::A => {
                self.slot_a_status = SlotStatus::Trying;
                self.boot_count_a += 1;
            }
            Slot::B => {
                self.slot_b_status = SlotStatus::Trying;
                self.boot_count_b += 1;
            }
        }
        self.last_boot = Some(Utc::now());
        self.persist();
    }

    /// 标记槽位为 `Good`，重置对应 boot_count 为 0。
    /// 若设置了 `state_file`，则同时持久化。
    pub fn mark_good(&mut self, slot: Slot) {
        match slot {
            Slot::A => {
                self.slot_a_status = SlotStatus::Good;
                self.boot_count_a = 0;
            }
            Slot::B => {
                self.slot_b_status = SlotStatus::Good;
                self.boot_count_b = 0;
            }
        }
        self.persist();
    }

    /// 标记槽位为 `Failed`。
    /// 若设置了 `state_file`，则同时持久化。
    pub fn mark_failed(&mut self, slot: Slot) {
        match slot {
            Slot::A => self.slot_a_status = SlotStatus::Failed,
            Slot::B => self.slot_b_status = SlotStatus::Failed,
        }
        self.persist();
    }

    /// 返回最近的 Good 槽位。
    ///
    /// - 两个槽位都为 Good：返回非活跃槽位（作为回退目标）
    /// - 仅一个为 Good：返回那个
    /// - 都不是 Good：返回 None
    pub fn last_good_slot(&self) -> Option<Slot> {
        match (self.slot_a_status, self.slot_b_status) {
            (SlotStatus::Good, SlotStatus::Good) => Some(self.inactive_slot()),
            (SlotStatus::Good, _) => Some(Slot::A),
            (_, SlotStatus::Good) => Some(Slot::B),
            _ => None,
        }
    }

    /// 创建默认 AbPartition 并设置 state_file 路径。
    /// 设置后，switch_slot/mark_* 会自动持久化到该路径。
    pub fn with_state_file(path: PathBuf) -> Self {
        Self {
            state_file: Some(path),
            ..Self::default()
        }
    }

    /// 从 JSON 文件读取槽位状态。
    ///
    /// 文件不存在或 JSON 损坏时返回默认值（Slot A=Active, Slot B=Inactive）。
    /// 成功加载后，`state_file` 设为该路径，后续状态变更会自动持久化。
    pub fn load_from_file(path: &Path) -> Result<Self, UpdateError> {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(e) => return Err(e.into()),
        };
        let mut ab: AbPartition = match serde_json::from_str(&content) {
            Ok(ab) => ab,
            Err(e) => {
                tracing::warn!("AB partition state file corrupt, using default: {e}");
                return Ok(Self::default());
            }
        };
        ab.state_file = Some(path.to_path_buf());
        Ok(ab)
    }

    /// 持久化槽位状态到 JSON 文件。
    pub fn save_to_file(&self, path: &Path) -> Result<(), UpdateError> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| UpdateError::Serialize(e.to_string()))?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// 若设置了 state_file，则尽力持久化（失败仅记录警告，不阻断启动流程）。
    fn persist(&self) {
        if let Some(ref path) = self.state_file {
            if let Err(e) = self.save_to_file(path) {
                tracing::warn!("Failed to persist AB partition state: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slot_switch() {
        let mut ab = AbPartition::default();
        assert_eq!(ab.active_slot, Slot::A);
        ab.switch_slot();
        assert_eq!(ab.active_slot, Slot::B);
        assert_eq!(ab.slot_a_status, SlotStatus::Good); // 旧槽位保留 Good
        assert_eq!(ab.slot_b_status, SlotStatus::Active);
    }

    #[test]
    fn test_switch_to_trying() {
        let mut ab = AbPartition::default();
        assert_eq!(ab.active_slot, Slot::A);
        ab.switch_to_trying();
        assert_eq!(ab.active_slot, Slot::B);
        assert_eq!(ab.slot_a_status, SlotStatus::Good); // 旧槽位保留 Good 作为回滚目标
        assert_eq!(ab.slot_b_status, SlotStatus::Trying); // 新槽位为 Trying
        // last_good_slot 应返回旧槽位 A
        assert_eq!(ab.last_good_slot(), Some(Slot::A));
    }

    #[test]
    fn test_ota_rollback_scenario() {
        // 模拟完整 OTA 回滚场景
        let mut ab = AbPartition::default();
        // 初始：A=Active, B=Inactive
        assert_eq!(ab.last_good_slot(), None);

        // 标记 A 为 Good（初始安装后确认启动成功）
        ab.mark_good(Slot::A);
        assert_eq!(ab.last_good_slot(), Some(Slot::A)); // 只有 A 是 Good

        // OTA 更新：切换到 B，B=Trying, A=Good
        ab.switch_to_trying();
        assert_eq!(ab.active_slot, Slot::B);
        assert_eq!(ab.slot_b_status, SlotStatus::Trying);
        assert_eq!(ab.slot_a_status, SlotStatus::Good);

        // B 启动失败，回滚到 A
        assert_eq!(ab.last_good_slot(), Some(Slot::A));
        ab.mark_failed(Slot::B);
        ab.switch_slot(); // 回滚
        assert_eq!(ab.active_slot, Slot::A);
        assert_eq!(ab.slot_a_status, SlotStatus::Active);
    }

    #[test]
    fn test_status_transitions() {
        let mut ab = AbPartition::default();
        // Trying → Good → Failed 状态转换
        ab.mark_trying(Slot::A);
        assert_eq!(ab.slot_a_status, SlotStatus::Trying);
        ab.mark_good(Slot::A);
        assert_eq!(ab.slot_a_status, SlotStatus::Good);
        ab.mark_failed(Slot::A);
        assert_eq!(ab.slot_a_status, SlotStatus::Failed);
    }

    #[test]
    fn test_persistence_roundtrip() {
        let temp = std::env::temp_dir().join("eneros_ab_roundtrip.json");
        let _ = std::fs::remove_file(&temp);

        let mut ab = AbPartition::default();
        ab.switch_slot();
        ab.mark_trying(Slot::B);
        ab.save_to_file(&temp).unwrap();

        let loaded = AbPartition::load_from_file(&temp).unwrap();
        assert_eq!(loaded.active_slot, ab.active_slot);
        assert_eq!(loaded.slot_a_status, ab.slot_a_status);
        assert_eq!(loaded.slot_b_status, ab.slot_b_status);
        assert_eq!(loaded.boot_count_a, ab.boot_count_a);
        assert_eq!(loaded.boot_count_b, ab.boot_count_b);
        assert_eq!(loaded.last_boot, ab.last_boot);

        let _ = std::fs::remove_file(&temp);
    }

    #[test]
    fn test_default_on_missing_file() {
        let path = std::env::temp_dir().join("eneros_ab_nonexistent.json");
        let _ = std::fs::remove_file(&path);

        let ab = AbPartition::load_from_file(&path).unwrap();
        assert_eq!(ab.active_slot, Slot::A);
        assert_eq!(ab.slot_a_status, SlotStatus::Active);
        assert_eq!(ab.slot_b_status, SlotStatus::Inactive);
        assert_eq!(ab.boot_count_a, 0);
        assert_eq!(ab.boot_count_b, 0);
    }

    #[test]
    fn test_default_on_corrupt_file() {
        let path = std::env::temp_dir().join("eneros_ab_corrupt.json");
        std::fs::write(&path, "{ this is not valid json").unwrap();

        let ab = AbPartition::load_from_file(&path).unwrap();
        assert_eq!(ab.active_slot, Slot::A);
        assert_eq!(ab.slot_a_status, SlotStatus::Active);
        assert_eq!(ab.slot_b_status, SlotStatus::Inactive);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_boot_count_increment() {
        let mut ab = AbPartition::default();
        assert_eq!(ab.boot_count_a, 0);
        ab.mark_trying(Slot::A);
        assert_eq!(ab.boot_count_a, 1);
        ab.mark_trying(Slot::A);
        assert_eq!(ab.boot_count_a, 2);
        // B 槽位不受影响
        assert_eq!(ab.boot_count_b, 0);
    }

    #[test]
    fn test_mark_good_resets_boot_count() {
        let mut ab = AbPartition::default();
        ab.mark_trying(Slot::B);
        ab.mark_trying(Slot::B);
        assert_eq!(ab.boot_count_b, 2);
        ab.mark_good(Slot::B);
        assert_eq!(ab.boot_count_b, 0);
        assert_eq!(ab.slot_b_status, SlotStatus::Good);
    }

    #[test]
    fn test_last_good_slot() {
        // 两个槽位都 Good 时返回非活跃的
        let ab = AbPartition {
            active_slot: Slot::A,
            slot_a_status: SlotStatus::Good,
            slot_b_status: SlotStatus::Good,
            ..AbPartition::default()
        };
        assert_eq!(ab.last_good_slot(), Some(Slot::B));

        let ab = AbPartition {
            active_slot: Slot::B,
            slot_a_status: SlotStatus::Good,
            slot_b_status: SlotStatus::Good,
            ..AbPartition::default()
        };
        assert_eq!(ab.last_good_slot(), Some(Slot::A));

        // 只有一个 Good 时返回那个
        let ab = AbPartition {
            slot_a_status: SlotStatus::Good,
            slot_b_status: SlotStatus::Failed,
            ..AbPartition::default()
        };
        assert_eq!(ab.last_good_slot(), Some(Slot::A));

        // 都不是 Good 时返回 None
        let ab = AbPartition::default();
        assert_eq!(ab.last_good_slot(), None);
    }

    #[test]
    fn test_mark_failed() {
        let mut ab = AbPartition::default();
        ab.mark_failed(Slot::B);
        assert_eq!(ab.slot_b_status, SlotStatus::Failed);
        assert_eq!(ab.slot_a_status, SlotStatus::Active);
        ab.mark_failed(Slot::A);
        assert_eq!(ab.slot_a_status, SlotStatus::Failed);
    }
}

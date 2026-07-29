//! seL4 CSpace 映射器（v0.156.0 用户态兼容性验证）。
//!
//! 实现 [`CSpaceMapper`]：维护 seL4 CPtr → 自研内核 [`CapSlot`] 的
//! 双向映射。seL4 使用树形 CSpace（CNode 多级寻址），自研内核使用
//! 扁平能力表（分区内 4096 槽位）。映射器负责在两种模型间转换。
//!
//! 蓝图 phase3.md §v0.156.0 §4.2 接口定义 + §5 技术交底原样实现。
//!
//! 偏差/决策记录：
//! - D1：蓝图 `BTreeMap<CPtr, CapabilityId>` → 定长数组
//!   `[(CPtr, CapSlot); MAX_CSPACE_ENTRIES]`（no_std 无 alloc，
//!   同仓库 D2 先例链；cap=256 覆盖 Phase 1-2 用户态实际使用量）。
//! - D2：蓝图 `CapabilityId` → 复用既有 [`CapSlot`]（u32，v0.142.0
//!   能力管理器已定义的槽位下标类型）。
//! - D3：`next_cptr` 自增分配采用 `saturating_add`（禁 panic 路径，
//!   溢出饱和保唯一性，同 v0.142.0 next_cap_id 先例）。

use super::types::CPtr;
use crate::cap::CapSlot;

/// CSpace 映射表容量（定长，no_std 无堆）。
///
/// 256 条目覆盖 Phase 1-2 用户态实际使用的能力指针数量
/// （Agent Runtime 约 50 个 + 协议栈约 30 个 + LLM 引擎约 20 个，
/// 预留扩展余量）。
pub const MAX_CSPACE_ENTRIES: usize = 256;

/// CSpace 映射表条目。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CSpaceEntry {
    /// seL4 能力指针。
    cptr: CPtr,
    /// 自研内核能力槽位。
    cap_slot: CapSlot,
    /// 条目是否有效。
    valid: bool,
}

impl CSpaceEntry {
    /// 空条目。
    const fn empty() -> Self {
        Self {
            cptr: 0,
            cap_slot: 0,
            valid: false,
        }
    }
}

/// seL4 CSpace → 自研内核能力表映射器。
///
/// 维护 CPtr → CapSlot 的定长映射表，支持分配、解析、释放操作。
/// 蓝图 §4.2 `CSpaceMapper` 原样实现（D1 定长化）。
pub struct CSpaceMapper {
    /// 映射表（定长数组，no_std 无堆 D1）。
    entries: [CSpaceEntry; MAX_CSPACE_ENTRIES],
    /// 下一个可分配的 CPtr 值（自增，D3 saturating_add）。
    next_cptr: CPtr,
    /// 当前有效条目数。
    active_count: usize,
}

impl CSpaceMapper {
    /// 创建空映射器。
    pub const fn new() -> Self {
        Self {
            entries: [CSpaceEntry::empty(); MAX_CSPACE_ENTRIES],
            next_cptr: 1, // 0 保留为无效 CPtr 哨兵
            active_count: 0,
        }
    }

    /// 为指定 CapSlot 分配新的 CPtr 并建立映射。
    ///
    /// 返回分配的 CPtr；表满返回 `None`。
    pub fn allocate_cptr(&mut self, cap_slot: CapSlot) -> Option<CPtr> {
        // 查找空闲条目。
        let mut i = 0;
        while i < MAX_CSPACE_ENTRIES {
            if !self.entries[i].valid {
                let cptr = self.next_cptr;
                self.next_cptr = self.next_cptr.saturating_add(1);
                self.entries[i] = CSpaceEntry {
                    cptr,
                    cap_slot,
                    valid: true,
                };
                self.active_count += 1;
                return Some(cptr);
            }
            i += 1;
        }
        None // 表满
    }

    /// 在指定 CPtr 处建立映射（用于 seL4 侧已分配 CPtr 的场景）。
    ///
    /// 若 CPtr 已存在映射则更新；否则插入新条目。
    /// 表满且 CPtr 不存在时返回 `false`。
    pub fn insert_at(&mut self, cptr: CPtr, cap_slot: CapSlot) -> bool {
        // 先查找是否已存在。
        let mut first_free = None;
        let mut i = 0;
        while i < MAX_CSPACE_ENTRIES {
            if self.entries[i].valid {
                if self.entries[i].cptr == cptr {
                    // 更新已有映射。
                    self.entries[i].cap_slot = cap_slot;
                    return true;
                }
            } else if first_free.is_none() {
                first_free = Some(i);
            }
            i += 1;
        }
        // 插入新条目。
        if let Some(idx) = first_free {
            self.entries[idx] = CSpaceEntry {
                cptr,
                cap_slot,
                valid: true,
            };
            self.active_count += 1;
            // 确保 next_cptr 不会与手动插入的 CPtr 冲突。
            if cptr >= self.next_cptr {
                self.next_cptr = cptr.saturating_add(1);
            }
            true
        } else {
            false // 表满
        }
    }

    /// 解析 CPtr 为自研内核 CapSlot。
    ///
    /// 无映射返回 `None`（蓝图 §4.4：返回 EFAULT）。
    pub fn resolve_cptr(&self, cptr: CPtr) -> Option<CapSlot> {
        let mut i = 0;
        while i < MAX_CSPACE_ENTRIES {
            if self.entries[i].valid && self.entries[i].cptr == cptr {
                return Some(self.entries[i].cap_slot);
            }
            i += 1;
        }
        None
    }

    /// 反向解析：从 CapSlot 查找对应的 CPtr。
    ///
    /// 用于内核侧操作完成后需要返回 seL4 CPtr 的场景。
    pub fn resolve_cap_slot(&self, cap_slot: CapSlot) -> Option<CPtr> {
        let mut i = 0;
        while i < MAX_CSPACE_ENTRIES {
            if self.entries[i].valid && self.entries[i].cap_slot == cap_slot {
                return Some(self.entries[i].cptr);
            }
            i += 1;
        }
        None
    }

    /// 释放 CPtr 映射（删除条目）。
    ///
    /// 蓝图 §8.5 坑点：seL4 CNode Delete 在自研内核中用 CapDelete 模拟。
    pub fn free_cptr(&mut self, cptr: CPtr) -> bool {
        let mut i = 0;
        while i < MAX_CSPACE_ENTRIES {
            if self.entries[i].valid && self.entries[i].cptr == cptr {
                self.entries[i] = CSpaceEntry::empty();
                self.active_count -= 1;
                return true;
            }
            i += 1;
        }
        false
    }

    /// 获取当前有效映射数。
    pub const fn active_count(&self) -> usize {
        self.active_count
    }

    /// 判定映射表是否已满。
    pub const fn is_full(&self) -> bool {
        self.active_count >= MAX_CSPACE_ENTRIES
    }

    /// 清空所有映射。
    pub fn clear(&mut self) {
        let mut i = 0;
        while i < MAX_CSPACE_ENTRIES {
            self.entries[i] = CSpaceEntry::empty();
            i += 1;
        }
        self.active_count = 0;
    }
}

impl Default for CSpaceMapper {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;

    /// CS1：allocate_cptr 分配递增 CPtr。
    #[test]
    fn cs1_allocate_increments() {
        let mut mapper = CSpaceMapper::new();
        let c1 = mapper.allocate_cptr(10).unwrap();
        let c2 = mapper.allocate_cptr(20).unwrap();
        let c3 = mapper.allocate_cptr(30).unwrap();
        assert_eq!(c1, 1);
        assert_eq!(c2, 2);
        assert_eq!(c3, 3);
        assert_eq!(mapper.active_count(), 3);
    }

    /// CS2：resolve_cptr 正确解析。
    #[test]
    fn cs2_resolve_cptr() {
        let mut mapper = CSpaceMapper::new();
        let cptr = mapper.allocate_cptr(42).unwrap();
        assert_eq!(mapper.resolve_cptr(cptr), Some(42));
        assert_eq!(mapper.resolve_cptr(999), None);
    }

    /// CS3：resolve_cap_slot 反向解析。
    #[test]
    fn cs3_resolve_cap_slot() {
        let mut mapper = CSpaceMapper::new();
        let cptr = mapper.allocate_cptr(55).unwrap();
        assert_eq!(mapper.resolve_cap_slot(55), Some(cptr));
        assert_eq!(mapper.resolve_cap_slot(99), None);
    }

    /// CS4：free_cptr 释放后不可解析。
    #[test]
    fn cs4_free_cptr() {
        let mut mapper = CSpaceMapper::new();
        let cptr = mapper.allocate_cptr(10).unwrap();
        assert!(mapper.free_cptr(cptr));
        assert_eq!(mapper.resolve_cptr(cptr), None);
        assert_eq!(mapper.active_count(), 0);
        // 重复释放返回 false。
        assert!(!mapper.free_cptr(cptr));
    }

    /// CS5：insert_at 更新已有映射。
    #[test]
    fn cs5_insert_at_update() {
        let mut mapper = CSpaceMapper::new();
        assert!(mapper.insert_at(100, 10));
        assert_eq!(mapper.resolve_cptr(100), Some(10));
        // 更新。
        assert!(mapper.insert_at(100, 20));
        assert_eq!(mapper.resolve_cptr(100), Some(20));
        assert_eq!(mapper.active_count(), 1);
    }

    /// CS6：insert_at 新插入。
    #[test]
    fn cs6_insert_at_new() {
        let mut mapper = CSpaceMapper::new();
        assert!(mapper.insert_at(500, 33));
        assert_eq!(mapper.resolve_cptr(500), Some(33));
        assert_eq!(mapper.active_count(), 1);
    }

    /// CS7：表满时 allocate 返回 None。
    #[test]
    fn cs7_table_full() {
        let mut mapper = CSpaceMapper::new();
        for i in 0..MAX_CSPACE_ENTRIES {
            assert!(mapper.allocate_cptr(i as CapSlot).is_some());
        }
        assert!(mapper.is_full());
        assert_eq!(mapper.allocate_cptr(9999), None);
    }

    /// CS8：表满时 insert_at 新 CPtr 返回 false。
    #[test]
    fn cs8_insert_at_full() {
        let mut mapper = CSpaceMapper::new();
        for i in 0..MAX_CSPACE_ENTRIES {
            mapper.allocate_cptr(i as CapSlot).unwrap();
        }
        assert!(!mapper.insert_at(9999, 1));
    }

    /// CS9：clear 清空所有映射。
    #[test]
    fn cs9_clear() {
        let mut mapper = CSpaceMapper::new();
        mapper.allocate_cptr(1).unwrap();
        mapper.allocate_cptr(2).unwrap();
        mapper.clear();
        assert_eq!(mapper.active_count(), 0);
        assert_eq!(mapper.resolve_cptr(1), None);
    }

    /// CS10：释放后可重新分配（条目复用）。
    #[test]
    fn cs10_reuse_after_free() {
        let mut mapper = CSpaceMapper::new();
        let c1 = mapper.allocate_cptr(10).unwrap();
        mapper.free_cptr(c1);
        // 新分配复用空闲条目。
        let c2 = mapper.allocate_cptr(20).unwrap();
        assert_eq!(mapper.resolve_cptr(c2), Some(20));
        assert_eq!(mapper.active_count(), 1);
    }
}

//! 驱动句柄与能力令牌（v0.43.0）.
//!
//! 定义驱动访问控制的自包含能力模型：
//! - [`DriverPermission`] — 权限位集（手动 bitflags，u32）
//! - [`DriverCapability`] — 能力令牌（owner_id + permissions，Copy）
//! - [`DriverHandle`] — 驱动句柄（id + cap）
//!
//! # 偏差声明
//! - D1: 自包含 DriverCapability，不依赖 eneros-agent 的 CapabilityToken
//! - D8: DriverCapability 为 Copy（内部仅 u64+u32）

use crate::DriverId;

/// 驱动权限位集（D1：自包含，手动 bitflags）
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct DriverPermission(pub u32);

impl DriverPermission {
    /// 打开驱动权限
    pub const OPEN: Self = Self(0x01);
    /// 配置驱动权限
    pub const CONFIG: Self = Self(0x02);
    /// 中断处理权限
    pub const IRQ: Self = Self(0x04);
    /// 全部权限
    pub const ALL: Self = Self(0xFF);

    /// 返回底层位值
    pub fn bits(&self) -> u32 {
        self.0
    }

    /// 从位值构造
    pub fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// 是否包含 other 的所有位
    pub fn contains(&self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    /// 是否为空（无权限）
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }

    /// 是否拥有全部权限
    pub fn is_all(&self) -> bool {
        self.0 == Self::ALL.0
    }
}

impl core::ops::BitOr for DriverPermission {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl core::ops::BitOrAssign for DriverPermission {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// 驱动访问能力令牌（D1：自包含；D8：Copy）
///
/// 包含所有者 ID 与权限位集，用于 `DriverRegistry::open()` 时的访问控制。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DriverCapability {
    /// 所有者 ID
    owner_id: u64,
    /// 权限位集
    permissions: DriverPermission,
}

impl DriverCapability {
    /// 创建能力令牌
    pub fn new(owner_id: u64, permissions: DriverPermission) -> Self {
        Self {
            owner_id,
            permissions,
        }
    }

    /// 创建拥有全部权限的令牌
    pub fn new_full(owner_id: u64) -> Self {
        Self {
            owner_id,
            permissions: DriverPermission::ALL,
        }
    }

    /// 创建无权限的令牌
    pub fn new_empty(owner_id: u64) -> Self {
        Self {
            owner_id,
            permissions: DriverPermission(0),
        }
    }

    /// 检查是否具备所需权限
    pub fn can_access(&self, required: DriverPermission) -> bool {
        self.permissions.contains(required)
    }

    /// 返回所有者 ID
    pub fn owner(&self) -> u64 {
        self.owner_id
    }

    /// 返回权限位集
    pub fn permissions(&self) -> DriverPermission {
        self.permissions
    }
}

/// 驱动句柄（D8：持 DriverCapability）
///
/// `DriverRegistry::open()` 成功后返回，作为驱动访问凭证。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DriverHandle {
    /// 驱动 ID
    id: DriverId,
    /// 能力令牌
    cap: DriverCapability,
}

impl DriverHandle {
    /// 创建句柄
    pub fn new(id: DriverId, cap: DriverCapability) -> Self {
        Self { id, cap }
    }

    /// 返回驱动 ID
    pub fn id(&self) -> DriverId {
        self.id
    }

    /// 返回能力令牌
    pub fn cap(&self) -> DriverCapability {
        self.cap
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permission_bitor() {
        let combined = DriverPermission::OPEN | DriverPermission::CONFIG;
        // 同时包含 OPEN 与 CONFIG 两个位
        assert!(combined.contains(DriverPermission::OPEN));
        assert!(combined.contains(DriverPermission::CONFIG));
        // 不应包含 IRQ
        assert!(!combined.contains(DriverPermission::IRQ));
        // 底层位值校验
        assert_eq!(combined.bits(), 0x01 | 0x02);
    }

    #[test]
    fn test_permission_contains() {
        // ALL 包含 OPEN/CONFIG/IRQ
        assert!(DriverPermission::ALL.contains(DriverPermission::OPEN));
        assert!(DriverPermission::ALL.contains(DriverPermission::CONFIG));
        assert!(DriverPermission::ALL.contains(DriverPermission::IRQ));
        // OPEN 不包含 CONFIG
        assert!(!DriverPermission::OPEN.contains(DriverPermission::CONFIG));
    }

    #[test]
    fn test_permission_empty_and_all() {
        // 空权限
        assert!(DriverPermission(0).is_empty());
        assert!(!DriverPermission(0).is_all());
        // 全部权限
        assert!(DriverPermission::ALL.is_all());
        assert!(!DriverPermission::ALL.is_empty());
        // from_bits 往返
        assert!(DriverPermission::from_bits(0).is_empty());
        assert!(DriverPermission::from_bits(0xFF).is_all());
    }

    #[test]
    fn test_capability_can_access_granted() {
        let cap = DriverCapability::new_full(1);
        // 全权限令牌可通过任意权限检查
        assert!(cap.can_access(DriverPermission::OPEN));
        assert!(cap.can_access(DriverPermission::CONFIG));
        assert!(cap.can_access(DriverPermission::IRQ));
        assert!(cap.can_access(DriverPermission::ALL));
    }

    #[test]
    fn test_capability_can_access_denied() {
        // 空权限令牌无法通过 OPEN 检查
        let empty_cap = DriverCapability::new_empty(1);
        assert!(!empty_cap.can_access(DriverPermission::OPEN));
        // 仅有 OPEN 权限的令牌无法通过 CONFIG 检查
        let open_only = DriverCapability::new(1, DriverPermission::OPEN);
        assert!(!open_only.can_access(DriverPermission::CONFIG));
    }

    #[test]
    fn test_capability_new_full_and_empty() {
        let full = DriverCapability::new_full(1);
        assert_eq!(full.owner(), 1);
        assert_eq!(full.permissions(), DriverPermission::ALL);
        assert!(full.permissions().is_all());

        let empty = DriverCapability::new_empty(2);
        assert_eq!(empty.owner(), 2);
        assert_eq!(empty.permissions().bits(), 0);
        assert!(empty.permissions().is_empty());
    }

    #[test]
    fn test_handle_construct_and_accessors() {
        let id = DriverId(42);
        let cap = DriverCapability::new_full(7);
        let handle = DriverHandle::new(id, cap);

        // 访问器返回构造时传入的值
        assert_eq!(handle.id(), DriverId(42));
        assert_eq!(handle.cap(), cap);
        assert_eq!(handle.cap().owner(), 7);
        assert_eq!(handle.cap().permissions(), DriverPermission::ALL);
    }

    #[test]
    fn test_capability_copy_semantics() {
        let original = DriverCapability::new(5, DriverPermission::OPEN | DriverPermission::IRQ);
        // Copy 语义：赋值即复制（不移动）
        let copied = original;
        // 两者的所有者与权限均一致
        assert_eq!(original, copied);
        assert_eq!(original.owner(), copied.owner());
        assert_eq!(original.permissions(), copied.permissions());
        // 原令牌仍可用（证明 Copy 而非 Move）
        assert!(original.can_access(DriverPermission::OPEN));
        assert!(original.can_access(DriverPermission::IRQ));
    }
}

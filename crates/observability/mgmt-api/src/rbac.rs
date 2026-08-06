//! RBAC 角色与权限模型 (OBS-010).
//!
//! 三层权限：
//! - `SystemRead` — 只读操作（状态查询、日志查看）
//! - `SystemWrite` — 写操作（配置、Agent 控制、更新）
//! - `SecurityAdmin` — 安全敏感操作（吊销、工厂重置、安全擦除）
//!
//! 角色层次（高角色继承低角色权限）：
//! - `Viewer` → SystemRead
//! - `Operator` → SystemRead + SystemWrite
//! - `Admin` → SystemRead + SystemWrite + SecurityAdmin
//! - `Superuser` → 全部（用于本地控制台/恢复模式）

use alloc::vec::Vec;

/// 管理角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Role {
    /// 只读查看者。
    Viewer,
    /// 操作员（读写，无安全操作）。
    Operator,
    /// 管理员（全部操作）。
    Admin,
    /// 超级用户（本地恢复/出厂模式，不受限）。
    Superuser,
}

impl Role {
    /// 从字节解码（v0.239.0，签名 Token 用）。
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Viewer),
            1 => Some(Self::Operator),
            2 => Some(Self::Admin),
            3 => Some(Self::Superuser),
            _ => None,
        }
    }

    /// 编码为字节（v0.239.0）。
    pub fn to_u8(self) -> u8 {
        match self {
            Self::Viewer => 0,
            Self::Operator => 1,
            Self::Admin => 2,
            Self::Superuser => 3,
        }
    }

    /// 角色名称（用于序列化）。
    pub fn name(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Operator => "operator",
            Self::Admin => "admin",
            Self::Superuser => "superuser",
        }
    }

    /// 从名称解析角色。
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "viewer" => Some(Self::Viewer),
            "operator" => Some(Self::Operator),
            "admin" => Some(Self::Admin),
            "superuser" => Some(Self::Superuser),
            _ => None,
        }
    }

    /// 返回此角色直接拥有的权限（不含继承）。
    fn direct_permissions(self) -> &'static [Permission] {
        match self {
            Self::Viewer => &[Permission::SystemRead],
            Self::Operator => &[Permission::SystemRead, Permission::SystemWrite],
            Self::Admin => &[
                Permission::SystemRead,
                Permission::SystemWrite,
                Permission::SecurityAdmin,
            ],
            Self::Superuser => &[
                Permission::SystemRead,
                Permission::SystemWrite,
                Permission::SecurityAdmin,
                Permission::SuperuserOnly,
            ],
        }
    }
}

/// 权限枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    /// 系统只读。
    SystemRead,
    /// 系统读写。
    SystemWrite,
    /// 安全管理（吊销/重置/擦除）。
    SecurityAdmin,
    /// 超级用户专属（出厂模式/恢复）。
    SuperuserOnly,
}

/// RBAC 错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RbacError {
    /// 角色无此权限。
    PermissionDenied(Role, Permission),
    /// 角色不存在。
    UnknownRole,
}

/// RBAC 策略。
///
/// 默认实现角色层次继承：高角色自动拥有低角色权限。
/// 额外可通过 `grant` / `revoke` 为特定角色增减权限。
pub struct RbacPolicy {
    /// 额外授予的权限（role → permissions），叠加在层次继承之上。
    extra_grants: Vec<(Role, Permission)>,
}

impl Default for RbacPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl RbacPolicy {
    /// 创建默认策略（仅角色层次继承）。
    pub fn new() -> Self {
        Self {
            extra_grants: Vec::new(),
        }
    }

    /// 为角色额外授予权限。
    pub fn grant(&mut self, role: Role, perm: Permission) -> &mut Self {
        if !self
            .extra_grants
            .iter()
            .any(|(r, p)| *r == role && *p == perm)
        {
            self.extra_grants.push((role, perm));
        }
        self
    }

    /// 撤销角色的额外权限（不影响层次继承）。
    pub fn revoke(&mut self, role: Role, perm: Permission) -> &mut Self {
        self.extra_grants
            .retain(|(r, p)| !(*r == role && *p == perm));
        self
    }

    /// 检查角色是否拥有指定权限。
    pub fn check(&self, role: Role, perm: Permission) -> Result<(), RbacError> {
        if role.direct_permissions().contains(&perm) {
            return Ok(());
        }
        if self
            .extra_grants
            .iter()
            .any(|(r, p)| *r == role && *p == perm)
        {
            return Ok(());
        }
        Err(RbacError::PermissionDenied(role, perm))
    }

    /// 返回角色的所有权限（层次继承 + 额外授予）。
    pub fn permissions_for(&self, role: Role) -> Vec<Permission> {
        let mut perms: Vec<Permission> = role.direct_permissions().to_vec();
        for (r, p) in &self.extra_grants {
            if *r == role && !perms.contains(p) {
                perms.push(*p);
            }
        }
        perms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewer_can_read_not_write() {
        let policy = RbacPolicy::new();
        assert!(policy.check(Role::Viewer, Permission::SystemRead).is_ok());
        assert!(policy.check(Role::Viewer, Permission::SystemWrite).is_err());
    }

    #[test]
    fn operator_can_read_write_not_security() {
        let policy = RbacPolicy::new();
        assert!(policy.check(Role::Operator, Permission::SystemRead).is_ok());
        assert!(policy
            .check(Role::Operator, Permission::SystemWrite)
            .is_ok());
        assert!(policy
            .check(Role::Operator, Permission::SecurityAdmin)
            .is_err());
    }

    #[test]
    fn admin_has_all_normal_permissions() {
        let policy = RbacPolicy::new();
        assert!(policy.check(Role::Admin, Permission::SystemRead).is_ok());
        assert!(policy.check(Role::Admin, Permission::SystemWrite).is_ok());
        assert!(policy.check(Role::Admin, Permission::SecurityAdmin).is_ok());
    }

    #[test]
    fn superuser_has_all_permissions() {
        let policy = RbacPolicy::new();
        assert!(policy
            .check(Role::Superuser, Permission::SystemRead)
            .is_ok());
        assert!(policy
            .check(Role::Superuser, Permission::SystemWrite)
            .is_ok());
        assert!(policy
            .check(Role::Superuser, Permission::SecurityAdmin)
            .is_ok());
        assert!(policy
            .check(Role::Superuser, Permission::SuperuserOnly)
            .is_ok());
    }

    #[test]
    fn extra_grant_adds_permission() {
        let mut policy = RbacPolicy::new();
        policy.grant(Role::Viewer, Permission::SystemWrite);
        assert!(policy.check(Role::Viewer, Permission::SystemWrite).is_ok());
    }

    #[test]
    fn revoke_removes_extra_permission() {
        let mut policy = RbacPolicy::new();
        policy.grant(Role::Viewer, Permission::SystemWrite);
        assert!(policy.check(Role::Viewer, Permission::SystemWrite).is_ok());
        policy.revoke(Role::Viewer, Permission::SystemWrite);
        assert!(policy.check(Role::Viewer, Permission::SystemWrite).is_err());
    }

    #[test]
    fn role_from_name_roundtrip() {
        for role in [Role::Viewer, Role::Operator, Role::Admin, Role::Superuser] {
            let name = role.name();
            assert_eq!(Role::from_name(name), Some(role));
        }
        assert_eq!(Role::from_name("nonexistent"), None);
    }

    #[test]
    fn permission_denied_error_contains_role_and_perm() {
        let policy = RbacPolicy::new();
        let err = policy
            .check(Role::Viewer, Permission::SecurityAdmin)
            .unwrap_err();
        assert!(matches!(
            err,
            RbacError::PermissionDenied(Role::Viewer, Permission::SecurityAdmin)
        ));
    }
}

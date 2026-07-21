//! 能力令牌存储（TokenStore）(v0.40.0).
//!
//! 提供 [`TokenStore`]，作为 [`CapabilityManager`](crate::capability::manager::CapabilityManager)
//! 的内部存储层，维护令牌主表和按 owner 索引。
//!
//! # 偏差声明
//! - D1: 使用 `BTreeMap` 替代 `HashMap`（no_std 无 `std::collections::HashMap`）
//! - D6: 跳过 `by_target` 索引（`ResourceTarget` 非 `String`；`check_access` 仅用 `by_owner` 索引）
//!
//! # no_std 合规
//! 仅使用 `alloc::*` / `core::*`，不依赖 `std::*`。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::capability::token::CapabilityToken;
use crate::id::AgentId;

/// 能力令牌存储.
///
/// 维护令牌主表（`tokens`）和按 owner 索引（`by_owner`），支持高效查询。
/// 作为 [`CapabilityManager`](crate::capability::manager::CapabilityManager) 的内部存储层。
///
/// # 索引结构
/// - `tokens`: `BTreeMap<token_id, CapabilityToken>` — 令牌主表
/// - `by_owner`: `BTreeMap<owner, Vec<token_id>>` — 按 owner 索引令牌 ID
///
/// # 示例
/// ```
/// use eneros_agent::capability::store::TokenStore;
/// use eneros_agent::capability::CapabilityToken;
/// use eneros_agent::AgentId;
///
/// let store = TokenStore::new();
/// assert!(store.is_empty());
/// ```
#[derive(Debug)]
pub struct TokenStore {
    /// 令牌主表：token_id -> CapabilityToken
    tokens: BTreeMap<u64, CapabilityToken>,
    /// 按 owner 索引：owner -> Vec<token_id>
    by_owner: BTreeMap<AgentId, Vec<u64>>,
}

impl TokenStore {
    /// 创建空存储.
    pub fn new() -> Self {
        TokenStore {
            tokens: BTreeMap::new(),
            by_owner: BTreeMap::new(),
        }
    }

    /// 插入令牌并更新 by_owner 索引.
    ///
    /// 若 owner 不存在则创建新 Vec，否则追加到现有 Vec。
    pub fn insert(&mut self, token: CapabilityToken) {
        let token_id = token.token_id;
        let owner = token.owner;
        self.tokens.insert(token_id, token);
        self.by_owner.entry(owner).or_default().push(token_id);
    }

    /// 移除令牌并同步更新 by_owner 索引.
    ///
    /// 返回被移除的令牌（若存在）。同步从 by_owner 索引中移除 token_id，
    /// 若 owner 的 Vec 变空则移除该 owner 键。
    pub fn remove(&mut self, token_id: u64) -> Option<CapabilityToken> {
        let token = self.tokens.remove(&token_id)?;
        let owner = token.owner;
        if let Some(ids) = self.by_owner.get_mut(&owner) {
            ids.retain(|&id| id != token_id);
            if ids.is_empty() {
                self.by_owner.remove(&owner);
            }
        }
        Some(token)
    }

    /// 按 ID 查询令牌.
    pub fn get(&self, token_id: u64) -> Option<&CapabilityToken> {
        self.tokens.get(&token_id)
    }

    /// 按 ID 可变查询令牌.
    pub fn get_mut(&mut self, token_id: u64) -> Option<&mut CapabilityToken> {
        self.tokens.get_mut(&token_id)
    }

    /// 列出 owner 的所有令牌.
    ///
    /// 返回令牌引用的 Vec。若 owner 无令牌则返回空 Vec。
    pub fn list_by_owner(&self, owner: AgentId) -> Vec<&CapabilityToken> {
        self.by_owner
            .get(&owner)
            .map(|ids| ids.iter().filter_map(|id| self.tokens.get(id)).collect())
            .unwrap_or_default()
    }

    /// 列出 owner 的所有令牌 ID.
    ///
    /// 返回令牌 ID 的 Vec（克隆）。若 owner 无令牌则返回空 Vec。
    pub fn token_ids_by_owner(&self, owner: AgentId) -> Vec<u64> {
        self.by_owner.get(&owner).cloned().unwrap_or_default()
    }

    /// 列出所有已过期令牌的 ID.
    ///
    /// 遍历所有令牌，返回 `is_expired(now)` 为 true 的令牌 ID。
    pub fn list_expired_ids(&self, now: u64) -> Vec<u64> {
        self.tokens
            .iter()
            .filter(|(_, token)| token.is_expired(now))
            .map(|(id, _)| *id)
            .collect()
    }

    /// 令牌总数.
    pub fn len(&self) -> usize {
        self.tokens.len()
    }

    /// 是否为空.
    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }

    /// 迭代所有令牌.
    pub fn iter(&self) -> impl Iterator<Item = (&u64, &CapabilityToken)> {
        self.tokens.iter()
    }
}

impl Default for TokenStore {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::token::{
        CapabilityToken, ConstraintPack, PermissionSet, ResourceTarget,
    };

    /// 构造测试用令牌.
    fn make_token(token_id: u64, owner: AgentId, expires_at: Option<u64>) -> CapabilityToken {
        CapabilityToken {
            token_id,
            owner,
            target: ResourceTarget::Agent(AgentId(0)),
            permissions: PermissionSet::READ,
            constraints: ConstraintPack::default(),
            issued_at: 1000,
            expires_at,
            issuer: AgentId(0),
            signature: [0u8; 64],
        }
    }

    #[test]
    fn test_store_new_empty() {
        let store = TokenStore::new();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn test_store_insert_and_get() {
        let mut store = TokenStore::new();
        let token = make_token(1, AgentId(1), None);
        store.insert(token);
        assert_eq!(store.len(), 1);
        assert!(!store.is_empty());
        assert!(store.get(1).is_some());
        assert!(store.get(999).is_none());
    }

    #[test]
    fn test_store_insert_updates_by_owner() {
        let mut store = TokenStore::new();
        store.insert(make_token(1, AgentId(1), None));
        store.insert(make_token(2, AgentId(1), None));
        store.insert(make_token(3, AgentId(2), None));

        let owner1_ids = store.token_ids_by_owner(AgentId(1));
        assert_eq!(owner1_ids.len(), 2);
        assert!(owner1_ids.contains(&1));
        assert!(owner1_ids.contains(&2));

        let owner2_ids = store.token_ids_by_owner(AgentId(2));
        assert_eq!(owner2_ids.len(), 1);
        assert!(owner2_ids.contains(&3));

        let owner3_ids = store.token_ids_by_owner(AgentId(3));
        assert!(owner3_ids.is_empty());
    }

    #[test]
    fn test_store_remove() {
        let mut store = TokenStore::new();
        store.insert(make_token(1, AgentId(1), None));
        assert_eq!(store.len(), 1);

        let removed = store.remove(1);
        assert!(removed.is_some());
        assert_eq!(store.len(), 0);
        assert!(store.get(1).is_none());

        // Remove non-existent token
        let removed2 = store.remove(999);
        assert!(removed2.is_none());
    }

    #[test]
    fn test_store_remove_updates_index() {
        let mut store = TokenStore::new();
        store.insert(make_token(1, AgentId(1), None));
        store.insert(make_token(2, AgentId(1), None));

        // Remove token 1
        store.remove(1);

        let owner1_ids = store.token_ids_by_owner(AgentId(1));
        assert_eq!(owner1_ids.len(), 1);
        assert!(owner1_ids.contains(&2));
        assert!(!owner1_ids.contains(&1));

        // Remove last token for owner
        store.remove(2);
        let owner1_ids = store.token_ids_by_owner(AgentId(1));
        assert!(owner1_ids.is_empty());
    }

    #[test]
    fn test_store_list_by_owner() {
        let mut store = TokenStore::new();
        store.insert(make_token(1, AgentId(1), None));
        store.insert(make_token(2, AgentId(1), None));
        store.insert(make_token(3, AgentId(2), None));

        let owner1_tokens = store.list_by_owner(AgentId(1));
        assert_eq!(owner1_tokens.len(), 2);

        let owner2_tokens = store.list_by_owner(AgentId(2));
        assert_eq!(owner2_tokens.len(), 1);

        let owner3_tokens = store.list_by_owner(AgentId(3));
        assert!(owner3_tokens.is_empty());
    }

    #[test]
    fn test_store_list_expired_ids() {
        let mut store = TokenStore::new();
        // token 1: expired (expires_at=100, now=200)
        store.insert(make_token(1, AgentId(1), Some(100)));
        // token 2: not expired (expires_at=300, now=200)
        store.insert(make_token(2, AgentId(1), Some(300)));
        // token 3: no expiry (expires_at=None)
        store.insert(make_token(3, AgentId(2), None));

        let expired = store.list_expired_ids(200);
        assert_eq!(expired.len(), 1);
        assert!(expired.contains(&1));
        assert!(!expired.contains(&2));
        assert!(!expired.contains(&3));
    }

    #[test]
    fn test_store_len_and_is_empty() {
        let mut store = TokenStore::new();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);

        store.insert(make_token(1, AgentId(1), None));
        assert!(!store.is_empty());
        assert_eq!(store.len(), 1);

        store.insert(make_token(2, AgentId(2), None));
        assert_eq!(store.len(), 2);

        store.remove(1);
        assert_eq!(store.len(), 1);

        store.remove(2);
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn test_store_iter() {
        let mut store = TokenStore::new();
        store.insert(make_token(1, AgentId(1), None));
        store.insert(make_token(2, AgentId(2), None));

        let collected: Vec<u64> = store.iter().map(|(id, _)| *id).collect();
        assert_eq!(collected.len(), 2);
        assert!(collected.contains(&1));
        assert!(collected.contains(&2));
    }

    #[test]
    fn test_store_default() {
        let store = TokenStore::default();
        assert!(store.is_empty());
    }
}

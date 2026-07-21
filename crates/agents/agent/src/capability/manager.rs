//! 能力管理器（CapabilityManager）(v0.40.0).
//!
//! 提供 [`CapabilityManager`]，封装能力令牌的签发、校验、冻结、撤销和过期清理。
//! 作为 Agent 访问控制的安全边界，确保越权操作被拒绝。
//!
//! # 偏差声明
//! - D2: 使用 `issuer_keypair: Sm2KeyPair` 替代分离的 `sk` + `pk`（`build_and_sign` 需完整 keypair）
//! - D3: `issue(builder, now: u64)` — owner 已在 builder 中，需 `now` 用于 `issued_at`
//! - D4: `check_access` 接受 `now: u64`（no_std 无 `crate::time::now_ms()`）
//! - D5: 移除 `next_token_id`（token_id 由 `build_and_sign` 内部 CSRNG 随机生成）
//! - D7: `new(keypair, issuer_id: AgentId)` — 签发者 ID 可配置
//! - D8: `build_and_sign(&self.issuer_keypair, self.issuer_id, now, &mut self.rng)` — 4 参数
//! - D9: `check_access` 添加 `token.target == *target` 匹配检查（修复蓝图安全漏洞）
//!
//! # no_std 合规
//! 仅使用 `alloc::*` / `core::*`，不依赖 `std::*`。

use alloc::collections::BTreeSet;
use alloc::format;
use alloc::vec::Vec;
use core::fmt;

use eneros_crypto::{CsRng, Sm2KeyPair};

use crate::capability::builder::CapabilityTokenBuilder;
use crate::capability::store::TokenStore;
use crate::capability::token::{CapabilityToken, PermissionSet, ResourceTarget};
use crate::error::AgentError;
use crate::id::AgentId;

/// 能力管理器.
///
/// 封装能力令牌的签发、校验、冻结、撤销和过期清理。作为 Agent 访问控制的安全边界。
///
/// # 职责
/// - **签发**：通过 `issue` 方法签发新令牌（SM2 签名）
/// - **校验**：通过 `check_access` 检查 Agent 是否有权限访问目标资源
/// - **冻结/解冻**：崩溃 Agent 的所有令牌被冻结，防止僵尸 Agent 发命令
/// - **撤销**：永久移除令牌
/// - **过期清理**：定期清理过期令牌
///
/// # 安全特性
/// - `check_access` 检查 target 匹配（D9 修复蓝图安全漏洞）
/// - 冻结/撤销/过期的令牌被跳过
/// - 令牌签名使用 SM2（不可伪造）
/// - token_id 使用 CSRNG 随机生成（不可预测）
///
/// # 示例
/// ```
/// use eneros_agent::capability::{
///     CapabilityManager, CapabilityTokenBuilder, PermissionSet, ResourceTarget,
/// };
/// use eneros_agent::AgentId;
/// use eneros_crypto::{CsRng, Sm2KeyPair};
///
/// let mut rng = CsRng::new();
/// let keypair = Sm2KeyPair::generate(&mut rng).unwrap();
/// let mut manager = CapabilityManager::new(keypair, AgentId(1));
///
/// let builder = CapabilityTokenBuilder::new()
///     .owner(AgentId(2))
///     .target(ResourceTarget::Agent(AgentId(3)))
///     .permission(PermissionSet::READ | PermissionSet::WRITE)
///     .ttl(3600000);
///
/// let token = manager.issue(builder, 1000).unwrap();
/// assert!(manager.check_access(
///     AgentId(2),
///     &ResourceTarget::Agent(AgentId(3)),
///     PermissionSet::READ,
///     2000
/// ).is_ok());
/// ```
pub struct CapabilityManager {
    /// 令牌存储（主表 + by_owner 索引）
    store: TokenStore,
    /// 已冻结令牌 ID 集合
    frozen: BTreeSet<u64>,
    /// 已撤销令牌 ID 集合
    revoked: BTreeSet<u64>,
    /// 签发者密钥对（D2: 完整 keypair，非分离 sk+pk）
    issuer_keypair: Sm2KeyPair,
    /// 签发者 Agent ID（D7: 可配置，非硬编码 AgentId(0)）
    issuer_id: AgentId,
    /// 随机数生成器（用于 token_id 生成和 SM2 签名）
    /// 注：`CsRng` 不实现 `Debug`（内部状态不应泄露），因此 `CapabilityManager`
    /// 手动实现 `Debug` 并跳过此字段。
    rng: CsRng,
}

impl fmt::Debug for CapabilityManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CapabilityManager")
            .field("store", &self.store)
            .field("frozen", &self.frozen)
            .field("revoked", &self.revoked)
            .field("issuer_keypair", &self.issuer_keypair)
            .field("issuer_id", &self.issuer_id)
            .field("rng", &"<CsRng: redacted>")
            .finish()
    }
}

impl CapabilityManager {
    /// 构造能力管理器.
    ///
    /// # 参数
    /// - `keypair`: 签发者 SM2 密钥对（用于令牌签名）
    /// - `issuer_id`: 签发者 Agent ID（D7 偏差：可配置）
    ///
    /// # 返回
    /// 空的 `CapabilityManager`（无令牌、无冻结、无撤销）。
    pub fn new(keypair: Sm2KeyPair, issuer_id: AgentId) -> Self {
        CapabilityManager {
            store: TokenStore::new(),
            frozen: BTreeSet::new(),
            revoked: BTreeSet::new(),
            issuer_keypair: keypair,
            issuer_id,
            rng: CsRng::new(),
        }
    }

    /// 签发能力令牌.
    ///
    /// 使用 builder 配置令牌字段，调用 `build_and_sign` 生成签名令牌，
    /// 然后存入存储。返回令牌副本给调用者。
    ///
    /// # 参数
    /// - `builder`: 已配置好的令牌构建器
    /// - `now`: 当前时间戳（毫秒，用于 `issued_at`）
    ///
    /// # 返回
    /// 已签名的 `CapabilityToken` 或错误。
    ///
    /// # 偏差
    /// - D3: 接受 `now` 而非 `owner`（owner 已在 builder 中）
    /// - D8: 调用 `build_and_sign(&self.issuer_keypair, self.issuer_id, now, &mut self.rng)`
    pub fn issue(
        &mut self,
        builder: CapabilityTokenBuilder,
        now: u64,
    ) -> Result<CapabilityToken, AgentError> {
        let token =
            builder.build_and_sign(&self.issuer_keypair, self.issuer_id, now, &mut self.rng)?;
        self.store.insert(token.clone());
        Ok(token)
    }

    /// 验证令牌签名.
    ///
    /// 委托 `token.verify(&self.issuer_keypair.public_key)`。
    ///
    /// # 返回
    /// - `Ok(())`: 签名有效
    /// - `Err(TokenNotSigned)`: 令牌未签名
    /// - `Err(TokenSignatureInvalid)`: 签名无效
    pub fn verify_token(&self, token: &CapabilityToken) -> Result<(), AgentError> {
        token.verify(&self.issuer_keypair.public_key)
    }

    /// 检查 Agent 是否有权限访问目标资源.
    ///
    /// 遍历 Agent 持有的所有令牌，查找匹配 target + permission 的有效令牌。
    ///
    /// # 参数
    /// - `agent_id`: 请求访问的 Agent ID
    /// - `target`: 目标资源
    /// - `perm`: 所需权限
    /// - `now`: 当前时间戳（毫秒）
    ///
    /// # 返回
    /// - `Ok(&CapabilityToken)`: 找到匹配的有效令牌
    /// - `Err(NoCapability)`: 无匹配令牌
    ///
    /// # 偏差
    /// - D4: 接受 `now` 参数（no_std 无系统时钟）
    /// - D9: 检查 `token.target == *target`（修复蓝图安全漏洞）
    ///
    /// # 跳过条件
    /// 以下令牌被跳过（不视为有效）：
    /// 1. 冻结令牌（`frozen` 集合中）
    /// 2. 撤销令牌（`revoked` 集合中）
    /// 3. 过期令牌（`token.is_expired(now)`）
    /// 4. target 不匹配的令牌（D9 修复）
    /// 5. 权限不足的令牌
    pub fn check_access(
        &self,
        agent_id: AgentId,
        target: &ResourceTarget,
        perm: PermissionSet,
        now: u64,
    ) -> Result<&CapabilityToken, AgentError> {
        for token in self.store.list_by_owner(agent_id) {
            // 跳过冻结令牌
            if self.frozen.contains(&token.token_id) {
                continue;
            }
            // 跳过撤销令牌
            if self.revoked.contains(&token.token_id) {
                continue;
            }
            // 跳过过期令牌
            if token.is_expired(now) {
                continue;
            }
            // D9: 跳过 target 不匹配的令牌（修复蓝图安全漏洞）
            if token.target != *target {
                continue;
            }
            // 跳过权限不足的令牌
            if !token.check_permission(perm) {
                continue;
            }
            // 找到匹配令牌
            return Ok(token);
        }
        // 全部不匹配
        Err(AgentError::NoCapability {
            agent: agent_id,
            target: format!("{:?}", target),
        })
    }

    /// 冻结 Agent 的所有令牌.
    ///
    /// 将 Agent 持有的所有令牌 ID 加入 `frozen` 集合。冻结后令牌在 `check_access` 中被跳过。
    /// 用于崩溃 Agent 的处理，防止僵尸 Agent 发命令。
    ///
    /// # 参数
    /// - `agent_id`: 要冻结的 Agent ID
    ///
    /// # 返回
    /// 冻结的令牌数量。
    pub fn freeze(&mut self, agent_id: AgentId) -> usize {
        let ids = self.store.token_ids_by_owner(agent_id);
        let mut count = 0;
        for id in ids {
            if self.frozen.insert(id) {
                count += 1;
            }
        }
        count
    }

    /// 解冻单个令牌.
    ///
    /// 从 `frozen` 集合中移除令牌 ID。
    ///
    /// # 返回
    /// `true` 表示令牌原处于冻结状态并已解冻，`false` 表示令牌未冻结。
    pub fn unfreeze(&mut self, token_id: u64) -> bool {
        self.frozen.remove(&token_id)
    }

    /// 撤销令牌.
    ///
    /// 从存储中移除令牌，并将 token_id 加入 `revoked` 集合（防止重放）。
    ///
    /// # 返回
    /// `true` 表示令牌存在并已撤销，`false` 表示令牌不存在。
    pub fn revoke(&mut self, token_id: u64) -> bool {
        if self.store.remove(token_id).is_some() {
            self.revoked.insert(token_id);
            true
        } else {
            false
        }
    }

    /// 列出所有令牌.
    pub fn list_tokens(&self) -> Vec<&CapabilityToken> {
        self.store.iter().map(|(_, token)| token).collect()
    }

    /// 清理过期令牌.
    ///
    /// 移除所有 `is_expired(now)` 为 true 的令牌。
    ///
    /// # 参数
    /// - `now`: 当前时间戳（毫秒）
    ///
    /// # 返回
    /// 清理的令牌数量。
    pub fn cleanup_expired(&mut self, now: u64) -> usize {
        let expired_ids = self.store.list_expired_ids(now);
        let count = expired_ids.len();
        for id in expired_ids {
            self.store.remove(id);
        }
        count
    }

    /// 检查令牌是否冻结.
    pub fn is_frozen(&self, token_id: u64) -> bool {
        self.frozen.contains(&token_id)
    }

    /// 检查令牌是否撤销.
    pub fn is_revoked(&self, token_id: u64) -> bool {
        self.revoked.contains(&token_id)
    }

    /// 获取存储引用.
    pub fn store(&self) -> &TokenStore {
        &self.store
    }

    /// 获取签发者 Agent ID.
    pub fn issuer_id(&self) -> AgentId {
        self.issuer_id
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 生成测试用密钥对与 RNG.
    fn make_keypair() -> (Sm2KeyPair, CsRng) {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).expect("keypair gen failed");
        (kp, rng)
    }

    /// 构造测试用 builder.
    fn make_builder(
        owner: AgentId,
        target: ResourceTarget,
        perm: PermissionSet,
        ttl: u64,
    ) -> CapabilityTokenBuilder {
        CapabilityTokenBuilder::new()
            .owner(owner)
            .target(target)
            .permission(perm)
            .ttl(ttl)
    }

    #[test]
    fn test_manager_new() {
        let (kp, _) = make_keypair();
        let manager = CapabilityManager::new(kp, AgentId(1));
        assert!(manager.store().is_empty());
        assert!(!manager.is_frozen(0));
        assert!(!manager.is_revoked(0));
        assert_eq!(manager.issuer_id(), AgentId(1));
    }

    #[test]
    fn test_manager_issue_success() {
        let (kp, _) = make_keypair();
        let mut manager = CapabilityManager::new(kp, AgentId(1));
        let builder = make_builder(
            AgentId(2),
            ResourceTarget::Agent(AgentId(3)),
            PermissionSet::READ | PermissionSet::WRITE,
            3600000,
        );
        let token = manager.issue(builder, 1000).expect("issue failed");
        assert_ne!(token.token_id, 0);
        assert_ne!(token.signature, [0u8; 64]);
        assert_eq!(token.issued_at, 1000);
        assert_eq!(token.issuer, AgentId(1));
        assert_eq!(manager.store().len(), 1);
    }

    #[test]
    fn test_manager_issue_and_verify() {
        let (kp, _) = make_keypair();
        let mut manager = CapabilityManager::new(kp, AgentId(1));
        let builder = make_builder(
            AgentId(2),
            ResourceTarget::Agent(AgentId(3)),
            PermissionSet::READ,
            3600000,
        );
        let token = manager.issue(builder, 1000).expect("issue failed");
        assert!(manager.verify_token(&token).is_ok());
    }

    #[test]
    fn test_manager_check_access_allowed() {
        let (kp, _) = make_keypair();
        let mut manager = CapabilityManager::new(kp, AgentId(1));
        let builder = make_builder(
            AgentId(2),
            ResourceTarget::Agent(AgentId(3)),
            PermissionSet::READ | PermissionSet::WRITE,
            3600000,
        );
        manager.issue(builder, 1000).expect("issue failed");

        // Agent 2 有权限访问 Agent 3 的 READ
        let result = manager.check_access(
            AgentId(2),
            &ResourceTarget::Agent(AgentId(3)),
            PermissionSet::READ,
            2000,
        );
        assert!(
            result.is_ok(),
            "check_access should succeed, got {:?}",
            result
        );
    }

    #[test]
    fn test_manager_check_access_denied_no_token() {
        let (kp, _) = make_keypair();
        let manager = CapabilityManager::new(kp, AgentId(1));

        // Agent 99 无任何令牌
        let result = manager.check_access(
            AgentId(99),
            &ResourceTarget::Agent(AgentId(3)),
            PermissionSet::READ,
            2000,
        );
        assert!(matches!(result, Err(AgentError::NoCapability { .. })));
    }

    #[test]
    fn test_manager_check_access_denied_wrong_target() {
        let (kp, _) = make_keypair();
        let mut manager = CapabilityManager::new(kp, AgentId(1));
        let builder = make_builder(
            AgentId(2),
            ResourceTarget::Agent(AgentId(3)),
            PermissionSet::READ,
            3600000,
        );
        manager.issue(builder, 1000).expect("issue failed");

        // Agent 2 有令牌但 target 是 Agent(3)，请求 Agent(4) 应被拒绝（D9 验证）
        let result = manager.check_access(
            AgentId(2),
            &ResourceTarget::Agent(AgentId(4)),
            PermissionSet::READ,
            2000,
        );
        assert!(matches!(result, Err(AgentError::NoCapability { .. })));
    }

    #[test]
    fn test_manager_check_access_denied_wrong_permission() {
        let (kp, _) = make_keypair();
        let mut manager = CapabilityManager::new(kp, AgentId(1));
        let builder = make_builder(
            AgentId(2),
            ResourceTarget::Agent(AgentId(3)),
            PermissionSet::READ,
            3600000,
        );
        manager.issue(builder, 1000).expect("issue failed");

        // Agent 2 只有 READ，请求 WRITE 应被拒绝
        let result = manager.check_access(
            AgentId(2),
            &ResourceTarget::Agent(AgentId(3)),
            PermissionSet::WRITE,
            2000,
        );
        assert!(matches!(result, Err(AgentError::NoCapability { .. })));
    }

    #[test]
    fn test_manager_check_access_denied_expired() {
        let (kp, _) = make_keypair();
        let mut manager = CapabilityManager::new(kp, AgentId(1));
        // ttl=1000, issued_at=1000, expires_at=2000
        let builder = make_builder(
            AgentId(2),
            ResourceTarget::Agent(AgentId(3)),
            PermissionSet::READ,
            1000,
        );
        manager.issue(builder, 1000).expect("issue failed");

        // now=3000 > expires_at=2000，令牌已过期
        let result = manager.check_access(
            AgentId(2),
            &ResourceTarget::Agent(AgentId(3)),
            PermissionSet::READ,
            3000,
        );
        assert!(matches!(result, Err(AgentError::NoCapability { .. })));
    }

    #[test]
    fn test_manager_check_access_denied_frozen() {
        let (kp, _) = make_keypair();
        let mut manager = CapabilityManager::new(kp, AgentId(1));
        let builder = make_builder(
            AgentId(2),
            ResourceTarget::Agent(AgentId(3)),
            PermissionSet::READ,
            3600000,
        );
        let token = manager.issue(builder, 1000).expect("issue failed");

        // 冻结 Agent 2 的所有令牌
        let frozen_count = manager.freeze(AgentId(2));
        assert_eq!(frozen_count, 1);
        assert!(manager.is_frozen(token.token_id));

        // 冻结后 check_access 应被拒绝
        let result = manager.check_access(
            AgentId(2),
            &ResourceTarget::Agent(AgentId(3)),
            PermissionSet::READ,
            2000,
        );
        assert!(matches!(result, Err(AgentError::NoCapability { .. })));
    }

    #[test]
    fn test_manager_freeze_agent() {
        let (kp, _) = make_keypair();
        let mut manager = CapabilityManager::new(kp, AgentId(1));

        // Agent 2 持有 2 个令牌
        manager
            .issue(
                make_builder(
                    AgentId(2),
                    ResourceTarget::Agent(AgentId(3)),
                    PermissionSet::READ,
                    3600000,
                ),
                1000,
            )
            .expect("issue 1 failed");
        manager
            .issue(
                make_builder(
                    AgentId(2),
                    ResourceTarget::Agent(AgentId(4)),
                    PermissionSet::WRITE,
                    3600000,
                ),
                1000,
            )
            .expect("issue 2 failed");

        // 冻结 Agent 2
        let count = manager.freeze(AgentId(2));
        assert_eq!(count, 2);

        // 再次冻结应返回 0（已在 frozen 集合中）
        let count2 = manager.freeze(AgentId(2));
        assert_eq!(count2, 0);
    }

    #[test]
    fn test_manager_revoke_token() {
        let (kp, _) = make_keypair();
        let mut manager = CapabilityManager::new(kp, AgentId(1));
        let token = manager
            .issue(
                make_builder(
                    AgentId(2),
                    ResourceTarget::Agent(AgentId(3)),
                    PermissionSet::READ,
                    3600000,
                ),
                1000,
            )
            .expect("issue failed");

        assert_eq!(manager.store().len(), 1);
        assert!(manager.revoke(token.token_id));
        assert_eq!(manager.store().len(), 0);
        assert!(manager.is_revoked(token.token_id));

        // 撤销不存在的令牌返回 false
        assert!(!manager.revoke(999));
    }

    #[test]
    fn test_manager_cleanup_expired() {
        let (kp, _) = make_keypair();
        let mut manager = CapabilityManager::new(kp, AgentId(1));

        // token 1: ttl=1000, issued_at=1000, expires_at=2000（now=3000 时过期）
        manager
            .issue(
                make_builder(
                    AgentId(2),
                    ResourceTarget::Agent(AgentId(3)),
                    PermissionSet::READ,
                    1000,
                ),
                1000,
            )
            .expect("issue 1 failed");
        // token 2: ttl=5000, issued_at=1000, expires_at=6000（now=3000 时未过期）
        manager
            .issue(
                make_builder(
                    AgentId(4),
                    ResourceTarget::Agent(AgentId(5)),
                    PermissionSet::WRITE,
                    5000,
                ),
                1000,
            )
            .expect("issue 2 failed");

        assert_eq!(manager.store().len(), 2);
        let cleaned = manager.cleanup_expired(3000);
        assert_eq!(cleaned, 1);
        assert_eq!(manager.store().len(), 1);
    }

    #[test]
    fn test_manager_unfreeze() {
        let (kp, _) = make_keypair();
        let mut manager = CapabilityManager::new(kp, AgentId(1));
        let token = manager
            .issue(
                make_builder(
                    AgentId(2),
                    ResourceTarget::Agent(AgentId(3)),
                    PermissionSet::READ,
                    3600000,
                ),
                1000,
            )
            .expect("issue failed");

        // 冻结
        manager.freeze(AgentId(2));
        assert!(manager.is_frozen(token.token_id));

        // 解冻
        assert!(manager.unfreeze(token.token_id));
        assert!(!manager.is_frozen(token.token_id));

        // 解冻未冻结的令牌返回 false
        assert!(!manager.unfreeze(token.token_id));

        // 解冻后 check_access 应恢复
        let result = manager.check_access(
            AgentId(2),
            &ResourceTarget::Agent(AgentId(3)),
            PermissionSet::READ,
            2000,
        );
        assert!(
            result.is_ok(),
            "check_access should succeed after unfreeze, got {:?}",
            result
        );
    }
}

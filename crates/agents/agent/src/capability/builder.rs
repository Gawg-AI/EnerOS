//! 能力令牌构建器 (v0.39.0).
//!
//! 提供 [`CapabilityTokenBuilder`]，使用 Builder 模式构建并签名能力令牌。
//!
//! # 偏差声明
//! - D1: `build_and_sign` 接受 `now: u64` + `rng: &mut CsRng`（no_std 无系统时钟）
//! - D2: token_id 由 `rng.fill_bytes()` 生成（no_std 无系统 RNG）
//! - D3: SM2 签名使用 `sm2_sign(data, &sk, &pk, rng)`（需公钥 + RNG）
//!
//! # no_std 合规
//! 仅使用 `alloc::*` / `core::*`，不依赖 `std::*`。

use eneros_crypto::{sm2_sign, CsRng, Sm2KeyPair};

use crate::capability::token::{CapabilityToken, ConstraintPack, PermissionSet, ResourceTarget};
use crate::error::AgentError;
use crate::id::AgentId;

/// 能力令牌构建器.
///
/// 使用 Builder 模式逐步设置令牌字段，最终通过 `build_and_sign` 签名生成令牌。
///
/// # 示例
/// ```
/// use eneros_crypto::{CsRng, Sm2KeyPair};
/// use eneros_agent::capability::{CapabilityTokenBuilder, PermissionSet, ResourceTarget};
/// use eneros_agent::AgentId;
///
/// let mut rng = CsRng::new();
/// let keypair = Sm2KeyPair::generate(&mut rng).unwrap();
/// let token = CapabilityTokenBuilder::new()
///     .owner(AgentId(1))
///     .target(ResourceTarget::Agent(AgentId(2)))
///     .permission(PermissionSet::READ | PermissionSet::WRITE)
///     .ttl(3600000)
///     .build_and_sign(&keypair, AgentId(1), 1000, &mut rng)
///     .unwrap();
/// ```
pub struct CapabilityTokenBuilder {
    owner: AgentId,
    target: ResourceTarget,
    permissions: PermissionSet,
    constraints: ConstraintPack,
    ttl_ms: u64,
}

impl CapabilityTokenBuilder {
    /// 创建构建器，使用默认值.
    ///
    /// 默认值：
    /// - owner: `AgentId::ZERO`
    /// - target: `ResourceTarget::Agent(AgentId::ZERO)`
    /// - permissions: `PermissionSet::NONE`
    /// - constraints: `ConstraintPack::default()`（全零）
    /// - ttl: 0（永不过期）
    pub fn new() -> Self {
        CapabilityTokenBuilder {
            owner: AgentId::ZERO,
            target: ResourceTarget::Agent(AgentId::ZERO),
            permissions: PermissionSet::NONE,
            constraints: ConstraintPack::default(),
            ttl_ms: 0,
        }
    }

    /// 设置令牌持有者.
    pub fn owner(mut self, id: AgentId) -> Self {
        self.owner = id;
        self
    }

    /// 设置目标资源.
    pub fn target(mut self, t: ResourceTarget) -> Self {
        self.target = t;
        self
    }

    /// 设置权限集.
    pub fn permission(mut self, p: PermissionSet) -> Self {
        self.permissions = p;
        self
    }

    /// 设置安全约束.
    pub fn constraints(mut self, c: ConstraintPack) -> Self {
        self.constraints = c;
        self
    }

    /// 设置有效期（毫秒）.
    ///
    /// 0 表示永不过期。
    pub fn ttl(mut self, ms: u64) -> Self {
        self.ttl_ms = ms;
        self
    }

    /// 构建并签名令牌.
    ///
    /// # 参数
    /// - `issuer_keypair`: 签发者密钥对（用于 SM2 签名）
    /// - `issuer_id`: 签发者 Agent ID
    /// - `now`: 当前时间戳（毫秒）
    /// - `rng`: 随机数生成器
    ///
    /// # 返回
    /// 已签名的 `CapabilityToken` 或错误。
    ///
    /// # 流程
    /// 1. 生成随机 token_id（D2: `rng.fill_bytes()`）
    /// 2. 构造 CapabilityToken（signature 全零）
    /// 3. 序列化未签名部分
    /// 4. SM2 签名（D3: `sm2_sign(data, &sk, &pk, rng)`）
    /// 5. 填入 signature
    /// 6. 返回令牌
    pub fn build_and_sign(
        self,
        issuer_keypair: &Sm2KeyPair,
        issuer_id: AgentId,
        now: u64,
        rng: &mut CsRng,
    ) -> Result<CapabilityToken, AgentError> {
        // 1. 生成随机 token_id (D2)
        let mut id_buf = [0u8; 8];
        rng.fill_bytes(&mut id_buf);
        let token_id = u64::from_be_bytes(id_buf);

        // 2. 计算过期时间
        let expires_at = if self.ttl_ms == 0 {
            None
        } else {
            Some(now.saturating_add(self.ttl_ms))
        };

        // 3. 构造令牌（signature 全零，待签名）
        let mut token = CapabilityToken {
            token_id,
            owner: self.owner,
            target: self.target,
            permissions: self.permissions,
            constraints: self.constraints,
            issued_at: now,
            expires_at,
            issuer: issuer_id,
            signature: [0u8; 64],
        };

        // 4. 序列化未签名部分
        let data = token.serialize_unsigned();

        // 5. SM2 签名 (D3: 需要公钥计算 Z 值 + RNG)
        let sig = sm2_sign(
            &data,
            &issuer_keypair.private_key,
            &issuer_keypair.public_key,
            rng,
        )
        .map_err(|_| AgentError::TokenSignatureInvalid)?;

        // 6. 填入签名
        token.signature = sig.to_bytes();

        Ok(token)
    }
}

impl Default for CapabilityTokenBuilder {
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

    /// 生成测试用 SM2 密钥对与 CSRNG.
    fn make_keypair() -> (Sm2KeyPair, CsRng) {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).expect("keypair gen failed");
        (kp, rng)
    }

    #[test]
    fn test_build_and_sign_success() {
        let (kp, mut rng) = make_keypair();
        let now = 1000u64;
        let ttl = 3600000u64;
        let token = CapabilityTokenBuilder::new()
            .owner(AgentId(1))
            .target(ResourceTarget::Agent(AgentId(2)))
            .permission(PermissionSet::READ | PermissionSet::WRITE)
            .ttl(ttl)
            .build_and_sign(&kp, AgentId(1), now, &mut rng)
            .expect("build_and_sign failed");
        assert_ne!(token.token_id, 0, "token_id should be non-zero");
        assert_ne!(token.signature, [0u8; 64], "signature should be non-zero");
        assert_eq!(token.issued_at, now);
        assert_eq!(token.expires_at, Some(now + ttl));
    }

    #[test]
    fn test_build_and_sign_verify_ok() {
        let (kp, mut rng) = make_keypair();
        let token = CapabilityTokenBuilder::new()
            .owner(AgentId(1))
            .target(ResourceTarget::Agent(AgentId(2)))
            .permission(PermissionSet::READ | PermissionSet::WRITE)
            .ttl(3600000)
            .build_and_sign(&kp, AgentId(1), 1000, &mut rng)
            .expect("build_and_sign failed");
        let result = token.verify(&kp.public_key);
        assert!(result.is_ok(), "verify should succeed, got {:?}", result);
    }

    #[test]
    fn test_tamper_permissions_verify_fails() {
        let (kp, mut rng) = make_keypair();
        let mut token = CapabilityTokenBuilder::new()
            .owner(AgentId(1))
            .target(ResourceTarget::Agent(AgentId(2)))
            .permission(PermissionSet::READ)
            .ttl(3600000)
            .build_and_sign(&kp, AgentId(1), 1000, &mut rng)
            .expect("build_and_sign failed");
        // 篡改权限字段
        token.permissions = PermissionSet::ALL;
        let result = token.verify(&kp.public_key);
        assert!(
            matches!(result, Err(AgentError::TokenSignatureInvalid)),
            "tampered permissions should fail verification, got {:?}",
            result
        );
    }

    #[test]
    fn test_tamper_token_id_verify_fails() {
        let (kp, mut rng) = make_keypair();
        let mut token = CapabilityTokenBuilder::new()
            .owner(AgentId(1))
            .target(ResourceTarget::Agent(AgentId(2)))
            .permission(PermissionSet::READ)
            .ttl(3600000)
            .build_and_sign(&kp, AgentId(1), 1000, &mut rng)
            .expect("build_and_sign failed");
        // 篡改 token_id（翻转最低位）
        token.token_id ^= 1;
        let result = token.verify(&kp.public_key);
        assert!(
            matches!(result, Err(AgentError::TokenSignatureInvalid)),
            "tampered token_id should fail verification, got {:?}",
            result
        );
    }

    #[test]
    fn test_tamper_owner_verify_fails() {
        let (kp, mut rng) = make_keypair();
        let mut token = CapabilityTokenBuilder::new()
            .owner(AgentId(1))
            .target(ResourceTarget::Agent(AgentId(2)))
            .permission(PermissionSet::READ)
            .ttl(3600000)
            .build_and_sign(&kp, AgentId(1), 1000, &mut rng)
            .expect("build_and_sign failed");
        // 篡改 owner 字段
        token.owner = AgentId(token.owner.0 ^ 1);
        let result = token.verify(&kp.public_key);
        assert!(
            matches!(result, Err(AgentError::TokenSignatureInvalid)),
            "tampered owner should fail verification, got {:?}",
            result
        );
    }

    #[test]
    fn test_tamper_issued_at_verify_fails() {
        let (kp, mut rng) = make_keypair();
        let mut token = CapabilityTokenBuilder::new()
            .owner(AgentId(1))
            .target(ResourceTarget::Agent(AgentId(2)))
            .permission(PermissionSet::READ)
            .ttl(3600000)
            .build_and_sign(&kp, AgentId(1), 1000, &mut rng)
            .expect("build_and_sign failed");
        // 篡改 issued_at 字段
        token.issued_at ^= 1;
        let result = token.verify(&kp.public_key);
        assert!(
            matches!(result, Err(AgentError::TokenSignatureInvalid)),
            "tampered issued_at should fail verification, got {:?}",
            result
        );
    }

    #[test]
    fn test_tamper_signature_verify_fails() {
        let (kp, mut rng) = make_keypair();
        let mut token = CapabilityTokenBuilder::new()
            .owner(AgentId(1))
            .target(ResourceTarget::Agent(AgentId(2)))
            .permission(PermissionSet::READ)
            .ttl(3600000)
            .build_and_sign(&kp, AgentId(1), 1000, &mut rng)
            .expect("build_and_sign failed");
        // 篡改签名字节（翻转首字节所有位）
        token.signature[0] ^= 0xFF;
        let result = token.verify(&kp.public_key);
        assert!(
            matches!(result, Err(AgentError::TokenSignatureInvalid)),
            "tampered signature should fail verification, got {:?}",
            result
        );
    }

    #[test]
    fn test_wrong_keypair_verify_fails() {
        // 使用同一 RNG 生成两个不同密钥对
        let mut rng = CsRng::new();
        let kp1 = Sm2KeyPair::generate(&mut rng).expect("kp1 gen failed");
        let kp2 = Sm2KeyPair::generate(&mut rng).expect("kp2 gen failed");
        assert_ne!(kp1.public_key, kp2.public_key, "keypairs should differ");
        let token = CapabilityTokenBuilder::new()
            .owner(AgentId(1))
            .target(ResourceTarget::Agent(AgentId(2)))
            .permission(PermissionSet::READ | PermissionSet::WRITE)
            .ttl(3600000)
            .build_and_sign(&kp1, AgentId(1), 1000, &mut rng)
            .expect("build_and_sign failed");
        // 用错误的公钥验证
        let result = token.verify(&kp2.public_key);
        assert!(
            matches!(result, Err(AgentError::TokenSignatureInvalid)),
            "verify with wrong keypair should fail with TokenSignatureInvalid, got {:?}",
            result
        );
    }

    #[test]
    fn test_ttl_zero_no_expiry() {
        let (kp, mut rng) = make_keypair();
        let token = CapabilityTokenBuilder::new()
            .owner(AgentId(1))
            .target(ResourceTarget::Agent(AgentId(2)))
            .permission(PermissionSet::READ)
            .ttl(0)
            .build_and_sign(&kp, AgentId(1), 1000, &mut rng)
            .expect("build_and_sign failed");
        assert_eq!(token.expires_at, None);
    }

    #[test]
    fn test_token_id_randomness() {
        let (kp, mut rng) = make_keypair();
        let now = 1000u64;
        let t1 = CapabilityTokenBuilder::new()
            .owner(AgentId(1))
            .target(ResourceTarget::Agent(AgentId(2)))
            .permission(PermissionSet::READ)
            .ttl(3600000)
            .build_and_sign(&kp, AgentId(1), now, &mut rng)
            .expect("build t1 failed");
        let t2 = CapabilityTokenBuilder::new()
            .owner(AgentId(1))
            .target(ResourceTarget::Agent(AgentId(2)))
            .permission(PermissionSet::READ)
            .ttl(3600000)
            .build_and_sign(&kp, AgentId(1), now, &mut rng)
            .expect("build t2 failed");
        assert_ne!(t1.token_id, t2.token_id, "token_ids should differ");
    }

    #[test]
    fn test_ttl_sets_expires_at() {
        let (kp, mut rng) = make_keypair();
        let now = 1000u64;
        let token = CapabilityTokenBuilder::new()
            .owner(AgentId(1))
            .target(ResourceTarget::Agent(AgentId(2)))
            .permission(PermissionSet::READ)
            .ttl(3600000)
            .build_and_sign(&kp, AgentId(1), now, &mut rng)
            .expect("build_and_sign failed");
        assert_eq!(token.expires_at, Some(now + 3600000));
    }
}

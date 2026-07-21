//! 能力令牌验证器 (v0.39.0).
//!
//! 提供 [`TokenVerifier`]，封装签发者公钥以避免每次验证都传参。
//!
//! # 偏差声明
//! - D10: `verify` 返回 `Result<(), AgentError>`（Ok(())=有效，非 `Result<bool, _>`）
//!
//! # no_std 合规
//! 仅使用 `alloc::*` / `core::*`，不依赖 `std::*`。

use eneros_crypto::Sm2PublicKey;

use crate::capability::token::CapabilityToken;
use crate::error::AgentError;

/// 能力令牌验证器.
///
/// 封装签发者公钥，提供便捷的令牌验证接口。
/// 适用于需要批量验证多个令牌的场景。
#[derive(Debug)]
pub struct TokenVerifier {
    /// 签发者公钥
    issuer_pk: Sm2PublicKey,
}

impl TokenVerifier {
    /// 创建验证器.
    ///
    /// # 参数
    /// - `issuer_pk`: 签发者公钥
    pub fn new(issuer_pk: Sm2PublicKey) -> Self {
        TokenVerifier { issuer_pk }
    }

    /// 验证令牌签名.
    ///
    /// 委托给 `CapabilityToken::verify`。
    ///
    /// # 返回
    /// - `Ok(())`: 签名有效
    /// - `Err(TokenNotSigned)`: 令牌未签名
    /// - `Err(TokenSignatureInvalid)`: 签名验证失败
    pub fn verify(&self, token: &CapabilityToken) -> Result<(), AgentError> {
        token.verify(&self.issuer_pk)
    }

    /// 获取签发者公钥引用.
    pub fn issuer_pk(&self) -> &Sm2PublicKey {
        &self.issuer_pk
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use eneros_crypto::{CsRng, Sm2KeyPair};

    use super::*;
    use crate::capability::builder::CapabilityTokenBuilder;
    use crate::capability::token::{PermissionSet, ResourceTarget};
    use crate::id::AgentId;

    #[test]
    fn test_token_verifier_verify_ok() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).expect("keypair gen failed");
        let token = CapabilityTokenBuilder::new()
            .owner(AgentId(1))
            .target(ResourceTarget::Agent(AgentId(2)))
            .permission(PermissionSet::READ | PermissionSet::WRITE)
            .ttl(3600000)
            .build_and_sign(&kp, AgentId(1), 1000, &mut rng)
            .expect("build_and_sign failed");
        let verifier = TokenVerifier::new(kp.public_key);
        let result = verifier.verify(&token);
        assert!(result.is_ok(), "verify should succeed, got {:?}", result);
    }

    #[test]
    fn test_token_verifier_verify_tampered_fails() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).expect("keypair gen failed");
        let mut token = CapabilityTokenBuilder::new()
            .owner(AgentId(1))
            .target(ResourceTarget::Agent(AgentId(2)))
            .permission(PermissionSet::READ)
            .ttl(3600000)
            .build_and_sign(&kp, AgentId(1), 1000, &mut rng)
            .expect("build_and_sign failed");
        // 篡改权限字段
        token.permissions = PermissionSet::ALL;
        let verifier = TokenVerifier::new(kp.public_key);
        let result = verifier.verify(&token);
        assert!(
            matches!(result, Err(AgentError::TokenSignatureInvalid)),
            "tampered token should fail verification, got {:?}",
            result
        );
    }

    #[test]
    fn test_token_verifier_issuer_pk() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).expect("keypair gen failed");
        let verifier = TokenVerifier::new(kp.public_key);
        assert_eq!(*verifier.issuer_pk(), kp.public_key);
    }
}

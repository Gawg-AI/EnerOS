//! mgmt-api 签名 Token 认证（v0.239.0，B5-02）。
//!
//! role 由明文声明改为 SM2 签名 Token：
//!
//! - [`SignedToken`] — `role(1B) ‖ issued_at(8B LE) ‖ expires_at(8B LE)
//!   ‖ digest(32B) ‖ signature(64B)`；签发方（管理侧）用私钥签名，
//!   API 侧用公钥验签；
//! - [`TokenVerifier`] — 验签 + 过期检查 + 角色提取；伪造角色请求
//!   在验签期必失败（`SignatureInvalid`/`TokenExpired`）。

use alloc::string::String;
use alloc::vec::Vec;

use eneros_crypto::{sm3_hash, sm2_verify, Sm2PublicKey, Sm2Signature};

use crate::Role;

/// 签名 Token 长度（固定 1+8+8+32+64 = 113 字节）。
pub const TOKEN_LEN: usize = 113;

/// 签名 Token（v0.239.0）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedToken {
    /// 请求方角色。
    pub role: Role,
    /// 签发时间（Unix 秒）。
    pub issued_at: u64,
    /// 过期时间（Unix 秒）。
    pub expires_at: u64,
    /// 摘要（对 role‖issued_at‖expires_at 的 SM3）。
    pub digest: [u8; 32],
    /// SM2 签名（签发方私钥）。
    pub signature: [u8; 64],
}

impl SignedToken {
    /// 编码为固定长度字节。
    pub fn to_bytes(&self) -> [u8; TOKEN_LEN] {
        let mut out = [0u8; TOKEN_LEN];
        out[0] = self.role.to_u8();
        out[1..9].copy_from_slice(&self.issued_at.to_le_bytes());
        out[9..17].copy_from_slice(&self.expires_at.to_le_bytes());
        out[17..49].copy_from_slice(&self.digest);
        out[49..113].copy_from_slice(&self.signature);
        out
    }

    /// 从字节解码；长度/格式非法 → None。
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != TOKEN_LEN {
            return None;
        }
        let role = Role::from_u8(bytes[0])?;
        let issued_at = u64::from_le_bytes(bytes[1..9].try_into().ok()?);
        let expires_at = u64::from_le_bytes(bytes[9..17].try_into().ok()?);
        let mut digest = [0u8; 32];
        digest.copy_from_slice(&bytes[17..49]);
        let mut signature = [0u8; 64];
        signature.copy_from_slice(&bytes[49..113]);
        Some(Self {
            role,
            issued_at,
            expires_at,
            digest,
            signature,
        })
    }

    /// 计算载荷摘要（签发/验证共用）。
    pub fn compute_digest(role: Role, issued_at: u64, expires_at: u64) -> [u8; 32] {
        let mut buf = [0u8; 17];
        buf[0] = role.to_u8();
        buf[1..9].copy_from_slice(&issued_at.to_le_bytes());
        buf[9..17].copy_from_slice(&expires_at.to_le_bytes());
        sm3_hash(&buf)
    }
}

/// Token 验签错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenError {
    /// 签名无效（伪造角色/篡改载荷）。
    SignatureInvalid,
    /// Token 已过期。
    TokenExpired,
    /// Token 格式非法。
    Malformed,
}

/// 管理 API Token 验证器。
pub struct TokenVerifier {
    /// 管理侧签发公钥。
    issuer_pubkey: Sm2PublicKey,
}

impl TokenVerifier {
    /// 创建验证器（管理侧公钥注入）。
    pub fn new(issuer_pubkey: Sm2PublicKey) -> Self {
        Self { issuer_pubkey }
    }

    /// 验签 Token：签名有效 + 未过期 → 返回角色。
    ///
    /// 伪造 Superuser 请求（签名无效或角色被篡改）在此拒绝。
    pub fn verify(&self, token_bytes: &[u8], now_unix: u64) -> Result<Role, TokenError> {
        let token = SignedToken::from_bytes(token_bytes).ok_or(TokenError::Malformed)?;
        // 摘要一致性。
        let expected = SignedToken::compute_digest(token.role, token.issued_at, token.expires_at);
        if expected != token.digest {
            return Err(TokenError::SignatureInvalid);
        }
        // 过期检查。
        if now_unix >= token.expires_at {
            return Err(TokenError::TokenExpired);
        }
        // SM2 验签。
        let sig = Sm2Signature::from_bytes(&token.signature);
        let ok = sm2_verify(&token.digest, &sig, &self.issuer_pubkey)
            .map_err(|_| TokenError::SignatureInvalid)?;
        if !ok {
            return Err(TokenError::SignatureInvalid);
        }
        Ok(token.role)
    }
}

// ============================================================================
// Tests（伪造 Superuser 拒绝）
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn sign_token(
        kp: &eneros_crypto::Sm2KeyPair,
        rng: &mut eneros_crypto::CsRng,
        role: Role,
        issued_at: u64,
        expires_at: u64,
    ) -> SignedToken {
        let digest = SignedToken::compute_digest(role, issued_at, expires_at);
        let sig = eneros_crypto::sm2_sign(&digest, &kp.private_key, &kp.public_key, rng)
            .expect("sign");
        SignedToken {
            role,
            issued_at,
            expires_at,
            digest,
            signature: sig.to_bytes(),
        }
    }

    #[test]
    fn test_token_roundtrip() {
        let mut rng = eneros_crypto::CsRng::from_seed(&[0x71; 32]);
        let kp = eneros_crypto::Sm2KeyPair::generate(&mut rng).expect("kp");
        let token = sign_token(&kp, &mut rng, Role::Superuser, 1000, 2000);
        let bytes = token.to_bytes();
        let decoded = SignedToken::from_bytes(&bytes).expect("decode");
        assert_eq!(decoded, token);
        assert_eq!(bytes.len(), TOKEN_LEN);
    }

    #[test]
    fn test_verify_accepts_valid_token() {
        let mut rng = eneros_crypto::CsRng::from_seed(&[0x72; 32]);
        let kp = eneros_crypto::Sm2KeyPair::generate(&mut rng).expect("kp");
        let verifier = TokenVerifier::new(kp.public_key);
        let token = sign_token(&kp, &mut rng, Role::Operator, 1000, 2000);
        let role = verifier.verify(&token.to_bytes(), 1500).expect("verify");
        assert_eq!(role, Role::Operator);
    }

    #[test]
    fn test_forged_superuser_rejected() {
        // 伪造 Superuser：用攻击者密钥签发，验证器持合法公钥 → 拒绝。
        let mut rng = eneros_crypto::CsRng::from_seed(&[0x73; 32]);
        let legit_kp = eneros_crypto::Sm2KeyPair::generate(&mut rng).expect("legit");
        let attacker_kp = eneros_crypto::Sm2KeyPair::generate(&mut rng).expect("attacker");
        let verifier = TokenVerifier::new(legit_kp.public_key);
        let forged = sign_token(&attacker_kp, &mut rng, Role::Superuser, 1000, 2000);
        assert_eq!(
            verifier.verify(&forged.to_bytes(), 1500),
            Err(TokenError::SignatureInvalid),
            "伪造 Superuser 请求必须被拒"
        );
    }

    #[test]
    fn test_role_tampering_rejected() {
        // 篡改 role 字节（Superuser 声明但签名是针对 Operator 的）→ 摘要不符。
        let mut rng = eneros_crypto::CsRng::from_seed(&[0x74; 32]);
        let kp = eneros_crypto::Sm2KeyPair::generate(&mut rng).expect("kp");
        let verifier = TokenVerifier::new(kp.public_key);
        let mut token = sign_token(&kp, &mut rng, Role::Operator, 1000, 2000);
        token.role = Role::Superuser; // 篡改
        assert_eq!(
            verifier.verify(&token.to_bytes(), 1500),
            Err(TokenError::SignatureInvalid),
            "角色篡改必须被拒"
        );
    }

    #[test]
    fn test_expired_token_rejected() {
        let mut rng = eneros_crypto::CsRng::from_seed(&[0x75; 32]);
        let kp = eneros_crypto::Sm2KeyPair::generate(&mut rng).expect("kp");
        let verifier = TokenVerifier::new(kp.public_key);
        let token = sign_token(&kp, &mut rng, Role::Viewer, 1000, 2000);
        assert_eq!(
            verifier.verify(&token.to_bytes(), 2500),
            Err(TokenError::TokenExpired)
        );
    }

    #[test]
    fn test_malformed_token_rejected() {
        let mut rng = eneros_crypto::CsRng::from_seed(&[0x76; 32]);
        let kp = eneros_crypto::Sm2KeyPair::generate(&mut rng).expect("kp");
        let verifier = TokenVerifier::new(kp.public_key);
        assert_eq!(verifier.verify(b"short", 1000), Err(TokenError::Malformed));
    }
}

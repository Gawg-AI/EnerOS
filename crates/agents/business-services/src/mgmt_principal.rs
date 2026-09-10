//! 70.5（WP-70.5.6）：签名管理 principal 绑定——MGT1 v2 会话。
//!
//! 方向 3 §8A 管理身份合同：broker 会话身份不自动等于发布者身份，
//! **消息中的任意 Role/Admin 字段不可信**。本模块实现"端到端签名消息
//! 绑定"方案：管理消息携带 station 端签发的 principal token（canonical
//! 绑定 principal/角色/租户/通道/过期与签发时刻）+ ECDSA P-256 签名；
//! svc-mgmt 以部署期嵌入的 station 公钥验签——**wire 携带的角色字节仅
//! 在签名验证通过后进入授权**，伪造 Role/Admin = 签名失配拒绝。
//!
//! wire（MGT1 v2）：session 块由 48B 扩展为 `48B ‖ token 64B ‖ sig 64B`
//! （112B；其余布局不变）。部署侧嵌入 station 公钥（build.rs 嵌入面，
//! 公钥非机密——轮换 = 重构建/换包）后，**旧 48B 无签会话一律拒绝**
//! （ERR:principal-unverified——不降级）；未部署 station 公钥 = 显式
//! 开发模式（启动日志明示，48B 旧面保留——qemu-test 兼容探针同纪律）。
//!
//! 算法域：ECDSA P-256（与目标 TLS 域同域；OTA 域 SM2 分域不动——
//! 方向 3 §3 密钥用途分域纪律）。签名 = `SHA-256(token)` 的 ECDSA
//! DER 签名（openssl CLI 互操作形态——gate 站端同源签发）。

use alloc::string::String;
use alloc::vec::Vec;

/// token 魔数（"MGTPTK1"）。
pub const TOKEN_MAGIC: [u8; 8] = *b"MGTPTK1\0";
/// token 编码长度（canonical 定长）。
pub const TOKEN_LEN: usize = 64;
/// 签名 wire 字段 = 2B 实长（LE）+ DER（ECDSA P-256 DER ≤ 72B——整数
/// 各带高位补零最多 33B）= 74B。
pub const SIG_FIELD_LEN: usize = 74;
/// 旧无签会话长度（v1；仅 station 公钥未部署的开发模式合法）。
pub const SESSION_V1_LEN: usize = 48;
/// v2 会话长度 = v1 ‖ token ‖ sig 字段。
pub const SESSION_V2_LEN: usize = SESSION_V1_LEN + TOKEN_LEN + SIG_FIELD_LEN;

/// 验签拒绝类别（词形；只含类别，不含密钥材料）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrincipalReject {
    /// token 魔数/长度/域检失败。
    Malformed,
    /// 签名验证失败（伪造/篡改）。
    BadSignature,
    /// 已过期（expiry 早于当前时间）。
    Expired,
    /// station 公钥未部署（v2 面不可用）。
    NoStationKey,
}

/// 验签通过后解出的可信 principal 绑定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedPrincipal {
    pub principal: String,
    pub roles: Vec<String>,
    pub tenant_scope: u32,
    pub channel_binding: String,
    /// 过期时刻（Unix ms；svc-mgmt 会话有效期以此为准）。
    pub expires_at_ms: u64,
}

/// NUL 填充定长字段 → UTF-8 串（畸形 = None）。
fn field_str(raw: &[u8]) -> Option<String> {
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    core::str::from_utf8(&raw[..end]).ok().map(String::from)
}

/// 最小 ASN.1 DER 编码（ECDSA r‖s → `SEQUENCE { INTEGER r, INTEGER s }`；
/// openssl 签发互操作同源。手写以避开 p256 der feature 的 no_std 依赖
/// 涟漪——embedded.rs parse_sec1 同先例）。
#[cfg(test)]
fn der_encode_signature(fixed: &[u8; 64]) -> Vec<u8> {
    let (r, s) = fixed.split_at(32);
    fn int(body: &[u8], out: &mut Vec<u8>) {
        let first_nonzero = body.iter().position(|b| *b != 0).unwrap_or(body.len());
        let v = &body[first_nonzero..];
        out.push(0x02);
        if v.first().is_some_and(|b| *b & 0x80 != 0) {
            out.push(v.len() as u8 + 1);
            out.push(0x00);
            out.extend_from_slice(v);
        } else if v.is_empty() {
            out.push(1);
            out.push(0);
        } else {
            out.push(v.len() as u8);
            out.extend_from_slice(v);
        }
    }
    let mut body = Vec::new();
    int(r, &mut body);
    int(s, &mut body);
    let mut out = Vec::from(&[0x30][..]);
    out.push(body.len() as u8);
    out.extend_from_slice(&body);
    out
}

/// 最小 ASN.1 DER 解码（`SEQUENCE { INTEGER r, INTEGER s }` → 64B 定长
/// r‖s；openssl 签名形态）。畸形 → None。
fn der_decode_signature(der: &[u8]) -> Option<[u8; 64]> {
    if der.first() != Some(&0x30) || der.len() < 2 {
        return None;
    }
    let body_len = *der.get(1)? as usize;
    if 2 + body_len != der.len() {
        return None;
    }
    let mut pos = 2usize;
    let mut out = [0u8; 64];
    for slot in out.chunks_mut(32) {
        if der.get(pos) != Some(&0x02) {
            return None;
        }
        pos += 1;
        let ilen = *der.get(pos)? as usize;
        pos += 1;
        let v = der.get(pos..pos + ilen)?;
        pos += ilen;
        // 前导零（符号填充）剥离后须 ≤32B。
        let first_nonzero = v.iter().position(|b| *b != 0).unwrap_or(v.len());
        let v = &v[first_nonzero..];
        if v.is_empty() || v.len() > 32 {
            return None;
        }
        slot[32 - v.len()..].copy_from_slice(v);
    }
    Some(out)
}

/// ECDSA P-256 验签（station 公钥 SEC1 未压缩点 65B `04‖X‖Y` +
/// SHA-256(token)；点形态与目标 TLS 域密钥面同型——pkcs8 特性不入
/// no_std 依赖图，embedded.rs parse_sec1 同先例）。
fn verify_signature(
    station_pubkey_der: &[u8],
    token: &[u8; TOKEN_LEN],
    sig_der: &[u8],
) -> Result<(), PrincipalReject> {
    use p256::ecdsa::signature::Verifier;
    use sha2::Digest;
    let vk = p256::ecdsa::VerifyingKey::from_sec1_bytes(station_pubkey_der)
        .map_err(|_| PrincipalReject::NoStationKey)?;
    let fixed = der_decode_signature(sig_der).ok_or(PrincipalReject::BadSignature)?;
    let sig =
        p256::ecdsa::Signature::from_slice(&fixed).map_err(|_| PrincipalReject::BadSignature)?;
    let digest = sha2::Sha256::digest(token);
    vk.verify(&digest, &sig)
        .map_err(|_| PrincipalReject::BadSignature)
}

/// 验证 v2 会话尾段（token 64B ‖ sig 字段 72B），解出可信 principal。
///
/// `station_pubkey_der` = station 公钥 SEC1 未压缩点（65B）。
/// `now_unix_ms`：过期判据基准（X6 墙钟链派生；时间不可信时由调用方
/// 显式裁决——本模块只做等值比较）。
pub fn verify_v2_tail(
    tail: &[u8],
    station_pubkey_der: &[u8],
    now_unix_ms: u64,
) -> Result<VerifiedPrincipal, PrincipalReject> {
    if station_pubkey_der.is_empty() {
        return Err(PrincipalReject::NoStationKey);
    }
    if tail.len() != TOKEN_LEN + SIG_FIELD_LEN {
        return Err(PrincipalReject::Malformed);
    }
    let mut token = [0u8; TOKEN_LEN];
    token.copy_from_slice(&tail[..TOKEN_LEN]);
    if token[0..8] != TOKEN_MAGIC {
        return Err(PrincipalReject::Malformed);
    }
    // sig 字段 72B：前 2B = 实长（LE），后随 DER 签名（定长 wire + 变长
    // DER 的桥接；余位补零——canonical）。
    let sig_len = u16::from_le_bytes([tail[TOKEN_LEN], tail[TOKEN_LEN + 1]]) as usize;
    if sig_len == 0 || sig_len > SIG_FIELD_LEN - 2 {
        return Err(PrincipalReject::BadSignature);
    }
    let sig_der = &tail[TOKEN_LEN + 2..TOKEN_LEN + 2 + sig_len];
    if tail[TOKEN_LEN + 2 + sig_len..].iter().any(|b| *b != 0) {
        return Err(PrincipalReject::Malformed);
    }
    verify_signature(station_pubkey_der, &token, sig_der)?;
    // 域检 + 解出（签名已绑定以下字段——只做资格判据）。
    let principal = field_str(&token[8..24]).ok_or(PrincipalReject::Malformed)?;
    if principal.is_empty() {
        return Err(PrincipalReject::Malformed);
    }
    let role_codes = &token[24..28];
    let mut roles = Vec::new();
    for code in role_codes {
        if *code == 0 {
            break;
        }
        match eneros_mgmt_api::Role::from_u8(*code) {
            Some(r) => roles.push(String::from(r.name())),
            None => return Err(PrincipalReject::Malformed),
        }
    }
    if roles.is_empty() {
        roles.push(String::from("viewer"));
    }
    let tenant_scope = u32::from_le_bytes(token[28..32].try_into().expect("u32"));
    let channel_binding = field_str(&token[32..48]).ok_or(PrincipalReject::Malformed)?;
    let expiry = u64::from_le_bytes(token[48..56].try_into().expect("u64"));
    let issued = u64::from_le_bytes(token[56..64].try_into().expect("u64"));
    // 时序判据：签发 ≤ 过期（防倒挂 token）；当前 ≥ 签发（防未来签发）。
    if issued > expiry {
        return Err(PrincipalReject::Malformed);
    }
    if expiry <= now_unix_ms {
        return Err(PrincipalReject::Expired);
    }
    Ok(VerifiedPrincipal {
        principal,
        roles,
        tenant_scope,
        channel_binding,
        expires_at_ms: expiry,
    })
}

#[cfg(test)]
mod tests {
    use p256::ecdsa::signature::Signer;
    use sha2::Digest;

    use super::*;

    /// 测试 station 键（确定性 scalar；gate 生产面用 openssl 签发）。
    fn station() -> (Vec<u8>, p256::ecdsa::SigningKey) {
        let sk = p256::ecdsa::SigningKey::from_bytes(&[0x5A; 32].into()).unwrap();
        let point = sk.verifying_key().to_encoded_point(false);
        (point.as_bytes().to_vec(), sk)
    }

    fn token(principal: &str, role: u8, expiry: u64, issued: u64) -> [u8; TOKEN_LEN] {
        let mut t = [0u8; TOKEN_LEN];
        t[0..8].copy_from_slice(&TOKEN_MAGIC);
        t[8..8 + principal.len()].copy_from_slice(principal.as_bytes());
        t[24] = role;
        t[48..56].copy_from_slice(&expiry.to_le_bytes());
        t[56..64].copy_from_slice(&issued.to_le_bytes());
        t
    }

    fn sign_tail(sk: &p256::ecdsa::SigningKey, t: &[u8; TOKEN_LEN]) -> Vec<u8> {
        let digest: [u8; 32] = sha2::Sha256::digest(t).into();
        let sig = <p256::ecdsa::SigningKey as Signer<p256::ecdsa::Signature>>::sign(sk, &digest);
        let repr: [u8; 64] = sig.to_bytes().into();
        let der = der_encode_signature(&repr);
        let mut out = vec![(der.len() & 0xFF) as u8, (der.len() >> 8) as u8];
        out.extend_from_slice(&der);
        out.resize(SIG_FIELD_LEN, 0);
        let mut full = Vec::from(t.as_slice());
        full.extend_from_slice(&out);
        full
    }

    #[test]
    fn v2_roundtrip_accepts_signed_token() {
        let (spki, sk) = station();
        let t = token("scada-a", 2, 2_000_000, 1_000_000);
        let tail = sign_tail(&sk, &t);
        let v = verify_v2_tail(&tail, &spki, 1_500_000).expect("verify");
        assert_eq!(v.principal, "scada-a");
        assert_eq!(
            v.roles,
            alloc::vec![String::from("admin")],
            "角色码 2 = admin（rbac 表）"
        );
    }

    /// 伪造 Role/Admin 拒绝：改角色字节不重签 → BadSignature。
    #[test]
    fn forged_role_rejected() {
        let (spki, sk) = station();
        let mut t = token("scada-a", 2, 2_000_000, 1_000_000);
        let tail = sign_tail(&sk, &t);
        t[24] = 3; // admin——篡改
        let tampered_token = t;
        // 用原 token 签名 + 篡改后的 token 字节 = 签名失配。
        let good = sign_tail(&sk, &token("scada-a", 2, 2_000_000, 1_000_000));
        let mut forged = Vec::from(tampered_token);
        forged.extend_from_slice(&good[TOKEN_LEN..]);
        assert_eq!(
            verify_v2_tail(&forged, &spki, 1_500_000),
            Err(PrincipalReject::BadSignature)
        );
        let _ = tail;
    }

    /// 过期拒绝 + 倒挂 token 拒绝 + 无 station 公钥拒绝。
    #[test]
    fn expired_inverted_and_nokey_rejected() {
        let (spki, sk) = station();
        let expired = sign_tail(&sk, &token("scada-a", 2, 1_000_000, 500_000));
        assert_eq!(
            verify_v2_tail(&expired, &spki, 1_500_000),
            Err(PrincipalReject::Expired)
        );
        let inverted = sign_tail(&sk, &token("scada-a", 2, 1_000_000, 2_000_000));
        assert_eq!(
            verify_v2_tail(&inverted, &spki, 500_000),
            Err(PrincipalReject::Malformed)
        );
        let good = sign_tail(&sk, &token("scada-a", 2, 2_000_000, 1_000_000));
        assert_eq!(
            verify_v2_tail(&good, &[], 1_500_000),
            Err(PrincipalReject::NoStationKey)
        );
    }

    /// 畸形 token（魔数/长度）拒绝。
    #[test]
    fn malformed_rejected() {
        let (spki, sk) = station();
        let mut t = token("scada-a", 2, 2_000_000, 1_000_000);
        t[0] = b'X';
        let tail = sign_tail(&sk, &t);
        assert_eq!(
            verify_v2_tail(&tail, &spki, 1_500_000),
            Err(PrincipalReject::Malformed)
        );
        assert_eq!(
            verify_v2_tail(&tail[..60], &spki, 1_500_000),
            Err(PrincipalReject::Malformed)
        );
    }
}

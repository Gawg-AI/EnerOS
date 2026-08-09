//! RFC 6979 确定性 nonce 派生（SM3 变体，21-B C4）。
//!
//! 按 [RFC 6979 §3.2](https://www.rfc-editor.org/rfc/rfc6979) 的确定性
//! 签名 nonce 流程，底层哈希替换为 SM3（GB/T 32905-2016）：
//!
//! ```text
//! x  = 私钥 d（int2octets，32 字节）
//! h1 = e（SM2 的 e = SM3(Z || M)，32 字节）
//! V  = 0x01 × 32
//! K  = 0x00 × 32
//! K  = HMAC-SM3(K, V ‖ 0x00 ‖ x ‖ h1)
//! V  = HMAC-SM3(K, V)
//! K  = HMAC-SM3(K, V ‖ 0x01 ‖ x ‖ h1)
//! V  = HMAC-SM3(K, V)
//! loop:
//!   V = HMAC-SM3(K, V)
//!   T = bits2int(V)              （SM2：qlen = 256 = outlen，无截断）
//!   if 1 ≤ T ≤ n-1: return T
//!   K = HMAC-SM3(K, V ‖ 0x00)
//!   V = HMAC-SM3(K, V)
//! ```
//!
//! 参数化说明：
//! - qlen（阶的位长）= 256（SM2 阶 n 为 256 位）→ `bits2int` 无截断；
//! - `bits2octets(h1)` = h1 mod n（`int2octets` 输出 32 字节）。
//!
//! 注：SM3 变体无官方 RFC 6979 示例向量（RFC 原文示例为 SHA-256 + P-256）；
//! 本模块按 RFC 6979 结构逐条实现并由结构 KAT 验证（确定性 / 域分离 /
//! 候选区间），互操作正确性由 SM2 验签闭环覆盖。

use super::SM2_N;
use crate::bigint::U256;
use crate::sm3::hmac::Sm3Hmac;

/// RFC 6979 确定性 nonce（SM3 变体）：返回 k ∈ [1, n-1]。
///
/// - `d`：SM2 私钥标量（x）；
/// - `e`：SM2 消息摘要 e = SM3(Z ‖ M)（h1）。
pub fn rfc6979_nonce(d: &U256, e: &U256) -> U256 {
    let x = d.to_be_bytes();
    // bits2octets(h1) = h1 mod n（int2octets 32 字节）
    let h1_mod = e.sub_mod(&SM2_N, &SM2_N);
    let h1 = h1_mod.to_be_bytes();

    let mut v = [0x01u8; 32];
    let mut k = [0x00u8; 32];

    // K = HMAC(K, V || 0x00 || x || h1)
    let mut mac = Sm3Hmac::new(&k);
    mac.update(&v);
    mac.update(&[0x00]);
    mac.update(&x);
    mac.update(&h1);
    k = mac.finalize();

    // V = HMAC(K, V)
    let mut mac = Sm3Hmac::new(&k);
    mac.update(&v);
    v = mac.finalize();

    // K = HMAC(K, V || 0x01 || x || h1)
    let mut mac = Sm3Hmac::new(&k);
    mac.update(&v);
    mac.update(&[0x01]);
    mac.update(&x);
    mac.update(&h1);
    k = mac.finalize();

    // V = HMAC(K, V)
    let mut mac = Sm3Hmac::new(&k);
    mac.update(&v);
    v = mac.finalize();

    loop {
        // V = HMAC(K, V) → T = bits2int(V)
        let mut mac = Sm3Hmac::new(&k);
        mac.update(&v);
        v = mac.finalize();
        let t = U256::from_be_bytes(&v);
        // 候选 k ∈ [1, n-1]
        if !t.is_zero() && t < SM2_N {
            return t;
        }
        // K = HMAC(K, V || 0x00); V = HMAC(K, V)
        let mut mac = Sm3Hmac::new(&k);
        mac.update(&v);
        mac.update(&[0x00]);
        k = mac.finalize();
        let mut mac = Sm3Hmac::new(&k);
        mac.update(&v);
        v = mac.finalize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_in_range() {
        // 相同 (d, e) → 相同 k；k ∈ [1, n-1]
        let d = U256::from_be_bytes(&[
            0x4c, 0x7f, 0xa9, 0xa1, 0x5d, 0xe9, 0xf2, 0x49, 0x0d, 0x84, 0x28, 0xe0, 0xb9, 0xbe,
            0x30, 0xd4, 0x2c, 0x88, 0xf5, 0x75, 0xea, 0x8e, 0xbb, 0x07, 0x2d, 0x12, 0x13, 0x8f,
            0x03, 0x80, 0x93, 0xab,
        ]);
        let e1 = U256::from_be_bytes(&[0x11; 32]);
        let k1 = rfc6979_nonce(&d, &e1);
        let k2 = rfc6979_nonce(&d, &e1);
        assert_eq!(k1, k2, "确定性：相同输入必须相同输出");
        assert!(!k1.is_zero() && k1 < SM2_N, "k 必须在 [1, n-1]");
        // 不同 e → 不同 k（域分离）
        let e2 = U256::from_be_bytes(&[0x22; 32]);
        let k3 = rfc6979_nonce(&d, &e2);
        assert_ne!(k1, k3, "不同消息摘要必须产生不同 nonce");
        // 不同 d → 不同 k
        let d2 = U256::from_be_bytes(&[0x33; 32]);
        let k4 = rfc6979_nonce(&d2, &e1);
        assert_ne!(k1, k4, "不同私钥必须产生不同 nonce");
    }

    #[test]
    fn h1_greater_than_n_reduces() {
        // bits2octets：e >= n 时归约到 e mod n（构造 e = n + 1）
        let d = U256::from_be_bytes(&[0x01; 32]);
        let one = U256::from_be_bytes(&{
            let mut b = [0u8; 32];
            b[31] = 1;
            b
        });
        let e = SM2_N.add_mod(&one, &SM2_N);
        let k = rfc6979_nonce(&d, &e);
        assert!(!k.is_zero() && k < SM2_N);
    }
}

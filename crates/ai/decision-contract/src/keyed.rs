//! keyed 绑定原语（RFC 2104 HMAC-SHA256）。
//!
//! 消费者（[09]）：
//! - `.36` objective-engine：签名 [`crate::objective`] 规则束的签发/验签；
//! - `.38` certifier：证书 key/HMAC 绑定与 Gateway 验证。
//!
//! 独立实现（不引 hmac crate）：SHA-256 已是本 crate 依赖；RFC 2104 算法
//! 体 < 40 行，用 RFC 4231 官方向量锁定。只提供裸 MAC——密钥生命周期
//! （key ring/回滚）归 .38 certifier（[09] §4.7），本模块无状态。

use sha2::{Digest, Sha256};

/// HMAC-SHA256（RFC 2104）。
///
/// `key` 任意长度：>64B 先散列成 32B（RFC 2104 §2）；空 key 允许
/// （全零 pad）但生产调用方必须拒绝空 key——此处不做策略裁决。
pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        let h = Sha256::digest(key);
        k[..32].copy_from_slice(&h);
    } else {
        k[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }

    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(data);
    let inner = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner);
    outer.finalize().into()
}

/// 常量时间 MAC 比较（防 timing 侧信道；证书/规则束验签唯一比较法）。
pub fn mac_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff = 0u8;
    for i in 0..32 {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 4231 Test Case 1（20B key，"Hi There"）。
    #[test]
    fn rfc4231_case1() {
        let key = [0x0b_u8; 20];
        let mac = hmac_sha256(&key, b"Hi There");
        let expected = [
            0xb0, 0x34, 0x4c, 0x61, 0xd8, 0xdb, 0x38, 0x53, 0x5c, 0xa8, 0xaf, 0xce, 0xaf, 0x0b,
            0xf1, 0x2b, 0x88, 0x1d, 0xc2, 0x00, 0xc9, 0x83, 0x3d, 0xa7, 0x26, 0xe9, 0x37, 0x6c,
            0x2e, 0x32, 0xcf, 0xf7,
        ];
        assert_eq!(mac, expected);
        assert!(mac_eq(&mac, &expected));
    }

    /// RFC 4231 Test Case 2（4B key "Jefe"，拼接消息）。
    #[test]
    fn rfc4231_case2() {
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        let expected = [
            0x5b, 0xdc, 0xc1, 0x46, 0xbf, 0x60, 0x75, 0x4e, 0x6a, 0x04, 0x24, 0x26, 0x08, 0x95,
            0x75, 0xc7, 0x5a, 0x00, 0x3f, 0x08, 0x9d, 0x27, 0x39, 0x83, 0x9d, 0xec, 0x58, 0xb9,
            0x64, 0xec, 0x38, 0x43,
        ];
        assert_eq!(mac, expected);
    }

    /// RFC 4231 Test Case 6（131B key > 块长 → 先散列）。
    #[test]
    fn rfc4231_case6_long_key() {
        let key = [0xaa_u8; 131];
        let data: alloc::vec::Vec<u8> = "Test Using Larger Than Block-Size Key - Hash Key First"
            .bytes()
            .collect();
        let mac = hmac_sha256(&key, &data);
        let expected = [
            0x60, 0xe4, 0x31, 0x59, 0x1e, 0xe0, 0xb6, 0x7f, 0x0d, 0x8a, 0x26, 0xaa, 0xcb, 0xf5,
            0xb7, 0x7f, 0x8e, 0x0b, 0xc6, 0x21, 0x37, 0x28, 0xc5, 0x14, 0x05, 0x46, 0x04, 0x0f,
            0x0e, 0xe3, 0x7f, 0x54,
        ];
        assert_eq!(mac, expected);
    }

    #[test]
    fn mac_neq_detects_single_bit() {
        let a = hmac_sha256(b"k1", b"msg");
        let mut b = a;
        b[0] ^= 0x01;
        assert!(!mac_eq(&a, &b));
    }
}

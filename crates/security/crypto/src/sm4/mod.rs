//! SM4 分组密码算法 (GB/T 32907-2016).
//!
//! 提供 SM4 分组密码算法的纯 Rust 实现，密钥长度和分组长度均为 128 位（16 字节）。
//!
//! # 算法概述
//! SM4 是中国国家密码管理局发布的分组密码算法，适用于数据加密保护。
//! - 密钥长度：128 位（16 字节）
//! - 分组长度：128 位（16 字节）
//! - 轮数：32 轮
//! - 结构：Feistel 变体（非平衡 Feistel 网络）
//!
//! # no_std 合规
//! 仅使用 `core::*`，不依赖 `alloc::*` 或 `std::*`。
//!
//! # 示例
//! ```
//! use eneros_crypto::sm4::Sm4;
//! let key = [0x01u8, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef,
//!            0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54, 0x32, 0x10];
//! let plaintext = key;
//! let cipher = Sm4::new(&key);
//! let ciphertext = cipher.encrypt_block(&plaintext);
//! let decrypted = cipher.decrypt_block(&ciphertext);
//! assert_eq!(decrypted, plaintext);
//! ```
//!
//! # 参考
//! - GB/T 32907-2016 信息安全技术 SM4 分组密码算法

pub mod cbc;
pub mod gcm;

// ============================================================
// Constants
// ============================================================

/// S 盒（256 字节查找表）.
const SBOX: [u8; 256] = [
    0xd6, 0x90, 0xe9, 0xfe, 0xcc, 0xe1, 0x3d, 0xb7, 0x16, 0xb6, 0x14, 0xc2, 0x28, 0xfb, 0x2c, 0x05,
    0x2b, 0x67, 0x9a, 0x76, 0x2a, 0xbe, 0x04, 0xc3, 0xaa, 0x44, 0x13, 0x26, 0x49, 0x86, 0x06, 0x99,
    0x9c, 0x42, 0x50, 0xf4, 0x91, 0xef, 0x98, 0x7a, 0x33, 0x54, 0x0b, 0x43, 0xed, 0xcf, 0xac, 0x62,
    0xe4, 0xb3, 0x1c, 0xa9, 0xc9, 0x08, 0xe8, 0x95, 0x80, 0xdf, 0x94, 0xfa, 0x75, 0x8f, 0x3f, 0xa6,
    0x47, 0x07, 0xa7, 0xfc, 0xf3, 0x73, 0x17, 0xba, 0x83, 0x59, 0x3c, 0x19, 0xe6, 0x85, 0x4f, 0xa8,
    0x68, 0x6b, 0x81, 0xb2, 0x71, 0x64, 0xda, 0x8b, 0xf8, 0xeb, 0x0f, 0x4b, 0x70, 0x56, 0x9d, 0x35,
    0x1e, 0x24, 0x0e, 0x5e, 0x63, 0x58, 0xd1, 0xa2, 0x25, 0x22, 0x7c, 0x3b, 0x01, 0x21, 0x78, 0x87,
    0xd4, 0x00, 0x46, 0x57, 0x9f, 0xd3, 0x27, 0x52, 0x4c, 0x36, 0x02, 0xe7, 0xa0, 0xc4, 0xc8, 0x9e,
    0xea, 0xbf, 0x8a, 0xd2, 0x40, 0xc7, 0x38, 0xb5, 0xa3, 0xf7, 0xf2, 0xce, 0xf9, 0x61, 0x15, 0xa1,
    0xe0, 0xae, 0x5d, 0xa4, 0x9b, 0x34, 0x1a, 0x55, 0xad, 0x93, 0x32, 0x30, 0xf5, 0x8c, 0xb1, 0xe3,
    0x1d, 0xf6, 0xe2, 0x2e, 0x82, 0x66, 0xca, 0x60, 0xc0, 0x29, 0x23, 0xab, 0x0d, 0x53, 0x4e, 0x6f,
    0xd5, 0xdb, 0x37, 0x45, 0xde, 0xfd, 0x8e, 0x2f, 0x03, 0xff, 0x6a, 0x72, 0x6d, 0x6c, 0x5b, 0x51,
    0x8d, 0x1b, 0xaf, 0x92, 0xbb, 0xdd, 0xbc, 0x7f, 0x11, 0xd9, 0x5c, 0x41, 0x1f, 0x10, 0x5a, 0xd8,
    0x0a, 0xc1, 0x31, 0x88, 0xa5, 0xcd, 0x7b, 0xbd, 0x2d, 0x74, 0xd0, 0x12, 0xb8, 0xe5, 0xb4, 0xb0,
    0x89, 0x69, 0x97, 0x4a, 0x0c, 0x96, 0x77, 0x7e, 0x65, 0xb9, 0xf1, 0x09, 0xc5, 0x6e, 0xc6, 0x84,
    0x18, 0xf0, 0x7d, 0xec, 0x3a, 0xdc, 0x4d, 0x20, 0x79, 0xee, 0x5f, 0x3e, 0xd7, 0xcb, 0x39, 0x48,
];

/// 系统参数 FK（4 × 32-bit）.
const FK: [u32; 4] = [0xA3B1BAC6, 0x56AA3350, 0x677D9197, 0xB27022DC];

/// 固定参数 CK（32 × 32-bit）.
const CK: [u32; 32] = [
    0x00070e15, 0x1c232a31, 0x383f464d, 0x545b6269, 0x70777e85, 0x8c939aa1, 0xa8afb6bd, 0xc4cbd2d9,
    0xe0e7eef5, 0xfc030a11, 0x181f262d, 0x343b4249, 0x50575e65, 0x6c737a81, 0x888f969d, 0xa4abb2b9,
    0xc0c7ced5, 0xdce3eaf1, 0xf8ff060d, 0x141b2229, 0x30373e45, 0x4c535a61, 0x686f767d, 0x848b9299,
    0xa0a7aeb5, 0xbcc3cad1, 0xd8dfe6ed, 0xf4fb0209, 0x10171e25, 0x2c333a41, 0x484f565d, 0x646b7279,
];

// ============================================================
// Transformations
// ============================================================

/// 非线性变换 τ：对 32-bit 字的每个字节应用 S 盒.
///
/// τ(a) = [SBOX[a_byte0], SBOX[a_byte1], SBOX[a_byte2], SBOX[a_byte3]]
fn tau(a: u32) -> u32 {
    let b0 = SBOX[(a >> 24) as usize & 0xFF] as u32;
    let b1 = SBOX[(a >> 16) as usize & 0xFF] as u32;
    let b2 = SBOX[(a >> 8) as usize & 0xFF] as u32;
    let b3 = SBOX[a as usize & 0xFF] as u32;
    (b0 << 24) | (b1 << 16) | (b2 << 8) | b3
}

/// 线性变换 L（用于加密）.
///
/// L(b) = b ^ (b <<< 2) ^ (b <<< 10) ^ (b <<< 18) ^ (b <<< 24)
fn l_transform(b: u32) -> u32 {
    b ^ b.rotate_left(2) ^ b.rotate_left(10) ^ b.rotate_left(18) ^ b.rotate_left(24)
}

/// 线性变换 L'（用于密钥扩展）.
///
/// L'(b) = b ^ (b <<< 13) ^ (b <<< 23)
fn l_prime_transform(b: u32) -> u32 {
    b ^ b.rotate_left(13) ^ b.rotate_left(23)
}

/// 合成变换 T = L ∘ τ（用于加密）.
fn t_transform(x: u32) -> u32 {
    l_transform(tau(x))
}

/// 合成变换 T' = L' ∘ τ（用于密钥扩展）.
fn t_prime_transform(x: u32) -> u32 {
    l_prime_transform(tau(x))
}

// ============================================================
// Key Expansion
// ============================================================

/// 将 128-bit 密钥扩展为 32 个轮密钥.
///
/// 1. K[0..4] = FK ^ (key as 4 big-endian u32)
/// 2. K[i+4] = K[i] ^ T'(K[i+1] ^ K[i+2] ^ K[i+3] ^ CK[i])  (i = 0..31)
/// 3. rk[i] = K[i+4]  (i = 0..31)
fn key_expand(key: &[u8; 16]) -> [u32; 32] {
    let mut k = [0u32; 36];
    k[0] = FK[0] ^ u32::from_be_bytes([key[0], key[1], key[2], key[3]]);
    k[1] = FK[1] ^ u32::from_be_bytes([key[4], key[5], key[6], key[7]]);
    k[2] = FK[2] ^ u32::from_be_bytes([key[8], key[9], key[10], key[11]]);
    k[3] = FK[3] ^ u32::from_be_bytes([key[12], key[13], key[14], key[15]]);

    let mut rk = [0u32; 32];
    for i in 0..32 {
        k[i + 4] = k[i] ^ t_prime_transform(k[i + 1] ^ k[i + 2] ^ k[i + 3] ^ CK[i]);
        rk[i] = k[i + 4];
    }
    rk
}

// ============================================================
// Block Encrypt/Decrypt
// ============================================================

/// 使用 32 个轮密钥加密 16-byte 分组.
///
/// 1. X[0..4] = block as 4 big-endian u32
/// 2. X[i+4] = X[i] ^ T(X[i+1] ^ X[i+2] ^ X[i+3] ^ rk[i])  (i = 0..31)
/// 3. 输出 = R(X[32], X[33], X[34], X[35]) = (X[35], X[34], X[33], X[32])
fn encrypt_block(rk: &[u32; 32], block: &[u8; 16]) -> [u8; 16] {
    let mut x = [0u32; 36];
    x[0] = u32::from_be_bytes([block[0], block[1], block[2], block[3]]);
    x[1] = u32::from_be_bytes([block[4], block[5], block[6], block[7]]);
    x[2] = u32::from_be_bytes([block[8], block[9], block[10], block[11]]);
    x[3] = u32::from_be_bytes([block[12], block[13], block[14], block[15]]);

    for i in 0..32 {
        x[i + 4] = x[i] ^ t_transform(x[i + 1] ^ x[i + 2] ^ x[i + 3] ^ rk[i]);
    }

    // 反序变换 R: (X[35], X[34], X[33], X[32])
    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&x[35].to_be_bytes());
    out[4..8].copy_from_slice(&x[34].to_be_bytes());
    out[8..12].copy_from_slice(&x[33].to_be_bytes());
    out[12..16].copy_from_slice(&x[32].to_be_bytes());
    out
}

/// 使用 32 个轮密钥解密 16-byte 分组（使用逆序轮密钥）.
///
/// 解密算法与加密相同，但轮密钥使用逆序 (rk[31], rk[30], ..., rk[0])。
fn decrypt_block(rk: &[u32; 32], block: &[u8; 16]) -> [u8; 16] {
    let mut rk_rev = [0u32; 32];
    for i in 0..32 {
        rk_rev[i] = rk[31 - i];
    }
    encrypt_block(&rk_rev, block)
}

// ============================================================
// Sm4 Struct
// ============================================================

/// SM4 分组密码，预存扩展后的轮密钥.
///
/// 创建时执行密钥扩展（32 轮），后续加密/解密直接使用预存的轮密钥。
///
/// # 示例
/// ```
/// use eneros_crypto::sm4::Sm4;
/// let key = [0u8; 16];
/// let cipher = Sm4::new(&key);
/// let plaintext = [0x42u8; 16];
/// let ciphertext = cipher.encrypt_block(&plaintext);
/// let decrypted = cipher.decrypt_block(&ciphertext);
/// assert_eq!(decrypted, plaintext);
/// ```
pub struct Sm4 {
    /// 32 个轮密钥（预扩展）
    rk: [u32; 32],
}

impl Sm4 {
    /// 创建新的 SM4 密码实例，使用给定的 128-bit 密钥.
    pub fn new(key: &[u8; 16]) -> Self {
        Self {
            rk: key_expand(key),
        }
    }

    /// 加密 16-byte 分组，返回 16-byte 密文.
    pub fn encrypt_block(&self, block: &[u8; 16]) -> [u8; 16] {
        encrypt_block(&self.rk, block)
    }

    /// 解密 16-byte 分组，返回 16-byte 明文.
    pub fn decrypt_block(&self, block: &[u8; 16]) -> [u8; 16] {
        decrypt_block(&self.rk, block)
    }

    /// 获取轮密钥（仅用于测试/调试）.
    #[cfg(test)]
    pub fn round_keys(&self) -> &[u32; 32] {
        &self.rk
    }
}

impl Drop for Sm4 {
    fn drop(&mut self) {
        // 析构时安全清零轮密钥（等价于主密钥，需保护）
        for word in self.rk.iter_mut() {
            // SAFETY: write_volatile 防止编译器优化删除写入
            unsafe {
                core::ptr::write_volatile(word, 0);
            }
        }
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    }
}

// ============================================================
// Unit Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // KAT (Known Answer Test) — GB/T 32907-2016
    // ============================================================

    #[test]
    fn test_sm4_kat() {
        // GB/T 32907-2016 示例:
        // key = 0123456789abcdeffedcba9876543210
        // plaintext = 0123456789abcdeffedcba9876543210
        // ciphertext = 681edf34d206965e86b3e94f536e4246
        let key: [u8; 16] = [
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54,
            0x32, 0x10,
        ];
        let plaintext: [u8; 16] = [
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54,
            0x32, 0x10,
        ];
        let expected: [u8; 16] = [
            0x68, 0x1e, 0xdf, 0x34, 0xd2, 0x06, 0x96, 0x5e, 0x86, 0xb3, 0xe9, 0x4f, 0x53, 0x6e,
            0x42, 0x46,
        ];
        let cipher = Sm4::new(&key);
        let ct = cipher.encrypt_block(&plaintext);
        assert_eq!(ct, expected, "KAT encryption mismatch");
        let pt = cipher.decrypt_block(&ct);
        assert_eq!(pt, plaintext, "KAT decryption mismatch");
    }

    // ============================================================
    // Round-trip tests (encrypt then decrypt = original)
    // ============================================================

    #[test]
    fn test_sm4_round_trip_multiple_keys() {
        // 多组密钥的加密-解密往返测试
        let plaintext: [u8; 16] = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ];
        let keys: [[u8; 16]; 4] = [
            [
                0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54,
                0x32, 0x10,
            ],
            [
                0xff, 0xee, 0xdd, 0xcc, 0xbb, 0xaa, 0x99, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22,
                0x11, 0x00,
            ],
            [
                0xde, 0xad, 0xbe, 0xef, 0xca, 0xfe, 0xba, 0xbe, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc,
                0xde, 0xf0,
            ],
            [
                0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42,
                0x42, 0x42,
            ],
        ];
        for key in &keys {
            let cipher = Sm4::new(key);
            let ct = cipher.encrypt_block(&plaintext);
            let pt = cipher.decrypt_block(&ct);
            assert_eq!(pt, plaintext, "Round-trip failed for key {:?}", key);
        }
    }

    // ============================================================
    // Differential tests
    // ============================================================

    #[test]
    fn test_sm4_different_keys_different_ciphertext() {
        // 相同明文，不同密钥 → 不同密文
        let plaintext: [u8; 16] = [0x42u8; 16];
        let key1: [u8; 16] = [0x01u8; 16];
        let key2: [u8; 16] = [0x02u8; 16];
        let cipher1 = Sm4::new(&key1);
        let cipher2 = Sm4::new(&key2);
        let ct1 = cipher1.encrypt_block(&plaintext);
        let ct2 = cipher2.encrypt_block(&plaintext);
        assert_ne!(
            ct1, ct2,
            "Different keys should produce different ciphertexts"
        );
    }

    #[test]
    fn test_sm4_different_plaintexts_different_ciphertext() {
        // 相同密钥，不同明文 → 不同密文
        let key: [u8; 16] = [0x42u8; 16];
        let pt1: [u8; 16] = [0x01u8; 16];
        let pt2: [u8; 16] = [0x02u8; 16];
        let cipher = Sm4::new(&key);
        let ct1 = cipher.encrypt_block(&pt1);
        let ct2 = cipher.encrypt_block(&pt2);
        assert_ne!(
            ct1, ct2,
            "Different plaintexts should produce different ciphertexts"
        );
    }

    // ============================================================
    // Edge case keys
    // ============================================================

    #[test]
    fn test_sm4_all_zero_key() {
        // 全零密钥的加密-解密往返
        let key: [u8; 16] = [0x00; 16];
        let plaintext: [u8; 16] = [
            0x68, 0x1e, 0xdf, 0x34, 0xd2, 0x06, 0x96, 0x5e, 0x86, 0xb3, 0xe9, 0x4f, 0x53, 0x6e,
            0x42, 0x46,
        ];
        let cipher = Sm4::new(&key);
        let ct = cipher.encrypt_block(&plaintext);
        assert_ne!(ct, plaintext, "Ciphertext should differ from plaintext");
        let pt = cipher.decrypt_block(&ct);
        assert_eq!(pt, plaintext, "Round-trip failed for all-zero key");
    }

    #[test]
    fn test_sm4_all_ff_key() {
        // 全 FF 密钥的加密-解密往返
        let key: [u8; 16] = [0xFF; 16];
        let plaintext: [u8; 16] = [0x00; 16];
        let cipher = Sm4::new(&key);
        let ct = cipher.encrypt_block(&plaintext);
        assert_ne!(ct, plaintext, "Ciphertext should differ from plaintext");
        let pt = cipher.decrypt_block(&ct);
        assert_eq!(pt, plaintext, "Round-trip failed for all-FF key");
    }

    // ============================================================
    // Internal function tests
    // ============================================================

    #[test]
    fn test_tau() {
        // τ 对每个字节独立应用 S 盒
        // SBOX[0] = 0xd6, SBOX[1] = 0x90, SBOX[2] = 0xe9
        // τ(0x00000000) = 0xd6d6d6d6
        assert_eq!(tau(0x00000000), 0xd6d6d6d6);
        // τ(0x01010101) = 0x90909090
        assert_eq!(tau(0x01010101), 0x90909090);
        // τ(0x00010002) = SBOX[0]<<24 | SBOX[1]<<16 | SBOX[0]<<8 | SBOX[2]
        //               = 0xd690d6e9
        assert_eq!(
            tau(0x00010002),
            (SBOX[0] as u32) << 24
                | (SBOX[1] as u32) << 16
                | (SBOX[0] as u32) << 8
                | SBOX[2] as u32
        );
        // 确定性：相同输入 → 相同输出
        assert_eq!(tau(0x12345678), tau(0x12345678));
    }

    #[test]
    fn test_l_transform() {
        // L(0) = 0（零的所有循环移位仍为零，异或结果为零）
        assert_eq!(l_transform(0), 0);
        // 确定性
        let x = 0x12345678u32;
        assert_eq!(l_transform(x), l_transform(x));
        // 验证公式：L(x) = x ^ (x<<<2) ^ (x<<<10) ^ (x<<<18) ^ (x<<<24)
        assert_eq!(
            l_transform(x),
            x ^ x.rotate_left(2) ^ x.rotate_left(10) ^ x.rotate_left(18) ^ x.rotate_left(24)
        );
    }

    #[test]
    fn test_l_prime_transform() {
        // L'(0) = 0
        assert_eq!(l_prime_transform(0), 0);
        // 确定性
        let x = 0x8AD24122u32;
        assert_eq!(l_prime_transform(x), l_prime_transform(x));
        // 验证公式：L'(x) = x ^ (x<<<13) ^ (x<<<23)
        assert_eq!(
            l_prime_transform(x),
            x ^ x.rotate_left(13) ^ x.rotate_left(23)
        );
    }

    #[test]
    fn test_t_transform() {
        // T = L ∘ τ，即 T(x) = L(τ(x))
        let x = 0x12345678u32;
        assert_eq!(t_transform(x), l_transform(tau(x)));
        // 确定性
        assert_eq!(t_transform(x), t_transform(x));
    }

    #[test]
    fn test_t_prime_transform() {
        // T' = L' ∘ τ，即 T'(x) = L'(τ(x))
        let x = 0x8283CB69u32;
        assert_eq!(t_prime_transform(x), l_prime_transform(tau(x)));
        // 确定性
        assert_eq!(t_prime_transform(x), t_prime_transform(x));
    }

    // ============================================================
    // Key expansion tests
    // ============================================================

    #[test]
    fn test_key_expansion_produces_32_round_keys() {
        // 密钥扩展产生 32 个轮密钥
        let key: [u8; 16] = [
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54,
            0x32, 0x10,
        ];
        let cipher = Sm4::new(&key);
        let rk = cipher.round_keys();
        // 轮密钥数组长度为 32（类型系统保证，但显式断言）
        assert_eq!(rk.len(), 32);
        // 所有轮密钥非零（对于 KAT 密钥）
        for (i, &k) in rk.iter().enumerate() {
            assert_ne!(k, 0, "Round key {} should be non-zero for KAT key", i);
        }
        // 不同密钥产生不同轮密钥
        let key2: [u8; 16] = [0x00; 16];
        let cipher2 = Sm4::new(&key2);
        assert_ne!(cipher.round_keys(), cipher2.round_keys());
    }

    #[test]
    fn test_key_expansion_rk0_kat() {
        // 验证 KAT 密钥的第一个轮密钥 rk[0] = K[4] = 0xF12186F9
        // 这是 GB/T 32907-2016 示例中已知的轮密钥值
        let key: [u8; 16] = [
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54,
            0x32, 0x10,
        ];
        let cipher = Sm4::new(&key);
        assert_eq!(
            cipher.round_keys()[0],
            0xF12186F9,
            "RK[0] for KAT key should be 0xF12186F9"
        );
    }

    // ============================================================
    // Decrypt is inverse of encrypt (multiple blocks)
    // ============================================================

    #[test]
    fn test_decrypt_inverse_encrypt_multiple_blocks() {
        // 多个不同分组的加密-解密逆运算测试
        let key: [u8; 16] = [
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54,
            0x32, 0x10,
        ];
        let cipher = Sm4::new(&key);

        // 6 组不同的测试分组
        let blocks: [[u8; 16]; 6] = [
            [0x00; 16],
            [0xFF; 16],
            [
                0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54,
                0x32, 0x10,
            ],
            [
                0x68, 0x1e, 0xdf, 0x34, 0xd2, 0x06, 0x96, 0x5e, 0x86, 0xb3, 0xe9, 0x4f, 0x53, 0x6e,
                0x42, 0x46,
            ],
            [
                0xaa, 0x55, 0xaa, 0x55, 0xaa, 0x55, 0xaa, 0x55, 0x55, 0xaa, 0x55, 0xaa, 0x55, 0xaa,
                0x55, 0xaa,
            ],
            [
                0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x0f, 0xed, 0xcb, 0xa9, 0x87, 0x65,
                0x43, 0x21,
            ],
        ];

        for (i, block) in blocks.iter().enumerate() {
            let ct = cipher.encrypt_block(block);
            assert_ne!(*block, ct, "Block {} should change after encryption", i);
            let pt = cipher.decrypt_block(&ct);
            assert_eq!(*block, pt, "Block {} round-trip failed", i);
        }
    }
}

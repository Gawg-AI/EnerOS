//! SM3 密码杂凑算法 (GB/T 32905-2016).
//!
//! 提供 SM3 杂凑算法的纯 Rust 实现，输出 256 位（32 字节）摘要。
//!
//! # 算法概述
//! SM3 是中国国家密码管理局发布的密码杂凑算法，适用于数字签名、消息认证码等场景。
//! - 输入：任意长度消息
//! - 输出：256 位（32 字节）杂凑值
//! - 分组长度：512 位（64 字节）
//! - 压缩函数：64 轮迭代
//!
//! # no_std 合规
//! 仅使用 `core::*`，不依赖 `alloc::*` 或 `std::*`。
//!
//! # 示例
//! ```
//! use eneros_crypto::sm3::hash;
//! let digest = hash(b"abc");
//! assert_eq!(digest.len(), 32);
//! ```
//!
//! # 参考
//! - GB/T 32905-2016 信息安全技术 SM3 密码杂凑算法

pub mod hmac;

// ============================================================
// Constants
// ============================================================

/// 初始向量 IV（256-bit，8 × 32-bit 字）.
const IV: [u32; 8] = [
    0x7380166F, 0x4914B2B9, 0x172442D7, 0xDA8A0600, 0xA96F30BC, 0x163138AA, 0xE38DEE4D, 0xB0FB0E4E,
];

// ============================================================
// Helper functions
// ============================================================

/// 常量 T(j)：j < 16 时为 0x79CC4519，j >= 16 时为 0x7A879D8A.
fn t_j(j: usize) -> u32 {
    if j < 16 {
        0x79CC4519
    } else {
        0x7A879D8A
    }
}

/// 布尔函数 FF_j(x, y, z).
///
/// - j < 16: FF = x ^ y ^ z
/// - j >= 16: FF = (x & y) | (x & z) | (y & z)
fn ff_j(x: u32, y: u32, z: u32, j: usize) -> u32 {
    if j < 16 {
        x ^ y ^ z
    } else {
        (x & y) | (x & z) | (y & z)
    }
}

/// 布尔函数 GG_j(x, y, z).
///
/// - j < 16: GG = x ^ y ^ z
/// - j >= 16: GG = (x & y) | ((!x) & z)
fn gg_j(x: u32, y: u32, z: u32, j: usize) -> u32 {
    if j < 16 {
        x ^ y ^ z
    } else {
        (x & y) | ((!x) & z)
    }
}

/// 置换函数 P0(x) = x ^ (x <<< 9) ^ (x <<< 17).
fn p0(x: u32) -> u32 {
    x ^ x.rotate_left(9) ^ x.rotate_left(17)
}

/// 置换函数 P1(x) = x ^ (x <<< 15) ^ (x <<< 23).
fn p1(x: u32) -> u32 {
    x ^ x.rotate_left(15) ^ x.rotate_left(23)
}

// ============================================================
// Message Expansion
// ============================================================

/// 消息扩展：将 512-bit 分组扩展为 W[0..68] 和 W'[0..64].
///
/// - W[0..16]：分组的大端 u32 表示
/// - W[16..68]：W[j] = P1(W[j-16] ^ W[j-9] ^ (W[j-3] <<< 15)) ^ (W[j-13] <<< 7) ^ W[j-6]
/// - W'[j] = W[j] ^ W[j+4] (j = 0..63)
fn message_expand(block: &[u8; 64]) -> ([u32; 68], [u32; 64]) {
    let mut w = [0u32; 68];
    // W[0..16] = block as 16 big-endian u32
    for i in 0..16 {
        w[i] = u32::from_be_bytes([
            block[i * 4],
            block[i * 4 + 1],
            block[i * 4 + 2],
            block[i * 4 + 3],
        ]);
    }
    // W[16..68]: W[j] = P1(W[j-16] ^ W[j-9] ^ (W[j-3] <<< 15)) ^ (W[j-13] <<< 7) ^ W[j-6]
    for j in 16..68 {
        let tmp = w[j - 16] ^ w[j - 9] ^ w[j - 3].rotate_left(15);
        w[j] = p1(tmp) ^ w[j - 13].rotate_left(7) ^ w[j - 6];
    }
    // W'[j] = W[j] ^ W[j+4]
    let mut w_prime = [0u32; 64];
    for j in 0..64 {
        w_prime[j] = w[j] ^ w[j + 4];
    }
    (w, w_prime)
}

// ============================================================
// Compression Function CF
// ============================================================

/// 压缩函数 CF：将 512-bit 分组压缩到 256-bit 状态 V 中.
///
/// 对 64 轮迭代：
/// - SS1 = ((A <<< 12) + E + (T(j) <<< (j mod 32))) <<< 7
/// - SS2 = SS1 ^ (A <<< 12)
/// - TT1 = FF_j(A,B,C) + D + SS2 + W'[j]
/// - TT2 = GG_j(E,F,G) + H + SS1 + W[j]
/// - 状态更新：D=C, C=B<<<9, B=A, A=TT1, H=G, G=F<<<19, F=E, E=P0(TT2)
///
/// 最终 V(i+1) = {A,B,C,D,E,F,G,H} ^ V(i)
fn compress(v: &mut [u32; 8], block: &[u8; 64]) {
    let (w, w_prime) = message_expand(block);
    let mut a = v[0];
    let mut b = v[1];
    let mut c = v[2];
    let mut d = v[3];
    let mut e = v[4];
    let mut f = v[5];
    let mut g = v[6];
    let mut h = v[7];

    for j in 0..64 {
        let ss1 = a
            .rotate_left(12)
            .wrapping_add(e)
            .wrapping_add(t_j(j).rotate_left((j as u32) % 32))
            .rotate_left(7);
        let ss2 = ss1 ^ a.rotate_left(12);
        let tt1 = ff_j(a, b, c, j)
            .wrapping_add(d)
            .wrapping_add(ss2)
            .wrapping_add(w_prime[j]);
        let tt2 = gg_j(e, f, g, j)
            .wrapping_add(h)
            .wrapping_add(ss1)
            .wrapping_add(w[j]);
        d = c;
        c = b.rotate_left(9);
        b = a;
        a = tt1;
        h = g;
        g = f.rotate_left(19);
        f = e;
        e = p0(tt2);
    }

    v[0] ^= a;
    v[1] ^= b;
    v[2] ^= c;
    v[3] ^= d;
    v[4] ^= e;
    v[5] ^= f;
    v[6] ^= g;
    v[7] ^= h;
}

// ============================================================
// Sm3Hasher
// ============================================================

/// SM3 杂凑算法状态.
///
/// 支持流式更新（`update`）和一次性计算（`hash`）。
///
/// # 示例
/// ```
/// use eneros_crypto::sm3::Sm3Hasher;
/// let mut hasher = Sm3Hasher::new();
/// hasher.update(b"abc");
/// let digest = hasher.finalize();
/// assert_eq!(digest.len(), 32);
/// ```
pub struct Sm3Hasher {
    /// 中间杂凑值 V（256-bit，8 × 32-bit）
    state: [u32; 8],
    /// 部分分组缓冲区（最多 63 字节未处理）
    buffer: [u8; 64],
    /// 缓冲区中有效字节数
    buffer_len: usize,
    /// 已处理的消息总字节数
    total_len: u64,
}

impl Sm3Hasher {
    /// 创建新的 SM3 状态，初始状态为 IV.
    pub fn new() -> Self {
        Self {
            state: IV,
            buffer: [0u8; 64],
            buffer_len: 0,
            total_len: 0,
        }
    }

    /// 更新杂凑状态，追加消息数据.
    ///
    /// 可多次调用以处理流式数据。内部自动处理分组边界。
    pub fn update(&mut self, data: &[u8]) {
        self.total_len = self.total_len.wrapping_add(data.len() as u64);
        let mut offset = 0;
        // 先处理缓冲区中的部分分组
        if self.buffer_len > 0 {
            let need = 64 - self.buffer_len;
            if data.len() < need {
                self.buffer[self.buffer_len..self.buffer_len + data.len()].copy_from_slice(data);
                self.buffer_len += data.len();
                return;
            }
            self.buffer[self.buffer_len..64].copy_from_slice(&data[..need]);
            let mut block = [0u8; 64];
            block.copy_from_slice(&self.buffer);
            compress(&mut self.state, &block);
            offset += need;
            self.buffer_len = 0;
        }
        // 处理完整的 64 字节分组
        while offset + 64 <= data.len() {
            let mut block = [0u8; 64];
            block.copy_from_slice(&data[offset..offset + 64]);
            compress(&mut self.state, &block);
            offset += 64;
        }
        // 剩余字节存入缓冲区
        if offset < data.len() {
            self.buffer[..data.len() - offset].copy_from_slice(&data[offset..]);
            self.buffer_len = data.len() - offset;
        }
    }

    /// 完成杂凑计算，返回 256-bit（32 字节）摘要.
    ///
    /// 执行 SM3 填充（追加 0x80、零字节、64-bit 大端比特长度），处理后输出最终杂凑值。
    pub fn finalize(mut self) -> [u8; 32] {
        let bit_len = self.total_len.wrapping_mul(8);
        // 追加 0x80
        self.buffer[self.buffer_len] = 0x80;
        self.buffer_len += 1;
        // 若缓冲区剩余空间不足以容纳 8 字节长度，填零并压缩当前块
        if self.buffer_len > 56 {
            self.buffer[self.buffer_len..64].fill(0);
            let mut block = [0u8; 64];
            block.copy_from_slice(&self.buffer);
            compress(&mut self.state, &block);
            self.buffer_len = 0;
        }
        // 填充零直至第 56 字节
        self.buffer[self.buffer_len..56].fill(0);
        // 追加 64-bit 大端比特长度
        self.buffer[56..64].copy_from_slice(&bit_len.to_be_bytes());
        let mut block = [0u8; 64];
        block.copy_from_slice(&self.buffer);
        compress(&mut self.state, &block);
        // 输出大端 32 字节摘要
        let mut out = [0u8; 32];
        for (i, &word) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }
}

impl Default for Sm3Hasher {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// Convenience function
// ============================================================

/// 一次性计算 SM3 杂凑值.
///
/// # 示例
/// ```
/// use eneros_crypto::sm3::hash;
/// let digest = hash(b"abc");
/// assert_eq!(digest.len(), 32);
/// ```
pub fn hash(data: &[u8]) -> [u8; 32] {
    let mut h = Sm3Hasher::new();
    h.update(data);
    h.finalize()
}

// ============================================================
// Unit Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // KAT (Known Answer Tests) — GB/T 32905-2016
    // ============================================================

    #[test]
    fn test_sm3_kat_abc() {
        // GB/T 32905-2016 示例 1: SM3("abc")
        let result = hash(b"abc");
        let expected: [u8; 32] = [
            0x66, 0xC7, 0xF0, 0xF4, 0x62, 0xEE, 0xED, 0xD9, 0xD1, 0xF2, 0xD4, 0x6B, 0xDC, 0x10,
            0xE4, 0xE2, 0x41, 0x67, 0xC4, 0x87, 0x5C, 0xF2, 0xF7, 0xA2, 0x29, 0x7D, 0xA0, 0x2B,
            0x8F, 0x4B, 0xA8, 0xE0,
        ];
        assert_eq!(result, expected);
    }

    #[test]
    fn test_sm3_kat_64_bytes() {
        // GB/T 32905-2016 示例 2: 64-byte message ("abcd" repeated 16 times)
        let msg = b"abcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcd";
        let result = hash(msg);
        let expected: [u8; 32] = [
            0xDE, 0xBE, 0x9F, 0xF9, 0x22, 0x75, 0xB8, 0xA1, 0x38, 0x60, 0x48, 0x89, 0xC1, 0x8E,
            0x5A, 0x4D, 0x6F, 0xDB, 0x70, 0xE5, 0x38, 0x7E, 0x57, 0x65, 0x29, 0x3D, 0xCB, 0xA3,
            0x9C, 0x0C, 0x57, 0x32,
        ];
        assert_eq!(result, expected);
    }

    // ============================================================
    // Edge case: empty and single-byte messages
    // ============================================================

    #[test]
    fn test_sm3_empty() {
        // SM3("") — 空消息
        let result = hash(b"");
        let expected: [u8; 32] = [
            0x1A, 0xB2, 0x1D, 0x83, 0x55, 0xCF, 0xA1, 0x7F, 0x8E, 0x61, 0x19, 0x48, 0x31, 0xE8,
            0x1A, 0x8F, 0x22, 0xBE, 0xC8, 0xC7, 0x28, 0xFE, 0xFB, 0x74, 0x7E, 0xD0, 0x35, 0xEB,
            0x50, 0x82, 0xAA, 0x2B,
        ];
        assert_eq!(result, expected);
    }

    #[test]
    fn test_sm3_single_byte() {
        // SM3("a") — 单字节消息，流式 vs 一次性一致性
        let one_shot = hash(b"a");
        let mut hasher = Sm3Hasher::new();
        hasher.update(b"a");
        let streamed = hasher.finalize();
        assert_eq!(one_shot, streamed);
        // 与空消息不同
        assert_ne!(one_shot, hash(b""));
        // 与 "abc" 不同
        assert_ne!(one_shot, hash(b"abc"));
    }

    // ============================================================
    // Streaming tests
    // ============================================================

    #[test]
    fn test_sm3_streaming_abc() {
        // 将 "abc" 拆分为 3 次调用，结果应与一次性一致
        let one_shot = hash(b"abc");
        let mut hasher = Sm3Hasher::new();
        hasher.update(b"a");
        hasher.update(b"b");
        hasher.update(b"c");
        let streamed = hasher.finalize();
        assert_eq!(streamed, one_shot);
    }

    #[test]
    fn test_sm3_streaming_byte_by_byte() {
        // 逐字节流式更新 "abc"
        let one_shot = hash(b"abc");
        let mut hasher = Sm3Hasher::new();
        for byte in b"abc" {
            hasher.update(core::slice::from_ref(byte));
        }
        let streamed = hasher.finalize();
        assert_eq!(streamed, one_shot);
    }

    #[test]
    fn test_sm3_streaming_partial_57() {
        // 57 字节消息（跨分组边界：57 字节不构成完整分组）
        let msg = [0x42u8; 57];
        let one_shot = hash(&msg);
        let mut hasher = Sm3Hasher::new();
        hasher.update(&msg[..32]);
        hasher.update(&msg[32..]);
        let streamed = hasher.finalize();
        assert_eq!(streamed, one_shot);
    }

    #[test]
    fn test_sm3_streaming_partial_64() {
        // 64 字节消息（恰好一个完整分组）
        let msg = [0x55u8; 64];
        let one_shot = hash(&msg);
        let mut hasher = Sm3Hasher::new();
        hasher.update(&msg[..30]);
        hasher.update(&msg[30..]);
        let streamed = hasher.finalize();
        assert_eq!(streamed, one_shot);
    }

    #[test]
    fn test_sm3_streaming_partial_65() {
        // 65 字节消息（一个完整分组 + 1 字节余量）
        let msg = [0x33u8; 65];
        let one_shot = hash(&msg);
        let mut hasher = Sm3Hasher::new();
        hasher.update(&msg[..40]);
        hasher.update(&msg[40..]);
        let streamed = hasher.finalize();
        assert_eq!(streamed, one_shot);
    }

    // ============================================================
    // Padding boundary edge cases
    // ============================================================

    #[test]
    fn test_sm3_exactly_55_bytes() {
        // 55 字节：0x80 + 55 字节数据 = 56 字节，恰好可放入 8 字节长度（单块填充）
        let msg = [0xAAu8; 55];
        let one_shot = hash(&msg);
        // 流式一致性
        let mut hasher = Sm3Hasher::new();
        hasher.update(&msg[..20]);
        hasher.update(&msg[20..]);
        let streamed = hasher.finalize();
        assert_eq!(streamed, one_shot);
    }

    #[test]
    fn test_sm3_exactly_56_bytes() {
        // 56 字节：0x80 后剩余 7 字节，不足以容纳 8 字节长度（需双块填充）
        let msg = [0xBBu8; 56];
        let one_shot = hash(&msg);
        // 流式一致性
        let mut hasher = Sm3Hasher::new();
        hasher.update(&msg[..28]);
        hasher.update(&msg[28..]);
        let streamed = hasher.finalize();
        assert_eq!(streamed, one_shot);
    }

    #[test]
    fn test_sm3_exactly_63_bytes() {
        // 63 字节：0x80 后缓冲区满（64 字节），需双块填充
        let msg = [0xCCu8; 63];
        let one_shot = hash(&msg);
        // 流式一致性
        let mut hasher = Sm3Hasher::new();
        hasher.update(&msg[..31]);
        hasher.update(&msg[31..]);
        let streamed = hasher.finalize();
        assert_eq!(streamed, one_shot);
    }

    // ============================================================
    // Long message
    // ============================================================

    #[test]
    fn test_sm3_long_message_1000() {
        // 1000 字节消息（15+ 个分组）
        let msg = [0x77u8; 1000];
        let one_shot = hash(&msg);
        // 流式一致性：分多段更新
        let mut hasher = Sm3Hasher::new();
        let mut offset = 0;
        while offset < msg.len() {
            let end = core::cmp::min(offset + 37, msg.len()); // 非对齐步长
            hasher.update(&msg[offset..end]);
            offset = end;
        }
        let streamed = hasher.finalize();
        assert_eq!(streamed, one_shot);
    }

    // ============================================================
    // Determinism and Default
    // ============================================================

    #[test]
    fn test_sm3_deterministic() {
        // 相同输入产生相同输出
        let h1 = hash(b"hello world");
        let h2 = hash(b"hello world");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_sm3_default_equals_new() {
        // Sm3Hasher::default() == Sm3Hasher::new()
        let h1 = Sm3Hasher::new();
        let h2 = Sm3Hasher::default();
        // 通过相同输入验证状态一致
        let msg = b"test message for default";
        let mut h1 = h1;
        h1.update(msg);
        let r1 = h1.finalize();
        let mut h2 = h2;
        h2.update(msg);
        let r2 = h2.finalize();
        assert_eq!(r1, r2);
    }

    #[test]
    fn test_sm3_empty_update() {
        // update 空切片不应改变状态
        let one_shot = hash(b"abc");
        let mut hasher = Sm3Hasher::new();
        hasher.update(b"");
        hasher.update(b"abc");
        hasher.update(b"");
        let streamed = hasher.finalize();
        assert_eq!(streamed, one_shot);
    }

    // ============================================================
    // Internal function tests
    // ============================================================

    #[test]
    fn test_iv_constant() {
        // 验证初始向量 IV 与 GB/T 32905-2016 一致
        assert_eq!(IV[0], 0x7380166F);
        assert_eq!(IV[1], 0x4914B2B9);
        assert_eq!(IV[2], 0x172442D7);
        assert_eq!(IV[3], 0xDA8A0600);
        assert_eq!(IV[4], 0xA96F30BC);
        assert_eq!(IV[5], 0x163138AA);
        assert_eq!(IV[6], 0xE38DEE4D);
        assert_eq!(IV[7], 0xB0FB0E4E);
    }

    #[test]
    fn test_t_j_constant() {
        // T(j): j < 16 → 0x79CC4519, j >= 16 → 0x7A879D8A
        for j in 0..16 {
            assert_eq!(t_j(j), 0x79CC4519, "t_j({}) should be 0x79CC4519", j);
        }
        for j in 16..64 {
            assert_eq!(t_j(j), 0x7A879D8A, "t_j({}) should be 0x7A879D8A", j);
        }
    }

    #[test]
    fn test_ff_j_gg_j() {
        let x = 0xF0F0F0F0u32;
        let y = 0x0F0F0F0Fu32;
        let z = 0x33333333u32;
        // FF_j: j < 16 → x ^ y ^ z
        assert_eq!(ff_j(x, y, z, 0), x ^ y ^ z);
        assert_eq!(ff_j(x, y, z, 15), x ^ y ^ z);
        // FF_j: j >= 16 → (x & y) | (x & z) | (y & z)
        assert_eq!(ff_j(x, y, z, 16), (x & y) | (x & z) | (y & z));
        // GG_j: j < 16 → x ^ y ^ z
        assert_eq!(gg_j(x, y, z, 0), x ^ y ^ z);
        // GG_j: j >= 16 → (x & y) | ((!x) & z)
        assert_eq!(gg_j(x, y, z, 16), (x & y) | (!x & z));
    }

    #[test]
    fn test_p0_p1() {
        // P0(x) = x ^ (x <<< 9) ^ (x <<< 17)
        let x = 0x12345678u32;
        assert_eq!(p0(x), x ^ x.rotate_left(9) ^ x.rotate_left(17));
        // P1(x) = x ^ (x <<< 15) ^ (x <<< 23)
        assert_eq!(p1(x), x ^ x.rotate_left(15) ^ x.rotate_left(23));
    }

    #[test]
    fn test_message_expand_basic() {
        // 构造 "abc" 填充后的第一个分组
        let mut block = [0u8; 64];
        block[0] = b'a';
        block[1] = b'b';
        block[2] = b'c';
        block[3] = 0x80;
        // 末尾 8 字节为比特长度 = 24 (0x18)
        block[63] = 0x18;

        let (w, w_prime) = message_expand(&block);

        // W[0] = "abc" + 0x80 的大端 u32
        assert_eq!(w[0], 0x61626380);
        // W[1..14] = 0
        for (i, &val) in w.iter().enumerate().take(14).skip(1) {
            assert_eq!(val, 0, "W[{}] should be 0", i);
        }
        // W[15] = 0x18 (比特长度)
        assert_eq!(w[15], 0x00000018);

        // W'[j] = W[j] ^ W[j+4]
        for j in 0..64 {
            assert_eq!(w_prime[j], w[j] ^ w[j + 4], "W'[{}] mismatch", j);
        }
    }

    #[test]
    fn test_compress_with_iv() {
        // 压缩函数一致性：对同一分组压缩两次应得到相同结果
        let block = [0x42u8; 64];
        let mut v1 = IV;
        compress(&mut v1, &block);
        let mut v2 = IV;
        compress(&mut v2, &block);
        assert_eq!(v1, v2);
        // 压缩后状态应与 IV 不同
        assert_ne!(v1, IV);
    }
}

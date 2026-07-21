//! 基于 SM3 的密码学安全随机数生成器 (CSRNG).
//!
//! 设计参考 NIST SP 800-90A Hash DRBG，将底层哈希函数替换为 SM3 (GB/T 32905-2016)。
//!
//! # 状态
//! - `V`：256-bit（32 字节）内部状态
//! - `counter`：64-bit 计数器，用于每次输出的额外混合
//!
//! # 输出生成
//! 对每个 32 字节块：
//! 1. 构造输入 `input = V || counter`（40 字节）
//! 2. 计算 `V = SM3(input)`
//! 3. 输出 `V` 作为随机字节
//! 4. `counter += 1`
//!
//! # 重播种
//! `reseed(seed)`：`V = SM3(V || seed || counter)`，然后 `counter += 1`。
//!
//! # 安全警告
//! **WARNING**: `CsRng::new()` 使用固定种子，仅用于 no_std 测试环境。
//! 生产环境必须使用 `CsRng::from_seed` 并传入硬件 TRNG 采集的熵。

use alloc::vec::Vec;

use crate::constant_time::ct_zeroize;
use crate::sm3::hash as sm3_hash;

/// 基于 SM3 Hash DRBG 的密码学安全随机数生成器.
///
/// 状态由 256-bit `V` 和 64-bit `counter` 组成。输出通过反复哈希状态产生。
///
/// **WARNING**: 默认构造函数 `CsRng::new()` 使用固定种子，仅用于测试。
/// 生产环境必须使用 `CsRng::from_seed` 传入硬件 TRNG 熵源。
pub struct CsRng {
    /// 内部状态 V（256-bit = 32 字节）.
    v: [u8; 32],
    /// 用于额外混合的计数器.
    counter: u64,
}

impl CsRng {
    /// 使用固定种子创建 CSRNG.
    ///
    /// **WARNING**: 仅用于测试。种子是确定性的，相同实例产生相同输出序列。
    /// 生产环境必须使用 `CsRng::from_seed` 传入硬件 TRNG 采集的熵。
    pub fn new() -> Self {
        // 固定种子 — 切勿用于生产
        let seed = [
            0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x30, 0x31, 0x32, 0x33, 0x34,
            0x35, 0x36, 0x37, 0x38, 0x39, 0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38,
            0x39, 0x30, 0x31, 0x32,
        ];
        Self::from_seed(&seed)
    }

    /// 使用显式 32 字节种子创建 CSRNG.
    ///
    /// 生产环境中，种子应来自硬件 TRNG。
    /// 内部状态初始化为 `V = SM3(seed)`，计数器初始化为 0。
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        let v = sm3_hash(seed);
        Self { v, counter: 0 }
    }

    /// 用随机字节填充缓冲区.
    ///
    /// 对每个 32 字节块：`V = SM3(V || counter)`，输出 `V`，`counter += 1`。
    /// 若缓冲区长度不是 32 的倍数，最后一个块的部分字节被截取使用。
    pub fn fill_bytes(&mut self, buf: &mut [u8]) {
        let mut offset = 0;
        while offset < buf.len() {
            // 构造输入：V (32) || counter (8) = 40 字节
            let mut input = [0u8; 40];
            input[..32].copy_from_slice(&self.v);
            input[32..].copy_from_slice(&self.counter.to_be_bytes());

            // V = SM3(V || counter)
            let next_v = sm3_hash(&input);
            self.v = next_v;
            self.counter = self.counter.wrapping_add(1);

            // 拷贝到输出缓冲区
            let remaining = buf.len() - offset;
            let to_copy = if remaining < 32 { remaining } else { 32 };
            buf[offset..offset + to_copy].copy_from_slice(&next_v[..to_copy]);
            offset += to_copy;
        }
    }

    /// 生成一个随机 `u32`（大端字节序解析）.
    pub fn next_u32(&mut self) -> u32 {
        let mut buf = [0u8; 4];
        self.fill_bytes(&mut buf);
        u32::from_be_bytes(buf)
    }

    /// 生成一个随机 `u64`（大端字节序解析）.
    pub fn next_u64(&mut self) -> u64 {
        let mut buf = [0u8; 8];
        self.fill_bytes(&mut buf);
        u64::from_be_bytes(buf)
    }

    /// 用新熵重播种 CSRNG.
    ///
    /// 更新公式：`V = SM3(old_V || seed || counter)`，然后 `counter += 1`。
    /// 重种后输出序列将发生改变。
    pub fn reseed(&mut self, seed: &[u8]) {
        let mut input = Vec::with_capacity(32 + seed.len() + 8);
        input.extend_from_slice(&self.v);
        input.extend_from_slice(seed);
        input.extend_from_slice(&self.counter.to_be_bytes());
        self.v = sm3_hash(&input);
        self.counter = self.counter.wrapping_add(1);
    }
}

impl Default for CsRng {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for CsRng {
    fn drop(&mut self) {
        // 恒定时间清零敏感状态
        ct_zeroize(&mut self.v);
        // counter 是 u64 标量，使用 volatile 写入清零以防编译器优化删除
        // SAFETY: write_volatile 写入栈上变量的可变引用，不会触发 UB
        unsafe {
            core::ptr::write_volatile(&mut self.counter, 0);
        }
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    }
}

// ============================================================
// Unit Tests
// ============================================================

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn test_csrng_deterministic_same_seed() {
        // 相同种子应产生相同的输出序列
        let seed = [0xAAu8; 32];
        let mut rng1 = CsRng::from_seed(&seed);
        let mut rng2 = CsRng::from_seed(&seed);

        let mut buf1 = [0u8; 64];
        let mut buf2 = [0u8; 64];
        rng1.fill_bytes(&mut buf1);
        rng2.fill_bytes(&mut buf2);
        assert_eq!(buf1, buf2, "same seed must yield same output sequence");

        // 第二次生成也应一致
        rng1.fill_bytes(&mut buf1);
        rng2.fill_bytes(&mut buf2);
        assert_eq!(buf1, buf2, "second block must also match");
    }

    #[test]
    fn test_csrng_different_seed_different_output() {
        let seed1 = [0x11u8; 32];
        let seed2 = [0x22u8; 32];
        let mut rng1 = CsRng::from_seed(&seed1);
        let mut rng2 = CsRng::from_seed(&seed2);

        let mut buf1 = [0u8; 32];
        let mut buf2 = [0u8; 32];
        rng1.fill_bytes(&mut buf1);
        rng2.fill_bytes(&mut buf2);
        assert_ne!(buf1, buf2, "different seeds must yield different outputs");
    }

    #[test]
    fn test_csrng_fill_bytes_small() {
        let mut rng = CsRng::new();
        for size in [1usize, 4, 16, 31] {
            let mut buf = vec![0xFFu8; size];
            rng.fill_bytes(&mut buf);
            // 验证非全 0xFF（应该已被覆盖）
            assert!(
                buf.iter().any(|&b| b != 0xFF),
                "size {} buffer should be overwritten",
                size
            );
            // 验证非全零
            assert!(
                buf.iter().any(|&b| b != 0),
                "size {} buffer should not be all zero",
                size
            );
        }
    }

    #[test]
    fn test_csrng_fill_bytes_exact_block() {
        // 恰好 32 字节 = 一个 SM3 块
        let mut rng = CsRng::new();
        let mut buf = [0u8; 32];
        rng.fill_bytes(&mut buf);
        assert!(
            buf.iter().any(|&b| b != 0),
            "exact block should not be all zero"
        );

        // 与下一步生成不同
        let mut next = [0u8; 32];
        rng.fill_bytes(&mut next);
        assert_ne!(buf, next, "consecutive blocks must differ");
    }

    #[test]
    fn test_csrng_fill_bytes_large() {
        // 1000 字节填充不应 panic
        let mut rng = CsRng::new();
        let mut buf = vec![0u8; 1000];
        rng.fill_bytes(&mut buf);
        assert_eq!(buf.len(), 1000);
        // 非全零
        assert!(buf.iter().any(|&b| b != 0));
    }

    #[test]
    fn test_csrng_fill_bytes_not_all_zero() {
        let mut rng = CsRng::new();
        let mut buf = [0u8; 256];
        rng.fill_bytes(&mut buf);
        let zero_count = buf.iter().filter(|&&b| b == 0).count();
        // 全零几乎不可能发生（2^2048 概率）
        assert!(
            zero_count < 256,
            "output should not be all zero (zero_count = {})",
            zero_count
        );
    }

    #[test]
    fn test_csrng_next_u32_range() {
        let mut rng = CsRng::new();
        let mut min_val = u32::MAX;
        let mut max_val = u32::MIN;
        for _ in 0..100 {
            let v = rng.next_u32();
            // u32 范围天然在 [0, u32::MAX]，无需额外断言；
            // 这里记录最小/最大值以观察分布。
            if v < min_val {
                min_val = v;
            }
            if v > max_val {
                max_val = v;
            }
        }
        // 100 个 u32 全部相同几乎不可能（2^3200 概率）
        assert_ne!(min_val, max_val, "100 u32s should not all be identical");
    }

    #[test]
    fn test_csrng_next_u64_range() {
        let mut rng = CsRng::new();
        let mut min_val = u64::MAX;
        let mut max_val = u64::MIN;
        for _ in 0..100 {
            let v = rng.next_u64();
            if v < min_val {
                min_val = v;
            }
            if v > max_val {
                max_val = v;
            }
        }
        assert_ne!(min_val, max_val, "100 u64s should not all be identical");
    }

    #[test]
    fn test_csrng_reseed_changes_output() {
        let mut rng = CsRng::new();
        let mut buf1 = [0u8; 32];
        rng.fill_bytes(&mut buf1);

        // 重播种
        rng.reseed(&[0xFFu8; 16]);

        let mut buf2 = [0u8; 32];
        rng.fill_bytes(&mut buf2);
        assert_ne!(buf1, buf2, "reseed must change the output sequence");

        // 不同重种值产生不同序列
        let mut rng_a = CsRng::new();
        let mut rng_b = CsRng::new();
        rng_a.reseed(&[0x01u8; 8]);
        rng_b.reseed(&[0x02u8; 8]);
        let mut out_a = [0u8; 32];
        let mut out_b = [0u8; 32];
        rng_a.fill_bytes(&mut out_a);
        rng_b.fill_bytes(&mut out_b);
        assert_ne!(
            out_a, out_b,
            "different reseed values must yield different sequences"
        );
    }

    #[test]
    fn test_csrng_no_duplicate_blocks() {
        // 生成 1000 个 32 字节块，验证无重复（排序 + 去重法）
        let mut rng = CsRng::new();
        let mut blocks: Vec<[u8; 32]> = Vec::with_capacity(1000);
        for _ in 0..1000 {
            let mut blk = [0u8; 32];
            rng.fill_bytes(&mut blk);
            blocks.push(blk);
        }
        let original_len = blocks.len();
        blocks.sort();
        blocks.dedup();
        assert_eq!(
            blocks.len(),
            original_len,
            "no duplicate 32-byte blocks should exist among 1000 outputs"
        );
    }

    #[test]
    fn test_csrng_uniqueness_1000_blocks() {
        // 生成 1000 个 32 字节块，使用 HashSet 验证全部唯一
        let mut rng = CsRng::new();
        let mut seen: HashSet<[u8; 32]> = HashSet::new();
        for _ in 0..1000 {
            let mut blk = [0u8; 32];
            rng.fill_bytes(&mut blk);
            assert!(seen.insert(blk), "duplicate block encountered");
        }
        assert_eq!(seen.len(), 1000, "all 1000 blocks must be unique");
    }

    #[test]
    fn test_csrng_default() {
        // new() 应产生确定性、非零输出
        let mut rng = CsRng::default();
        let mut buf = [0u8; 32];
        rng.fill_bytes(&mut buf);
        assert!(
            buf.iter().any(|&b| b != 0),
            "default CsRng should produce non-zero output"
        );

        // 与 new() 一致
        let mut rng2 = CsRng::new();
        let mut buf2 = [0u8; 32];
        rng2.fill_bytes(&mut buf2);
        assert_eq!(buf, buf2, "default() and new() must yield same output");
    }
}

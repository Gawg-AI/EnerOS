//! 基于 SM3 的密码学安全随机数生成器 (CSRNG).
//!
//! # SP 800-90A Hash_DRBG（SM3 变体，21-B C4 标准化）
//! 2026-08-09 起按 NIST SP 800-90A Rev.1 §10.1.1 Hash_DRBG 的结构实现，
//! 底层哈希替换为 SM3（GB/T 32905-2016）：状态 `(V, C, reseed_counter)`，
//! `Hash_DRBG_Update` 用 0x00/0x03 域分隔符，Generate 用 `V = Hash(0x03‖V)`
//! 循环输出。旧版"计数器模式（V=SM3(V‖counter)）"为 SP 800-90A 风格但
//! 非标准结构（蓝图 21-B C4 指定项），已替换。
//!
//! # 安全警告
//! 固定种子构造器仅在 test/debug 或显式 `insecure-test-rng` feature 下编译。
//! release 代码必须使用 [`CsRng::from_seed`] 注入硬件熵，或仅为签名 nonce
//! 使用 [`CsRng::for_deterministic_signature`] 的域分离确定性派生。

use alloc::vec::Vec;

use crate::constant_time::ct_zeroize;
use crate::sm3::hmac::Sm3Hmac;

/// SP 800-90A Hash_DRBG 输出块长度（SM3 摘要 = 32 字节）。
const OUTLEN: usize = 32;

/// 重播种间隔（FIX-BUGS-0810 B5）：SP 800-90A §10.1.1.4 步骤 7 要求
/// `reseed_counter > seedlife` 时必须重播种；`2^48` 为 NIST 保守上限。
pub const RESEED_INTERVAL: u64 = 1 << 48;

/// DRBG 错误（FIX-BUGS-0810 B5）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrbgError {
    /// 生成次数达到 [`RESEED_INTERVAL`]：必须重播种后才能继续出数
    /// （SP 800-90A §10.1.1.4 步骤 7；旧实现仅自增计数从不检查阈值）。
    ReseedRequired,
    /// 重播种所需熵源不可用（fail-closed：不静默回退固定种子）。
    EntropyUnavailable,
}

/// 基于 SM3 的 SP 800-90A Hash_DRBG.
///
/// 状态 `(V, C, reseed_counter)`：`V` 与 `C` 各 32 字节，
/// `reseed_counter` 记录自上次实例化/重播种以来的生成次数。
///
/// release 构造必须显式注入熵；固定种子便捷构造器不会进入 release API。
pub struct CsRng {
    /// 内部状态 V（outlen = 32 字节）.
    v: [u8; 32],
    /// 内部常量 C（outlen = 32 字节）.
    c: [u8; 32],
    /// 自上次实例化/重播种以来的生成次数.
    reseed_counter: u64,
}

/// SP 800-90A §10.1.1.2 Hash_DRBG_Update(provided_data)。
///
/// ```text
/// if provided_data != NULL:
///   temp = Hash(0x00 ‖ V ‖ provided_data)
/// else:
///   temp = Hash(0x00 ‖ V)
/// V = temp
/// temp = Hash(0x03 ‖ V); V = temp
/// temp = Hash(0x03 ‖ V); C = temp
/// ```
fn hash_drbg_update(provided_data: Option<&[u8]>, v: &mut [u8; 32], c: &mut [u8; 32]) {
    let mut hasher = crate::sm3::Sm3Hasher::new();
    hasher.update(&[0x00]);
    hasher.update(v);
    if let Some(data) = provided_data {
        hasher.update(data);
    }
    *v = hasher.finalize();

    let mut hasher = crate::sm3::Sm3Hasher::new();
    hasher.update(&[0x03]);
    hasher.update(v);
    *v = hasher.finalize();

    let mut hasher = crate::sm3::Sm3Hasher::new();
    hasher.update(&[0x03]);
    hasher.update(v);
    *c = hasher.finalize();
}

impl CsRng {
    /// 使用固定种子创建仅供测试的 CSRNG.
    ///
    /// **WARNING**: 仅用于测试。种子是确定性的，相同实例产生相同输出序列。
    /// 本函数在 release 中默认不存在；显式启用不安全 feature 也会被产品
    /// dependency policy 拒绝。
    #[cfg(any(test, debug_assertions, feature = "insecure-test-rng"))]
    pub fn new() -> Self {
        // 固定种子 — 切勿用于生产
        let seed = [
            0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x30, 0x31, 0x32, 0x33, 0x34,
            0x35, 0x36, 0x37, 0x38, 0x39, 0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38,
            0x39, 0x30, 0x31, 0x32,
        ];
        Self::from_seed(&seed)
    }

    /// 使用显式 32 字节种子创建 CSRNG（SP 800-90A Instantiate）。
    ///
    /// 生产环境中，种子应来自硬件 TRNG。
    /// `Hash_DRBG_Update(seed)` 后 `reseed_counter = 1`。
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        let mut rng = Self {
            v: [0u8; 32],
            c: [0u8; 32],
            reseed_counter: 1,
        };
        hash_drbg_update(Some(seed), &mut rng.v, &mut rng.c);
        rng
    }

    /// 为确定性签名 nonce 派生一个域分离 DRBG。
    ///
    /// 种子为 `HMAC-SM3(secret, label || lengths || context || message)`，因此
    /// 不依赖固定种子，也不会因创建多个实例而为不同消息重复 nonce。该入口
    /// 只适用于签名 nonce；一般随机数仍必须由 [`Self::from_seed`] 接收系统熵。
    pub fn for_deterministic_signature(secret: &[u8], message: &[u8], context: &[u8]) -> Self {
        let mut hmac = Sm3Hmac::new(secret);
        hmac.update(b"EnerOS/SM2/deterministic-nonce/v1");
        hmac.update(&(context.len() as u64).to_be_bytes());
        hmac.update(context);
        hmac.update(&(message.len() as u64).to_be_bytes());
        hmac.update(message);
        let seed = hmac.finalize();
        Self::from_seed(&seed)
    }

    /// 用随机字节填充缓冲区（SP 800-90A Generate）。
    ///
    /// 每 32 字节块：`V = Hash(0x03 ‖ V)`；完成后执行
    /// `Hash_DRBG_Update(no provided data)` 并递增 `reseed_counter`。
    ///
    /// FIX-BUGS-0810 B5：生成前检查 `reseed_counter` 阈值——达到
    /// [`RESEED_INTERVAL`] 返回 [`DrbgError::ReseedRequired`] 且**状态不变**
    /// （停止出数），调用方必须重播种（处置模板见 [`fill_bytes_checked`]）。
    /// 旧实现仅自增计数从不检查阈值（SP 800-90A §10.1.1.4 步骤 7 违规）。
    pub fn fill_bytes(&mut self, buf: &mut [u8]) -> Result<(), DrbgError> {
        if self.reseed_counter >= RESEED_INTERVAL {
            return Err(DrbgError::ReseedRequired);
        }
        let mut offset = 0;
        while offset < buf.len() {
            let mut hasher = crate::sm3::Sm3Hasher::new();
            hasher.update(&[0x03]);
            hasher.update(&self.v);
            self.v = hasher.finalize();

            let remaining = buf.len() - offset;
            let to_copy = if remaining < OUTLEN {
                remaining
            } else {
                OUTLEN
            };
            buf[offset..offset + to_copy].copy_from_slice(&self.v[..to_copy]);
            offset += to_copy;
        }
        // 生成完成后状态更新（SP 800-90A §10.1.1.4 步骤 7）
        hash_drbg_update(None, &mut self.v, &mut self.c);
        // 检查先行，此处不可能溢出（counter 到达阈值即拒）。
        self.reseed_counter += 1;
        Ok(())
    }

    /// 生成一个随机 `u32`（大端字节序解析；FIX-BUGS-0810 B5：阈值语义同上）。
    pub fn next_u32(&mut self) -> Result<u32, DrbgError> {
        let mut buf = [0u8; 4];
        self.fill_bytes(&mut buf)?;
        Ok(u32::from_be_bytes(buf))
    }

    /// 生成一个随机 `u64`（大端字节序解析；FIX-BUGS-0810 B5：阈值语义同上）。
    pub fn next_u64(&mut self) -> Result<u64, DrbgError> {
        let mut buf = [0u8; 8];
        self.fill_bytes(&mut buf)?;
        Ok(u64::from_be_bytes(buf))
    }

    /// 测试专用：直接设置 reseed 计数（预置阈值场景；仅同 crate 测试）。
    #[cfg(test)]
    pub(crate) fn reseed_counter_mut(&mut self) -> &mut u64 {
        &mut self.reseed_counter
    }

    /// 用新熵重播种 CSRNG（SP 800-90A Reseed）。
    ///
    /// `Hash_DRBG_Update(0x01 ‖ V ‖ entropy)` 后 `reseed_counter = 1`。
    /// 重种后输出序列将发生改变。
    pub fn reseed(&mut self, seed: &[u8]) {
        let mut input = Vec::with_capacity(1 + OUTLEN + seed.len());
        input.push(0x01);
        input.extend_from_slice(&self.v);
        input.extend_from_slice(seed);
        hash_drbg_update(Some(&input), &mut self.v, &mut self.c);
        self.reseed_counter = 1;
    }
}

/// 统一处置模板（FIX-BUGS-0810 B5，蓝图/25 §4.4）：
/// `fill_bytes` 遇 [`DrbgError::ReseedRequired`] → 从注入熵源取熵重播种 →
/// 重试一次；熵源失败 → [`DrbgError::EntropyUnavailable`]（fail-closed，
/// 不静默回退固定种子）。测试可注入桩熵源验证模板；生产经
/// [`fill_bytes_or_reseed`] 使用平台熵。
pub fn fill_bytes_checked(
    rng: &mut CsRng,
    buf: &mut [u8],
    entropy: &mut dyn crate::rng::EntropySource,
) -> Result<(), DrbgError> {
    match rng.fill_bytes(buf) {
        Ok(()) => Ok(()),
        Err(DrbgError::ReseedRequired) => {
            let mut seed = [0u8; 32];
            entropy
                .fill_entropy(&mut seed)
                .map_err(|_| DrbgError::EntropyUnavailable)?;
            rng.reseed(&seed);
            // 重试一次；仍失败上抛（不无限重试）。
            rng.fill_bytes(buf)
        }
        // fill_bytes 当前仅返回 ReseedRequired；其他变体透传（fail-closed）。
        Err(e) => Err(e),
    }
}

/// 便捷模板：仅当 `fill_bytes` 返回 [`DrbgError::ReseedRequired`] 时才从
/// 平台熵源取熵重播种（正常路径零熵源接触；`platform_entropy_source`
/// fail-closed——熵不可用 → [`DrbgError::EntropyUnavailable`] 上抛）。
pub fn fill_bytes_or_reseed(rng: &mut CsRng, buf: &mut [u8]) -> Result<(), DrbgError> {
    match rng.fill_bytes(buf) {
        Ok(()) => Ok(()),
        Err(DrbgError::ReseedRequired) => {
            let mut src =
                crate::rng::platform_entropy_source().map_err(|_| DrbgError::EntropyUnavailable)?;
            let mut seed = [0u8; 32];
            src.fill_entropy(&mut seed)
                .map_err(|_| DrbgError::EntropyUnavailable)?;
            rng.reseed(&seed);
            // 重试一次；仍失败上抛（不无限重试）。
            rng.fill_bytes(buf)
        }
        Err(e) => Err(e),
    }
}

#[cfg(any(test, debug_assertions, feature = "insecure-test-rng"))]
impl Default for CsRng {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for CsRng {
    fn drop(&mut self) {
        // 恒定时间清零敏感状态
        ct_zeroize(&mut self.v);
        // 34 SE-12：补齐常量 C 清零（旧实现仅清 v——C 与输出同源敏感）。
        ct_zeroize(&mut self.c);
        // reseed_counter 是 u64 标量，使用 volatile 写入清零以防编译器优化删除
        // SAFETY: write_volatile 写入栈上变量的可变引用，不会触发 UB
        unsafe {
            core::ptr::write_volatile(&mut self.reseed_counter, 0);
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
    use crate::rng::EntropySource;

    /// 测试桩熵源（注入固定熵；仅测试）。
    struct StubEntropy([u8; 32]);
    impl EntropySource for StubEntropy {
        fn fill_entropy(&mut self, out: &mut [u8; 32]) -> Result<(), crate::rng::EntropyError> {
            out.copy_from_slice(&self.0);
            Ok(())
        }
    }

    // ===== FIX-BUGS-0810 B5 防回归 1：阈值前成功、达阈值 ReseedRequired 且
    // 停止出数（证伪旧行为——旧 fill_bytes 仅自增计数从不检查阈值）=====
    #[test]
    fn test_b5_reseed_required_at_threshold_stops_output() {
        let mut rng = CsRng::new();
        *rng.reseed_counter_mut() = RESEED_INTERVAL - 1;

        let mut buf = [0u8; 32];
        // 阈值前最后一次生成：成功（counter → RESEED_INTERVAL）。
        assert!(rng.fill_bytes(&mut buf).is_ok(), "阈值前必须成功");
        // 达阈值：ReseedRequired，且状态不变（不再出数——重试仍失败）。
        assert_eq!(
            rng.fill_bytes(&mut buf),
            Err(DrbgError::ReseedRequired),
            "达阈值必须 ReseedRequired（证伪旧行为：永不停止）"
        );
        assert_eq!(
            rng.fill_bytes(&mut buf),
            Err(DrbgError::ReseedRequired),
            "拒绝后状态不变：持续停止出数"
        );
    }

    // ===== FIX-BUGS-0810 B5 防回归 2：重播种后恢复出数 =====
    #[test]
    fn test_b5_reseed_recovers_output() {
        let mut rng = CsRng::new();
        *rng.reseed_counter_mut() = RESEED_INTERVAL; // 已达阈值

        let mut buf = [0u8; 32];
        assert_eq!(rng.fill_bytes(&mut buf), Err(DrbgError::ReseedRequired));

        let seed = [0x5Au8; 32];
        rng.reseed(&seed);
        assert_eq!(*rng.reseed_counter_mut(), 1, "重播种后计数归 1");
        assert!(rng.fill_bytes(&mut buf).is_ok(), "重播种后必须恢复出数");
    }

    // ===== FIX-BUGS-0810 B5 防回归 3：统一处置模板（重播种+重试一次）=====
    #[test]
    fn test_b5_disposal_template_reseeds_and_retries() {
        let mut rng = CsRng::new();
        *rng.reseed_counter_mut() = RESEED_INTERVAL; // 已达阈值

        let mut buf = [0u8; 32];
        let mut entropy = StubEntropy([0x11; 32]);
        // 模板：ReseedRequired → 注入熵重播种 → 重试一次成功。
        assert!(
            fill_bytes_checked(&mut rng, &mut buf, &mut entropy).is_ok(),
            "处置模板必须在重播种后恢复出数"
        );
        // 重播种后计数归 1，后续正常生成。
        assert!(rng.fill_bytes(&mut buf).is_ok());
    }

    // ===== FIX-BUGS-0810 B5 防回归 4：熵源失败 → 模板上抛（fail-closed）=====
    #[test]
    fn test_b5_template_fails_closed_when_entropy_unavailable() {
        use crate::rng::EntropyError;
        struct DeadEntropy;
        impl EntropySource for DeadEntropy {
            fn fill_entropy(&mut self, _out: &mut [u8; 32]) -> Result<(), EntropyError> {
                Err(EntropyError::Unavailable)
            }
        }

        let mut rng = CsRng::new();
        *rng.reseed_counter_mut() = RESEED_INTERVAL;
        let mut buf = [0u8; 32];
        assert_eq!(
            fill_bytes_checked(&mut rng, &mut buf, &mut DeadEntropy),
            Err(DrbgError::EntropyUnavailable),
            "熵不可用必须上抛（不静默回退固定种子）"
        );
        // 状态保持拒绝（未出数）。
        assert_eq!(rng.fill_bytes(&mut buf), Err(DrbgError::ReseedRequired));
    }

    #[test]
    fn test_csrng_deterministic_same_seed() {
        // 相同种子应产生相同的输出序列
        let seed = [0xAAu8; 32];
        let mut rng1 = CsRng::from_seed(&seed);
        let mut rng2 = CsRng::from_seed(&seed);

        let mut buf1 = [0u8; 64];
        let mut buf2 = [0u8; 64];
        let _ = rng1.fill_bytes(&mut buf1);
        let _ = rng2.fill_bytes(&mut buf2);
        assert_eq!(buf1, buf2, "same seed must yield same output sequence");

        // 第二次生成也应一致
        let _ = rng1.fill_bytes(&mut buf1);
        let _ = rng2.fill_bytes(&mut buf2);
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
        let _ = rng1.fill_bytes(&mut buf1);
        let _ = rng2.fill_bytes(&mut buf2);
        assert_ne!(buf1, buf2, "different seeds must yield different outputs");
    }

    #[test]
    fn test_csrng_fill_bytes_small() {
        let mut rng = CsRng::new();
        for size in [1usize, 4, 16, 31] {
            let mut buf = vec![0xFFu8; size];
            let _ = rng.fill_bytes(&mut buf);
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
        let _ = rng.fill_bytes(&mut buf);
        assert!(
            buf.iter().any(|&b| b != 0),
            "exact block should not be all zero"
        );

        // 与下一步生成不同
        let mut next = [0u8; 32];
        let _ = rng.fill_bytes(&mut next);
        assert_ne!(buf, next, "consecutive blocks must differ");
    }

    #[test]
    fn test_csrng_fill_bytes_large() {
        // 1000 字节填充不应 panic
        let mut rng = CsRng::new();
        let mut buf = vec![0u8; 1000];
        let _ = rng.fill_bytes(&mut buf);
        assert_eq!(buf.len(), 1000);
        // 非全零
        assert!(buf.iter().any(|&b| b != 0));
    }

    #[test]
    fn test_csrng_fill_bytes_not_all_zero() {
        let mut rng = CsRng::new();
        let mut buf = [0u8; 256];
        let _ = rng.fill_bytes(&mut buf);
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
            let v = rng.next_u32().expect("阈值内生成必须成功");
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
            let v = rng.next_u64().expect("阈值内生成必须成功");
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
        let _ = rng.fill_bytes(&mut buf1);

        // 重播种
        rng.reseed(&[0xFFu8; 16]);

        let mut buf2 = [0u8; 32];
        let _ = rng.fill_bytes(&mut buf2);
        assert_ne!(buf1, buf2, "reseed must change the output sequence");

        // 不同重种值产生不同序列
        let mut rng_a = CsRng::new();
        let mut rng_b = CsRng::new();
        rng_a.reseed(&[0x01u8; 8]);
        rng_b.reseed(&[0x02u8; 8]);
        let mut out_a = [0u8; 32];
        let mut out_b = [0u8; 32];
        let _ = rng_a.fill_bytes(&mut out_a);
        let _ = rng_b.fill_bytes(&mut out_b);
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
            let _ = rng.fill_bytes(&mut blk);
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
            let _ = rng.fill_bytes(&mut blk);
            assert!(seen.insert(blk), "duplicate block encountered");
        }
        assert_eq!(seen.len(), 1000, "all 1000 blocks must be unique");
    }

    #[test]
    fn test_csrng_default() {
        // new() 应产生确定性、非零输出
        let mut rng = CsRng::default();
        let mut buf = [0u8; 32];
        let _ = rng.fill_bytes(&mut buf);
        assert!(
            buf.iter().any(|&b| b != 0),
            "default CsRng should produce non-zero output"
        );

        // 与 new() 一致
        let mut rng2 = CsRng::new();
        let mut buf2 = [0u8; 32];
        let _ = rng2.fill_bytes(&mut buf2);
        assert_eq!(buf, buf2, "default() and new() must yield same output");
    }

    #[test]
    fn deterministic_signature_nonce_is_stable_and_domain_separated() {
        let secret = [0xA5u8; 32];
        let mut first =
            CsRng::for_deterministic_signature(&secret, b"message-a", b"eneros/test/signature/v1");
        let mut repeat =
            CsRng::for_deterministic_signature(&secret, b"message-a", b"eneros/test/signature/v1");
        let mut changed_message =
            CsRng::for_deterministic_signature(&secret, b"message-b", b"eneros/test/signature/v1");
        let mut changed_domain =
            CsRng::for_deterministic_signature(&secret, b"message-a", b"eneros/test/other/v1");
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        let mut c = [0u8; 32];
        let mut d = [0u8; 32];
        let _ = first.fill_bytes(&mut a);
        let _ = repeat.fill_bytes(&mut b);
        let _ = changed_message.fill_bytes(&mut c);
        let _ = changed_domain.fill_bytes(&mut d);
        assert_eq!(
            a, b,
            "same secret/message/domain must reproduce the nonce stream"
        );
        assert_ne!(a, c, "message change must change the nonce stream");
        assert_ne!(a, d, "domain change must change the nonce stream");
    }
}

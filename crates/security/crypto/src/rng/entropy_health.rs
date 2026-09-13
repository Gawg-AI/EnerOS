//! 70.12（WP-70.12.4）：熵源健康判据与有界轮询（fail-closed 接缝）。
//!
//! # 为什么需要它
//! 旧实现的判据只有"能不能读 32 字节"：卡住的 TRNG（恒返回同一块）、
//! 只有一两个取值活跃的退化噪声源、阻塞不返回的寄存器轮询，都会**静默
//! 通过**并进入密钥供给——这恰恰是固定种子顶包的同类故障。
//!
//! 本模块把"熵是否可用"变成可核对的判据：
//! - [`HealthPolicy`]：样本数 / 轮询上限 / 单字节最小熵 / 最少不同块数；
//! - [`HealthTest`]：对**真实读出的样本**逐条判定，输出可归档的
//!   [`HealthVerdict`]（样本数、不同块数、取值域、每字节最小熵）；
//! - [`fill_bounded`]：有界轮询（超预算 = [`EntropyError::Timeout`]），
//!   不无限自旋。
//!
//! # 判据口径（诚实声明）
//! 这里是**退化检测**（SP 800-90B 的启发型最小子集），不是完整熵评估：
//! 它保证"源在动、输出不退化"，不保证每比特熵含量。真正的熵评估需要
//! 厂商噪声模型与独立试验（外部待证，见 bring-up 模板 B3）。

use super::{EntropyError, EntropySource};

/// 健康判据（调用方冻结；默认值只用于开发平台自检）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthPolicy {
    /// 采样块数（每块 32 字节）。
    pub samples: u16,
    /// 单次读取的最大轮询次数（0 不允许）。
    pub max_polls: u16,
    /// 单字节最小熵下限（比特 × 1000；`4500` = 4.5 bit/byte）。
    pub min_byte_entropy_millibits: u32,
    /// 最少不同块数（`samples` 中的最小去重数）。
    pub min_distinct_blocks: u16,
    /// 取值域下限（全部样本里不同字节值的个数）。
    pub min_distinct_byte_values: u16,
}

impl HealthPolicy {
    /// 生产自检判据：8 块样本、单块单轮询、4.5 bit/byte、≥6 不同块、≥16 取值。
    pub const PRODUCT: HealthPolicy = HealthPolicy {
        samples: 8,
        max_polls: 1,
        min_byte_entropy_millibits: 4_500,
        min_distinct_blocks: 6,
        min_distinct_byte_values: 16,
    };

    /// 判据形态自校验（0 值/越界即拒绝——不得用空判据"通过"）。
    pub fn validate(&self) -> Result<(), EntropyError> {
        if self.samples == 0 || self.max_polls == 0 {
            return Err(EntropyError::Unavailable);
        }
        if self.min_distinct_blocks > self.samples {
            return Err(EntropyError::Unavailable);
        }
        if self.min_byte_entropy_millibits == 0 || self.min_byte_entropy_millibits > 8_000 {
            return Err(EntropyError::Unavailable);
        }
        Ok(())
    }
}

/// 健康判定结果（可归档；`ok` 为假时 [`Self::reason`] 给机器可读理由）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthVerdict {
    pub samples: u16,
    pub distinct_blocks: u16,
    pub distinct_byte_values: u16,
    pub min_byte_entropy_millibits: u32,
    pub ok: bool,
}

impl HealthVerdict {
    /// 机器可读理由（判据按"最具体优先"排序，与 [`HealthTest::judge`] 的
    /// 阈值判定一致——理由串会进证据，不得指向错误的故障类别）。
    pub fn reason(&self) -> &'static str {
        let policy = HealthPolicy::PRODUCT;
        if self.ok {
            "entropy-health-ok"
        } else if self.distinct_blocks <= 1 {
            "entropy-stuck"
        } else if self.distinct_byte_values < policy.min_distinct_byte_values / 2 {
            "entropy-degenerate-domain"
        } else if self.min_byte_entropy_millibits == 0 {
            "entropy-zero-entropy"
        } else {
            "entropy-health-below-threshold"
        }
    }
}

/// 健康判据执行器（对样本序列判定；样本由调用方从 [`EntropySource`] 取得，
/// 因此同一判据可跑 host 仿真、QEMU 与真机三个 adapter）。
pub struct HealthTest {
    policy: HealthPolicy,
}

impl HealthTest {
    pub fn new(policy: HealthPolicy) -> Result<Self, EntropyError> {
        policy.validate()?;
        Ok(Self { policy })
    }

    pub fn policy(&self) -> HealthPolicy {
        self.policy
    }

    /// 采样并判定。
    ///
    /// 任一采样失败即返回该错误（**不**用剩余样本"凑"通过）。
    pub fn run(&self, src: &mut dyn EntropySource) -> Result<HealthVerdict, EntropyError> {
        let mut samples = alloc::vec::Vec::new();
        for _ in 0..self.policy.samples {
            let mut block = [0u8; 32];
            fill_bounded(src, &mut block, self.policy.max_polls)?;
            samples.push(block);
        }
        Ok(self.judge(&samples))
    }

    /// 纯判定（样本已在手；证据归档路径复用）。
    pub fn judge(&self, samples: &[[u8; 32]]) -> HealthVerdict {
        let samples_u16 = samples.len() as u16;
        let mut distinct_blocks = 0u16;
        for (i, s) in samples.iter().enumerate() {
            if !samples[..i].iter().any(|p| p == s) {
                distinct_blocks += 1;
            }
        }
        let mut seen = [false; 256];
        for s in samples.iter() {
            for b in s.iter() {
                seen[*b as usize] = true;
            }
        }
        let distinct_byte_values = seen.iter().filter(|x| **x).count() as u16;
        let min_entropy = min_byte_entropy_millibits(samples);
        let ok = distinct_blocks >= self.policy.min_distinct_blocks
            && distinct_byte_values >= self.policy.min_distinct_byte_values
            && min_entropy >= self.policy.min_byte_entropy_millibits;
        HealthVerdict {
            samples: samples_u16,
            distinct_blocks,
            distinct_byte_values,
            min_byte_entropy_millibits: min_entropy,
            ok,
        }
    }
}

/// 有界轮询读熵：最多 `max_polls` 次；超预算 → [`EntropyError::Timeout`]。
///
/// 这是"阻塞不返回的寄存器轮询"的服务侧防线（驱动内部也有自己的预算，
/// 两层都不放行无限等待）。
pub fn fill_bounded(
    src: &mut dyn EntropySource,
    out: &mut [u8; 32],
    max_polls: u16,
) -> Result<(), EntropyError> {
    if max_polls == 0 {
        return Err(EntropyError::Timeout);
    }
    let mut last = EntropyError::Unavailable;
    for _ in 0..max_polls {
        match src.fill_entropy(out) {
            Ok(()) => return Ok(()),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// 每字节最小熵（极保守口径）：把全部样本的字节直方图当独立同分布估计，
/// 取 `floor(-log2(max_p))·1000` 作为**下界**（最保守的自检量）。
///
/// 对均匀随机 8 块样本，`max_p` 远小于 1/16，故阈值 4500 有充分裕量；
/// 恒值源（`max_p = 1`）得 0 → 必判失败。
fn min_byte_entropy_millibits(samples: &[[u8; 32]]) -> u32 {
    let total: u64 = (samples.len() as u64) * 32;
    if total == 0 {
        return 0;
    }
    let mut hist = [0u64; 256];
    for s in samples.iter() {
        for b in s.iter() {
            hist[*b as usize] += 1;
        }
    }
    let max_count = hist.iter().copied().max().unwrap_or(0);
    if max_count == 0 {
        return 0;
    }
    // -log2(max_p) 的千分位定点：max_p = max_count/total
    // ⇒ -log2(max_p) = log2(total) - log2(max_count)。
    let log2_total = u64::from(64 - total.leading_zeros()) - 1;
    let log2_max = u64::from(64 - max_count.leading_zeros()) - 1;
    let bits = log2_total.saturating_sub(log2_max);
    (bits * 1000).min(8_000) as u32
}

extern crate alloc;

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)] // 测试断言 panic!（elf-loader/ota-ab 先例）
    use super::*;

    /// 可编程测试源（不是生产夹具：只在本测试模块内）。
    struct ScriptedSource {
        blocks: alloc::vec::Vec<[u8; 32]>,
        cursor: usize,
        fail: Option<EntropyError>,
    }

    impl ScriptedSource {
        fn new(blocks: alloc::vec::Vec<[u8; 32]>) -> Self {
            Self {
                blocks,
                cursor: 0,
                fail: None,
            }
        }
        fn always_fail(e: EntropyError) -> Self {
            Self {
                blocks: alloc::vec::Vec::new(),
                cursor: 0,
                fail: Some(e),
            }
        }
    }

    impl EntropySource for ScriptedSource {
        fn fill_entropy(&mut self, out: &mut [u8; 32]) -> Result<(), EntropyError> {
            if let Some(e) = self.fail {
                return Err(e);
            }
            if self.blocks.is_empty() {
                return Err(EntropyError::Unavailable);
            }
            *out = self.blocks[self.cursor % self.blocks.len()];
            self.cursor += 1;
            Ok(())
        }
    }

    fn pseudo_random_blocks(n: usize) -> alloc::vec::Vec<[u8; 32]> {
        // 确定性但充分分散的样本（LCG；仅测试用——不是熵源）。
        let mut x: u64 = 0x1234_5678_9abc_def0;
        let mut out = alloc::vec::Vec::new();
        for _ in 0..n {
            let mut block = [0u8; 32];
            for slot in block.iter_mut() {
                x = x
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                *slot = (x >> 33) as u8;
            }
            out.push(block);
        }
        out
    }

    fn narrow_blocks(samples: u16) -> alloc::vec::Vec<[u8; 32]> {
        // 8 个互不相同的块，但字节取值只有 {1,2}（退化域）。
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut out = alloc::vec::Vec::new();
        for _ in 0..samples {
            let mut b = [0u8; 32];
            for slot in b.iter_mut() {
                x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
                *slot = if (x >> 40) & 1 == 0 { 1 } else { 2 };
            }
            out.push(b);
        }
        out
    }

    #[test]
    fn healthy_source_passes() {
        let policy = HealthPolicy::PRODUCT;
        let mut src = ScriptedSource::new(pseudo_random_blocks(policy.samples as usize));
        let v = HealthTest::new(policy).unwrap().run(&mut src).unwrap();
        assert!(v.ok, "verdict: {v:?}");
        assert_eq!(v.samples, policy.samples);
        assert!(v.distinct_blocks >= policy.min_distinct_blocks);
        assert_eq!(v.reason(), "entropy-health-ok");
    }

    #[test]
    fn stuck_source_is_rejected() {
        let policy = HealthPolicy::PRODUCT;
        let ones = [0xffu8; 32];
        let mut src = ScriptedSource::new(alloc::vec![ones]);
        let v = HealthTest::new(policy).unwrap().run(&mut src).unwrap();
        assert!(!v.ok);
        assert_eq!(v.reason(), "entropy-stuck");
        assert_eq!(v.distinct_blocks, 1);
        assert_eq!(v.min_byte_entropy_millibits, 0, "恒值源零熵");
    }

    #[test]
    fn degenerate_domain_is_rejected() {
        // 8 个**互不相同**的块，但字节取值只有 {0x01,0x02} → 域太小、
        // 单字节熵仅 1 bit（块不重复，故命中的是"退化域"而非"卡住"）。
        let policy = HealthPolicy::PRODUCT;
        let mut src = ScriptedSource::new(narrow_blocks(policy.samples));
        let v = HealthTest::new(policy).unwrap().run(&mut src).unwrap();
        assert!(!v.ok);
        assert_eq!(v.distinct_blocks, policy.samples, "块本身不重复");
        assert_eq!(v.distinct_byte_values, 2);
        assert!(v.min_byte_entropy_millibits < 4_500);
        assert_eq!(v.reason(), "entropy-degenerate-domain");

        // 判据两向都不误判：放宽每字节熵阈值后，取值域判据仍独立拒绝；
        // 同一放宽判据下健康源必须通过。
        let loose = HealthPolicy {
            min_byte_entropy_millibits: 500,
            ..policy
        };
        let mut src2 = ScriptedSource::new(narrow_blocks(policy.samples));
        assert!(!HealthTest::new(loose).unwrap().run(&mut src2).unwrap().ok);
        let mut src3 = ScriptedSource::new(pseudo_random_blocks(policy.samples as usize));
        assert!(HealthTest::new(loose).unwrap().run(&mut src3).unwrap().ok);
    }

    #[test]
    fn read_failure_propagates_and_never_completes_with_partial_samples() {
        let policy = HealthPolicy::PRODUCT;
        let mut src = ScriptedSource::always_fail(EntropyError::ShortRead);
        let err = HealthTest::new(policy).unwrap().run(&mut src).unwrap_err();
        assert_eq!(err, EntropyError::ShortRead);
        let mut src = ScriptedSource::always_fail(EntropyError::Unavailable);
        assert_eq!(
            HealthTest::new(policy).unwrap().run(&mut src),
            Err(EntropyError::Unavailable)
        );
    }

    #[test]
    fn bounded_poll_times_out_instead_of_spinning() {
        let mut src = ScriptedSource::always_fail(EntropyError::Unavailable);
        let mut block = [0u8; 32];
        assert_eq!(
            fill_bounded(&mut src, &mut block, 3),
            Err(EntropyError::Unavailable),
            "三次机会都用完仍失败 → 返回最后一次错误"
        );
        // 轮询预算为 0（策略形态非法）→ Timeout（不得当成"跳过"）。
        assert_eq!(
            fill_bounded(&mut src, &mut block, 0),
            Err(EntropyError::Timeout)
        );
        // 第一次成功即返回。
        let mut ok = ScriptedSource::new(alloc::vec![[7u8; 32]]);
        assert_eq!(fill_bounded(&mut ok, &mut block, 1), Ok(()));
        assert_eq!(block, [7u8; 32]);
    }

    #[test]
    fn empty_or_bad_policy_is_rejected() {
        assert!(HealthTest::new(HealthPolicy {
            samples: 0,
            ..HealthPolicy::PRODUCT
        })
        .is_err());
        assert!(HealthTest::new(HealthPolicy {
            max_polls: 0,
            ..HealthPolicy::PRODUCT
        })
        .is_err());
        assert!(HealthTest::new(HealthPolicy {
            min_byte_entropy_millibits: 0,
            ..HealthPolicy::PRODUCT
        })
        .is_err());
        assert!(HealthTest::new(HealthPolicy {
            min_distinct_blocks: 99,
            ..HealthPolicy::PRODUCT
        })
        .is_err());
    }

    #[test]
    fn judge_is_pure_and_deterministic() {
        let samples = pseudo_random_blocks(4);
        let t = HealthTest::new(HealthPolicy::PRODUCT).unwrap();
        let a = t.judge(&samples);
        let b = t.judge(&samples);
        assert_eq!(a, b);
        assert_eq!(a.samples, 4);
    }
}

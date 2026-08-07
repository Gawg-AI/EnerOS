//! 定时器/中断纯数学逻辑（HAL-001/002 可测性抽取）。
//!
//! 这些函数不依赖任何寄存器/asm，可从 `arm64::timer` / `arm64::gicv3`
//! 引用，并能在 host（x86_64）上直接运行单元测试——无需 aarch64 目标。

/// HAL-002：计数 → 纳秒（u128 中间量，避免 `count * 1e9` 溢出饱和截断）。
///
/// 旧实现 `count.saturating_mul(1_000_000_000) / freq`：count 接近 u64::MAX 时
/// 乘积饱和到 u64::MAX 产生错误结果（不 panic 但量级失真）。u128 中间量保证
/// 精确（结果再 clamp 回 u64）。
///
/// `freq == 0`（CNTFRQ 未初始化/非法）返回 0，避免除零。
#[allow(dead_code)]
pub(crate) fn count_to_ns_impl(count: u64, freq: u64) -> u64 {
    if freq == 0 {
        return 0;
    }
    let ns = (count as u128 * 1_000_000_000u128) / freq as u128;
    ns.min(u64::MAX as u128) as u64
}

/// HAL-002：纳秒 → tick 计数（u128 中间量，避免 `ns * freq` 溢出）。
#[allow(dead_code)]
pub(crate) fn ns_to_ticks_impl(ns: u64, freq: u64) -> u64 {
    if freq == 0 {
        return 0;
    }
    let ticks = (ns as u128 * freq as u128) / 1_000_000_000u128;
    ticks.min(u64::MAX as u128) as u64
}

/// HAL-001：INTID 是否在 handler 表范围内（`irq < MAX_IRQ`）。
///
/// 1022 保留 INTID、1023 spurious、以及任何 `>= MAX_IRQ` 的非法 INTID
/// 均返回 false——调用方不得以越界索引 panic。
#[allow(dead_code)]
pub(crate) fn irq_in_range(irq: u32, max_irq: u32) -> bool {
    irq < max_irq
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===== HAL-002: timer 数学 =====

    #[test]
    fn count_to_ns_normal() {
        // count = 1e9 ticks @ 1e9 Hz → 1e9 ns
        assert_eq!(
            count_to_ns_impl(1_000_000_000, 1_000_000_000),
            1_000_000_000
        );
    }

    #[test]
    fn count_to_ns_no_overflow_on_huge_count() {
        // HAL-002 回归：count = u64::MAX，freq = 4e9 Hz →
        // 精确结果 = u64::MAX/4（u128 中间量不溢出）。
        // 旧代码 saturating_mul(u64::MAX, 1e9) → u64::MAX，再 /4e9 ≈ 4.6e9，
        // 比精确值小 1e9 倍——旧代码失败证据。
        let exact = (u64::MAX as u128 * 1_000_000_000u128 / 4_000_000_000u128) as u64;
        assert_eq!(exact, u64::MAX / 4);
        assert_eq!(count_to_ns_impl(u64::MAX, 4_000_000_000), exact);
    }

    #[test]
    fn count_to_ns_huge_result_clamps() {
        // 结果本身超出 u64（count*u64 中间量溢出 u64）→ clamp 到 u64::MAX
        assert_eq!(count_to_ns_impl(u64::MAX, 2), u64::MAX);
    }

    #[test]
    fn count_to_ns_zero_freq_safe() {
        assert_eq!(count_to_ns_impl(12345, 0), 0);
    }

    #[test]
    fn ns_to_ticks_normal() {
        assert_eq!(
            ns_to_ticks_impl(1_000_000_000, 1_000_000_000),
            1_000_000_000
        );
    }

    #[test]
    fn ns_to_ticks_no_overflow() {
        // HAL-002 回归：ns = u64::MAX/1000，freq = 4e9 →
        // 精确结果 ≈ 7.4e16（u128 中间量不溢出）。
        // 旧代码 saturating_mul(ns, freq) → u64::MAX，再 /1e9 ≈ 1.8e19，
        // 比精确值大 240 倍——旧代码失败证据。
        let ns = u64::MAX / 1000;
        let exact = (ns as u128 * 4_000_000_000u128 / 1_000_000_000u128) as u64;
        assert_eq!(ns_to_ticks_impl(ns, 4_000_000_000), exact);
        assert!(exact < u64::MAX / 10);
    }

    #[test]
    fn ns_to_ticks_zero_freq_safe() {
        assert_eq!(ns_to_ticks_impl(999, 0), 0);
    }

    // ===== HAL-001: INTID 边界 =====

    #[test]
    fn irq_in_range_basics() {
        assert!(irq_in_range(0, 256));
        assert!(irq_in_range(255, 256));
        assert!(!irq_in_range(256, 256));
        assert!(!irq_in_range(1022, 256)); // 保留 INTID
        assert!(!irq_in_range(1023, 256)); // spurious
        assert!(!irq_in_range(4096, 256));
        assert!(!irq_in_range(u32::MAX, 256));
    }
}

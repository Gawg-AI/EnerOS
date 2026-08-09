//! 21-B B3：TSN 无真实数据面时的降级路径与指标。
//!
//! v1.0.0 的 TSN 交付为 trait/算法层（`v1.0.0_scope = "trait-only"`，
//! Cargo.toml `[package.metadata.eneros]` 声明）——无真实 netlink/taprio
//! 网卡下发。依赖 TSN 真数据面的 release 路径**必须**走本模块的降级
//! 分支并记录指标（可观测），禁止静默假装 TSN 生效。

use core::sync::atomic::{AtomicU64, Ordering};

/// 数据面可用性检测（v1.0.0 事实：无真 TSN 驱动 → 恒 `false`）。
///
/// 真机 TSN 网卡驱动接入后，由驱动注册点改为返回 `true`（单一事实源：
/// 驱动注册成功 = 可用）。**禁止**绕过本检测直接调用真数据面。
pub fn dataplane_available() -> bool {
    // v1.0.0：无 netlink/taprio 实现（ADR：trait-only 范围）。
    false
}

/// TSN 降级指标计数器（原子，no_std 可观测）。
///
/// 调用方（指标上报/日志）读取 `count()`；测试断言降级路径被触发。
#[derive(Debug, Default)]
pub struct DegradationCounter {
    count: AtomicU64,
}

impl DegradationCounter {
    /// 创建计数器（初始 0）.
    pub const fn new() -> Self {
        Self {
            count: AtomicU64::new(0),
        }
    }

    /// 记录一次降级触发.
    pub fn record_degradation(&self) {
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    /// 累计降级次数（指标可观测）.
    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }
}

/// 降级执行：数据面不可用（`available == false`）时执行 `fallback` 并
/// 记录指标；可用时不降级（返回 `false`，调用方走真实数据面）。
///
/// `available` 由调用方从 [`dataplane_available`] 获取（测试可注入
/// 两种取值覆盖两个分支）。
///
/// # 返回值
/// - `true`：本次走了降级分支（fallback 已执行、指标已记录）
/// - `false`：数据面可用，未降级
pub fn degraded_or<F: FnOnce()>(
    available: bool,
    counter: &DegradationCounter,
    fallback: F,
) -> bool {
    if available {
        return false;
    }
    fallback();
    counter.record_degradation();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_dataplane_unavailable() {
        // v1.0.0 事实：无真 TSN 数据面。
        assert!(!dataplane_available());
    }

    #[test]
    fn degraded_branch_runs_fallback_and_records_metric() {
        let counter = DegradationCounter::new();
        let mut fallback_ran = false;
        let degraded = degraded_or(false, &counter, || fallback_ran = true);
        assert!(degraded, "数据面不可用必须走降级");
        assert!(fallback_ran, "fallback 必须执行");
        assert_eq!(counter.count(), 1, "降级指标必须记录");
    }

    #[test]
    fn available_branch_skips_fallback() {
        let counter = DegradationCounter::new();
        let mut fallback_ran = false;
        let degraded = degraded_or(true, &counter, || fallback_ran = true);
        assert!(!degraded, "数据面可用时不降级");
        assert!(!fallback_ran, "可用时 fallback 不执行");
        assert_eq!(counter.count(), 0, "可用时不记录降级指标");
    }
}

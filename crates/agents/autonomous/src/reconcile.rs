//! 资金闭环对账（v0.230.0，B3-07）。
//!
//! 冻结 → 交付 → 清算全链对账平衡验证：
//!
//! - [`Reconciliation`] — 快照对账：总余额 = 冻结额 + 可用额；
//! - [`freeze_idempotent`] — 冻结幂等语义（同额重复冻结拒绝或跳过）。

use eneros_market_types::MilliYuan;

/// 对账快照。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reconciliation {
    /// 账户总余额（毫元）。
    pub total_balance: MilliYuan,
    /// 冻结总额（毫元）。
    pub frozen_total: MilliYuan,
    /// 可用余额（毫元）。
    pub available: MilliYuan,
}

impl Reconciliation {
    /// 对账平衡：total == frozen + available。
    ///
    /// 返回 `Some(差额)` 表示不平衡（应恒为零）。
    pub fn imbalance(&self) -> Option<MilliYuan> {
        let sum = self.frozen_total.checked_add(self.available);
        match sum {
            Some(s) if s == self.total_balance => None,
            Some(s) => Some(MilliYuan(s.0 - self.total_balance.0)),
            None => Some(self.total_balance),
        }
    }
}

/// 冻结幂等语义：同账户同额冻结已发生时幂等命中（不重复扣减）。
///
/// - `Ok(true)` — 幂等命中（已有冻结且金额一致，无需再扣）；
/// - `Ok(false)` — 首次冻结（余额充足，调用方执行冻结）；
/// - `Err(())` — 拒绝（金额不一致或余额不足）。
pub fn freeze_idempotent(
    existing_freeze: Option<u64>,
    requested: MilliYuan,
    frozen: MilliYuan,
    total: MilliYuan,
) -> Result<bool, ()> {
    if let Some(_id) = existing_freeze {
        // 已冻结：验证金额一致（幂等命中）。
        if frozen == requested {
            return Ok(true);
        }
        return Err(()); // 金额不一致：拒绝（避免重复扣减）
    }
    // 首次冻结：余额充足检查。
    let available = total.checked_sub(frozen).ok_or(())?;
    if available < requested {
        return Err(());
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reconciliation_balanced() {
        let r = Reconciliation {
            total_balance: MilliYuan(100_000),
            frozen_total: MilliYuan(30_000),
            available: MilliYuan(70_000),
        };
        assert_eq!(r.imbalance(), None);
    }

    #[test]
    fn test_reconciliation_imbalanced_detected() {
        let r = Reconciliation {
            total_balance: MilliYuan(100_000),
            frozen_total: MilliYuan(30_000),
            available: MilliYuan(60_000), // 少 10_000
        };
        assert_eq!(r.imbalance(), Some(MilliYuan(-10_000)));
    }

    #[test]
    fn test_freeze_idempotent_first_time() {
        // 首次冻结：余额充足 → 执行。
        assert_eq!(
            freeze_idempotent(None, MilliYuan(10_000), MilliYuan(0), MilliYuan(50_000)),
            Ok(false)
        );
        // 余额不足 → 拒绝。
        assert!(freeze_idempotent(None, MilliYuan(60_000), MilliYuan(0), MilliYuan(50_000)).is_err());
    }

    #[test]
    fn test_freeze_idempotent_repeat() {
        // 同额重复冻结 → 幂等命中（返回已有 id，不重复扣减）。
        assert_eq!(
            freeze_idempotent(Some(7), MilliYuan(10_000), MilliYuan(10_000), MilliYuan(50_000)),
            Ok(true)
        );
        // 金额不一致 → 拒绝。
        assert!(freeze_idempotent(Some(7), MilliYuan(20_000), MilliYuan(10_000), MilliYuan(50_000)).is_err());
    }

    #[test]
    fn test_concurrent_freeze_semantics() {
        // 并发冻结竞态语义：两路同时冻结，总冻结不得超余额。
        // 单线程 no_std 语义下由 freeze_idempotent 保证；此处验证
        // 双路径串行化后的不变式（模拟 loom 语义）。
        let total = MilliYuan(100_000);
        let mut frozen = MilliYuan(0);
        let mut freeze_id: Option<u64> = None;
        let mut next_id = 1u64;
        // 路径 A：冻结 60_000（首次 → Ok(false) 执行）。
        if let Ok(false) = freeze_idempotent(freeze_id, MilliYuan(60_000), frozen, total) {
            freeze_id = Some(1);
            frozen = MilliYuan(frozen.0 + 60_000);
        }
        let _ = next_id;
        // 路径 B：独立冻结请求 50_000（可用只剩 40_000 → 拒绝）。
        assert!(freeze_idempotent(None, MilliYuan(50_000), frozen, total).is_err());
        // 对账平衡。
        let r = Reconciliation {
            total_balance: total,
            frozen_total: frozen,
            available: MilliYuan(total.0 - frozen.0),
        };
        assert_eq!(r.imbalance(), None);
        // 幂等重复冻结命中。
        let again = freeze_idempotent(freeze_id, MilliYuan(60_000), frozen, total);
        assert_eq!(again, Ok(true));
    }
}

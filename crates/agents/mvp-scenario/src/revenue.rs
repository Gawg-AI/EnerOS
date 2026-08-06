//! 收益对比器 — 追踪双脑 EMS vs 传统 EMS 收益（D12：合并 RevenueTracker/Comparator）.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

pub use eneros_market_types::MilliYuan;

/// 收益对比器（ARC-003：收益金额毫元定点）。
///
/// 记录双脑 EMS（LLM + Solver）与传统 EMS（规则策略）每 tick 的收益
/// （LP 目标值经毫元定点折算——LP 求解器内部保持 f64 数值，报告层定点）。
/// 计算 MVP 出口标准要求的"收益提升 ≥ 10%"指标。
pub struct RevenueComparator {
    /// 双脑 EMS 每次记录的收益（毫元）.
    pub dual_brain_revenue: Vec<MilliYuan>,
    /// 传统 EMS 每次记录的收益（毫元）.
    pub traditional_revenue: Vec<MilliYuan>,
}

impl RevenueComparator {
    /// 创建空对比器.
    pub fn new() -> Self {
        Self {
            dual_brain_revenue: Vec::new(),
            traditional_revenue: Vec::new(),
        }
    }

    /// 记录双脑 EMS 收益（元 f64 → 毫元定点）.
    pub fn record_dual_brain(&mut self, revenue: f64) {
        self.dual_brain_revenue.push(MilliYuan::from_yuan_f64(revenue));
    }

    /// 记录传统 EMS 收益（元 f64 → 毫元定点）.
    pub fn record_traditional(&mut self, revenue: f64) {
        self.traditional_revenue.push(MilliYuan::from_yuan_f64(revenue));
    }

    /// 双脑 EMS 累计收益（毫元）.
    pub fn dual_brain_total(&self) -> MilliYuan {
        let mut t = MilliYuan(0);
        for v in &self.dual_brain_revenue {
            if let Some(x) = t.checked_add(*v) {
                t = x;
            }
        }
        t
    }

    /// 传统 EMS 累计收益（毫元）.
    pub fn traditional_total(&self) -> MilliYuan {
        let mut t = MilliYuan(0);
        for v in &self.traditional_revenue {
            if let Some(x) = t.checked_add(*v) {
                t = x;
            }
        }
        t
    }

    /// 收益提升百分比 = (dual - trad) / trad * 100.
    ///
    /// B14 修复：当 `traditional_total == 0` 时提升比**无定义**，返回
    /// `f64::NAN`——`meets_target()` 中 `NaN >= 10.0` 为 false，出口标准
    /// 不达标（旧实现返回 `f64::INFINITY`，`INFINITY >= 10.0` 恒真，
    /// 使 trad=0 时出口测试恒过——自欺指标）。
    pub fn improvement_pct(&self) -> f64 {
        let trad = self.traditional_total();
        if trad.0 == 0 {
            return f64::NAN;
        }
        let dual = self.dual_brain_total();
        (dual.0 as f64 - trad.0 as f64) / trad.0 as f64 * 100.0
    }

    /// 是否达到 Phase 1 出口标准：`improvement_pct() >= 10.0`.
    ///
    /// trad=0（提升比无定义）时返回 false（B14，fail-closed）。
    pub fn meets_target(&self) -> bool {
        let pct = self.improvement_pct();
        if pct.is_nan() {
            return false;
        }
        pct >= 10.0
    }

    /// 生成结构化对比报告.
    ///
    /// 格式：双脑总收益 / 传统总收益 / 提升百分比 / 是否达标.
    pub fn report(&self) -> String {
        let dual = self.dual_brain_total();
        let trad = self.traditional_total();
        let pct = self.improvement_pct();
        let pass = if self.meets_target() { "PASS" } else { "FAIL" };
        // INFINITY 时显示为 "inf"（core::fmt::Float 的默认行为）.
        format!(
            "RevenueComparator {{ dual_brain_total: {}毫元, traditional_total: {}毫元, improvement_pct: {:.1}, meets_target_10pct: {} }}",
            dual.0, trad.0, pct, pass
        )
    }
}

impl Default for RevenueComparator {
    fn default() -> Self {
        Self::new()
    }
}

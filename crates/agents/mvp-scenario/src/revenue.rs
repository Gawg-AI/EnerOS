//! 收益对比器 — 追踪双脑 EMS vs 传统 EMS 收益（D12：合并 RevenueTracker/Comparator）.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// 收益对比器.
///
/// 记录双脑 EMS（LLM + Solver）与传统 EMS（规则策略）每 tick 的收益，
/// 计算 MVP 出口标准要求的"收益提升 ≥ 10%"指标。
pub struct RevenueComparator {
    /// 双脑 EMS 每次记录的收益（元）.
    pub dual_brain_revenue: Vec<f64>,
    /// 传统 EMS 每次记录的收益（元）.
    pub traditional_revenue: Vec<f64>,
}

impl RevenueComparator {
    /// 创建空对比器.
    pub fn new() -> Self {
        Self {
            dual_brain_revenue: Vec::new(),
            traditional_revenue: Vec::new(),
        }
    }

    /// 记录双脑 EMS 收益.
    pub fn record_dual_brain(&mut self, revenue: f64) {
        self.dual_brain_revenue.push(revenue);
    }

    /// 记录传统 EMS 收益.
    pub fn record_traditional(&mut self, revenue: f64) {
        self.traditional_revenue.push(revenue);
    }

    /// 双脑 EMS 累计收益.
    pub fn dual_brain_total(&self) -> f64 {
        self.dual_brain_revenue.iter().sum()
    }

    /// 传统 EMS 累计收益.
    pub fn traditional_total(&self) -> f64 {
        self.traditional_revenue.iter().sum()
    }

    /// 收益提升百分比 = (dual - trad) / trad * 100.
    ///
    /// 当 `traditional_total == 0` 时返回 `f64::INFINITY`（避免除零）.
    pub fn improvement_pct(&self) -> f64 {
        let trad = self.traditional_total();
        if trad == 0.0 {
            return f64::INFINITY;
        }
        (self.dual_brain_total() - trad) / trad * 100.0
    }

    /// 是否达到 Phase 1 出口标准：`improvement_pct() >= 10.0`.
    pub fn meets_target(&self) -> bool {
        self.improvement_pct() >= 10.0
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
            "RevenueComparator {{ dual_brain_total: {}, traditional_total: {}, improvement_pct: {}, meets_target_10pct: {} }}",
            dual, trad, pct, pass
        )
    }
}

impl Default for RevenueComparator {
    fn default() -> Self {
        Self::new()
    }
}

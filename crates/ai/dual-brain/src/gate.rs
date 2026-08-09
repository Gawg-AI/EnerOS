//! 22 §2.W5.2：AI 决策闸——全局速率上限 + 能量预算（防 AI 失控）。
//!
//! [`DecisionGate`] 在 coordinator 层对每条 AI 决策执行两道闸：
//!
//! 1. **速率闸**：滑动窗口（N 决策/分钟）——超限拒绝（防高频下发）；
//! 2. **能量闸**：滚动周期内累计调度能量（kWh）不超配置上限——超限拒绝。
//!
//! fail-closed：配置上限为 0 = 拒绝一切决策（宁可误伤不失控）；
//! 被拒计数独立统计（`decision.rejected.rate` / `.energy` 指标，
//! 与 W2.4 的 `decision.rejected.<reason>` 约定一致），调用方据此告警。

use alloc::collections::VecDeque;

/// 闸门判定结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateDecision {
    /// 放行（调用方随后 `record` 记账）。
    Allow,
    /// 速率超限（丢弃 + 告警）。
    RateLimited,
    /// 能量超限（拒绝 + 告警）。
    EnergyExceeded,
}

/// 决策闸（两闸一体；`now_ms` 由调用方提供，no_std 无系统时钟）。
pub struct DecisionGate {
    /// 速率上限（决策/分钟；0 = 全拒，fail-closed）。
    max_per_minute: u32,
    /// 滑动窗口（决策时间戳毫秒）。
    window: VecDeque<u64>,
    /// 能量上限（kWh/滚动周期；0.0 = 全拒）。
    max_kwh_per_period: f64,
    /// 滚动周期（毫秒）。
    period_ms: u64,
    /// 周期起点。
    period_start_ms: u64,
    /// 周期内已记账能量（kWh）。
    used_kwh: f64,
    /// 速率拒绝计数。
    rejected_rate: u64,
    /// 能量拒绝计数。
    rejected_energy: u64,
}

impl DecisionGate {
    /// 新建决策闸。
    ///
    /// - `max_per_minute`：速率上限，0 = 拒绝一切（fail-closed）；
    /// - `max_kwh_per_period`：周期能量上限，0.0 = 拒绝一切；
    /// - `period_ms`：能量滚动周期。
    pub fn new(max_per_minute: u32, max_kwh_per_period: f64, period_ms: u64) -> Self {
        Self {
            max_per_minute,
            window: VecDeque::new(),
            max_kwh_per_period,
            period_ms,
            period_start_ms: 0,
            used_kwh: 0.0,
            rejected_rate: 0,
            rejected_energy: 0,
        }
    }

    /// 检查本次决策是否放行（不记账；通过后调用 [`Self::record`]）。
    ///
    /// `energy_kwh` = 本次决策拟调度的能量（充/放合计，非负）。
    pub fn check(&mut self, now_ms: u64, energy_kwh: f64) -> GateDecision {
        match self.check_rate(now_ms) {
            GateDecision::Allow => self.check_energy(now_ms, energy_kwh),
            other => other,
        }
    }

    /// 速率闸（单独判定；execute 入口对所有决策路径调用——
    /// 快/慢/降级全过闸，防 FastPath 绕过）。
    pub fn check_rate(&mut self, now_ms: u64) -> GateDecision {
        // 速率闸：滑动窗口（1 分钟）。
        let cutoff = now_ms.saturating_sub(60_000);
        while self.window.front().is_some_and(|&t| t < cutoff) {
            self.window.pop_front();
        }
        let rate_ok = self.max_per_minute > 0 && (self.window.len() as u32) < self.max_per_minute;
        if !rate_ok {
            self.rejected_rate += 1;
            return GateDecision::RateLimited;
        }
        GateDecision::Allow
    }

    /// 能量闸（单独判定；调度解确定后调用）。
    pub fn check_energy(&mut self, now_ms: u64, energy_kwh: f64) -> GateDecision {
        // 能量闸：滚动周期（重置过期周期）。
        if self.period_ms > 0 && now_ms.saturating_sub(self.period_start_ms) >= self.period_ms {
            self.period_start_ms = now_ms;
            self.used_kwh = 0.0;
        }
        let energy_ok = self.max_kwh_per_period > 0.0
            && (self.used_kwh + energy_kwh.max(0.0)) <= self.max_kwh_per_period + 1e-9;
        if !energy_ok {
            self.rejected_energy += 1;
            return GateDecision::EnergyExceeded;
        }
        GateDecision::Allow
    }

    /// 放行后记账（决策已执行/下发）：登记速率窗口 + 累计能量。
    pub fn record(&mut self, now_ms: u64, energy_kwh: f64) {
        self.window.push_back(now_ms);
        self.used_kwh += energy_kwh.max(0.0);
    }

    /// 速率拒绝计数（指标 `decision.rejected.rate`）。
    pub fn rejected_rate(&self) -> u64 {
        self.rejected_rate
    }

    /// 能量拒绝计数（指标 `decision.rejected.energy`）。
    pub fn rejected_energy(&self) -> u64 {
        self.rejected_energy
    }

    /// 周期内已记账能量（kWh）。
    pub fn used_kwh(&self) -> f64 {
        self.used_kwh
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 验收：速率超限被拒 + 告警计数（第 N+1 条拒绝）。
    #[test]
    fn test_rate_limit_rejects_and_counts() {
        let mut gate = DecisionGate::new(2, 100.0, 60_000);
        // 前 2 条放行。
        for i in 0..2 {
            assert_eq!(gate.check(1000 + i * 100, 1.0), GateDecision::Allow);
            gate.record(1000 + i * 100, 1.0);
        }
        // 第 3 条（同一分钟窗口）→ 速率拒绝。
        assert_eq!(gate.check(1300, 1.0), GateDecision::RateLimited);
        assert_eq!(gate.rejected_rate(), 1);
        // 下一分钟窗口清空 → 放行。
        assert_eq!(gate.check(70_000, 1.0), GateDecision::Allow);
    }

    /// 验收：能量超限被拒（周期内累计超上限）。
    #[test]
    fn test_energy_budget_rejects() {
        let mut gate = DecisionGate::new(100, 10.0, 60_000);
        // 2 次 4kWh：8 ≤ 10 放行。
        assert_eq!(gate.check(0, 4.0), GateDecision::Allow);
        gate.record(0, 4.0);
        assert_eq!(gate.check(100, 4.0), GateDecision::Allow);
        gate.record(100, 4.0);
        // 第 3 次 4kWh：12 > 10 → 能量拒绝。
        assert_eq!(gate.check(200, 4.0), GateDecision::EnergyExceeded);
        assert_eq!(gate.rejected_energy(), 1);
        assert_eq!(gate.used_kwh(), 8.0, "被拒决策不记账");
        // 周期滚动（> 60s）→ 重置 → 放行。
        assert_eq!(gate.check(61_000, 4.0), GateDecision::Allow);
    }

    /// 验收：fail-closed——上限 0 = 全部拒绝。
    #[test]
    fn test_fail_closed_zero_config() {
        let mut gate = DecisionGate::new(0, 0.0, 60_000);
        assert_eq!(gate.check(0, 0.0), GateDecision::RateLimited);
        assert_eq!(gate.check(1, 0.0), GateDecision::RateLimited);
        // 能量 0 也全拒（fail-closed）。
        let mut gate2 = DecisionGate::new(100, 0.0, 60_000);
        assert_eq!(gate2.check(0, 1.0), GateDecision::EnergyExceeded);
    }

    /// 验收：速率窗口只计放行决策（被拒不计入）。
    #[test]
    fn test_rejected_decision_not_recorded() {
        let mut gate = DecisionGate::new(1, 100.0, 60_000);
        assert_eq!(gate.check(0, 1.0), GateDecision::Allow);
        gate.record(0, 1.0);
        assert_eq!(gate.check(100, 1.0), GateDecision::RateLimited);
        // 窗口仍只有 1 条；窗口外后放行。
        assert_eq!(gate.check(61_000, 1.0), GateDecision::Allow);
    }
}

//! 模型生命周期（[11] §3.7）：
//! `Registered → OfflineEvaluated → Shadow → Canary → Approved →
//! Suspended/Retired`，任一时刻可回滚到上一 Approved 包。
//!
//! 纪律：
//! - Offline 必须带评测套件摘要匹配 + 通过报告（golden/对抗/偏差/安全/
//!   拒绝率）；
//! - Shadow 只读真实快照，**不进入 ObjectiveModule**（本模块在输出面打
//!   `shadow` 标记，消费方不得转发）；
//! - Canary 只允许低风险、白名单 purpose；
//! - rollback：健康/输出/拒绝/延迟/漂移阈值触发，回到上一 Approved 包；
//! - 模型更新不改变规则/安全阈值；新包新 digest（registry 侧强制）。

use alloc::vec::Vec;

use crate::error::LifecycleStage;

/// 生命周期状态（[11] §3.7 唯一路径）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifecycle {
    Registered,
    OfflineEvaluated,
    Shadow,
    Canary,
    Approved,
    Suspended,
    Retired,
}

impl Lifecycle {
    pub fn stage(&self) -> LifecycleStage {
        match self {
            Lifecycle::Registered => LifecycleStage::Registered,
            Lifecycle::OfflineEvaluated => LifecycleStage::OfflineEvaluated,
            Lifecycle::Shadow => LifecycleStage::Shadow,
            Lifecycle::Canary => LifecycleStage::Canary,
            Lifecycle::Approved => LifecycleStage::Approved,
            Lifecycle::Suspended => LifecycleStage::Suspended,
            Lifecycle::Retired => LifecycleStage::Retired,
        }
    }

    /// 是否允许推理（Shadow 读真实快照 → 推理；但输出不进入
    /// ObjectiveModule——消费侧按 [`crate::harness::HarnessOutcome::shadow`]
    /// 过滤）。
    pub fn may_infer(&self) -> bool {
        matches!(
            self,
            Lifecycle::Shadow | Lifecycle::Canary | Lifecycle::Approved
        )
    }

    /// 候选是否可进入 ObjectiveModule（Canary 允许但 purpose 白名单在
    /// 调用侧再查）。
    pub fn may_forward(&self) -> bool {
        matches!(self, Lifecycle::Approved)
    }
}

/// 离线评测报告（门：全部项通过才允许离开 Registered）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OfflineEvalReport {
    /// golden 通过率（0..=1）。
    pub golden_pass_rate: f64,
    /// 对抗样本通过率（0..=1）。
    pub adversarial_pass_rate: f64,
    /// 安全拒绝率（越权限输出被门拦截的比例；1 = 全拦）。
    pub safety_refusal_rate: f64,
    /// 偏差幅度（0..=1；如终值偏差）。
    pub deviation: f64,
    /// 输出门拒绝率（schema 层面）。
    pub gate_refusal_rate: f64,
}

impl OfflineEvalReport {
    /// 通过判据：golden/对抗 ≥ min，安全拒绝 ≥ min_refusal，
    /// 偏差/gate 拒绝 ≤ max。
    pub fn passes(&self, p: &LifecyclePolicy) -> bool {
        self.golden_pass_rate >= p.min_golden_pass_rate
            && self.adversarial_pass_rate >= p.min_adversarial_pass_rate
            && self.safety_refusal_rate >= p.min_safety_refusal_rate
            && self.deviation <= p.max_deviation
            && self.gate_refusal_rate <= p.max_gate_refusal_rate
    }
}

/// 实时健康信号（[11] §3.7 rollback 触发输入）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HealthSignal {
    /// 输出门拒绝率（最近窗口）。
    OutputRefusalRate(f64),
    /// 推理延迟（ns）。
    LatencyNs(u64),
    /// 漂移幅度（0..=1）。
    Drift(f64),
    /// 与影子真值不一致率（0..=1）。
    DisagreementRate(f64),
}

/// 生命周期政策（阈值；构造时校验 0..=1 与正延迟）。
#[derive(Debug, Clone, PartialEq)]
pub struct LifecyclePolicy {
    pub min_golden_pass_rate: f64,
    pub min_adversarial_pass_rate: f64,
    pub min_safety_refusal_rate: f64,
    pub max_deviation: f64,
    pub max_gate_refusal_rate: f64,
    /// 实时：拒绝率/漂移/不一致率阈值。
    pub max_refusal_rate: f64,
    pub max_drift: f64,
    pub max_disagreement: f64,
    /// 延迟超时（ns）→ Suspended。
    pub max_latency_ns: u64,
}

impl Default for LifecyclePolicy {
    fn default() -> Self {
        Self {
            min_golden_pass_rate: 1.0,
            min_adversarial_pass_rate: 1.0,
            min_safety_refusal_rate: 1.0,
            max_deviation: 0.0,
            max_gate_refusal_rate: 0.0,
            max_refusal_rate: 0.05,
            max_drift: 0.05,
            max_disagreement: 0.05,
            max_latency_ns: 5_000_000_000,
        }
    }
}

impl LifecyclePolicy {
    /// 健康信号是否触发暂挂（fail-closed：任何一项越界 → 暂挂）。
    pub fn violated_by(&self, s: &HealthSignal) -> bool {
        match s {
            HealthSignal::OutputRefusalRate(r) => *r > self.max_refusal_rate,
            HealthSignal::LatencyNs(ns) => *ns > self.max_latency_ns,
            HealthSignal::Drift(d) => *d > self.max_drift,
            HealthSignal::DisagreementRate(r) => *r > self.max_disagreement,
        }
    }
}

/// 包生命周期记录（registry 内每包一份）。
#[derive(Debug, Clone, PartialEq)]
pub struct PackageLifecycle {
    pub package_id: [u8; 16],
    pub state: Lifecycle,
    /// 上一 Approved 包（回滚目标；[11] §3.7 rollback 回到上一 Approved）。
    pub rollback_to: Option<[u8; 16]>,
    /// 评测报告（Approved 路径必需）。
    pub eval: Option<OfflineEvalReport>,
}

impl PackageLifecycle {
    pub fn registered(package_id: [u8; 16], rollback_to: Option<[u8; 16]>) -> Self {
        Self {
            package_id,
            state: Lifecycle::Registered,
            rollback_to,
            eval: None,
        }
    }

    /// Registered → OfflineEvaluated（Offline 门：评测报告通过）。
    pub fn finish_eval(&mut self, report: &OfflineEvalReport, policy: &LifecyclePolicy) {
        if self.state != Lifecycle::Registered {
            return;
        }
        if !report.passes(policy) {
            return; // 门不过：停在 Registered（不能进入推理面）。
        }
        self.state = Lifecycle::OfflineEvaluated;
        self.eval = Some(report.clone());
    }

    /// OfflineEvaluated → Shadow。
    pub fn enter_shadow(&mut self) {
        if self.state == Lifecycle::OfflineEvaluated {
            self.state = Lifecycle::Shadow;
        }
    }

    /// Shadow → Canary（shadow 健康由调用方确认；本方法只做状态门）。
    pub fn enter_canary(&mut self) {
        if self.state == Lifecycle::Shadow {
            self.state = Lifecycle::Canary;
        }
    }

    /// Canary → Approved（canary 门由调用方确认——低风险 purpose 白名单
    /// 在 harness 层）。
    pub fn approve(&mut self) {
        if self.state == Lifecycle::Canary {
            self.state = Lifecycle::Approved;
        }
    }

    /// 健康信号 → Suspended（fail-closed：信号越界即停；不立即退役）。
    pub fn suspend_on(&mut self, s: &HealthSignal, policy: &LifecyclePolicy) -> bool {
        if !self.state.may_infer() {
            return false;
        }
        if policy.violated_by(s) {
            self.state = Lifecycle::Suspended;
            return true;
        }
        false
    }

    /// Suspended → Retired（人工/运维调用；不可逆）。
    pub fn retire(&mut self) {
        if self.state == Lifecycle::Suspended {
            self.state = Lifecycle::Retired;
        }
    }

    /// 回滚：当前包退回 Registered 并返回上一 Approved 包 id。
    pub fn rollback(&mut self) -> Option<[u8; 16]> {
        if !matches!(self.state, Lifecycle::Suspended | Lifecycle::Approved) {
            return None;
        }
        self.state = Lifecycle::Registered;
        self.rollback_to
    }
}

/// 回滚触发后的选择结果（registry::rollback 使用）。
#[derive(Debug, Clone, PartialEq)]
pub enum RollbackAction {
    /// 回滚到候选包（上一 Approved）成功。
    RolledBack { new_active: [u8; 16] },
    /// 无上一 Approved 包：当前包停用（系统无模型可用，不降级规则）。
    NoFallback,
}

/// 生命周期簿（registry 持有；重启恢复点）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LifecycleBook {
    pub entries: Vec<PackageLifecycle>,
}

impl LifecycleBook {
    pub fn get_mut(&mut self, id: &[u8; 16]) -> Option<&mut PackageLifecycle> {
        self.entries.iter_mut().find(|e| e.package_id == *id)
    }
    pub fn get(&self, id: &[u8; 16]) -> Option<&PackageLifecycle> {
        self.entries.iter().find(|e| e.package_id == *id)
    }
    pub fn all(&self) -> &[PackageLifecycle] {
        &self.entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good_report() -> OfflineEvalReport {
        OfflineEvalReport {
            golden_pass_rate: 1.0,
            adversarial_pass_rate: 1.0,
            safety_refusal_rate: 1.0,
            deviation: 0.0,
            gate_refusal_rate: 0.0,
        }
    }

    #[test]
    fn lifecycle_path_registered_to_approved() {
        let mut lc = PackageLifecycle::registered([1; 16], None);
        assert!(!lc.state.may_infer());
        // Offline 门不过 → 停在 Registered。
        let mut bad = good_report();
        bad.adversarial_pass_rate = 0.5;
        lc.finish_eval(&bad, &LifecyclePolicy::default());
        assert_eq!(lc.state, Lifecycle::Registered);
        // 门过 → OfflineEvaluated。
        lc.finish_eval(&good_report(), &LifecyclePolicy::default());
        assert_eq!(lc.state, Lifecycle::OfflineEvaluated);
        assert!(!lc.state.may_infer());
        lc.enter_shadow();
        assert_eq!(lc.state, Lifecycle::Shadow);
        assert!(lc.state.may_infer());
        assert!(!lc.state.may_forward());
        lc.enter_canary();
        assert_eq!(lc.state, Lifecycle::Canary);
        lc.approve();
        assert_eq!(lc.state, Lifecycle::Approved);
        assert!(lc.state.may_forward());
    }

    #[test]
    fn health_signal_suspends_then_rollback() {
        let mut lc = PackageLifecycle::registered([1; 16], Some([2; 16]));
        lc.finish_eval(&good_report(), &LifecyclePolicy::default());
        lc.enter_shadow();
        lc.enter_canary();
        lc.approve();
        let suspended = lc.suspend_on(
            &HealthSignal::OutputRefusalRate(0.9),
            &LifecyclePolicy::default(),
        );
        assert!(suspended);
        assert_eq!(lc.state, Lifecycle::Suspended);
        assert_eq!(lc.rollback(), Some([2; 16]));
        assert_eq!(lc.state, Lifecycle::Registered);
    }

    #[test]
    fn benign_signal_no_suspend() {
        let mut lc = PackageLifecycle::registered([1; 16], None);
        lc.finish_eval(&good_report(), &LifecyclePolicy::default());
        lc.enter_shadow();
        let s = lc.suspend_on(&HealthSignal::Drift(0.01), &LifecyclePolicy::default());
        assert!(!s);
        assert_eq!(lc.state, Lifecycle::Shadow);
        let s = lc.suspend_on(
            &HealthSignal::LatencyNs(10_000_000_000),
            &LifecyclePolicy::default(),
        );
        assert!(s);
        assert_eq!(lc.state, Lifecycle::Suspended);
    }
}

//! RecoveryManager — 恢复过渡管理器（D10：纯状态，不持有 protocol）.
//!
//! [`RecoveryManager`] 维护从降级值到 Agent 设定值的线性插值过渡状态。
//! 所有 I/O 操作（读写点表）由 [`crate::flow::WatchdogDegradeFlow`] 执行，
//! `RecoveryManager` 仅负责状态计算（D10）。

/// 恢复过渡管理器（纯状态，D10）.
///
/// 不持有 protocol 引用；`transition_step` 返回插值结果，由调用方下发。
#[derive(Debug, Clone)]
pub struct RecoveryManager {
    /// 降级前保存的设定值（Normal → Degrading 时保存）。
    pub saved_setpoint: Option<f64>,
    /// 过渡开始时间（纳秒），`None` 表示未开始过渡。
    pub transition_start_ns: Option<u64>,
    /// 过渡总时长（纳秒）。
    pub transition_duration_ns: u64,
    /// 当前过渡进度（0.0 ~ 1.0）。
    pub progress: f64,
    /// 降级时的设定值（过渡起点）。
    pub degraded_setpoint: f64,
    /// Agent 恢复后的设定值（过渡终点）。
    pub agent_setpoint: Option<f64>,
}

impl RecoveryManager {
    /// 创建恢复管理器.
    pub fn new(transition_duration_ns: u64) -> Self {
        Self {
            saved_setpoint: None,
            transition_start_ns: None,
            transition_duration_ns,
            progress: 0.0,
            degraded_setpoint: 0.0,
            agent_setpoint: None,
        }
    }

    /// 保存降级前设定值（Normal → Degrading 转换时调用）.
    pub fn save_setpoint(&mut self, current_value: f64) {
        self.saved_setpoint = Some(current_value);
    }

    /// 启动过渡（Degraded → Recovering 转换时调用）.
    ///
    /// 记录 `transition_start_ns`、`degraded_setpoint`、`agent_setpoint`，
    /// 重置 `progress = 0.0`。
    pub fn start_transition(&mut self, degraded_value: f64, agent_value: f64, now_ns: u64) {
        self.transition_start_ns = Some(now_ns);
        self.degraded_setpoint = degraded_value;
        self.agent_setpoint = Some(agent_value);
        self.progress = 0.0;
    }

    /// 过渡步进（Recovering 状态每 tick 调用）.
    ///
    /// 计算 `progress = min(1.0, elapsed / transition_duration)`，
    /// 返回线性插值 `degraded + (agent - degraded) * progress`。
    ///
    /// 若 `progress >= 1.0`（过渡完成）或未启动过渡，返回 `None`。
    pub fn transition_step(&mut self, now_ns: u64) -> Option<f64> {
        let start_ns = self.transition_start_ns?;
        let agent = self.agent_setpoint?;

        let elapsed = now_ns.saturating_sub(start_ns);
        let progress = if self.transition_duration_ns == 0 {
            1.0
        } else {
            let ratio = elapsed as f64 / self.transition_duration_ns as f64;
            if ratio > 1.0 {
                1.0
            } else {
                ratio
            }
        };
        self.progress = progress;

        if progress >= 1.0 {
            return None;
        }

        let value = self.degraded_setpoint + (agent - self.degraded_setpoint) * progress;
        Some(value)
    }

    /// 过渡是否完成（`progress >= 1.0`）。
    pub fn is_complete(&self) -> bool {
        self.progress >= 1.0
    }

    /// 完成过渡（Recovering → Normal 转换时调用）.
    ///
    /// 重置 `transition_start_ns = None`，设 `progress = 1.0`。
    pub fn complete(&mut self) {
        self.transition_start_ns = None;
        self.progress = 1.0;
    }
}

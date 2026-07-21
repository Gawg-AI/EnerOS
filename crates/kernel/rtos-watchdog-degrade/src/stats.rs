//! FlowStats / FlowReport — 端到端降级流程统计与报告（D4：不使用 AtomicU64）.
//!
//! [`FlowStats`] 累计跨 tick 统计（普通 `u64`，单线程，D4）；
//! [`FlowReport`] 描述单次 tick 汇总。

use eneros_rtos_cmd_exec::stats::ExecutorReport;
use eneros_rtos_degrade::stats::DegradeReport;
use eneros_watchdog::WatchdogStatus;

use crate::heartbeat::HeartbeatStatus;
use crate::state::DegradeState;

/// 端到端降级流程累计统计（D4：普通 u64，非 AtomicU64）.
#[derive(Debug, Clone, Default)]
pub struct FlowStats {
    /// 状态转换总次数。
    pub state_transitions: u64,
    /// 紧急停机次数（进入 Emergency 状态）。
    pub emergency_count: u64,
    /// 恢复完成次数（Recovering → Normal）。
    pub recovery_count: u64,
    /// 心跳超时次数（Timeout + Dead）。
    pub heartbeat_timeouts: u64,
    /// 降级引擎评估次数。
    pub degrade_evaluations: u64,
    /// 命令执行器 tick 次数。
    pub cmds_executed: u64,
}

/// 单次 tick 汇总报告.
///
/// 注意：不派生 `Clone`，因为 `WatchdogStatus`（v0.13.0）未实现 `Clone`。
#[derive(Debug)]
pub struct FlowReport {
    /// tick 结束后的状态。
    pub state: DegradeState,
    /// 本次 tick 是否发生状态转换。
    pub state_changed: bool,
    /// 心跳检查结果。
    pub heartbeat: HeartbeatStatus,
    /// 命令执行器报告（Normal 状态有效，其余为默认）。
    pub cmd_report: ExecutorReport,
    /// 降级引擎报告（Degrading/Degraded 状态有效，其余为默认）。
    pub degrade_report: DegradeReport,
    /// 看门狗检查结果。
    pub watchdog: WatchdogStatus,
}

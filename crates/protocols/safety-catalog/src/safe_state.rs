//! 设备 safe-state 目录 (CTL-001)。
//!
//! MASTER §9.4：定义 safe state——掉网、掉电、传感器坏、时间坏、存储满、Agent 失控、
//! 控制服务 crash。每个设备类别在每种异常条件下必须落入已定义的安全方向（fail-safe），
//! 而非继续动作。

use crate::hazard::Severity;

/// 触发进入 safe-state 的异常条件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafeStateTrigger {
    /// 电网失压 / 孤岛。
    GridLoss,
    /// 辅助电源掉电。
    PowerLoss,
    /// 传感器失效 / 坏值 / 超量程。
    SensorFault,
    /// 可信时间不可用（证书 / 防回滚依赖）。
    UntrustedTime,
    /// 持久存储满 / 损坏。
    StorageFault,
    /// Agent 失控 / 死循环 / OOM。
    AgentRunaway,
    /// 控制服务 crash。
    ControlServiceCrash,
    /// 通信丢失（协议对端断链）。
    CommLoss,
    /// 安全监视器超时未喂狗。
    SafetyWatchdog,
    /// 过流故障（保护动作触发）。
    OvercurrentFault,
    /// 过温故障。
    ThermalFault,
}

/// safe-state 动作方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafeAction {
    /// 分断并网点（形成电气孤岛隔离）。
    OpenPcc,
    /// 停机变流器（封波，不输出电流）。
    BlockInverter,
    /// 停止充电（仅允许放电到安全 SOC）。
    StopCharge,
    /// 停止放电（仅允许充电到安全 SOC）。
    StopDischarge,
    /// 切换到最近有效的安全命令，TTL 到期进入 `SafeDefault`。
    HoldLastSafeCommand,
    /// 进入全局安全默认（全部分断、封波、零功率）。
    SafeDefault,
    /// 降级到确定性本地策略（禁用 LLM/AI 路径）。
    DegradeLocal,
}

/// 设备类别的 safe-state 规则。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SafeState {
    /// 设备类别（如 `"pcs"`、`"bms"`、`"pcc_breaker"`、`"pv_inverter"`）。
    pub device_class: &'static str,
    pub trigger: SafeStateTrigger,
    pub action: SafeAction,
    /// 该 safe-state 兜底覆盖的危险严重度上限（即至少能覆盖到该严重度的事故）。
    pub covers_severity: Severity,
    pub rationale: &'static str,
}

impl SafeState {
    pub const fn new(
        device_class: &'static str,
        trigger: SafeStateTrigger,
        action: SafeAction,
        covers_severity: Severity,
        rationale: &'static str,
    ) -> Self {
        Self {
            device_class,
            trigger,
            action,
            covers_severity,
            rationale,
        }
    }
}

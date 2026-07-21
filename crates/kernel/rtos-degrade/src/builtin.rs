//! 内置降级规则（5 条，D12）.
//!
//! 覆盖常见故障场景：
//! - [`AgentDeadRule`] — Agent 死亡/心跳超时 → SafeDefault（priority=100）
//! - [`ControlBusDownRule`] — 控制总线断开 → HoldOutput（priority=90）
//! - [`DeviceCommFailRule`] — 设备通信失败 → SafeDefault（priority=80）
//! - [`LowBatteryRule`] — 低电量 → StopCharge（priority=70）
//! - [`OverTempRule`] — 过温 → StopCharge（priority=60）

use crate::context::DegradeContext;
use crate::mode::DegradeMode;
use crate::rule::DegradeRule;

/// Agent 心跳超时阈值（5 秒，纳秒）.
pub const HEARTBEAT_TIMEOUT_NS: u64 = 5_000_000_000;

/// 低电量阈值（SOC < 10%）.
pub const LOW_BATTERY_THRESHOLD: f64 = 10.0;

/// 过温阈值（温度 > 80℃）.
pub const OVER_TEMP_THRESHOLD: f64 = 80.0;

/// Agent 死亡规则 — Agent 不存活或心跳超时 → SafeDefault.
pub struct AgentDeadRule;

impl DegradeRule for AgentDeadRule {
    fn name(&self) -> &str {
        "agent_dead"
    }

    fn priority(&self) -> u8 {
        100
    }

    fn evaluate(&self, ctx: &DegradeContext) -> Option<DegradeMode> {
        if !ctx.agent_alive {
            return Some(DegradeMode::SafeDefault);
        }
        let elapsed = ctx.now_ns.saturating_sub(ctx.agent_last_heartbeat_ns);
        if elapsed > HEARTBEAT_TIMEOUT_NS {
            return Some(DegradeMode::SafeDefault);
        }
        None
    }
}

/// 控制总线断开规则 — 总线不活跃 → HoldOutput.
pub struct ControlBusDownRule;

impl DegradeRule for ControlBusDownRule {
    fn name(&self) -> &str {
        "control_bus_down"
    }

    fn priority(&self) -> u8 {
        90
    }

    fn evaluate(&self, ctx: &DegradeContext) -> Option<DegradeMode> {
        if !ctx.control_bus_active {
            return Some(DegradeMode::HoldOutput);
        }
        None
    }
}

/// 设备通信失败规则 — 通信异常 → SafeDefault.
pub struct DeviceCommFailRule;

impl DegradeRule for DeviceCommFailRule {
    fn name(&self) -> &str {
        "device_comm_fail"
    }

    fn priority(&self) -> u8 {
        80
    }

    fn evaluate(&self, ctx: &DegradeContext) -> Option<DegradeMode> {
        if !ctx.device_comm_ok {
            return Some(DegradeMode::SafeDefault);
        }
        None
    }
}

/// 低电量规则 — SOC < 10% → StopCharge.
pub struct LowBatteryRule;

impl DegradeRule for LowBatteryRule {
    fn name(&self) -> &str {
        "low_battery"
    }

    fn priority(&self) -> u8 {
        70
    }

    fn evaluate(&self, ctx: &DegradeContext) -> Option<DegradeMode> {
        if ctx.battery_soc < LOW_BATTERY_THRESHOLD {
            return Some(DegradeMode::StopCharge);
        }
        None
    }
}

/// 过温规则 — 温度 > 80℃ → StopCharge.
pub struct OverTempRule;

impl DegradeRule for OverTempRule {
    fn name(&self) -> &str {
        "over_temp"
    }

    fn priority(&self) -> u8 {
        60
    }

    fn evaluate(&self, ctx: &DegradeContext) -> Option<DegradeMode> {
        if ctx.temperature > OVER_TEMP_THRESHOLD {
            return Some(DegradeMode::StopCharge);
        }
        None
    }
}

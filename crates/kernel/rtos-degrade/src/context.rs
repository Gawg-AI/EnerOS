//! 降级上下文 — 规则评估输入.
//!
//! [`DegradeContext`] 携带当前系统状态快照，供 [`crate::rule::DegradeRule`] 评估。
//! 采用 builder 模式构造，所有字段可通过 `with_*` 链式设置。

/// 降级评估上下文（系统状态快照）.
///
/// 包含 Agent 心跳、控制总线、设备通信、电池/电网/温度等状态（D10），
/// 以及当前时间 `now_ns`（D5：参数注入替代 `MonotonicTime::now()`）。
#[derive(Debug, Clone, Copy, Default)]
pub struct DegradeContext {
    /// 当前时间（纳秒，D5 参数注入）。
    pub now_ns: u64,
    /// Agent 是否存活。
    pub agent_alive: bool,
    /// Agent 最后心跳时间（纳秒）。
    pub agent_last_heartbeat_ns: u64,
    /// 控制总线是否活跃。
    pub control_bus_active: bool,
    /// 设备通信是否正常。
    pub device_comm_ok: bool,
    /// 电池 SOC（0~100）。
    pub battery_soc: f64,
    /// 电网频率（Hz）。
    pub grid_frequency: f64,
    /// 温度（℃）。
    pub temperature: f64,
}

impl DegradeContext {
    /// 创建默认上下文（全零/全 false）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置当前时间（纳秒）。
    pub fn with_now_ns(mut self, v: u64) -> Self {
        self.now_ns = v;
        self
    }

    /// 设置 Agent 是否存活。
    pub fn with_agent_alive(mut self, v: bool) -> Self {
        self.agent_alive = v;
        self
    }

    /// 设置 Agent 最后心跳时间（纳秒）。
    pub fn with_agent_last_heartbeat_ns(mut self, v: u64) -> Self {
        self.agent_last_heartbeat_ns = v;
        self
    }

    /// 设置控制总线是否活跃。
    pub fn with_control_bus_active(mut self, v: bool) -> Self {
        self.control_bus_active = v;
        self
    }

    /// 设置设备通信是否正常。
    pub fn with_device_comm_ok(mut self, v: bool) -> Self {
        self.device_comm_ok = v;
        self
    }

    /// 设置电池 SOC。
    pub fn with_battery_soc(mut self, v: f64) -> Self {
        self.battery_soc = v;
        self
    }

    /// 设置电网频率。
    pub fn with_grid_frequency(mut self, v: f64) -> Self {
        self.grid_frequency = v;
        self
    }

    /// 设置温度。
    pub fn with_temperature(mut self, v: f64) -> Self {
        self.temperature = v;
        self
    }
}

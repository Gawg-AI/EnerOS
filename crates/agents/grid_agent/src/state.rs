//! GridState + DataQuality + GridAgent — 电网状态感知 Agent.

use alloc::boxed::Box;
use alloc::vec::Vec;

use eneros_agent::{AgentDescriptor, AgentState, AgentType};
use eneros_energy_market_agent::{AgentRuntime, AgentRuntimeError, HeartbeatStatus};

use crate::publisher::GridPublisher;
use crate::sampler::GridSampler;

/// 数据质量（保守默认 Invalid）.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DataQuality {
    /// 数据有效.
    Good,
    /// 数据无效（保守默认）.
    #[default]
    Invalid,
    /// 数据不确定.
    Uncertain,
}

/// 电网状态（12 字段）.
///
/// 描述电网实时运行状态，由采样器周期性采集。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GridState {
    /// 频率（Hz）
    pub frequency: f32,
    /// A 相电压（V）
    pub voltage_a: f32,
    /// B 相电压（V）
    pub voltage_b: f32,
    /// C 相电压（V）
    pub voltage_c: f32,
    /// A 相电流（A）
    pub current_a: f32,
    /// B 相电流（A）
    pub current_b: f32,
    /// C 相电流（A）
    pub current_c: f32,
    /// 有功功率（kW）
    pub active_power: f32,
    /// 无功功率（kVar）
    pub reactive_power: f32,
    /// 功率因数
    pub power_factor: f32,
    /// 时间戳（ms，外部提供）
    pub timestamp: u64,
    /// 数据质量
    pub quality: DataQuality,
}

/// 电网状态感知 Agent.
///
/// 周期性采样电网状态，执行异常检测，并通过发布器对外发布状态与告警。
/// 沿用 device-agent 模式：实现 `AgentRuntime` trait + sync `on_tick(now_ms)` API。
pub struct GridAgent {
    /// Agent 描述符
    pub descriptor: AgentDescriptor,
    /// 电网状态采样器
    pub sampler: Box<dyn GridSampler>,
    /// 状态/告警发布器
    pub publisher: Box<dyn GridPublisher>,
    /// 最新电网状态
    pub state: GridState,
    /// 异常检测器列表（任一返回 true 即触发告警）
    pub anomaly_handlers: Vec<fn(&GridState) -> bool>,
    /// 采样间隔（ms）
    pub sample_interval_ms: u64,
    /// Agent 生命周期状态
    pub agent_state: AgentState,
    /// tick 计数
    pub tick_count: u64,
}

impl GridAgent {
    /// 创建 Grid Agent.
    ///
    /// # 参数
    /// * `name` - Agent 名称
    /// * `sampler` - 电网状态采样器
    /// * `publisher` - 状态/告警发布器
    /// * `sample_interval_ms` - 采样间隔（ms）
    /// * `now_ms` - 当前时间戳（外部提供，遵循 no_std 惯例）
    pub fn new(
        name: &str,
        sampler: Box<dyn GridSampler>,
        publisher: Box<dyn GridPublisher>,
        sample_interval_ms: u64,
        now_ms: u64,
    ) -> Self {
        GridAgent {
            descriptor: AgentDescriptor::new(AgentType::Grid, name, now_ms),
            sampler,
            publisher,
            state: GridState::default(),
            anomaly_handlers: Vec::new(),
            sample_interval_ms,
            agent_state: AgentState::Created,
            tick_count: 0,
        }
    }

    /// 注册异常检测器（追加到 `anomaly_handlers`）.
    pub fn register_anomaly_detector(&mut self, detector: fn(&GridState) -> bool) {
        self.anomaly_handlers.push(detector);
    }

    /// 返回最新电网状态引用.
    pub fn current_state(&self) -> &GridState {
        &self.state
    }
}

impl AgentRuntime for GridAgent {
    fn descriptor(&self) -> &AgentDescriptor {
        &self.descriptor
    }

    fn on_start(&mut self, _now_ms: u64) -> Result<(), AgentRuntimeError> {
        self.agent_state = AgentState::Running;
        Ok(())
    }

    fn on_tick(&mut self, now_ms: u64) -> Result<(), AgentRuntimeError> {
        let new_state = self.sampler.sample(now_ms).map_err(|_| {
            AgentRuntimeError::DeviceError(alloc::string::String::from("grid sample failed"))
        })?;
        self.state = new_state;

        let has_anomaly = self.anomaly_handlers.iter().any(|d| d(&self.state));
        if has_anomaly {
            self.publisher.publish_alert(&self.state).map_err(|_| {
                AgentRuntimeError::DeviceError(alloc::string::String::from(
                    "grid alert publish failed",
                ))
            })?;
        }
        self.publisher.publish_state(&self.state).map_err(|_| {
            AgentRuntimeError::DeviceError(alloc::string::String::from("grid state publish failed"))
        })?;

        self.tick_count += 1;
        Ok(())
    }

    fn on_stop(&mut self, _now_ms: u64) -> Result<(), AgentRuntimeError> {
        self.agent_state = AgentState::Dead;
        Ok(())
    }

    fn on_heartbeat(&self, _now_ms: u64) -> HeartbeatStatus {
        if self.agent_state == AgentState::Running {
            HeartbeatStatus::Alive
        } else {
            HeartbeatStatus::Dead
        }
    }
}

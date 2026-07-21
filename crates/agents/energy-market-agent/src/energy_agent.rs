//! 能源调度 Agent — 编排双脑协调器执行储能调度（D9/D10/D11/D14）.

use alloc::boxed::Box;

use eneros_agent::{AgentDescriptor, AgentState, AgentType};
use eneros_dual_brain::coordinator::DualBrainMockEngine;
use eneros_dual_brain::{DualBrainCoordinator, MockCommandSink};
use eneros_energy_lp_model::config::ScheduleConfig;
use eneros_energy_lp_model::result::ScheduleResult;
use eneros_fast_path::state::RealtimeState;
use eneros_llm_engine::engine::LlmEngine;
use eneros_safety_validator::state::SystemState;
use eneros_solver_core::mock::MockSolver;

use crate::error::AgentRuntimeError;
use crate::market::MarketChannel;
use crate::runtime::{AgentRuntime, HeartbeatStatus};

/// 能源调度 Agent.
///
/// 持有 `DualBrainCoordinator<MockSolver>`，在 `on_tick` 中执行双脑链路。
/// 通过 `market_channel` 接收 Market Agent 转发的市场数据。
pub struct EnergyAgent {
    /// Agent 描述符.
    pub descriptor: AgentDescriptor,
    /// 双脑协调器（泛型 `MockSolver`）.
    pub coordinator: DualBrainCoordinator<MockSolver>,
    /// 市场数据通道（接收 Market Agent 转发的数据）.
    pub market_channel: MarketChannel,
    /// 当前调度方案（双脑执行结果）.
    pub current_schedule: Option<ScheduleResult>,
    /// 当前电价缓存（从市场数据更新）.
    pub current_price: f64,
    /// 生命周期状态.
    pub state: AgentState,
    /// tick 计数.
    pub tick_count: u64,
}

impl EnergyAgent {
    /// 构造 Energy Agent.
    ///
    /// `descriptor = AgentDescriptor::new(AgentType::Energy, name, now_ms)`（D7）。
    /// `coordinator = DualBrainCoordinator::new(config, llm, solver, sink)`（D9）。
    pub fn new(name: &str, config: ScheduleConfig, now_ms: u64) -> Self {
        let descriptor = AgentDescriptor::new(AgentType::Energy, name, now_ms);
        let llm_engine: Box<dyn LlmEngine> = Box::new(DualBrainMockEngine::new());
        let solver = MockSolver::new();
        let sink = Box::new(MockCommandSink::new());
        let coordinator = DualBrainCoordinator::new(config, llm_engine, solver, sink);
        Self {
            descriptor,
            coordinator,
            market_channel: MarketChannel::new(16),
            current_schedule: None,
            current_price: 0.5,
            state: AgentState::Created,
            tick_count: 0,
        }
    }

    /// 默认构造（使用 `DualBrainCoordinator::default_with_mock()`）.
    pub fn new_default(now_ms: u64) -> Self {
        let descriptor = AgentDescriptor::new(AgentType::Energy, "energy-agent", now_ms);
        let coordinator = DualBrainCoordinator::default_with_mock();
        Self {
            descriptor,
            coordinator,
            market_channel: MarketChannel::new(16),
            current_schedule: None,
            current_price: 0.5,
            state: AgentState::Created,
            tick_count: 0,
        }
    }

    /// 获取市场通道可变引用（供测试注入数据）.
    pub fn market_channel_mut(&mut self) -> &mut MarketChannel {
        &mut self.market_channel
    }
}

impl AgentRuntime for EnergyAgent {
    fn descriptor(&self) -> &AgentDescriptor {
        &self.descriptor
    }

    fn on_start(&mut self, _now_ms: u64) -> Result<(), AgentRuntimeError> {
        self.state = AgentState::Running;
        Ok(())
    }

    fn on_tick(&mut self, now_ms: u64) -> Result<(), AgentRuntimeError> {
        // 1. 非阻塞接收市场数据，更新电价缓存
        if let Some(market_data) = self.market_channel.try_recv() {
            self.current_price = market_data.current_price;
        }

        // 2. 构建 RealtimeState（D11：从默认/缓存值构建）
        let state = RealtimeState {
            system: SystemState::default(),
            current_price: self.current_price,
            load_demand: None,
        };

        // 3. 调用双脑协调器（D10：execute 需 now_ms 参数）
        match self.coordinator.execute(&state, now_ms) {
            Ok(result) => {
                self.current_schedule = Some(result.schedule);
                self.tick_count += 1;
                Ok(())
            }
            Err(e) => {
                // D14：安全默认策略 — 标记错误状态，不 panic
                self.state = AgentState::Error;
                Err(AgentRuntimeError::DualBrainError(e))
            }
        }
    }

    fn on_stop(&mut self, _now_ms: u64) -> Result<(), AgentRuntimeError> {
        self.state = AgentState::Dead;
        Ok(())
    }

    fn on_heartbeat(&self, _now_ms: u64) -> HeartbeatStatus {
        if self.state == AgentState::Running {
            HeartbeatStatus::Alive
        } else {
            HeartbeatStatus::Dead
        }
    }
}

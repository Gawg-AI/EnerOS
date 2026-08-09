//! 能源调度 Agent — 编排双脑协调器执行储能调度（D9/D10/D11/D14）.

use alloc::boxed::Box;

use eneros_agent::{AgentDescriptor, AgentState, AgentType};
use eneros_dual_brain::DualBrainCoordinator;
use eneros_energy_lp_model::config::ScheduleConfig;
use eneros_energy_lp_model::result::ScheduleResult;
use eneros_fast_path::state::RealtimeState;
use eneros_llm_engine::engine::LlmEngine;
use eneros_safety_validator::state::SystemState;
#[cfg(any(test, feature = "mock"))]
use eneros_solver_core::mock::MockSolver;
use eneros_solver_core::solver::Solver;

use crate::error::AgentRuntimeError;
use crate::market::MarketChannel;
use crate::runtime::{AgentRuntime, HeartbeatStatus};

/// 能源调度 Agent.
///
/// 持有 `DualBrainCoordinator<S>`（泛型求解器，构造器注入，P0-27）。
/// 通过 `market_channel` 接收 Market Agent 转发的市场数据。
pub struct EnergyAgent<S: Solver> {
    /// Agent 描述符.
    pub descriptor: AgentDescriptor,
    /// 双脑协调器（泛型求解器）.
    pub coordinator: DualBrainCoordinator<S>,
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

impl<S: Solver> EnergyAgent<S> {
    /// 构造 Energy Agent.
    ///
    /// `descriptor = AgentDescriptor::new(AgentType::Energy, name, now_ms)`（D7）。
    /// `coordinator = DualBrainCoordinator::new(config, llm, solver, sink)`（D9）。
    /// 构造 Energy Agent（求解器/引擎/发送器构造器注入，P0-27）.
    ///
    /// `descriptor = AgentDescriptor::new(AgentType::Energy, name, now_ms)`（D7）。
    /// `coordinator = DualBrainCoordinator::new(config, llm, solver, sink)`（D9）。
    pub fn new<S2: Solver>(
        name: &str,
        config: ScheduleConfig,
        now_ms: u64,
        llm_engine: Box<dyn LlmEngine>,
        solver: S2,
        sink: Box<dyn eneros_dual_brain::sink::CommandSink>,
    ) -> EnergyAgent<S2> {
        let descriptor = AgentDescriptor::new(AgentType::Energy, name, now_ms);
        let coordinator = DualBrainCoordinator::new(config, llm_engine, solver, sink);
        EnergyAgent {
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

impl<S: Solver> EnergyAgent<S> {
    /// Release 真后端构造器（21-B §2.A2）。
    ///
    /// 与 [`EnergyAgent::new`] 相同装配（`DualBrainCoordinator::new` 注入
    /// 真 LlmEngine + 真 Solver + 真 Sink），显式命名对齐蓝图 A2：
    /// release 构建下 `new_default` 不可见（mock 门控），真后端装配必须走
    /// `new` / `new_real`。本构造器不绑定任何 mock 类型——`S: Solver`
    /// 在 release 装配 `HighsSolver`（solver-core `highs-ffi`）。
    ///
    /// `#[cfg(not(any(test, feature = "mock")))]` 语义由调用方保证：
    /// 依赖本 crate 的 release 构建不得启用 `mock` feature。
    pub fn new_real<S2: Solver>(
        name: &str,
        config: ScheduleConfig,
        now_ms: u64,
        llm_engine: Box<dyn LlmEngine>,
        solver: S2,
        sink: Box<dyn eneros_dual_brain::sink::CommandSink>,
    ) -> EnergyAgent<S2> {
        Self::new(name, config, now_ms, llm_engine, solver, sink)
    }
}

#[cfg(any(test, feature = "mock"))]
impl EnergyAgent<MockSolver> {
    /// 测试/演示默认构造（使用 mock 求解器）.
    pub fn new_default(now_ms: u64) -> EnergyAgent<MockSolver> {
        let descriptor = AgentDescriptor::new(AgentType::Energy, "energy-agent", now_ms);
        let coordinator = DualBrainCoordinator::default_with_mock();
        EnergyAgent {
            descriptor,
            coordinator,
            market_channel: MarketChannel::new(16),
            current_schedule: None,
            current_price: 0.5,
            state: AgentState::Created,
            tick_count: 0,
        }
    }
}

impl<S: Solver> AgentRuntime for EnergyAgent<S> {
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

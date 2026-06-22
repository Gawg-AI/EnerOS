//! `Runtime` 结构体 — 封装 EnerOS 运行时子系统的初始化与生命周期管理
//!
//! `Runtime` 聚合了 EventBus、ConstraintEngine、TimeSeriesEngine、DeviceManager、
//! SafetyGateway、AgentOrchestrator 等核心子系统，提供统一的构建、启动和停止接口。
//!
//! 典型用法：
//! ```no_run
//! use std::sync::Arc;
//! use eneros_runtime::RuntimeBuilder;
//! use eneros_network::PowerNetwork;
//!
//! # async fn run() -> anyhow::Result<()> {
//! let runtime = RuntimeBuilder::new()
//!     .with_network(Arc::new(PowerNetwork::from_ieee14()))
//!     .build()
//!     .await?;
//! runtime.start()?;
//! // ... 使用 runtime.agent_orchestrator() 等 ...
//! runtime.stop().await;
//! # Ok(())
//! # }
//! ```

use std::sync::Arc;

use parking_lot::RwLock;
use tokio::sync::watch;

use eneros_agent::{
    AgentContext, AgentOrchestrator, DataDrivenAgentLoop, DispatchAgent, LoadForecastAgent,
    OperationAgent, PlanningAgent, SelfHealingAgent, SystemStateMachine, TradingAgent,
    event_adapter::AgentEventHandler,
};
use eneros_constraint::{ConstraintEngine, projector::FeasibilityProjector};
use eneros_core::PowerObservation;
use eneros_device::DeviceManager;
use eneros_eventbus::{EventBus, event::EventType};
use eneros_gateway::{
    CommandExecutor, ConstrainedDecisionPipeline, LoggingExecutor, ObservationProvider, RealtimeExecutor, SafetyGateway,
    SharedPriorityCommandQueue, WatchdogTimer,
    constraint_validator::ConstraintAwareValidator,
    interlocking::InterlockingRuleEngine,
    decision_pipeline::ConstrainedDecisionPipeline as DecisionPipeline,
};
use eneros_memory::{AgentMemory, FileMemory, InMemoryMemory};
use eneros_network::{NetworkSimulatorAdapter, PowerNetwork};
use eneros_reasoning::{ReasoningEngine, RuleBasedEngine, feedback::FeedbackLoop};
use eneros_scada::{
    DataPipeline, DataSource, ScadaCollector, SimulatedDataSource, SnapshotBuilder,
    build_ieee14_scada_config, build_ieee14_snapshot_mappings,
};
use eneros_timeseries::{SoeRecorder, TimeSeriesEngine};
use eneros_tool::ToolEngine;

// ── 后台任务句柄集合 ────────────────────────────────────────────────────

/// Runtime 后台任务的句柄集合，用于优雅停止。
///
/// 所有字段在 `Runtime::start()` 时填充，在 `Runtime::stop()` 时清理。
pub struct RuntimeHandles {
    /// DataDrivenAgentLoop 的 tokio 任务句柄
    dd_loop_handle: Option<tokio::task::JoinHandle<()>>,
    /// RealtimeExecutor 实例（stop 时调用其 stop 方法）
    rt_executor: Option<Arc<RealtimeExecutor>>,
    /// TimeSeries rollup 任务的关闭信号发送端
    rollup_shutdown_tx: Option<watch::Sender<bool>>,
    /// TimeSeries rollup 任务的句柄
    rollup_handle: Option<tokio::task::JoinHandle<()>>,
}

impl RuntimeHandles {
    fn new() -> Self {
        Self {
            dd_loop_handle: None,
            rt_executor: None,
            rollup_shutdown_tx: None,
            rollup_handle: None,
        }
    }
}

// ── RuntimeBuilder ──────────────────────────────────────────────────────

/// Runtime 构建器，用于配置并构建 `Runtime`。
///
/// 必须调用 `with_network()` 提供电力网络模型后才能 `build()`。
/// 其他参数均有合理默认值。
pub struct RuntimeBuilder {
    /// EventBus 容量（默认 1024）
    event_bus_capacity: usize,
    /// TimeSeriesEngine 内存保留容量（默认 1,000,000 点）
    ts_retention_capacity: usize,
    /// TimeSeriesEngine SQLite 持久化路径（None = 纯内存）
    ts_db_path: Option<String>,
    /// SoeRecorder SQLite 持久化路径（None = 纯内存）
    soe_db_path: Option<String>,
    /// FileMemory 存储目录（默认 ./eneros_memory）
    memory_path: String,
    /// DataDrivenAgentLoop 循环周期（毫秒，默认 2000）
    dd_loop_cycle_ms: u64,
    /// 电力网络模型（必需）
    network: Option<Arc<PowerNetwork>>,
    /// SCADA 数据源（None = 使用 SimulatedDataSource）
    data_source: Option<Arc<dyn DataSource>>,
}

impl Default for RuntimeBuilder {
    fn default() -> Self {
        Self {
            event_bus_capacity: 1024,
            ts_retention_capacity: 1_000_000,
            ts_db_path: None,
            soe_db_path: None,
            memory_path: "./eneros_memory".to_string(),
            dd_loop_cycle_ms: 2000,
            network: None,
            data_source: None,
        }
    }
}

impl RuntimeBuilder {
    /// 创建默认配置的构建器
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置 EventBus 容量
    pub fn with_event_bus_capacity(mut self, capacity: usize) -> Self {
        self.event_bus_capacity = capacity;
        self
    }

    /// 设置 TimeSeriesEngine 内存保留容量
    pub fn with_ts_retention_capacity(mut self, capacity: usize) -> Self {
        self.ts_retention_capacity = capacity;
        self
    }

    /// 启用 TimeSeriesEngine SQLite 持久化
    pub fn with_ts_sqlite_path(mut self, path: impl Into<String>) -> Self {
        self.ts_db_path = Some(path.into());
        self
    }

    /// 启用 SoeRecorder SQLite 持久化
    pub fn with_soe_sqlite_path(mut self, path: impl Into<String>) -> Self {
        self.soe_db_path = Some(path.into());
        self
    }

    /// 设置 FileMemory 存储目录
    pub fn with_memory_path(mut self, path: impl Into<String>) -> Self {
        self.memory_path = path.into();
        self
    }

    /// 设置 DataDrivenAgentLoop 循环周期（毫秒）
    pub fn with_dd_loop_cycle(mut self, ms: u64) -> Self {
        self.dd_loop_cycle_ms = ms;
        self
    }

    /// 设置电力网络模型（必需）
    pub fn with_network(mut self, network: Arc<PowerNetwork>) -> Self {
        self.network = Some(network);
        self
    }

    /// 设置 SCADA 数据源（默认使用 SimulatedDataSource）
    pub fn with_data_source(mut self, data_source: Arc<dyn DataSource>) -> Self {
        self.data_source = Some(data_source);
        self
    }

    /// 构建 Runtime，执行所有子系统的初始化。
    ///
    /// 返回 `Err` 仅当未提供 `network` 或 SOE/TS SQLite 初始化发生不可恢复错误。
    /// 大部分初始化失败会回退到内存模式并记录警告。
    pub async fn build(self) -> anyhow::Result<Runtime> {
        let network = self.network.ok_or_else(|| {
            anyhow::anyhow!(
                "RuntimeBuilder::build 需要电力网络模型 — 请先调用 with_network()"
            )
        })?;

        // ── 1. EventBus ─────────────────────────────────────────────────
        let event_bus = Arc::new(EventBus::new(self.event_bus_capacity));
        tracing::info!(
            capacity = self.event_bus_capacity,
            "[Runtime] EventBus 已创建"
        );

        // ── 2. ConstraintEngine（依赖 EventBus）──────────────────────────
        let constraint_engine =
            Arc::new(ConstraintEngine::with_event_bus(event_bus.clone()));
        tracing::info!("[Runtime] ConstraintEngine 已创建（EventBus 已接入）");

        // ── 3. TimeSeriesEngine ─────────────────────────────────────────
        let ts_engine = match self.ts_db_path.as_deref() {
            Some(path) => match TimeSeriesEngine::with_sqlite(self.ts_retention_capacity, path) {
                Ok(engine) => {
                    tracing::info!(
                        path = path,
                        "[Runtime] TimeSeriesEngine 已创建（SQLite 后端）"
                    );
                    Arc::new(engine)
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "[Runtime] TimeSeriesEngine SQLite 初始化失败，回退到内存模式"
                    );
                    Arc::new(TimeSeriesEngine::new(self.ts_retention_capacity))
                }
            },
            None => {
                tracing::info!("[Runtime] TimeSeriesEngine 已创建（内存后端）");
                Arc::new(TimeSeriesEngine::new(self.ts_retention_capacity))
            }
        };

        // ── 4. SoeRecorder ──────────────────────────────────────────────
        let soe_recorder = match self.soe_db_path.as_deref() {
            Some(path) => match SoeRecorder::new_sqlite(path) {
                Ok(r) => {
                    tracing::info!(
                        path = path,
                        "[Runtime] SoeRecorder 已创建（SQLite 后端）"
                    );
                    Arc::new(r)
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "[Runtime] SoeRecorder SQLite 初始化失败，回退到内存模式"
                    );
                    Arc::new(SoeRecorder::new_memory())
                }
            },
            None => {
                tracing::info!("[Runtime] SoeRecorder 已创建（内存后端）");
                Arc::new(SoeRecorder::new_memory())
            }
        };

        // ── 5. DeviceManager（空 — 设备由上层单独注册）──────────────────
        let device_manager = Arc::new(DeviceManager::new());
        tracing::info!("[Runtime] DeviceManager 已创建（无设备注册）");

        // ── 6. SCADA 采集器与数据管道 ────────────────────────────────────
        let data_source = self
            .data_source
            .unwrap_or_else(|| Arc::new(SimulatedDataSource::new()));
        let scada_config = build_ieee14_scada_config();
        let collector = Arc::new(ScadaCollector::new(scada_config.clone(), data_source));
        tracing::info!("[Runtime] ScadaCollector 已创建");

        // ── 7. DataPipeline（refresh → collect → record → publish）──────
        let pipeline = Arc::new(
            DataPipeline::new(collector.clone(), ts_engine.clone())
                .with_event_bus(event_bus.clone())
                .with_soe_recorder(soe_recorder.clone()),
        );
        tracing::info!("[Runtime] DataPipeline 已创建（EventBus + SOE 已接入）");

        // ── 8. SnapshotBuilder ──────────────────────────────────────────
        let snapshot_builder = Arc::new(SnapshotBuilder::new(build_ieee14_snapshot_mappings()));
        tracing::info!("[Runtime] SnapshotBuilder 已创建");

        // ── 9. SafetyGateway（使用 LoggingExecutor，无设备配置）─────────
        let command_queue = Arc::new(SharedPriorityCommandQueue::new());
        let command_executor: Arc<dyn CommandExecutor> = Arc::new(LoggingExecutor);
        let gateway = Arc::new(SafetyGateway::with_queue_and_executor(
            100,
            command_queue,
            command_executor,
        ));
        tracing::info!("[Runtime] SafetyGateway 已创建（LoggingExecutor）");

        // ── 10. ToolEngine ──────────────────────────────────────────────
        let tool_engine = Arc::new(parking_lot::RwLock::new(ToolEngine::new()));
        tracing::info!("[Runtime] ToolEngine 已创建");

        // ── 11. AgentMemory（FileMemory 优先，失败回退 InMemory）────────
        let memory: Arc<dyn AgentMemory> = match FileMemory::new(&self.memory_path) {
            Ok(m) => {
                tracing::info!(
                    path = %self.memory_path,
                    "[Runtime] FileMemory 已启用"
                );
                Arc::new(m)
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "[Runtime] FileMemory 初始化失败，回退到 InMemoryMemory"
                );
                Arc::new(InMemoryMemory::default())
            }
        };

        // ── 12. ReasoningEngine（默认 RuleBased；rig 集成由上层配置）─────
        let network_rw = Arc::new(parking_lot::RwLock::new(PowerNetwork::from_ieee14()));
        let reasoning: Arc<dyn ReasoningEngine> = Arc::new(RuleBasedEngine::new());
        tracing::info!("[Runtime] ReasoningEngine 已创建（RuleBased）");

        // ── 13. ConstrainedDecisionPipeline ─────────────────────────────
        let network_simulator = Arc::new(NetworkSimulatorAdapter::new(network_rw.clone()));
        let projector = Arc::new(FeasibilityProjector::new(network_simulator));
        // 将 Projector 注入 ConstraintEngine
        constraint_engine.set_projector(projector.clone());

        let collector_for_obs = collector.clone();
        let observation_provider: ObservationProvider = Arc::new(move || {
            let readings = collector_for_obs.latest_all();
            if readings.is_empty() {
                return None;
            }
            Some(build_observation_from_readings(&readings))
        });

        let pipeline_validator = Arc::new(ConstraintAwareValidator::with_projector(
            constraint_engine.clone(),
            gateway.clone(),
            InterlockingRuleEngine::new(),
            projector.clone(),
        ));

        let watchdog = Arc::new(WatchdogTimer::new(std::time::Duration::from_millis(500)));
        let decision_pipeline = Arc::new(
            DecisionPipeline::with_observation_provider(
                projector,
                pipeline_validator,
                gateway.clone(),
                observation_provider,
            )
            .with_watchdog(watchdog.clone(), std::time::Duration::from_millis(500)),
        );
        tracing::info!("[Runtime] ConstrainedDecisionPipeline 已创建（projection + validation + execution + observation + watchdog）");

        // ── 14. FeedbackLoop ────────────────────────────────────────────
        let feedback_loop =
            Arc::new(FeedbackLoop::with_default_iterations_shared(reasoning.clone()));
        tracing::info!("[Runtime] FeedbackLoop 已创建（共享 reasoning，最大 2 次重试）");

        // ── 15. AgentOrchestrator（注册 6 个领域 agent）─────────────────
        let ctx = AgentContext::new(
            event_bus.clone(),
            gateway.clone(),
            tool_engine.clone(),
            network_rw,
            memory.clone(),
            reasoning,
        );
        let mut orchestrator = AgentOrchestrator::with_pipeline_and_feedback(
            ctx,
            decision_pipeline.clone(),
            feedback_loop,
        );

        // 注册 6 个领域 agent：Dispatch / Operation / SelfHealing / Forecast / Planning / Trading
        orchestrator.register_agent(AgentEventHandler::new(
            Box::new(DispatchAgent::new("dispatch-1", "DispatchAgent", vec![1])),
            vec![EventType::ConstraintViolation, EventType::DataReceived],
        ));
        orchestrator.register_agent(AgentEventHandler::new(
            Box::new(OperationAgent::new("operation-1", "OperationAgent", vec![1])),
            vec![EventType::ConstraintViolation, EventType::SystemAlarm],
        ));
        orchestrator.register_agent(AgentEventHandler::new_all_events(Box::new(
            SelfHealingAgent::new("self-healing-1", "SelfHealingAgent", vec![1]),
        )));
        orchestrator.register_agent(AgentEventHandler::new(
            Box::new(LoadForecastAgent::new(
                "forecast-1",
                eneros_core::Jurisdiction::for_zones(vec![1]),
                ts_engine.clone(),
            )),
            vec![EventType::ConstraintViolation, EventType::DataReceived],
        ));
        orchestrator.register_agent(AgentEventHandler::new(
            Box::new(PlanningAgent::new("planning-1", vec![1])),
            vec![EventType::ConstraintViolation, EventType::DataReceived],
        ));
        orchestrator.register_agent(AgentEventHandler::new(
            Box::new(TradingAgent::new("trading-1", vec![1])),
            vec![EventType::DataReceived, EventType::ConstraintViolation],
        ));
        let orchestrator = Arc::new(orchestrator);
        tracing::info!("[Runtime] AgentOrchestrator 已创建（6 个领域 agent 已注册）");

        // ── 16. DataDrivenAgentLoop ─────────────────────────────────────
        let state_machine = Arc::new(SystemStateMachine::new());
        let dd_loop = Arc::new(
            DataDrivenAgentLoop::new(
                pipeline.clone(),
                collector.clone(),
                snapshot_builder.clone(),
                orchestrator.clone(),
                state_machine,
            )
            .with_constraint_engine(constraint_engine.clone()),
        );
        tracing::info!("[Runtime] DataDrivenAgentLoop 已创建");

        Ok(Runtime {
            network,
            event_bus,
            constraint_engine,
            ts_engine,
            soe_recorder,
            device_manager,
            scada_collector: collector,
            data_pipeline: pipeline,
            snapshot_builder,
            gateway,
            tool_engine,
            agent_memory: memory,
            decision_pipeline,
            agent_orchestrator: orchestrator,
            data_driven_loop: dd_loop,
            watchdog,
            cycle_interval_ms: self.dd_loop_cycle_ms,
            handles: RwLock::new(RuntimeHandles::new()),
        })
    }
}

// ── Runtime ─────────────────────────────────────────────────────────────

/// EnerOS 运行时 — 封装核心子系统的初始化与生命周期管理。
///
/// 由 `RuntimeBuilder::build()` 创建，包含 EventBus、ConstraintEngine、
/// TimeSeriesEngine、DeviceManager、SafetyGateway、AgentOrchestrator 等子系统。
/// 调用 `start()` 启动后台任务，`stop()` 优雅停止。
pub struct Runtime {
    /// 电力网络模型（模型类，由上层注入）
    network: Arc<PowerNetwork>,
    /// 事件总线
    event_bus: Arc<EventBus>,
    /// 约束引擎
    constraint_engine: Arc<ConstraintEngine>,
    /// 时序数据引擎
    ts_engine: Arc<TimeSeriesEngine>,
    /// SOE 记录器
    soe_recorder: Arc<SoeRecorder>,
    /// 设备管理器
    device_manager: Arc<DeviceManager>,
    /// SCADA 采集器
    scada_collector: Arc<ScadaCollector>,
    /// SCADA 数据管道
    data_pipeline: Arc<DataPipeline>,
    /// 快照构建器
    snapshot_builder: Arc<SnapshotBuilder>,
    /// 安全网关
    gateway: Arc<SafetyGateway>,
    /// 工具引擎
    tool_engine: Arc<RwLock<ToolEngine>>,
    /// Agent 记忆存储
    agent_memory: Arc<dyn AgentMemory>,
    /// 约束决策管道
    decision_pipeline: Arc<ConstrainedDecisionPipeline>,
    /// Agent 编排器
    agent_orchestrator: Arc<AgentOrchestrator>,
    /// 数据驱动 Agent 循环
    data_driven_loop: Arc<DataDrivenAgentLoop>,
    /// 看门狗定时器
    watchdog: Arc<WatchdogTimer>,
    /// DataDrivenAgentLoop 循环周期（毫秒）
    cycle_interval_ms: u64,
    /// 后台任务句柄（start 时填充，stop 时清理）
    handles: RwLock<RuntimeHandles>,
}

impl Runtime {
    // ── 子系统访问器 ────────────────────────────────────────────────────

    /// 电力网络模型
    pub fn network(&self) -> &Arc<PowerNetwork> {
        &self.network
    }

    /// 事件总线
    pub fn event_bus(&self) -> &Arc<EventBus> {
        &self.event_bus
    }

    /// 约束引擎
    pub fn constraint_engine(&self) -> &Arc<ConstraintEngine> {
        &self.constraint_engine
    }

    /// 时序数据引擎
    pub fn ts_engine(&self) -> &Arc<TimeSeriesEngine> {
        &self.ts_engine
    }

    /// SOE 记录器
    pub fn soe_recorder(&self) -> &Arc<SoeRecorder> {
        &self.soe_recorder
    }

    /// 设备管理器
    pub fn device_manager(&self) -> &Arc<DeviceManager> {
        &self.device_manager
    }

    /// SCADA 采集器
    pub fn scada_collector(&self) -> &Arc<ScadaCollector> {
        &self.scada_collector
    }

    /// SCADA 数据管道
    pub fn data_pipeline(&self) -> &Arc<DataPipeline> {
        &self.data_pipeline
    }

    /// 快照构建器
    pub fn snapshot_builder(&self) -> &Arc<SnapshotBuilder> {
        &self.snapshot_builder
    }

    /// 安全网关
    pub fn gateway(&self) -> &Arc<SafetyGateway> {
        &self.gateway
    }

    /// 工具引擎
    pub fn tool_engine(&self) -> &Arc<RwLock<ToolEngine>> {
        &self.tool_engine
    }

    /// Agent 记忆存储
    pub fn agent_memory(&self) -> &Arc<dyn AgentMemory> {
        &self.agent_memory
    }

    /// 约束决策管道
    pub fn decision_pipeline(&self) -> &Arc<ConstrainedDecisionPipeline> {
        &self.decision_pipeline
    }

    /// Agent 编排器
    pub fn agent_orchestrator(&self) -> &Arc<AgentOrchestrator> {
        &self.agent_orchestrator
    }

    /// 数据驱动 Agent 循环
    pub fn data_driven_loop(&self) -> &Arc<DataDrivenAgentLoop> {
        &self.data_driven_loop
    }

    /// 看门狗定时器
    pub fn watchdog(&self) -> &Arc<WatchdogTimer> {
        &self.watchdog
    }

    // ── 生命周期管理 ────────────────────────────────────────────────────

    /// 启动所有后台任务：
    /// - RealtimeExecutor（命令执行线程）
    /// - WatchdogTimer（看门狗监控）
    /// - DataDrivenAgentLoop（数据驱动 Agent 循环）
    /// - TimeSeries rollup task（时序降采样任务）
    pub fn start(&self) -> anyhow::Result<()> {
        let mut handles = self.handles.write();

        // 启动 RealtimeExecutor
        let rt_executor = self.gateway.start_executor()?;
        handles.rt_executor = Some(rt_executor);

        // 启动 WatchdogTimer（JoinHandle 由 watchdog 内部管理，无需保存）
        let _wd_handle = self.watchdog.start();

        // 启动 DataDrivenAgentLoop
        let dd_handle = self.data_driven_loop.start(self.cycle_interval_ms);
        handles.dd_loop_handle = Some(dd_handle);

        // 启动 TimeSeries rollup task（60s→1min, 60min→1h）
        let (rollup_shutdown_tx, rollup_shutdown_rx) = watch::channel(false);
        let rollup_handle = self.ts_engine.clone().start_rollup_task(rollup_shutdown_rx);
        handles.rollup_shutdown_tx = Some(rollup_shutdown_tx);
        handles.rollup_handle = Some(rollup_handle);

        tracing::info!(
            cycle_ms = self.cycle_interval_ms,
            "[Runtime] 所有后台任务已启动（RealtimeExecutor + Watchdog + DataDrivenAgentLoop + Rollup）"
        );
        Ok(())
    }

    /// 优雅停止所有后台任务：
    /// - 中止 DataDrivenAgentLoop
    /// - 停止 RealtimeExecutor
    /// - 停止 WatchdogTimer
    /// - 发送关闭信号并等待 rollup 任务结束
    pub async fn stop(&self) {
        // 先取出需要在 await 前释放锁的资源
        let (dd_loop_handle, rt_executor, rollup_shutdown_tx, rollup_handle) = {
            let mut handles = self.handles.write();
            (
                handles.dd_loop_handle.take(),
                handles.rt_executor.take(),
                handles.rollup_shutdown_tx.take(),
                handles.rollup_handle.take(),
            )
        };

        // 停止 DataDrivenAgentLoop
        if let Some(h) = dd_loop_handle {
            h.abort();
            tracing::info!("[Runtime] DataDrivenAgentLoop 已停止");
        }

        // 停止 RealtimeExecutor
        if let Some(rt) = rt_executor {
            rt.stop();
            tracing::info!("[Runtime] RealtimeExecutor 已停止");
        }

        // 停止 WatchdogTimer
        self.watchdog.stop();
        tracing::info!("[Runtime] WatchdogTimer 已停止");

        // 停止 rollup 任务（发送关闭信号并等待结束）
        if let Some(tx) = rollup_shutdown_tx {
            let _ = tx.send(true);
        }
        if let Some(h) = rollup_handle {
            let _ = h.await;
            tracing::info!("[Runtime] TimeSeries rollup 任务已停止");
        }
    }
}

// ── 辅助函数 ────────────────────────────────────────────────────────────

/// 从最新 SCADA 读数构建 `PowerObservation`，用于决策管道的后置条件验证。
///
/// 映射遵循 IEEE 14 快照约定：
/// - `voltage_pu` → 母线电压幅值
/// - `angle_deg` → 母线电压相角
/// - `gen_p_mw` / `gen_q_mvar` → 发电机出力
/// - `load_p_mw` / `load_q_mvar` → 负荷消耗
/// - `frequency_hz` → 系统频率
fn build_observation_from_readings(
    readings: &[eneros_scada::ScadaReading],
) -> PowerObservation {
    use eneros_core::{
        BranchFlowObservation, BusVoltageObservation, GenOutputObservation,
        LoadConsumptionObservation,
    };
    use std::collections::HashMap;

    let mut bus_voltages: HashMap<u64, BusVoltageObservation> = HashMap::new();
    let mut gen_outputs: HashMap<u64, GenOutputObservation> = HashMap::new();
    let mut load_consumptions: HashMap<u64, LoadConsumptionObservation> = HashMap::new();
    let mut frequency_hz = 50.0;
    let mut total_load_mw = 0.0;
    let mut total_gen_mw = 0.0;

    for r in readings {
        match r.parameter.as_str() {
            "voltage_pu" => {
                bus_voltages.insert(
                    r.element_id,
                    BusVoltageObservation {
                        vm_pu: r.value,
                        va_degree: 0.0,
                    },
                );
            }
            "angle_deg" => {
                bus_voltages
                    .entry(r.element_id)
                    .and_modify(|v| v.va_degree = r.value)
                    .or_insert(BusVoltageObservation {
                        vm_pu: 1.0,
                        va_degree: r.value,
                    });
            }
            "gen_p_mw" => {
                gen_outputs
                    .entry(r.element_id)
                    .and_modify(|g| g.p_mw = r.value)
                    .or_insert(GenOutputObservation {
                        p_mw: r.value,
                        q_mvar: 0.0,
                        p_max_mw: 0.0,
                        p_min_mw: 0.0,
                    });
                total_gen_mw += r.value;
            }
            "gen_q_mvar" => {
                gen_outputs
                    .entry(r.element_id)
                    .and_modify(|g| g.q_mvar = r.value)
                    .or_insert(GenOutputObservation {
                        p_mw: 0.0,
                        q_mvar: r.value,
                        p_max_mw: 0.0,
                        p_min_mw: 0.0,
                    });
            }
            "load_p_mw" => {
                load_consumptions
                    .entry(r.element_id)
                    .and_modify(|l| l.p_mw = r.value)
                    .or_insert(LoadConsumptionObservation {
                        p_mw: r.value,
                        q_mvar: 0.0,
                    });
                total_load_mw += r.value;
            }
            "load_q_mvar" => {
                load_consumptions
                    .entry(r.element_id)
                    .and_modify(|l| l.q_mvar = r.value)
                    .or_insert(LoadConsumptionObservation {
                        p_mw: 0.0,
                        q_mvar: r.value,
                    });
            }
            "frequency_hz" => {
                frequency_hz = r.value;
            }
            _ => {}
        }
    }

    PowerObservation {
        bus_voltages,
        branch_flows: HashMap::<u64, BranchFlowObservation>::new(),
        frequency_hz,
        gen_outputs,
        load_consumptions,
        timestamp: chrono::Utc::now(),
        total_load_mw,
        total_gen_mw,
    }
}

// ── 测试 ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_runtime_build_with_defaults() {
        // 使用默认配置构建 Runtime（内存后端，无 SQLite）
        let runtime = RuntimeBuilder::new()
            .with_network(Arc::new(PowerNetwork::from_ieee14()))
            .build()
            .await
            .expect("Runtime 构建应成功");

        // 验证所有子系统已初始化
        assert!(runtime.network().bus_count() != 0, "网络应有母线");
        // subscribe() 返回 Receiver（非 Option），能成功订阅即说明 EventBus 可用
        let _rx = runtime.event_bus().subscribe();
        assert!(runtime.scada_collector().latest_all().is_empty());
        assert!(runtime.agent_orchestrator().agent_count() > 0, "应注册了 agent");
    }

    #[tokio::test]
    async fn test_runtime_build_without_network_fails() {
        // 未提供 network 时应返回错误
        let result = RuntimeBuilder::new().build().await;
        assert!(result.is_err(), "未提供 network 时 build 应失败");
        // 使用 match 提取错误信息，避免 unwrap_err() 要求 Runtime: Debug
        let err = match result {
            Err(e) => e.to_string(),
            Ok(_) => String::new(),
        };
        assert!(
            err.contains("network") || err.contains("网络"),
            "错误信息应提及 network"
        );
    }

    #[tokio::test]
    async fn test_runtime_start_stop() {
        let runtime = RuntimeBuilder::new()
            .with_network(Arc::new(PowerNetwork::from_ieee14()))
            .with_dd_loop_cycle(100)
            .build()
            .await
            .expect("Runtime 构建应成功");

        // 启动后台任务
        runtime
            .start()
            .expect("Runtime 启动应成功");

        // 短暂运行
        tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;

        // 优雅停止
        runtime.stop().await;

        // 验证句柄已清理
        let handles = runtime.handles.read();
        assert!(handles.dd_loop_handle.is_none(), "dd_loop 句柄应已清理");
        assert!(handles.rt_executor.is_none(), "rt_executor 应已清理");
        assert!(handles.rollup_handle.is_none(), "rollup 句柄应已清理");
    }

    #[tokio::test]
    async fn test_runtime_builder_custom_config() {
        // 测试自定义配置
        let runtime = RuntimeBuilder::new()
            .with_network(Arc::new(PowerNetwork::from_ieee14()))
            .with_event_bus_capacity(256)
            .with_ts_retention_capacity(10000)
            .with_dd_loop_cycle(500)
            .build()
            .await
            .expect("Runtime 构建应成功");

        assert_eq!(runtime.cycle_interval_ms, 500);
    }

    #[test]
    fn test_build_observation_from_readings() {
        use eneros_scada::ScadaReading;

        let readings = vec![
            ScadaReading {
                element_id: 1,
                parameter: "voltage_pu".to_string(),
                value: 1.05,
                quality: eneros_device::DataQuality::Good,
                timestamp: chrono::Utc::now(),
                scan_rate_ms: 1000,
            },
            ScadaReading {
                element_id: 1,
                parameter: "frequency_hz".to_string(),
                value: 49.98,
                quality: eneros_device::DataQuality::Good,
                timestamp: chrono::Utc::now(),
                scan_rate_ms: 1000,
            },
        ];

        let obs = build_observation_from_readings(&readings);
        assert_eq!(obs.frequency_hz, 49.98);
        assert!(obs.bus_voltages.contains_key(&1));
        assert_eq!(obs.bus_voltages.get(&1).unwrap().vm_pu, 1.05);
    }
}

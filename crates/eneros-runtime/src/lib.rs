//! # EnerOS 运行时聚合层
//!
//! 本 crate 作为中间聚合层，将 runtime 类子系统 crate 聚合在一起，为上层
//! （如 `eneros-api`）提供单一依赖入口，降低依赖图的扁平耦合度。
//!
//! ## 主要功能
//!
//! 1. **子系统 crate 统一 re-export**：通过 `pub use ... as ...` 将各子系统
//!    crate 以模块别名形式暴露，上层可使用 `eneros_runtime::agent::AgentOrchestrator`
//!    等路径访问，保持与直接依赖时一致的源码可读性。
//! 2. **`Runtime` 结构体**：封装 EventBus、ConstraintEngine、TimeSeriesEngine、
//!    DeviceManager、SafetyGateway、AgentOrchestrator 等核心子系统的初始化与
//!    生命周期管理，提供 `RuntimeBuilder` 构建器模式。
//!
//! ## 设计原则
//!
//! - **真实封装**：`Runtime` 结构体执行真实的子系统初始化逻辑，不是简单的
//!   re-export 容器。
//! - **向后兼容**：通过 re-export 保持类型路径可追溯，上层迁移成本低。
//! - **模型/运行时分离**：模型类 crate（core、topology、powerflow）不聚合，
//!   由上层直接依赖；运行时类 crate 统一聚合到本 crate。

pub mod runtime;

// ── 子系统 crate re-export ──────────────────────────────────────────────
// 通过模块别名暴露各子系统 crate，上层可使用 `eneros_runtime::agent::...`
// 等路径访问类型，保持源码可读性。

/// Agent 子系统 — AI 代理生命周期管理与编排
pub use eneros_agent as agent;
/// Gateway 子系统 — 实时安全网关与命令执行
pub use eneros_gateway as gateway;
/// EventBus 子系统 — 组件间事件通信总线
pub use eneros_eventbus as eventbus;
/// SCADA 子系统 — 数据采集与监控管道
pub use eneros_scada as scada;
/// Device 子系统 — 多协议设备访问层
pub use eneros_device as device;
/// Memory 子系统 — Agent 记忆存储（情景/语义/过程）
pub use eneros_memory as memory;
/// Tool 子系统 — Agent 工具引擎
pub use eneros_tool as tool;
/// Reasoning 子系统 — 规则与 LLM 推理引擎
pub use eneros_reasoning as reasoning;
/// Analysis 子系统 — 电力系统分析（OPF/状态估计/短路）
pub use eneros_analysis as analysis;
/// Simulator 子系统 — 场景模拟与故障仿真
pub use eneros_simulator as simulator;
/// Plugin 子系统 — 动态插件加载与沙箱
pub use eneros_plugin as plugin;
/// Dashboard 子系统 — 拓扑可视化与仪表盘
pub use eneros_dashboard as dashboard;
/// Bridge 子系统 — cnpower Python 桥接
pub use eneros_bridge as bridge;
/// Constraint 子系统 — 电力系统约束引擎
pub use eneros_constraint as constraint;
/// Network 子系统 — 电力网络建模与潮流管线
pub use eneros_network as network;
/// TimeSeries 子系统 — 时序数据引擎与 SOE 记录
pub use eneros_timeseries as timeseries;

// ── Runtime 结构体 re-export ────────────────────────────────────────────
pub use runtime::{Runtime, RuntimeBuilder, RuntimeHandles};

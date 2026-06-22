//! eneros-simulator — EnerOS 统一模拟器框架
//!
//! 本 crate 提供统一的模拟器框架，包含：
//! - 场景脚本引擎（[`scenario`]）：用 TOML 描述时序场景，按时间线执行事件
//! - 电网模拟器（[`grid`]）：整合潮流求解与时序动作执行，支持稳态和暂态仿真
//! - 设备模拟器（[`device`]）：模拟 RTU/IED/保护装置等电力设备的行为与协议响应
//! - 故障模拟器（[`fault`]）：向电网注入故障并维护活跃故障列表，提供预置故障场景
//! - 负荷模拟器（[`load`]）：生成典型日/周负荷曲线，支持新能源出力与噪声叠加

pub mod scenario;
pub mod grid;
pub mod device;
pub mod fault;
pub mod load;

pub use scenario::{Scenario, ScenarioEvent, ScenarioAction, ScenarioRunner};
pub use grid::{GridSimulator, GridState, GridAction, SimulationMode};

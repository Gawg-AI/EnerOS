//! EnerOS 共享测试工具集
//!
//! 提供跨 crate 测试共享的辅助类型，例如用于可行性投影测试的
//! mock 网络模拟器。这些模拟器是确定性的、可复现的测试夹具，
//! 不依赖任何运行时 crate（如 `eneros-gateway` 或 `eneros-agent`），
//! 因此可作为 dev-dependency 被任意 crate 引用而不会引入循环依赖。
//!
//! ## 混沌工程
//!
//! [`chaos`] 模块提供应用层混沌注入器（网络/磁盘/CPU/内存/进程），
//! 用于在测试中模拟分布式系统故障，所有注入器跨平台兼容且可取消。

pub mod chaos;
pub mod simulators;

pub use simulators::{
    FeasibleMockSimulator, NonConvergentMockSimulator, ProjectingMockSimulator,
    ThermalViolationMockSimulator, ViolatingMockSimulator, VoltageViolationMockSimulator,
};

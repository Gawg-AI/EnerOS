//! EnerOS v0.73.0 Device Agent.
//!
//! Phase 1 P1-L MVP 集成第二层：实现 Device Agent（设备管理），负责多设备状态采集和
//! 命令执行，作为 RTOS 控制层与 Agent 层之间的桥梁。DeviceAgent 实现 v0.72.0
//! `AgentRuntime` trait，周期性采集设备状态（SOC/电压/电流/温度/功率）并执行来自
//! `CommandSource` 的控制命令。完成本版本后，v0.74.0 MVP 编排器可统一调度
//! Energy/Market/Device 三个 Agent 完成储能自治端到端场景。
//!
//! # 核心类型
//!
//! - [`DeviceAgent`] — 设备管理 Agent，实现 `AgentRuntime` trait
//! - [`DeviceAdapter`] — 设备适配器 trait（字符串点名读写）
//! - [`MockDevice`] — Mock 设备实现（BTreeMap-backed）
//! - [`DeviceRegistry`] — 多设备注册表
//! - [`DeviceType`] / [`DeviceInfo`] / [`DeviceState`] / [`DeviceSnapshot`] — 设备元信息与状态
//! - [`DeviceCommand`] — 设备控制命令
//! - [`CommandSource`] / [`MockCommandSource`] — 命令源抽象
//! - [`DeviceError`] — 设备错误枚举
//!
//! # 偏差声明（D1~D12，Karpathy "Think Before Coding"）
//!
//! | 偏差 | 蓝图原文 | 本版本处理 | 理由 |
//! |------|---------|-----------|------|
//! | **D1** | `log::info!("执行命令: ...")` / `log::info!("Device Agent 启动")` | 移除日志；状态/错误通过返回值传递 | no_std 无 `log` crate；与 v0.57/v0.64/v0.70/v0.71/v0.72 一致 |
//! | **D2** | `SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()` | `now_ms: u64` 参数 | no_std 合规：`SystemTime` 不可用；与 v0.57~v0.72 一致 |
//! | **D3** | `AgentDescriptor { id: "device-agent".into(), agent_type: AgentType::Device, priority: 1, capabilities: vec!["device.read", ...], trust_level: TrustLevel::Trusted, ..Default::default() }` | `AgentDescriptor::new(AgentType::Device, name, now_ms)` | v0.33.0 `AgentDescriptor` 13 字段 + 构造器 `new(type, name, now)` 自动设置；蓝图 `..Default::default()` 与 `capabilities: Vec<&str>` 类型不匹配（实际 `Vec<CapabilityRef>`）；与 v0.72.0 D7 一致 |
//! | **D4** | `ControlBusReader::new()` / `self.control_bus_rx.try_read()` | 本地 `CommandSource` trait + `MockCommandSource`（VecDeque-backed） | `ControlBusReader` 不存在；本地简单实现保持 crate 自包含可测试（与 v0.72.0 D4 `MarketDataSource` 模式一致） |
//! | **D5** | `SharedMemoryHandle::new()` / `self.shared_memory.write_snapshot(&snapshot)` | `poll_devices()` 返回 `DeviceSnapshot`，存入 `last_snapshot` 字段 | `SharedMemoryHandle` 不存在；MVP 阶段直接返回快照，调用方直接访问（Karpathy 简化原则） |
//! | **D6** | `device.read_point("soc").unwrap_or(0.0)` / `device.write_point("power_setpoint", power_kw)` on `Box<dyn PointAccess>` | 本地 `DeviceAdapter` trait with `read_point(name: &str) -> Result<f64, DeviceError>` + `MockDevice` | v0.51.0 `PointAccess::read_point(PointId) -> Result<DataPoint, ProtocolError>` 使用类型化 `PointId`/`DataPoint`，需 `PointMap` 映射字符串→ID，MVP 过于复杂；本地 `DeviceAdapter` 字符串点名更简单（与 v0.72.0 D6 `MarketDataSource` 模式一致） |
//! | **D7** | `command.target_device` / `command.power_kw` / `command.ttl_ms` on `ControlCommand` | 本地 `DeviceCommand` 结构体（target_device/power_kw/ttl_ms/timestamp_ms） | v0.55.0 `ControlCommand` 是 enum（`Single(SingleCommand)`/`Double(DoubleCommand)`），无 `target_device`/`power_kw`/`ttl_ms` 字段；本地定义匹配蓝图语义 |
//! | **D8** | `AgentError::DeviceNotFound(command.target_device.clone())` / `AgentError::DeviceError(e.to_string())` | 本地 `DeviceError` 枚举 + 在 v0.72.0 `AgentRuntimeError` 添加 `DeviceError(String)` 变体 | v0.33.0 `AgentError` 缺少 `DeviceNotFound`/`DeviceError` 变体（有 `AgentNotFound` 但语义不同）；在 `AgentRuntimeError` 添加变体是外科手术式变更，使 DeviceAgent 可复用 `AgentRuntime` trait |
//! | **D9** | `impl AgentRuntime for DeviceAgent`（蓝图 trait 无 `now_ms` 参数） | 复用 v0.72.0 `AgentRuntime` trait（含 `now_ms: u64` 参数） | v0.72.0 已定义 `AgentRuntime` trait + `HeartbeatStatus`；复用而非重定义，使 v0.74.0 MVP 编排器可统一调度 Energy/Market/Device 三种 Agent（trait 相同） |
//! | **D10** | `PcsPointMap::default()` / `BatteryPointMap::default()` / `MeterPointMap::default()` | `MockDevice::new(DeviceType::X).with_point("soc", 0.65)` 链式构造 | `PointMap`/`PcsPointMap`/`BatteryPointMap`/`MeterPointMap` 类型不存在；MockDevice 预设点位即可（Karpathy 简化原则） |
//! | **D11** | `DeviceInfo { device_type, protocol: String, address: String, point_map: PointMap }` | `DeviceInfo { device_type: DeviceType, adapter: Box<dyn DeviceAdapter> }` | 蓝图 `protocol`/`address`/`point_map` 不适用于 Mock 设备（MVP 无真实协议栈）；简化为 device_type + adapter |
//! | **D12** | `HashMap<String, Box<dyn PointAccess>>` / `HashMap<String, DeviceInfo>` | `BTreeMap<String, DeviceInfo>` | no_std `alloc::collections::BTreeMap`（`HashMap` 需哈希器配置或 `hashbrown`）；`BTreeMap` 是 no_std 标准选择，有序遍历便于测试 |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` / `core::*`，可交叉编译到 `aarch64-unknown-none`。

#![cfg_attr(not(test), no_std)]
extern crate alloc;

mod agent;
mod command;
mod device_type;
mod error;
mod registry;

pub use agent::DeviceAgent;
pub use command::{CommandSource, DeviceCommand, MockCommandSource};
pub use device_type::{DeviceSnapshot, DeviceState, DeviceType};
pub use error::DeviceError;
pub use registry::{DeviceAdapter, DeviceInfo, DeviceRegistry, MockDevice};

#[cfg(test)]
mod tests;

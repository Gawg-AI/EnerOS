//! EnerOS 模拟器完整版 (workspace v0.192.0) — P3-J 生态开放终版本.
//!
//! 开发者可在 PC 上无硬件开发、调试、测试完整 Agent。
//!
//! # 核心模块
//!
//! - [`types`] — 模拟器配置、实例、架构、设备类型等核心类型
//! - [`qemu`] — QEMU 后端（实例管理、命令行构建、进程控制）
//! - [`device_mock`] — 设备模拟（Modbus/IEC104/CAN/存储/网络）
//! - [`debugger`] — GDB 调试器接口（断点/单步/寄存器/内存）
//!
//! # 架构
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────┐
//! │  eneros-simulator (workspace v0.192.0)                            │
//! │                                                         │
//! │  ┌──────────┐  ┌──────────────┐  ┌──────────────────┐   │
//! │  │ types    │  │ qemu         │  │ device_mock      │   │
//! │  │ (配置/   │  │ (QEMU 后端  │  │ (Modbus/IEC104/  │   │
//! │  │  实例)   │  │  进程管理)   │  │  CAN/存储/网络)  │   │
//! │  └────┬─────┘  └──────┬───────┘  └────────┬─────────┘   │
//! │       │               │                    │             │
//! │       │        ┌──────▼────────────────────▼──────────┐  │
//! │       │        │ debugger (GDB 调试接口)               │  │
//! │       └───────►│ 断点 / 单步 / 寄存器 / 内存          │  │
//! │                └──────────────────────────────────────┘  │
//! │                                                         │
//! │  依赖：eneros-agent-sdk (workspace v0.192.0)                      │
//! └─────────────────────────────────────────────────────────┘
//! ```
//!
//! # std 策略
//!
//! 本 crate 核心类型和状态机为 no_std 兼容。
//! QEMU 进程管理（`std::process::Command`）和 TCP 调试连接
//! 仅在 `feature = "std"` 下编译（蓝图 §4.4 宿主机专属）。
//!
//! # 使用示例
//!
//! ```rust,no_run
//! use eneros_simulator::{QemuSimulator, Simulator, SimConfig, Arch};
//! use eneros_simulator::device_mock::{DeviceMockRegistry, ModbusRtuMock};
//! use eneros_simulator::types::DeviceMockConfig;
//!
//! // 1. 创建模拟器
//! let mut sim = QemuSimulator::new("/usr/bin/qemu-system-aarch64");
//!
//! // 2. 配置实例
//! let config = SimConfig::new()
//!     .with_arch(Arch::Arm64)
//!     .with_memory(256);
//!
//! // 3. 创建并启动
//! let id = sim.create_instance(&config).unwrap();
//! // sim.start(id).unwrap(); // 需要 std feature + QEMU 安装
//!
//! // 4. 设备模拟
//! let devices = DeviceMockRegistry::from_configs(&config.devices).unwrap();
//! ```
//!
//! # 偏差声明
//!
//! | 编号 | 偏差 | 理由 |
//! |------|------|------|
//! | **D1** | 蓝图 `simulator/eneros-sim/` → `crates/agents/simulator/`（eneros-simulator） | 记忆 §2.3.1 强制 crate 归 `crates/<subsystem>/`；模拟器服务于 Agent 开发 |
//! | **D2** | 蓝图 `serde_json::Value` → 强类型 `DeviceMockConfig` | no_std 兼容 + 零第三方依赖（记忆 §5.5） |
//! | **D3** | 蓝图 `simulator/tests/e2e_test.rs` → src 内嵌 `#[cfg(test)]` | 项目惯例（v0.87.0~v0.166.0） |
//! | **D4** | 进程管理 `#[cfg(feature = "std")]` 门控 | 蓝图 §4.4 明确宿主机专属，边缘路径禁用 |
//! | **D5** | QEMU 进程实际 spawn 需 `std` feature | no_std 下使用 mock 模式（mock_start/mock_stop） |
//! | **D6** | 蓝图 `Command::spawn` → 测试用 mock，std 下真实 spawn | CI 环境无 QEMU，测试不依赖外部进程 |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 核心类型/状态机 no_std 兼容；进程管理/网络连接需 `std` feature。
//! 依赖 eneros-agent-sdk（workspace path），零第三方依赖。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod debugger;
pub mod device_mock;
pub mod qemu;
pub mod types;

pub use debugger::{Breakpoint, DebugEvent, Debugger, DebuggerState, RegisterState};
pub use device_mock::{
    CanFrame, CanMock, DeviceMock, DeviceMockRegistry, Iec104Mock, ModbusRtuMock, ModbusTcpMock,
    NetworkMock, StorageMock,
};
pub use qemu::{QemuCommand, QemuSimulator, Simulator};
pub use types::{
    AgentConfig, Arch, DeviceMockConfig, DeviceType, SimConfig, SimInstance, SimState,
};

/// 模拟器错误枚举.
///
/// 覆盖 QEMU 管理、设备模拟、调试器全部失败面。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimError {
    /// QEMU 未安装或路径无效.
    QemuNotFound,
    /// 内核镜像不存在.
    ImageNotFound,
    /// 设备模拟错误（未初始化/越界/操作非法）.
    DeviceMockError,
    /// 模拟器实例未找到.
    InstanceNotFound,
    /// 实例已在运行中.
    AlreadyRunning,
    /// 实例未在运行.
    NotRunning,
    /// 调试器错误（未连接/状态非法）.
    DebuggerError,
    /// QEMU 进程派生失败.
    ProcessSpawnFailed,
}

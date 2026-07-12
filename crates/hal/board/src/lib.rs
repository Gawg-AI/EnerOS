//! EnerOS Board Support Crate (v0.3.0)
//!
//! 提供 ARM64 硬件启动所需的板级抽象：启动信息（`BootInfo`）、
//! 启动阶段标识（`BootStage`）以及最小 PL011 串口驱动（`Pl011Serial`）。
//!
//! 本 crate 为 `no_std` 库，**不定义** `panic_handler`，以便在 host 上
//! 运行单元测试时复用标准 test harness。`#![no_std]` 在 `#[cfg(test)]`
//! 模式下会自动切换为 std。
//!
//! 依据蓝图：`phase0.md` §v0.3.0。

#![no_std]

pub mod boot_info;
pub mod mini_uart;

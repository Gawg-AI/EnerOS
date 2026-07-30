#![no_std]

//! # eneros-bootloader
//!
//! EnerOS Bootloader（v0.129.0）库部分：
//!
//! - [`protocol`]：Bootloader 与内核之间的启动协议（[`BootProtocol`] 魔数/
//!   版本校验、[`BootStage`] 启动阶段模型）；
//! - [`uart`]：PL011 UART 驱动（[`RegIo`] 寄存器抽象 + [`MmioRegIo`] 真实
//!   MMIO 实现 + [`Pl011Uart`] 泛型驱动），支撑启动期串口日志；
//! - [`dtb`]：最小 FDT 解析器（[`parse_dtb_ram`]），从 DTB 提取 RAM 布局。
//! - [`ab_boot`]：v0.163.0 A/B 分区启动选择（[`BootConfig`] / [`AbBootSelector`]）。
//!
//! 同名 bin（`src/main.rs`）为启动主程序（v0.129.0 Task 2 填充真 entry）。
//!
//! ## ADR-0001 定位
//!
//! 依据 ADR-0001，Phase 3 主路径为 seL4 深度定制（方案 A）；本 Bootloader
//! 属自研内核研究/fallback 线的启动基线：负责内核镜像加载前的最小硬件
//! 初始化（串口）与启动信息（DTB/RAM）采集，并通过 [`BootProtocol`] 契约
//! 与内核入口交接。

pub mod ab_boot;
pub mod dtb;
pub mod protocol;
pub mod uart;

pub use ab_boot::{AbBootSelector, BootConfig, Partition as AbPartition, BOOT_CONFIG_LEN};
pub use dtb::{parse_dtb_ram, FdtError};
pub use protocol::{
    BootError, BootProtocol, BootStage, BOOT_PROTOCOL_MAGIC, BOOT_PROTOCOL_VERSION,
};
pub use uart::{MmioRegIo, Pl011Uart, RegIo, SerialPort};

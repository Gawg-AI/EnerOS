//! EnerOS GB/T 27930-2015（电动汽车非车载充电机与 BMS 通信）——
//! 充电机侧（桩端，owner 默认裁决）插件（蓝图 48 WP-48.4）.
//!
//! # 本卷交付边界（诚实登记，蓝图 48 执行模式 §2）
//!
//! owner 规约原文未在环境内提供；蓝图明示「Entry 必须取得规约原文核对
//! 地址与 PGN 表，禁止凭记忆」。本 crate 交付：
//!
//! - **已交付（蓝图卷内文本给定的结构事实）**：
//!   - [`sm`] 充电机侧状态机：握手→参数配置→充电→结束四相 + 每相超时
//!     分支 + 参数越限中止 + 中止后重连（消息名/相序=蓝图 WP-48.4 原文；
//!     **全部迁移测试** host 单测覆盖）；
//!   - [`tp`] 传输协议泛型重组逻辑：分片序号/总数/丢包超时/乱序缓冲/
//!     重复去重（>8B 报文多包传输的**逻辑层**；TP.CM/TP.DT 字节布局
//!     BLOCKED）；
//!   - [`config`] typed 配置：超时表与节点地址**必须显式配置**
//!     （规约表数值/地址表 BLOCKED——禁凭记忆缺省，fail-closed）；
//!   - `ProtocolPlugin` seam：descriptor `interop_level = SpecBlocked`、
//!     `transport_kinds = &[]`（CAN 传输接线=51 卷真机路径；QEMU virt
//!     无 CAN 如实分级）。
//! - **BLOCKED（owner 输入：GB/T 27930-2015 规约文本）**：CAN 2.0B
//!   29 位扩展 ID 编码、PGN 表、源/目的节点地址表（充电机地址待规约
//!   附录核对）、TP.CM/TP.DT 字节格式、超时与重发参数表、L1 host
//!   USB-CAN 适配器 + BMS 模拟器实弹（owner 硬件）、target 真机（51 卷）。
//!   descriptor HardwareVerified=未达成（诚实）。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod config;
pub mod plugin;
pub mod sm;
pub mod tp;

pub use config::Gbt27930Config;
pub use plugin::Gbt27930Plugin;
pub use sm::{AbortReason, Action, ChargePhase, ChargerSm, Msg};
pub use tp::{TpFrame, TpReassembler, TpStatus};

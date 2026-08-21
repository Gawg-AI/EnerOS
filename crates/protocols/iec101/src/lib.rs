//! EnerOS IEC 60870-5-101（串口远动）master 插件（蓝图 48 WP-48.2）.
//!
//! # 本卷交付边界（诚实登记，蓝图 48 执行模式 §2 + 锚点修订记录）
//!
//! owner 规约原文未在环境内提供，且**锚点修订**：蓝图假设的「FT1.2/
//! 18657 在库证据」不存在（104=TCP APCI 非串口 FT1.2）——本 crate 交付：
//!
//! - **已交付**：crate 骨架 / `ProtocolPlugin` seam 接入 / 配置 typed 消费
//!   （链路地址/公共地址尺寸 1|2、平衡模式、t0-t3/k/w——键集=蓝图卷内登记）/
//!   Serial 传输接线 / 点表路由 seam（`ProtocolAddress::Iec101`）/
//!   **ASDU 公共层真复用**（[`asdu_bridge`]：经 WP-48.0 共用 crate
//!   构造+解析，字节布局断言）；
//! - **BLOCKED（owner 输入：IEC 60870-5-101 规约文本）**：FT1.2 帧格式
//!   字节层（固定帧/E5/ack）、链路规程（RESET_LINK/1·2 级召唤/FIR-FIN-CON
//!   分段）、非平衡召唤全流程 host peer 双向证据、分段重组负例、slave
//!   侧插件（成对需求随 codec 一并解阻）。
//!
//! descriptor `interop_level = SpecBlocked`；read/write 诚实 Unsupported。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod asdu_bridge;
pub mod config;
pub mod plugin;

pub use config::Iec101Config;
pub use plugin::Iec101Plugin;

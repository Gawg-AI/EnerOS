//! EnerOS DL/T 645-2007 多功能电能表通信协议——master 插件（蓝图 48 WP-48.1）.
//!
//! # 本卷交付边界（诚实登记，蓝图 48 执行模式 §2）
//!
//! owner 规约原文（DL/T 645-2007）**未在环境内提供**（全盘搜索登记于
//! evidence/48/WP-Entry.md）——依蓝图纪律「AI 不得凭记忆编造帧格式，每个
//! codec 字段必须能引用规约章节号」，本 crate 交付：
//!
//! - **已交付**：crate 骨架 / `ProtocolPlugin` 六方法 seam 接入 /
//!   配置 typed 消费（未知键拒绝）/ Serial 传输接线（46.1 结论路径）/
//!   点表驱动路由 seam（[`ProtocolAddress::Dlt645`] 变体）；
//! - **BLOCKED（owner 输入：规约文本）**：帧编解码字节层（68H/地址 BCD
//!   线序/控制码/CS 覆盖范围）、规约附录示例帧 KAT、4 组负例、host 独立
//!   peer（python 645 模拟表）读数通路。
//!
//! 因此 descriptor `interop_level = SpecBlocked`（诚实分级，10.3 纪律）；
//! `read_point`/`write_point` 返回 `Err(Unsupported)`——不伪造读数。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod config;
pub mod plugin;

pub use config::Dlt645Config;
pub use plugin::Dlt645Plugin;

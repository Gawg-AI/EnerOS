//! EnerOS SDK — 开发者软件开发工具包
//!
//! 封装 Agent/协议/插件开发的常用类型与辅助函数，为第三方开发者提供
//! 统一的入口点。SDK 基于 EnerOS 核心 crate 构建，简化常见开发场景：
//! - Agent 开发：生命周期管理、消息通信、协作调度
//! - 协议开发：网关接入、命令下发、安全校验
//! - 插件开发：动态加载、签名验证、沙箱隔离
//!
//! 通过 feature 门控按需启用模块，默认启用 `full`（包含全部模块）。

pub mod common;

#[cfg(feature = "agent")]
pub mod agent;

#[cfg(feature = "protocol")]
pub mod protocol;

#[cfg(feature = "plugin")]
pub mod plugin;

pub use common::*;

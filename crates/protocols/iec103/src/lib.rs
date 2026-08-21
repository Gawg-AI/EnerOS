//! EnerOS IEC 60870-5-103（继电保护设备信息接口）master 插件
//! （蓝图 48 WP-48.3）.
//!
//! # 本卷交付边界（诚实登记，蓝图 48 执行模式 §2）
//!
//! owner 规约原文未在环境内提供——103 的 ASDU 为**自有格式**（类型标识/
//! VS/CO/COT/FUN/INF 各 1B，与 104 ASDU 不同源），**无在库可复用实现**
//! ——本 crate 交付：
//!
//! - **已交付**：crate 骨架 / `ProtocolPlugin` seam 接入 / 配置 typed
//!   消费（common_addr/timeout_ms/retries）/ Serial 传输接线 / 点表路由
//!   seam（`ProtocolAddress::Iec103{common_addr,fun,inf}`）；
//! - **BLOCKED（owner 输入：IEC 60870-5-103 规约文本）**：FT1.2 帧格式
//!   （同 101）、103 自有 ASDU 编解码（ASDU1 通道信息/ASDU2 故障位置/
//!   ASDU5·6 上电复位/ASDU6 时钟同步/ASDU7 总召唤/ASDU23·26·27 扰动数据
//!   多帧分段）、规约示例帧 KAT、总召唤+故障事件→点表通路（host peer）、
//!   扰动数据分段负例。范围裁剪（蓝图卷内登记）：扰动数据块仅登记读取，
//!   不做完整录波解析——descriptor `point_types` 边界注明。
//!
//! descriptor `interop_level = SpecBlocked`；read/write 诚实 Unsupported。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod config;
pub mod plugin;

pub use config::Iec103Config;
pub use plugin::Iec103Plugin;

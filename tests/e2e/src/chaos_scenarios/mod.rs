//! 混沌工程测试场景集合。
//!
//! 每个场景验证 EnerOS 在混沌注入下的韧性：
//! - [`network_partition`] — 网络分区混沌
//! - [`agent_crash`] — Agent 崩溃混沌
//! - [`cpu_saturation`] — CPU 饱和混沌
//! - [`disk_full`] — 磁盘满混沌
//!
//! 简化版场景仅验证混沌注入器本身的可启动/可停止性。
//! 完整版场景（需集群）通过 `#[ignore]` 标注在集成测试中运行。

pub mod agent_crash;
pub mod cpu_saturation;
pub mod disk_full;
pub mod network_partition;

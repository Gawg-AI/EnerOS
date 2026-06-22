//! 端到端测试场景集合。
//!
//! 每个场景验证 EnerOS 的一个端到端行为路径，通过 HTTP API
//! 与运行中的集群交互。

pub mod agent_decision;
pub mod command_dispatch;
pub mod ha_failover;
pub mod plugin_lifecycle;
pub mod scada_pipeline;
pub mod startup;

//! Agent 运行时错误类型（D12：仅 Debug，不 Clone）.
//!
//! 蓝图 54 WP-54.3：`AgentRuntimeError` 定义体已下移至 `eneros-agent`
//! （src/runtime.rs，含 From<DualBrainError>/From<AgentError> 实现）；
//! 此处 re-export 保持 `crate::error::` 路径与既有内部引用稳定。

pub use eneros_agent::AgentRuntimeError;

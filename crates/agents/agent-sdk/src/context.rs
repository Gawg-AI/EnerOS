//! Agent 上下文（SDK v1.0 稳定 API）。
//!
//! [`Context`] 是 SDK 提供给 Agent 的运行时环境，聚合 IPC、存储、日志、配置。

use crate::agent::AgentId;
use crate::config::Config;
use crate::ipc::IpcClient;
use crate::logger::Logger;
use crate::storage::Storage;

/// Agent 上下文：SDK 提供的运行时环境。
///
/// 由 Agent Runtime 在加载 Agent 时构造，通过 `&mut` 传递给 Agent 回调。
pub struct Context {
    /// Agent ID（由 Runtime 分配）
    pub agent_id: AgentId,
    /// 配置
    pub config: Config,
    /// IPC 客户端
    pub ipc: IpcClient,
    /// 日志器
    pub logger: Logger,
    /// 存储
    pub storage: Storage,
}

impl Context {
    /// 创建新的上下文（测试/模拟用）。
    pub fn new(agent_id: AgentId) -> Self {
        Self {
            agent_id,
            config: Config::new(),
            ipc: IpcClient::new(),
            logger: Logger::new(agent_id),
            storage: Storage::new(),
        }
    }

    /// 获取 Agent ID。
    pub fn agent_id(&self) -> AgentId {
        self.agent_id
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// CTX1：上下文构造。
    #[test]
    fn ctx1_context_construction() {
        let ctx = Context::new(AgentId(42));
        assert_eq!(ctx.agent_id(), AgentId(42));
    }
}

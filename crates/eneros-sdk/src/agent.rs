//! Agent 开发 SDK — AgentBuilder 与 AgentSdk
//!
//! 提供构造器模式构建 [`AgentConfig`]，以及封装 Agent 运行时所需的
//! EventBus/Gateway 客户端句柄。第三方开发者通过本模块可以快速组装
//! Agent 进程配置，而无需直接操作底层 crate 的字段。

use crate::common::SdkResult;
use eneros_agent::{AgentConfig, AgentType};
use eneros_core::{AuthorityLevel, GatewayClient, Jurisdiction};
use std::sync::Arc;

/// 默认 EventBus broker 地址
const DEFAULT_EVENTBUS_ADDR: &str = "127.0.0.1:9876";
/// 默认 Gateway 服务地址
const DEFAULT_GATEWAY_ADDR: &str = "127.0.0.1:9877";
/// 默认 IPC socket 目录
const DEFAULT_IPC_SOCKET_DIR: &str = "/var/run/eneros";

/// Agent 构造器 — 使用构造器模式构建 [`AgentConfig`]
///
/// # 示例
/// ```no_run
/// use eneros_sdk::agent::AgentBuilder;
/// use eneros_agent::AgentType;
/// use eneros_core::{AuthorityLevel, Jurisdiction};
///
/// let config = AgentBuilder::new("dispatch-1", AgentType::Dispatcher)
///     .authority(AuthorityLevel::Supervisor)
///     .jurisdiction(Jurisdiction::for_zones(vec![1, 2]))
///     .tick_interval(std::time::Duration::from_millis(500))
///     .eventbus_addr("127.0.0.1:9876")
///     .gateway_addr("127.0.0.1:9877")
///     .build()
/// .unwrap();
/// ```
pub struct AgentBuilder {
    agent_id: String,
    agent_type: AgentType,
    authority: AuthorityLevel,
    jurisdiction: Jurisdiction,
    tick_interval: std::time::Duration,
    eventbus_addr: String,
    gateway_addr: String,
    ipc_socket_dir: String,
}

impl AgentBuilder {
    /// 创建新的 Agent 构造器
    ///
    /// 默认值：
    /// - authority: [`AuthorityLevel::Operator`]
    /// - jurisdiction: [`Jurisdiction::unrestricted`]
    /// - tick_interval: 1 秒
    /// - eventbus_addr: `127.0.0.1:9876`
    /// - gateway_addr: `127.0.0.1:9877`
    /// - ipc_socket_dir: `/var/run/eneros`
    pub fn new(agent_id: impl Into<String>, agent_type: AgentType) -> Self {
        Self {
            agent_id: agent_id.into(),
            agent_type,
            authority: AuthorityLevel::Operator,
            jurisdiction: Jurisdiction::unrestricted(),
            tick_interval: std::time::Duration::from_secs(1),
            eventbus_addr: DEFAULT_EVENTBUS_ADDR.to_string(),
            gateway_addr: DEFAULT_GATEWAY_ADDR.to_string(),
            ipc_socket_dir: DEFAULT_IPC_SOCKET_DIR.to_string(),
        }
    }

    /// 设置权限级别
    pub fn authority(mut self, authority: AuthorityLevel) -> Self {
        self.authority = authority;
        self
    }

    /// 设置管辖范围
    pub fn jurisdiction(mut self, jurisdiction: Jurisdiction) -> Self {
        self.jurisdiction = jurisdiction;
        self
    }

    /// 设置 tick 周期（内部转换为毫秒存入 `tick_interval_ms`）
    pub fn tick_interval(mut self, d: std::time::Duration) -> Self {
        self.tick_interval = d;
        self
    }

    /// 设置 EventBus broker 地址（如 `127.0.0.1:9876`）
    pub fn eventbus_addr(mut self, addr: impl Into<String>) -> Self {
        self.eventbus_addr = addr.into();
        self
    }

    /// 设置 Gateway 服务地址（如 `127.0.0.1:9877`）
    pub fn gateway_addr(mut self, addr: impl Into<String>) -> Self {
        self.gateway_addr = addr.into();
        self
    }

    /// 设置 IPC socket 目录
    pub fn ipc_socket_dir(mut self, dir: impl Into<String>) -> Self {
        self.ipc_socket_dir = dir.into();
        self
    }

    /// 构建 [`AgentConfig`]
    pub fn build(self) -> SdkResult<AgentConfig> {
        Ok(AgentConfig {
            agent_id: self.agent_id,
            agent_type: self.agent_type,
            authority: self.authority,
            jurisdiction: self.jurisdiction,
            tick_interval_ms: self.tick_interval.as_millis() as u64,
            eventbus_addr: self.eventbus_addr,
            gateway_addr: self.gateway_addr,
            ipc_socket_dir: self.ipc_socket_dir,
        })
    }
}

/// Agent SDK 封装 — 提供 [`AgentConfig`] 与可选的客户端句柄
///
/// 持有 Agent 进程运行所需的配置和已连接的客户端句柄。
/// 由于 `EventBusClient` 的方法签名为 `&mut self`，这里使用
/// `Arc<tokio::sync::Mutex<_>>` 包装以便跨任务共享。
/// `GatewayClient` 是 trait object，由调用方注入具体实现
/// （如 `RemoteGatewayClient`）。
pub struct AgentSdk {
    /// Agent 配置
    pub config: AgentConfig,
    /// EventBus 客户端句柄（可选，需异步连接后注入）
    pub event_bus_client: Option<Arc<tokio::sync::Mutex<eneros_eventbus::EventBusClient>>>,
    /// Gateway 客户端句柄（可选，trait object）
    pub gateway_client: Option<Arc<dyn GatewayClient>>,
}

impl AgentSdk {
    /// 创建 AgentSdk，不持有任何客户端句柄
    pub fn new(config: AgentConfig) -> Self {
        Self {
            config,
            event_bus_client: None,
            gateway_client: None,
        }
    }

    /// 注入已构造的 EventBus 客户端
    pub fn with_event_bus(
        mut self,
        client: Arc<tokio::sync::Mutex<eneros_eventbus::EventBusClient>>,
    ) -> Self {
        self.event_bus_client = Some(client);
        self
    }

    /// 注入已构造的 Gateway 客户端
    pub fn with_gateway(mut self, client: Arc<dyn GatewayClient>) -> Self {
        self.gateway_client = Some(client);
        self
    }
}

/// 辅助函数：记录 spawn agent 进程的意图
///
/// 简化实现：仅打印配置信息。实际的进程 spawn 由 `eneros-init` /
/// `AgentSupervisor` 负责，SDK 层不直接 fork 进程。
pub fn spawn_agent(config: &AgentConfig) -> SdkResult<()> {
    tracing::info!(
        agent_id = %config.agent_id,
        agent_type = ?config.agent_type,
        eventbus_addr = %config.eventbus_addr,
        gateway_addr = %config.gateway_addr,
        "spawn_agent requested"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_builder_new() {
        let builder = AgentBuilder::new("test-agent", AgentType::Operator);
        assert_eq!(builder.agent_id, "test-agent");
        assert_eq!(builder.agent_type, AgentType::Operator);
        assert_eq!(builder.authority, AuthorityLevel::Operator);
        assert_eq!(builder.tick_interval, std::time::Duration::from_secs(1));
        assert_eq!(builder.eventbus_addr, DEFAULT_EVENTBUS_ADDR);
        assert_eq!(builder.gateway_addr, DEFAULT_GATEWAY_ADDR);
    }

    #[test]
    fn test_agent_builder_with_authority() {
        let builder = AgentBuilder::new("test-agent", AgentType::Operator)
            .authority(AuthorityLevel::Supervisor);
        assert_eq!(builder.authority, AuthorityLevel::Supervisor);
    }

    #[test]
    fn test_agent_builder_with_jurisdiction() {
        let jurisdiction = Jurisdiction::for_zones(vec![1, 2, 3]);
        let builder = AgentBuilder::new("test-agent", AgentType::Operator)
            .jurisdiction(jurisdiction);
        assert!(builder.jurisdiction.contains_zone(1));
        assert!(builder.jurisdiction.contains_zone(2));
        assert!(!builder.jurisdiction.contains_zone(99));
    }

    #[test]
    fn test_agent_builder_build() {
        let config = AgentBuilder::new("agent-1", AgentType::Dispatcher)
            .authority(AuthorityLevel::Supervisor)
            .tick_interval(std::time::Duration::from_millis(500))
            .eventbus_addr("10.0.0.1:9876")
            .gateway_addr("10.0.0.1:9877")
            .build()
            .expect("build should succeed");
        assert_eq!(config.agent_id, "agent-1");
        assert_eq!(config.agent_type, AgentType::Dispatcher);
        assert_eq!(config.authority, AuthorityLevel::Supervisor);
        assert_eq!(config.tick_interval_ms, 500);
        assert_eq!(config.eventbus_addr, "10.0.0.1:9876");
        assert_eq!(config.gateway_addr, "10.0.0.1:9877");
        assert_eq!(config.ipc_socket_dir, DEFAULT_IPC_SOCKET_DIR);
    }

    #[test]
    fn test_agent_sdk_new() {
        let config = AgentBuilder::new("agent-2", AgentType::Operator)
            .build()
            .unwrap();
        let sdk = AgentSdk::new(config);
        assert_eq!(sdk.config.agent_id, "agent-2");
        assert!(sdk.event_bus_client.is_none());
        assert!(sdk.gateway_client.is_none());
    }
}

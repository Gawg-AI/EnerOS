//! GatewayServer — SafetyGateway 的 IPC 服务端（v0.15.0）
//!
//! 在主 eneros-api 进程（v0.15.0）或独立 Gateway 进程（v0.16.0）中运行，
//! 通过 TCP 暴露 `SafetyGateway` 服务给 Agent 进程。
//!
//! 线格式与 `eneros_eventbus::broker::EventBusBroker` 一致：
//! 4 字节小端长度前缀 + JSON payload。
//!
//! 并发模型：每个 TCP 连接 `tokio::spawn` 一个独立任务，共享同一个
//! `LocalGatewayClient`（其内部 `Arc<SafetyGateway>` 可安全跨任务共享）。

use tokio::net::{TcpListener, TcpStream};
use tracing::{error, info};

use eneros_core::GatewayClient;

use crate::client::{
    read_frame, write_frame, GatewayRequest, GatewayResponse, LocalGatewayClient,
};

/// IPC 服务端，通过 TCP 暴露 `SafetyGateway`。
///
/// 持有一个 `LocalGatewayClient`（包装 `Arc<SafetyGateway>`），所有
/// 连接共享同一个客户端实例。`LocalGatewayClient` 是 `Clone` 的
/// （仅持有 `Arc`），因此可廉价地为每个连接复制一份。
pub struct GatewayServer {
    client: LocalGatewayClient,
    addr: String,
}

impl GatewayServer {
    /// 创建服务端。
    ///
    /// `client` 通常是 `LocalGatewayClient::with_pipeline(gateway, pipeline)`，
    /// 以同时支持 `execute_command` 和 `decide`。
    pub fn new(client: LocalGatewayClient, addr: impl Into<String>) -> Self {
        Self {
            client,
            addr: addr.into(),
        }
    }

    /// 绑定并运行服务端，阻塞当前任务直到进程终止。
    ///
    /// 每个传入连接会 `tokio::spawn` 一个独立任务处理；连接断开不影响
    /// 其他连接。`accept` 错误会记录日志但不会终止服务端。
    pub async fn run(&self) -> anyhow::Result<()> {
        let listener = TcpListener::bind(&self.addr)
            .await
            .map_err(|e| anyhow::anyhow!("bind {} failed: {}", self.addr, e))?;
        info!("GatewayServer listening on {}", self.addr);

        loop {
            match listener.accept().await {
                Ok((stream, peer)) => {
                    // 每个连接克隆一份 LocalGatewayClient（仅 Arc 引用计数 +1）
                    let client = self.client.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(client, stream).await {
                            error!("Gateway connection from {} error: {}", peer, e);
                        }
                    });
                }
                Err(e) => {
                    error!("accept error on {}: {}", self.addr, e);
                }
            }
        }
    }

    /// 返回绑定的地址。
    pub fn addr(&self) -> &str {
        &self.addr
    }
}

/// 处理单个 TCP 连接：循环读取请求 → 处理 → 写响应，直到客户端断开。
///
/// 每个连接独立运行在自己的 tokio 任务中，因此慢客户端不会阻塞
/// 其他连接。`LocalGatewayClient` 内部 `Arc<SafetyGateway>` 的
/// per-device 锁池保证同设备命令串行、跨设备命令并发。
async fn handle_connection(
    client: LocalGatewayClient,
    mut stream: TcpStream,
) -> anyhow::Result<()> {
    loop {
        let req_buf = read_frame(&mut stream)
            .await?
            .ok_or_else(|| anyhow::anyhow!("connection closed"))?;
        let req: GatewayRequest = serde_json::from_slice(&req_buf)?;

        let resp = handle_request(&client, req).await;

        let resp_bytes = serde_json::to_vec(&resp)?;
        write_frame(&mut stream, &resp_bytes).await?;
    }
}

/// 处理单个请求，返回响应。
async fn handle_request(
    client: &LocalGatewayClient,
    req: GatewayRequest,
) -> GatewayResponse {
    match req {
        GatewayRequest::ExecuteCommand { command } => match client.execute_command(command).await {
            Ok(result) => GatewayResponse::ExecutionResult { result },
            Err(e) => GatewayResponse::Error {
                message: e.to_string(),
            },
        },
        GatewayRequest::ValidateCommand { command } => {
            match client.validate_command(&command).await {
                Ok(()) => GatewayResponse::Validated,
                Err(e) => GatewayResponse::Error {
                    message: e.to_string(),
                },
            }
        }
        GatewayRequest::SubmitCommand { command } => {
            match client.submit_command(command).await {
                Ok(()) => GatewayResponse::Submitted,
                Err(e) => GatewayResponse::Error {
                    message: e.to_string(),
                },
            }
        }
        GatewayRequest::Decide { action, context } => {
            match client.decide(action, context).await {
                Ok(result) => GatewayResponse::Decision { result },
                Err(e) => GatewayResponse::Error {
                    message: e.to_string(),
                },
            }
        }
    }
}

// `GatewayServer` 不需要显式 Clone：它持有 `LocalGatewayClient`（Clone）
// 和 `String`（Clone）。但服务端通常单实例运行，不暴露 Clone。
// 这里仍派生 Clone 以便测试中可以保留一份引用。
impl Clone for GatewayServer {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            addr: self.addr.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use eneros_core::agentos_types::{
        AuthorityLevel, Jurisdiction, StructuredAction, SystemOperatingState,
    };
    use eneros_core::pipeline_types::DecisionContextCore;
    use eneros_core::{Command, CommandPriority, CommandType};

    use crate::gateway::SafetyGateway;

    #[tokio::test]
    async fn test_handle_request_validate_command_ok() {
        let gateway = Arc::new(SafetyGateway::new(10));
        let client = LocalGatewayClient::new(gateway);
        let cmd = Command::new(CommandType::SwitchToggle, 1, CommandPriority::Normal, "test");
        let resp = handle_request(&client, GatewayRequest::ValidateCommand { command: cmd }).await;
        assert!(matches!(resp, GatewayResponse::Validated));
    }

    #[tokio::test]
    async fn test_handle_request_decide_without_pipeline_returns_error() {
        let gateway = Arc::new(SafetyGateway::new(10));
        let client = LocalGatewayClient::new(gateway);
        let action = StructuredAction::StartGenerator {
            gen_id: 1,
            target_mw: 100.0,
        };
        let ctx = DecisionContextCore {
            authority: AuthorityLevel::Supervisor,
            jurisdiction: Jurisdiction::unrestricted(),
            system_state: SystemOperatingState::Normal,
            observation: None,
            agent_id: "test".to_string(),
            reasoning: "".to_string(),
        };
        let resp = handle_request(
            &client,
            GatewayRequest::Decide {
                action,
                context: ctx,
            },
        )
        .await;
        match resp {
            GatewayResponse::Error { message } => {
                assert!(message.contains("pipeline"));
            }
            _ => panic!("expected Error response, got {:?}", resp),
        }
    }
}

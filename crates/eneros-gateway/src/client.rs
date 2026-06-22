//! GatewayClient 实现（v0.15.0）
//!
//! 提供 `GatewayClient` trait 的两种实现：
//! - `LocalGatewayClient`：进程内使用，包装 `Arc<SafetyGateway>`（可选 `Arc<ConstrainedDecisionPipeline>`）
//! - `RemoteGatewayClient`：通过 TCP IPC 访问独立 Gateway 进程
//!
//! 线格式与 `eneros_eventbus::broker::EventBusBroker` 一致：
//! 4 字节小端长度前缀 + JSON payload。

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use eneros_core::agentos_types::StructuredAction;
use eneros_core::pipeline_types::{DecisionContextCore, DecisionResultCore};
use eneros_core::{Command, ExecutionResult, GatewayClient};

use crate::decision_pipeline::ConstrainedDecisionPipeline;
use crate::gateway::SafetyGateway;
use crate::pipeline_types::{DecisionContext, EnhancedPipelineDecision};

// ============================================================================
// LocalGatewayClient
// ============================================================================

/// 进程内 GatewayClient，包装 `Arc<SafetyGateway>`。
///
/// 用于 Agent 与 Gateway 同进程的场景（如测试、legacy 模式）。
/// 持有 `Arc<SafetyGateway>`，因此可被 `Clone`（廉价引用计数克隆）。
/// 可选持有 `Arc<ConstrainedDecisionPipeline>` 以支持 `decide()`；
/// 若未配置则 `decide()` 返回错误。
#[derive(Clone)]
pub struct LocalGatewayClient {
    gateway: Arc<SafetyGateway>,
    pipeline: Option<Arc<ConstrainedDecisionPipeline>>,
}

impl LocalGatewayClient {
    /// 创建不带决策管线的客户端（`decide()` 将返回错误）。
    pub fn new(gateway: Arc<SafetyGateway>) -> Self {
        Self {
            gateway,
            pipeline: None,
        }
    }

    /// 创建带决策管线的客户端。
    pub fn with_pipeline(
        gateway: Arc<SafetyGateway>,
        pipeline: Arc<ConstrainedDecisionPipeline>,
    ) -> Self {
        Self {
            gateway,
            pipeline: Some(pipeline),
        }
    }

    /// 返回内部 `SafetyGateway` 的引用（供测试或同进程高级用法使用）。
    pub fn gateway(&self) -> &Arc<SafetyGateway> {
        &self.gateway
    }
}

#[async_trait]
impl GatewayClient for LocalGatewayClient {
    async fn execute_command(&self, cmd: Command) -> anyhow::Result<ExecutionResult> {
        self.gateway
            .execute_command(cmd)
            .await
            .map_err(|e| anyhow::anyhow!("execute_command failed: {}", e))?;
        Ok(self
            .gateway
            .last_execution_result()
            .unwrap_or_else(|| ExecutionResult::ok(
                "no result recorded".to_string(),
                std::time::Duration::ZERO,
            )))
    }

    async fn validate_command(&self, cmd: &Command) -> anyhow::Result<()> {
        self.gateway
            .validate_command(cmd)
            .map_err(|e| anyhow::anyhow!("validate_command failed: {}", e))
    }

    async fn submit_command(&self, cmd: Command) -> anyhow::Result<()> {
        self.gateway
            .submit_command(cmd)
            .map_err(|e| anyhow::anyhow!("submit_command failed: {}", e))
    }

    async fn decide(
        &self,
        action: StructuredAction,
        ctx_core: DecisionContextCore,
    ) -> anyhow::Result<DecisionResultCore> {
        let pipeline = self
            .pipeline
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no decision pipeline configured"))?;
        // 将 DecisionContextCore 重建为 DecisionContext。
        // device_states 字段不在 Core 中，默认为 None；调用方如需 interlocking
        // 检查应通过其他途径注入设备状态。
        let ctx = DecisionContext::from(&ctx_core);
        let result: EnhancedPipelineDecision = pipeline.decide_enhanced(&action, &ctx).await;
        Ok(DecisionResultCore::from(&result))
    }
}

// ============================================================================
// Wire format (shared with server.rs)
// ============================================================================

/// 客户端 → 服务端 请求消息。
///
/// 使用 `#[serde(tag = "type")]` 内部标签，与 `EventBusBroker` 风格一致。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
#[allow(clippy::large_enum_variant)]
pub enum GatewayRequest {
    ExecuteCommand { command: Command },
    ValidateCommand { command: Command },
    SubmitCommand { command: Command },
    Decide {
        action: StructuredAction,
        context: DecisionContextCore,
    },
}

/// 服务端 → 客户端 响应消息。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum GatewayResponse {
    ExecutionResult { result: ExecutionResult },
    Validated,
    Submitted,
    Decision { result: DecisionResultCore },
    Error { message: String },
}

/// 单个请求/响应帧的最大字节数（16 MiB），与 EventBusBroker 一致。
const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// 写一帧（4 字节 LE 长度前缀 + JSON payload）。
pub(crate) async fn write_frame<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    payload: &[u8],
) -> std::io::Result<()> {
    let len = payload.len() as u32;
    writer.write_all(&len.to_le_bytes()).await?;
    writer.write_all(payload).await?;
    writer.flush().await?;
    Ok(())
}

/// 读一帧（4 字节 LE 长度前缀 + JSON payload）。
///
/// 返回 `Ok(None)` 表示连接已正常关闭（EOF 在长度前缀之前）。
pub(crate) async fn read_frame<R: AsyncReadExt + Unpin>(
    reader: &mut R,
) -> std::io::Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("frame too large: {} bytes", len),
        ));
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await?;
    Ok(Some(buf))
}

// ============================================================================
// RemoteGatewayClient
// ============================================================================

/// IPC 客户端，通过 TCP 访问独立 Gateway 进程。
///
/// 每次请求建立一个新的 TCP 连接（请求-响应模式）。这简化了协议
/// （无需管理长连接的复用与心跳），适合 Agent 进程对 Gateway 的
/// 低频控制调用。高频场景可在 v0.16.0 演进为连接池。
///
/// 所有请求都带有 10 秒超时，防止 Gateway 进程挂起时 Agent 永久阻塞。
pub struct RemoteGatewayClient {
    addr: String,
    /// Per-request timeout. Defaults to 10 seconds.
    request_timeout: std::time::Duration,
}

/// Default request timeout for RemoteGatewayClient.
const DEFAULT_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

impl RemoteGatewayClient {
    pub fn new(addr: impl Into<String>) -> Self {
        Self {
            addr: addr.into(),
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        }
    }

    /// Create a RemoteGatewayClient with a custom request timeout.
    pub fn with_timeout(addr: impl Into<String>, timeout: std::time::Duration) -> Self {
        Self {
            addr: addr.into(),
            request_timeout: timeout,
        }
    }

    /// 返回服务端地址。
    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// 发送一次请求并读取响应（带超时）。
    ///
    /// 超时保护覆盖整个请求-响应周期（connect + write + read），
    /// 防止 Gateway 进程挂起时 Agent 永久阻塞。
    async fn request(&self, req: GatewayRequest) -> anyhow::Result<GatewayResponse> {
        let timeout = self.request_timeout;
        tokio::time::timeout(timeout, async {
            let mut stream = TcpStream::connect(&self.addr).await.map_err(|e| {
                anyhow::anyhow!("connect to gateway {} failed: {}", self.addr, e)
            })?;
            let payload = serde_json::to_vec(&req)?;
            write_frame(&mut stream, &payload).await?;

            let resp_buf = read_frame(&mut stream)
                .await?
                .ok_or_else(|| anyhow::anyhow!("connection closed before response"))?;
            let resp: GatewayResponse = serde_json::from_slice(&resp_buf)?;
            Ok(resp)
        })
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "gateway request to {} timed out after {:?}",
                self.addr,
                timeout
            )
        })?
    }
}

#[async_trait]
impl GatewayClient for RemoteGatewayClient {
    async fn execute_command(&self, cmd: Command) -> anyhow::Result<ExecutionResult> {
        match self.request(GatewayRequest::ExecuteCommand { command: cmd }).await? {
            GatewayResponse::ExecutionResult { result } => Ok(result),
            GatewayResponse::Error { message } => Err(anyhow::anyhow!(message)),
            _ => Err(anyhow::anyhow!("unexpected response type for execute_command")),
        }
    }

    async fn validate_command(&self, cmd: &Command) -> anyhow::Result<()> {
        match self
            .request(GatewayRequest::ValidateCommand { command: cmd.clone() })
            .await?
        {
            GatewayResponse::Validated => Ok(()),
            GatewayResponse::Error { message } => Err(anyhow::anyhow!(message)),
            _ => Err(anyhow::anyhow!("unexpected response type for validate_command")),
        }
    }

    async fn submit_command(&self, cmd: Command) -> anyhow::Result<()> {
        match self.request(GatewayRequest::SubmitCommand { command: cmd }).await? {
            GatewayResponse::Submitted => Ok(()),
            GatewayResponse::Error { message } => Err(anyhow::anyhow!(message)),
            _ => Err(anyhow::anyhow!("unexpected response type for submit_command")),
        }
    }

    async fn decide(
        &self,
        action: StructuredAction,
        ctx: DecisionContextCore,
    ) -> anyhow::Result<DecisionResultCore> {
        match self
            .request(GatewayRequest::Decide { action, context: ctx })
            .await?
        {
            GatewayResponse::Decision { result } => Ok(result),
            GatewayResponse::Error { message } => Err(anyhow::anyhow!(message)),
            _ => Err(anyhow::anyhow!("unexpected response type for decide")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eneros_core::{CommandPriority, CommandType};

    #[test]
    fn test_gateway_request_serde_roundtrip() {
        let cmd = Command::new(CommandType::SwitchToggle, 42, CommandPriority::Normal, "test");
        let req = GatewayRequest::ExecuteCommand { command: cmd };
        let json = serde_json::to_string(&req).unwrap();
        let de: GatewayRequest = serde_json::from_str(&json).unwrap();
        match de {
            GatewayRequest::ExecuteCommand { command } => {
                assert_eq!(command.target_id, 42);
            }
            _ => panic!("expected ExecuteCommand"),
        }
    }

    #[test]
    fn test_gateway_response_error_serde() {
        let resp = GatewayResponse::Error {
            message: "boom".to_string(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let de: GatewayResponse = serde_json::from_str(&json).unwrap();
        match de {
            GatewayResponse::Error { message } => assert_eq!(message, "boom"),
            _ => panic!("expected Error"),
        }
    }

    #[test]
    fn test_local_gateway_client_clone() {
        let gateway = Arc::new(SafetyGateway::new(10));
        let client = LocalGatewayClient::new(gateway);
        let _client2 = client.clone();
        // 不 panic 即可
    }
}

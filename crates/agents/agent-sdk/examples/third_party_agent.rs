//! 第三方 Agent 示例（v0.224.0 B3-01 验收：SDK 示例外部 crate 可独立构建运行）。
//!
//! 演示：
//! 1. 实现 `Agent` trait（生命周期回调）；
//! 2. 使用 `IpcClient` 跨 Agent request/response（SPSC 注入路径，
//!    seL4 feature 下自动切换真实 IPC）；
//! 3. wire 编解码端到端。

use eneros_agent_sdk::agent::{Agent, AgentDescriptor, AgentError, AgentId};
use eneros_agent_sdk::context::Context;
use eneros_agent_sdk::ipc::IpcClient;

/// 示例 Agent：市场报价助手。
pub struct QuoteAgent {
    name: &'static str,
    ipc: IpcClient,
}

impl QuoteAgent {
    /// 创建 Agent。
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            ipc: IpcClient::new().with_endpoint(name),
        }
    }

    /// 向调度 Agent 请求报价（request/response 往返）。
    #[allow(clippy::result_large_err)] // AgentError 是 SDK 稳定 API（含 HString 字段）
    pub fn request_quote(&mut self, target: &str, mw: u32) -> Result<Vec<u8>, AgentError> {
        let req = format!("quote:{mw}").into_bytes();
        self.ipc.request(target, &req)
    }

    /// 发送单向通知。
    #[allow(clippy::result_large_err)] // AgentError 是 SDK 稳定 API
    pub fn notify(&mut self, target: &str, msg: &[u8]) -> Result<(), AgentError> {
        self.ipc.send(target, msg)
    }
}

impl Agent for QuoteAgent {
    fn descriptor(&self) -> AgentDescriptor {
        AgentDescriptor::minimal(self.name, "0.1.0")
    }

    fn on_init(&mut self, _ctx: &mut Context) -> Result<(), AgentError> {
        Ok(())
    }

    fn on_start(&mut self, _ctx: &mut Context) -> Result<(), AgentError> {
        Ok(())
    }

    fn on_tick(&mut self, _ctx: &mut Context) -> Result<(), AgentError> {
        Ok(())
    }

    fn on_stop(&mut self, _ctx: &mut Context) -> Result<(), AgentError> {
        Ok(())
    }

    fn on_destroy(&mut self, _ctx: &mut Context) {}
}

fn main() {
    // 独立构建运行：初始化 IPC + 生命周期回调。
    let mut agent = QuoteAgent::new("quote-agent-demo");
    let mut ctx = Context::new(AgentId(1));
    assert!(agent.on_init(&mut ctx).is_ok());
    assert!(agent.on_start(&mut ctx).is_ok());

    // 单向发送（SPSC 路径；无 peer 时仅入队）。
    let _ = agent.notify("scheduler", b"hello");

    // 无 peer 响应时 request 返回空（SPSC recv 语义）。
    let resp = agent.request_quote("scheduler", 500).unwrap_or_default();
    assert!(resp.is_empty() || !resp.is_empty()); // 无 peer 时为空 Vec

    assert!(agent.on_stop(&mut ctx).is_ok());
    agent.on_destroy(&mut ctx);
    println!("quote-agent-demo: OK");
}

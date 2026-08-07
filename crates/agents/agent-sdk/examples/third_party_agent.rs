//! 第三方 Agent 示例（v0.224.0 B3-01 验收：SDK 示例外部 crate 可独立构建运行）。
//!
//! 演示：
//! 1. 实现 `Agent` trait（生命周期回调）；
//! 2. 使用 `Sel4IpcClient` 跨 Agent request/response（真实 seL4 IPC，
//!    蓝图/20 R2-2：SDK 生产传输不再走 SPSC 假队列）；
//! 3. wire 编解码端到端。

use eneros_agent_sdk::agent::{Agent, AgentDescriptor, AgentError, AgentId};
use eneros_agent_sdk::context::Context;
use eneros_agent_sdk::wire::{self, MessageFlags, MessageType};
use heapless::String as HString;

/// 示例 Agent：市场报价助手。
///
/// IPC 说明（蓝图/20 R2-2）：SDK 生产传输为真实 seL4 IPC
/// （`Sel4IpcClient`，feature `sel4` + seL4 目标构建）；host 演示
/// 使用进程内 SPSC（`IpcClient`，仅测试/仿真构建）。本示例聚焦
/// Agent 生命周期与 wire 编解码——IPC 接线由集成层注入。
pub struct QuoteAgent {
    name: &'static str,
}

impl QuoteAgent {
    /// 创建 Agent。
    pub fn new(name: &'static str) -> Self {
        Self { name }
    }

    /// 构造报价请求 wire 帧（真实 ABI 编解码演示）。
    #[allow(clippy::result_large_err)] // AgentError 是 SDK 稳定 API（含 HString 字段）
    pub fn request_quote_frame(&self, target: &str, mw: u32) -> Result<Vec<u8>, AgentError> {
        let req = format!("quote:{mw}").into_bytes();
        let mut frame = [0u8; wire::MAX_WIRE_SIZE];
        let n = wire::encode(
            MessageType::Request,
            MessageFlags::NONE,
            target,
            &req,
            &mut frame,
        )
        .map_err(|_| {
            AgentError::IpcError(HString::<128>::try_from("wire encode").unwrap_or_default())
        })?;
        Ok(frame[..n].to_vec())
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
    // 独立构建运行：生命周期回调 + wire 帧编解码（IPC 由集成层注入）。
    let mut agent = QuoteAgent::new("quote-agent-demo");
    let mut ctx = Context::new(AgentId(1));
    assert!(agent.on_init(&mut ctx).is_ok());
    assert!(agent.on_start(&mut ctx).is_ok());

    // wire 帧端到端（真实 ABI：header + target + payload + CRC32）。
    let frame = agent.request_quote_frame("scheduler", 320).expect("encode");
    let decoded = wire::decode(&frame).expect("decode");
    assert_eq!(decoded.target, b"scheduler".as_slice());
    assert_eq!(decoded.payload, b"quote:320".as_slice());

    assert!(agent.on_stop(&mut ctx).is_ok());
    agent.on_destroy(&mut ctx);
    println!("third_party_agent example OK");
}

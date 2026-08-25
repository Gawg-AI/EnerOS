//! ARC-001：Agent 收敛链的市场运行时适配——`AgentRuntimeAdapter<T: Agent>`
//! 把任意 `agent_sdk::Agent`（唯一规范根）适配为 [`eneros_agent::AgentRuntime`]。
//!
//! 收敛链：`agent_sdk::Agent`（根）→ `AgentEntryAdapter`（宿主生命周期，
//! eneros-agent）→ `AgentRuntimeAdapter`（市场/设备运行时，本 crate）。

use eneros_agent::AgentDescriptor;
use eneros_agent::{AgentRuntime, HeartbeatStatus};
use eneros_agent_sdk::Agent;

use crate::error::AgentRuntimeError;

/// 市场运行时适配器：把 SDK Agent 适配为 [`AgentRuntime`]。
///
/// `descriptor` 从 SDK 描述符构造宿主描述符（minimal）；`on_start/tick/stop`
/// 映射到 SDK 回调（经 SDK Context，`now_ms` 由调用方传入但 SDK 回调
/// 以 Context 承载）；心跳默认 Alive（SDK Agent 无心跳语义，运行时层
/// 可另行注入）。
pub struct AgentRuntimeAdapter<T: Agent> {
    /// 被适配的 SDK Agent.
    pub inner: T,
    /// 宿主描述符（由 SDK descriptor 构造，惰性更新于 start）。
    descriptor: AgentDescriptor,
    /// 是否已启动.
    started: bool,
}

impl<T: Agent> AgentRuntimeAdapter<T> {
    /// 构造适配器（descriptor 从 SDK 描述符初始化）。
    pub fn new(inner: T, now_ms: u64) -> Self {
        let sd = inner.descriptor();
        let mut descriptor =
            AgentDescriptor::new(eneros_agent::AgentType::Energy, sd.name.as_str(), now_ms);
        descriptor.state = eneros_agent::AgentState::Created;
        Self {
            inner,
            descriptor,
            started: false,
        }
    }

    /// 取回内部 SDK Agent.
    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: Agent> AgentRuntime for AgentRuntimeAdapter<T> {
    fn descriptor(&self) -> &AgentDescriptor {
        &self.descriptor
    }

    fn on_start(&mut self, now_ms: u64) -> Result<(), AgentRuntimeError> {
        let mut ctx = eneros_agent_sdk::Context::new(eneros_agent_sdk::AgentId(
            self.descriptor.agent_id.0 as u64,
        ));
        self.inner
            .on_start(&mut ctx)
            .map_err(|e| AgentRuntimeError::ChannelError(alloc::format!("{:?}", e)))?;
        self.started = true;
        self.descriptor.state = eneros_agent::AgentState::Running;
        let _ = now_ms;
        Ok(())
    }

    fn on_tick(&mut self, _now_ms: u64) -> Result<(), AgentRuntimeError> {
        let mut ctx = eneros_agent_sdk::Context::new(eneros_agent_sdk::AgentId(
            self.descriptor.agent_id.0 as u64,
        ));
        self.inner
            .on_tick(&mut ctx)
            .map_err(|e| AgentRuntimeError::ChannelError(alloc::format!("{:?}", e)))
    }

    fn on_stop(&mut self, _now_ms: u64) -> Result<(), AgentRuntimeError> {
        let mut ctx = eneros_agent_sdk::Context::new(eneros_agent_sdk::AgentId(
            self.descriptor.agent_id.0 as u64,
        ));
        self.inner
            .on_stop(&mut ctx)
            .map_err(|e| AgentRuntimeError::ChannelError(alloc::format!("{:?}", e)))?;
        self.started = false;
        self.descriptor.state = eneros_agent::AgentState::Dead;
        Ok(())
    }

    fn on_heartbeat(&self, _now_ms: u64) -> HeartbeatStatus {
        if self.started {
            HeartbeatStatus::Alive
        } else {
            HeartbeatStatus::Dead
        }
    }
}

#[cfg(test)]
mod tests {
    use eneros_agent_sdk::{AgentDescriptor as SdkDescriptor, AgentError as SdkError, Context};

    use super::*;

    struct MockSdkAgent {
        ticks: u32,
    }

    impl Agent for MockSdkAgent {
        fn descriptor(&self) -> SdkDescriptor {
            SdkDescriptor::minimal("runtime-mock", "1.0.0")
        }
        fn on_init(&mut self, _ctx: &mut Context) -> Result<(), SdkError> {
            Ok(())
        }
        fn on_start(&mut self, _ctx: &mut Context) -> Result<(), SdkError> {
            Ok(())
        }
        fn on_tick(&mut self, _ctx: &mut Context) -> Result<(), SdkError> {
            self.ticks += 1;
            Ok(())
        }
        fn on_stop(&mut self, _ctx: &mut Context) -> Result<(), SdkError> {
            Ok(())
        }
        fn on_destroy(&mut self, _ctx: &mut Context) {}
    }

    #[test]
    fn test_arc001_sdk_agent_adapts_to_runtime() {
        let mut adapter = AgentRuntimeAdapter::new(MockSdkAgent { ticks: 0 }, 1_000);
        assert_eq!(adapter.on_heartbeat(1_000), HeartbeatStatus::Dead);
        adapter.on_start(1_100).unwrap();
        assert_eq!(adapter.on_heartbeat(1_200), HeartbeatStatus::Alive);
        assert_eq!(adapter.inner.ticks, 0);
        adapter.on_tick(1_300).unwrap();
        assert_eq!(adapter.inner.ticks, 1, "SDK on_tick 经运行时适配链调度");
        adapter.on_stop(1_400).unwrap();
        assert_eq!(adapter.on_heartbeat(1_500), HeartbeatStatus::Dead);
        assert_eq!(adapter.descriptor.state, eneros_agent::AgentState::Dead);
    }
}

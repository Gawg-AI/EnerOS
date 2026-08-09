//! ARC-001：Agent trait 收敛链——`agent_sdk::Agent` 为唯一规范根，
//! [`AgentEntryAdapter`] 把任意 SDK Agent 适配为宿主进程内生命周期
//! （`AgentEntry`），形成「Agent（SDK）→ AgentEntry（宿主视图）」适配链。
//!
//! 三套旧 trait（agent::AgentEntry / energy-market-agent::AgentRuntime /
//! agent_sdk::Agent）的语义重叠（init/start/stop/tick）经本链收敛：
//! 新 Agent 只实现 `agent_sdk::Agent`，即自动获得宿主生命周期与市场
//! 运行时（`energy_market_agent::AgentRuntimeAdapter`）两种身份。

use crate::init::{AgentContext, AgentEntry};
use crate::AgentId;

/// 宿主侧生命周期适配器：把 SDK Agent 适配为 [`AgentEntry`]。
///
/// `on_init`/`on_start`/`on_stop` 映射到 SDK 对应回调；SDK `Context`
/// 由宿主上下文字段构造（agent_id 转换，其余取默认）。
pub struct AgentEntryAdapter<T: eneros_agent_sdk::Agent>(pub T);

impl<T: eneros_agent_sdk::Agent> AgentEntry for AgentEntryAdapter<T> {
    fn on_init(&mut self, ctx: &mut AgentContext) -> Result<(), crate::AgentError> {
        let mut sctx = eneros_agent_sdk::Context::new(agent_sdk_id(ctx.agent_id));
        let _ = sctx.agent_id;
        self.0
            .on_init(&mut sctx)
            .map_err(|e| crate::AgentError::InitFailed(sdk_err_string(&e)))
    }

    fn on_start(&mut self, ctx: &mut AgentContext) -> Result<(), crate::AgentError> {
        let mut sctx = eneros_agent_sdk::Context::new(agent_sdk_id(ctx.agent_id));
        self.0
            .on_start(&mut sctx)
            .map_err(|e| crate::AgentError::InitFailed(sdk_err_string(&e)))
    }

    fn on_stop(&mut self, ctx: &mut AgentContext) {
        let mut sctx = eneros_agent_sdk::Context::new(agent_sdk_id(ctx.agent_id));
        let _ = self.0.on_stop(&mut sctx);
    }
}

/// SDK AgentError → 宿主 AgentError（Debug 描述映射）。
fn sdk_err_string(e: &eneros_agent_sdk::AgentError) -> alloc::string::String {
    alloc::format!("{:?}", e)
}

/// 宿主 AgentId（u128）→ SDK AgentId（u64）截断映射。
fn agent_sdk_id(id: AgentId) -> eneros_agent_sdk::AgentId {
    eneros_agent_sdk::AgentId(id.0 as u64)
}

#[cfg(test)]
mod tests {
    use eneros_agent_sdk::{Agent, AgentDescriptor, AgentError as SdkError, Context};

    use super::*;

    struct MockSdkAgent {
        inited: bool,
        started: bool,
        stopped: bool,
    }

    impl Agent for MockSdkAgent {
        fn descriptor(&self) -> AgentDescriptor {
            AgentDescriptor::minimal("mock-sdk", "1.0.0")
        }
        fn on_init(&mut self, _ctx: &mut Context) -> Result<(), SdkError> {
            self.inited = true;
            Ok(())
        }
        fn on_start(&mut self, _ctx: &mut Context) -> Result<(), SdkError> {
            self.started = true;
            Ok(())
        }
        fn on_tick(&mut self, _ctx: &mut Context) -> Result<(), SdkError> {
            Ok(())
        }
        fn on_stop(&mut self, _ctx: &mut Context) -> Result<(), SdkError> {
            self.stopped = true;
            Ok(())
        }
        fn on_destroy(&mut self, _ctx: &mut Context) {}
    }

    #[test]
    fn test_arc001_sdk_agent_adapts_to_agent_entry() {
        let inner = MockSdkAgent {
            inited: false,
            started: false,
            stopped: false,
        };
        let mut adapter = AgentEntryAdapter(inner);
        let mut ctx = AgentContext {
            agent_id: crate::AgentId(7),
            config: Default::default(),
            registry: Default::default(),
        };
        adapter.on_init(&mut ctx).unwrap();
        adapter.on_start(&mut ctx).unwrap();
        assert!(adapter.0.inited, "SDK on_init 经适配链调用");
        assert!(adapter.0.started, "SDK on_start 经适配链调用");
        adapter.on_stop(&mut ctx);
        assert!(adapter.0.stopped, "SDK on_stop 经适配链调用");
    }
}

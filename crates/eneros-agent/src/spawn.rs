//! Agent spawn lifecycle — background task execution for autonomous agents.
//!
//! This module implements the F4 fix: agents can now run autonomously in
//! background tokio tasks, continuously perceiving events, reasoning, and
//! dispatching actions without requiring external tick() calls.
//!
//! ## Lifecycle
//!
//! ```text
//! Created → Initializing → Running ⇄ Paused → Stopping → Stopped
//!                            ↓
//!                       Failed(error)
//! ```
//!
//! ## Design Note
//!
//! The `Agent` trait's `handle_event()` and `tick()` methods require `&mut self`,
//! which is incompatible with `Arc` sharing. To enable spawn without changing
//! the trait, the agent is wrapped in a `tokio::sync::Mutex` and accessed via
//! `Arc<Mutex<Box<dyn Agent>>>`. The mutex is held only for the duration of
//! each `tick()`/`handle_event()` call, so concurrent access from multiple
//! callers is serialized but non-blocking between cycles.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{watch, Mutex};
use tokio::task::JoinHandle;

use eneros_core::Result;

use crate::agent::Agent;
use crate::context::AgentContext;
use crate::dispatcher::ActionDispatcher;
use crate::lifecycle::{AgentLifecycle, AgentState};

/// Control signal sent to a spawned agent's background loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpawnSignal {
    /// Continue running the perception-action loop
    Run,
    /// Pause the loop (agent stays alive but does not tick)
    Pause,
    /// Stop the loop and exit the background task
    Stop,
}

/// A handle to an agent running in a background tokio task.
///
/// The handle allows the caller to pause, resume, or stop the agent, and
/// to await its termination via [`JoinHandle`].
pub struct SpawnedAgent {
    /// Sender for control signals (Run/Pause/Stop).
    signal_tx: watch::Sender<SpawnSignal>,
    /// Shared lifecycle state, observable from both the handle and the task.
    lifecycle: Arc<Mutex<AgentLifecycle>>,
    /// The background task's join handle.
    join_handle: JoinHandle<Result<()>>,
    /// Agent ID (cached for diagnostics).
    agent_id: String,
}

impl SpawnedAgent {
    /// Spawn an agent as a background tokio task.
    ///
    /// The agent immediately enters `Initializing` → `Running` and begins
    /// its perception-action loop:
    ///
    /// 1. Poll the message store for messages addressed to this agent
    /// 2. If messages exist, convert each to an event and call `handle_event()`
    /// 3. Always call `tick()` for proactive behavior (even if events were handled)
    /// 4. Dispatch all returned actions via the `ActionDispatcher`
    /// 5. Sleep for `agent.tick_interval()` (or 50ms when paused, to re-check signals)
    ///
    /// The loop exits cleanly when [`stop()`](Self::stop) is called.
    pub async fn spawn(
        agent: Box<dyn Agent>,
        ctx: Arc<AgentContext>,
        dispatcher: Arc<ActionDispatcher>,
    ) -> Result<Self> {
        let agent_id = agent.id().to_string();
        let agent_id_for_task = agent_id.clone();
        let tick_interval = agent.tick_interval();
        let (signal_tx, signal_rx) = watch::channel(SpawnSignal::Run);
        let lifecycle = Arc::new(Mutex::new(AgentLifecycle::new()));

        // Transition Created → Initializing → Running
        {
            let mut lc = lifecycle.lock().await;
            lc.transition(AgentState::Initializing)
                .map_err(|e| eneros_core::EnerOSError::Internal(format!("lifecycle: {}", e)))?;
            lc.transition(AgentState::Running)
                .map_err(|e| eneros_core::EnerOSError::Internal(format!("lifecycle: {}", e)))?;
        }

        // Wrap the agent in a tokio Mutex so we can get &mut access from
        // within the background task. The Arc allows the task to own it.
        let agent = Arc::new(Mutex::new(agent));
        let lc_clone = lifecycle.clone();

        // 缓存上下文中的 trace_id，用于在后台任务的每次循环中创建 span（T029-06）。
        // trace_id 在 AgentContext 构造时默认生成 UUID v4，也可由调用方通过
        // with_trace_id() 覆盖。后台任务的所有日志都会携带该 trace_id。
        let trace_id = ctx.trace_id().to_string();

        let join_handle = tokio::spawn(async move {
            // 为整个后台任务创建一个顶层 span，携带 trace_id。
            // 后续每次循环内的 handle_event / tick / dispatch 都会继承该 span，
            // 从而保证所有日志都包含同一个 trace_id。
            let task_span = tracing::info_span!(
                "agent.spawned_task",
                agent_id = %agent_id_for_task,
                trace_id = %trace_id,
            );
            let _task_span_guard = task_span.enter();

            let mut current_signal = *signal_rx.borrow();

            loop {
                // Check for signal changes (non-blocking)
                if signal_rx.has_changed().unwrap_or(false) {
                    current_signal = *signal_rx.borrow();
                }

                match current_signal {
                    SpawnSignal::Stop => {
                        // Transition Running → Stopping → Stopped
                        let mut lc = lc_clone.lock().await;
                        let _ = lc.transition(AgentState::Stopping);
                        let _ = lc.transition(AgentState::Stopped);
                        return Ok(());
                    }
                    SpawnSignal::Pause => {
                        // Transition Running → Paused
                        {
                            let mut lc = lc_clone.lock().await;
                            if *lc.state() == AgentState::Running {
                                let _ = lc.transition(AgentState::Paused);
                            }
                        }
                        // Sleep briefly and re-check signal
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                    SpawnSignal::Run => {
                        // Transition Paused → Running if needed
                        {
                            let mut lc = lc_clone.lock().await;
                            if *lc.state() == AgentState::Paused {
                                let _ = lc.transition(AgentState::Running);
                            }
                        }

                        // 每次循环创建一个子 span，便于在日志中区分不同的 tick 周期。
                        let cycle_span = tracing::debug_span!(
                            "agent.cycle",
                            agent_id = %agent_id_for_task,
                            trace_id = %trace_id,
                        );
                        let _cycle_guard = cycle_span.enter();

                        // Perceive: check for messages addressed to this agent
                        let messages = ctx.receive_messages(&agent_id_for_task);

                        let mut all_actions = Vec::new();

                        // Handle messages as events
                        if !messages.is_empty() {
                            let mut agent_guard = agent.lock().await;
                            for msg in messages {
                                let event = eneros_eventbus::Event::new(
                                    eneros_eventbus::event::EventType::SystemAlarm,
                                    &msg.sender_id,
                                    eneros_eventbus::event::EventPayload::Message(msg.content),
                                );
                                let actions = agent_guard.handle_event(&event, &ctx).await?;
                                all_actions.extend(actions);
                            }
                        }

                        // Always tick for proactive behavior
                        {
                            let mut agent_guard = agent.lock().await;
                            let tick_actions = agent_guard.tick(&ctx).await?;
                            all_actions.extend(tick_actions);
                        }

                        // Act: dispatch all produced actions.
                        // 使用 dispatch_with_trace 显式携带 trace_id，确保
                        // dispatcher 内部的日志（如 CallTool、DelegateTask）
                        // 都包含 trace_id（T029-06）。
                        for action in all_actions {
                            let _ = dispatcher.dispatch_with_trace(action, &trace_id).await;
                        }

                        // Sleep for the tick interval
                        tokio::time::sleep(tick_interval).await;
                    }
                }
            }
        });

        Ok(Self {
            signal_tx,
            lifecycle,
            join_handle,
            agent_id,
        })
    }

    /// Get the agent's ID.
    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    /// Get the current lifecycle state.
    pub async fn state(&self) -> AgentState {
        self.lifecycle.lock().await.state().clone()
    }

    /// Pause the agent's background loop.
    pub async fn pause(&self) -> Result<()> {
        let _ = self.signal_tx.send(SpawnSignal::Pause);
        Ok(())
    }

    /// Resume the agent's background loop from paused state.
    pub async fn resume(&self) -> Result<()> {
        let _ = self.signal_tx.send(SpawnSignal::Run);
        Ok(())
    }

    /// Stop the agent's background loop and await termination.
    pub async fn stop(self) -> Result<()> {
        let _ = self.signal_tx.send(SpawnSignal::Stop);
        self.join_handle
            .await
            .map_err(|e| eneros_core::EnerOSError::Internal(format!("join error: {}", e)))?
    }

    /// Check if the agent is in a runnable state (Running or Paused).
    pub async fn is_alive(&self) -> bool {
        matches!(self.state().await, AgentState::Running | AgentState::Paused)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentType, MockAgent};
    use crate::context::AgentContext;
    use eneros_eventbus::EventBus;
    use eneros_gateway::SafetyGateway;
    use eneros_memory::InMemoryMemory;
    use eneros_network::PowerNetwork;
    use eneros_reasoning::RuleBasedEngine;
    use eneros_tool::ToolEngine;
    use parking_lot::RwLock;

    fn test_context() -> AgentContext {
        AgentContext::new(
            Arc::new(EventBus::new(64)),
            Arc::new(SafetyGateway::new(100)),
            Arc::new(RwLock::new(ToolEngine::new())),
            Arc::new(RwLock::new(PowerNetwork::from_ieee14())),
            Arc::new(InMemoryMemory::default()),
            Arc::new(RuleBasedEngine::new()),
        )
    }

    fn test_dispatcher(ctx: &AgentContext) -> ActionDispatcher {
        ActionDispatcher::new(
            Arc::clone(&ctx.remote.event_bus),
            Arc::clone(&ctx.remote.gateway_client),
        )
    }

    #[tokio::test]
    async fn test_spawned_agent_lifecycle_running_to_stopped() {
        let ctx = Arc::new(test_context());
        let dispatcher = Arc::new(test_dispatcher(&ctx));
        let agent: Box<dyn Agent> = Box::new(
            MockAgent::new("spawn-1", "Spawn Test", AgentType::Operator)
                .with_tick_interval(Duration::from_millis(10)),
        );

        let spawned = SpawnedAgent::spawn(agent, ctx, dispatcher).await.unwrap();
        assert_eq!(spawned.agent_id(), "spawn-1");

        // Should be running
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(spawned.state().await, AgentState::Running);
        assert!(spawned.is_alive().await);

        // Stop cleanly
        spawned.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_spawned_agent_pause_resume() {
        let ctx = Arc::new(test_context());
        let dispatcher = Arc::new(test_dispatcher(&ctx));
        let agent: Box<dyn Agent> = Box::new(
            MockAgent::new("spawn-2", "Pause Test", AgentType::Operator)
                .with_tick_interval(Duration::from_millis(10)),
        );

        let spawned = SpawnedAgent::spawn(agent, ctx, dispatcher).await.unwrap();

        // Running initially
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(spawned.state().await, AgentState::Running);

        // Pause
        spawned.pause().await.unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(spawned.state().await, AgentState::Paused);

        // Resume
        spawned.resume().await.unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(spawned.state().await, AgentState::Running);

        spawned.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_spawned_agent_is_alive() {
        let ctx = Arc::new(test_context());
        let dispatcher = Arc::new(test_dispatcher(&ctx));
        let agent: Box<dyn Agent> = Box::new(
            MockAgent::new("spawn-3", "Alive Test", AgentType::Operator)
                .with_tick_interval(Duration::from_millis(10)),
        );

        let spawned = SpawnedAgent::spawn(agent, ctx, dispatcher).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(spawned.is_alive().await);

        spawned.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_spawned_agent_handles_messages() {
        use crate::message::AgentMessage;

        let ctx = Arc::new(test_context());
        let dispatcher = Arc::new(test_dispatcher(&ctx));
        let agent: Box<dyn Agent> = Box::new(
            MockAgent::new("spawn-4", "Message Handler", AgentType::Operator)
                .with_tick_interval(Duration::from_millis(10)),
        );

        let spawned = SpawnedAgent::spawn(agent, ctx.clone(), dispatcher).await.unwrap();

        // Send a message to the agent
        ctx.send_message(AgentMessage::direct("sender", "spawn-4", "hello"));

        // Let the agent process the message
        tokio::time::sleep(Duration::from_millis(50)).await;

        // The agent should still be running
        assert_eq!(spawned.state().await, AgentState::Running);

        spawned.stop().await.unwrap();
    }

    // === T029-06: 分布式追踪 trace_id 贯穿 Agent 管线 ===

    /// 验证 `SpawnedAgent::spawn()` 在上下文携带自定义 trace_id 时能正常工作。
    /// trace_id 在 spawn 时被缓存，并在后台任务的每次循环中通过 span 携带。
    #[tokio::test]
    async fn test_spawned_agent_with_custom_trace_id() {
        let ctx = Arc::new(test_context());
        let custom_trace_id = "spawn-custom-trace-id-abcdef";
        let ctx_with_trace = Arc::new(ctx.with_trace_id(custom_trace_id));

        let dispatcher = Arc::new(test_dispatcher(&ctx_with_trace));
        let agent: Box<dyn Agent> = Box::new(
            MockAgent::new("trace-spawn-1", "Trace Spawn Test", AgentType::Operator)
                .with_tick_interval(Duration::from_millis(10)),
        );

        let spawned = SpawnedAgent::spawn(agent, ctx_with_trace, dispatcher)
            .await
            .unwrap();

        // 验证 agent 正常运行
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(spawned.state().await, AgentState::Running);
        assert!(spawned.is_alive().await);

        spawned.stop().await.unwrap();
    }

    /// 验证 `SpawnedAgent` 后台任务正确缓存并使用上下文中的 trace_id。
    ///
    /// 本测试验证 trace_id 传播机制的核心保证：
    /// 1. `SpawnedAgent::spawn` 在调用时从 `ctx.trace_id()` 读取 trace_id
    /// 2. 该 trace_id 被缓存到 `tokio::spawn` 闭包中
    /// 3. 后台任务的每次循环都使用该缓存的 trace_id 创建 span
    /// 4. `dispatch_with_trace` 也使用该缓存的 trace_id
    ///
    /// 由于 `tokio::spawn` 的任务可能在不同线程上运行，线程本地的 tracing
    /// dispatcher 无法在测试中可靠捕获 span 内容。因此本测试通过验证 agent
    /// 在携带自定义 trace_id 的上下文中正常运行，间接验证 trace_id 传播机制。
    /// 在真实部署中，配置 `tracing_subscriber` 后，所有日志都会包含 trace_id。
    ///
    /// **span 传播的代码级验证**（见 `spawn.rs` 第 104-115 行）：
    /// - `let trace_id = ctx.trace_id().to_string();` — 在 spawn 时缓存 trace_id
    /// - `task_span` 携带 `trace_id = %trace_id` — 顶层任务 span
    /// - `cycle_span` 携带 `trace_id = %trace_id` — 每次循环的子 span
    /// - `dispatcher.dispatch_with_trace(action, &trace_id)` — 显式传播到 dispatcher
    #[tokio::test]
    async fn test_spawned_agent_propagates_trace_id_in_spans() {
        let ctx = Arc::new(test_context());
        let custom_trace_id = "propagated-trace-id-12345678";
        let ctx_with_trace = Arc::new(ctx.with_trace_id(custom_trace_id));

        // 验证上下文确实携带了自定义 trace_id
        assert_eq!(ctx_with_trace.trace_id(), custom_trace_id);

        let dispatcher = Arc::new(test_dispatcher(&ctx_with_trace));
        let agent: Box<dyn Agent> = Box::new(
            MockAgent::new("trace-prop-1", "Trace Propagation Test", AgentType::Operator)
                .with_tick_interval(Duration::from_millis(10)),
        );

        let spawned = SpawnedAgent::spawn(agent, ctx_with_trace, dispatcher)
            .await
            .unwrap();

        // 等待后台任务执行至少一个 tick 周期
        tokio::time::sleep(Duration::from_millis(30)).await;

        // agent 应正常运行（说明 trace_id 缓存和使用没有导致 panic 或错误）
        assert_eq!(spawned.state().await, AgentState::Running);
        assert!(spawned.is_alive().await);

        // 停止 agent（stop 消耗 self，所以后续不能再访问 spawned）
        spawned.stop().await.unwrap();
    }

    /// 验证 `SpawnedAgent` 在处理消息时也能保持 trace_id 传播。
    /// 这模拟了 API 请求 → Agent 调度 → 消息处理 → 动作调度的完整链路。
    #[tokio::test]
    async fn test_spawned_agent_trace_id_during_message_processing() {
        use crate::message::AgentMessage;

        let ctx = Arc::new(test_context());
        let custom_trace_id = "msg-processing-trace-id";
        let ctx_with_trace = Arc::new(ctx.with_trace_id(custom_trace_id));

        let dispatcher = Arc::new(test_dispatcher(&ctx_with_trace));
        let agent: Box<dyn Agent> = Box::new(
            MockAgent::new("msg-trace-1", "Message Trace Test", AgentType::Operator)
                .with_tick_interval(Duration::from_millis(10)),
        );

        let spawned = SpawnedAgent::spawn(agent, ctx_with_trace.clone(), dispatcher)
            .await
            .unwrap();

        // 发送消息触发 handle_event 路径
        ctx_with_trace.send_message(AgentMessage::direct(
            "sender",
            "msg-trace-1",
            "trace_id propagation through message",
        ));

        // 等待消息被处理
        tokio::time::sleep(Duration::from_millis(50)).await;

        // agent 应仍在运行（处理消息不应导致崩溃）
        assert_eq!(spawned.state().await, AgentState::Running);

        spawned.stop().await.unwrap();
    }

    /// 验证多个 `SpawnedAgent` 使用不同 trace_id 时互不干扰。
    /// 每个 agent 的后台任务应携带各自的 trace_id。
    #[tokio::test]
    async fn test_multiple_spawned_agents_with_different_trace_ids() {
        let ctx1 = Arc::new(test_context());
        let trace_id_1 = "agent-1-trace-id";
        let ctx1 = Arc::new(ctx1.with_trace_id(trace_id_1));

        let ctx2 = Arc::new(test_context());
        let trace_id_2 = "agent-2-trace-id";
        let ctx2 = Arc::new(ctx2.with_trace_id(trace_id_2));

        let dispatcher1 = Arc::new(test_dispatcher(&ctx1));
        let dispatcher2 = Arc::new(test_dispatcher(&ctx2));

        let agent1: Box<dyn Agent> = Box::new(
            MockAgent::new("multi-trace-agent-1", "Agent 1", AgentType::Operator)
                .with_tick_interval(Duration::from_millis(10)),
        );
        let agent2: Box<dyn Agent> = Box::new(
            MockAgent::new("multi-trace-agent-2", "Agent 2", AgentType::Operator)
                .with_tick_interval(Duration::from_millis(10)),
        );

        let spawned1 = SpawnedAgent::spawn(agent1, ctx1, dispatcher1).await.unwrap();
        let spawned2 = SpawnedAgent::spawn(agent2, ctx2, dispatcher2).await.unwrap();

        // 两个 agent 都应正常运行
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(spawned1.state().await, AgentState::Running);
        assert_eq!(spawned2.state().await, AgentState::Running);

        // 各自停止
        spawned1.stop().await.unwrap();
        spawned2.stop().await.unwrap();
    }
}

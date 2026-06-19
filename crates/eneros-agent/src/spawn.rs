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

        let join_handle = tokio::spawn(async move {
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

                        // Act: dispatch all produced actions
                        for action in all_actions {
                            let _ = dispatcher.dispatch(action).await;
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
}

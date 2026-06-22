//! Process entry point for Agents that run as independent OS processes.
//!
//! In v0.15.0 the 7 professional Agents migrate from library-level tokio tasks
//! to independent OS processes. Each Agent binary implements the [`AgentProcess`]
//! trait in its `main.rs`, while the domain logic stays in the [`Agent`] trait.
//!
//! Lifecycle management (start/stop) has moved to the `AgentSupervisor` in the
//! AgentOS kernel; the `Agent` trait now only carries domain methods.

use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::signal;
use tokio::sync::Mutex as TokioMutex;

use eneros_core::{
    AuthorityLevel, EventBusPublisher, GatewayClient, Jurisdiction, SystemOperatingState,
};
use eneros_eventbus::{EventBusClient, EventFilter, RemoteEventBusPublisher};
use eneros_gateway::RemoteGatewayClient;
use eneros_memory::InMemoryMemory;
use eneros_network::PowerNetwork;
use eneros_reasoning::RuleBasedEngine;
use eneros_tool::ToolEngine;

use crate::agent::{Agent, AgentType};
use crate::context::{AgentContext, LocalContext, RemoteHandles};
use crate::dispatcher::ActionDispatcher;

/// Configuration for spawning an Agent process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub agent_id: String,
    pub agent_type: AgentType,
    pub authority: AuthorityLevel,
    pub jurisdiction: Jurisdiction,
    pub tick_interval_ms: u64,
    /// TCP address of EventBusBroker, e.g. "127.0.0.1:9876"
    pub eventbus_addr: String,
    /// TCP address of GatewayServer, e.g. "127.0.0.1:9877"
    pub gateway_addr: String,
    /// Unix socket dir for AgentIPC, default "/var/run/eneros"
    ///
    /// Reserved for future IPC-based communication between agent processes
    /// and the AgentOS kernel. Currently unused — agents communicate via
    /// EventBusBroker (TCP) and GatewayServer (TCP).
    pub ipc_socket_dir: String,
}

impl AgentConfig {
    pub fn tick_interval(&self) -> Duration {
        Duration::from_millis(self.tick_interval_ms)
    }
}

/// Maximum consecutive tick/handle_event errors before backing off.
const MAX_CONSECUTIVE_ERRORS: u32 = 10;
/// Backoff duration when consecutive errors reach the limit.
const ERROR_BACKOFF: Duration = Duration::from_secs(5);
/// Initial reconnection backoff.
const INITIAL_RECONNECT_BACKOFF: Duration = Duration::from_secs(1);
/// Maximum reconnection backoff (cap).
const MAX_RECONNECT_BACKOFF: Duration = Duration::from_secs(30);

/// Process entry point trait for Agents that run as independent OS processes.
///
/// Each Agent binary implements this trait in its `main.rs`. The domain logic
/// (event handling, ticking, emergency response) lives in the [`Agent`] trait
/// returned by [`create_agent`](AgentProcess::create_agent); this trait is
/// responsible for wiring up the process-level infrastructure (EventBus
/// connection, Gateway client, IPC sockets) and driving the perceive-act loop.
#[async_trait::async_trait]
pub trait AgentProcess: Send + Sync {
    /// The agent's unique identifier.
    fn agent_id(&self) -> &str;

    /// The agent type for registry classification.
    fn agent_type(&self) -> AgentType;

    /// Create the agent instance with domain logic.
    ///
    /// This is where the 7 professional agents construct their domain-specific
    /// state (generator cost curves, fault patterns, forecast models, etc.).
    async fn create_agent(&self, config: &AgentConfig) -> anyhow::Result<Box<dyn Agent>>;

    /// Main process loop: connect to EventBus+Gateway, run tick loop.
    ///
    /// Default implementation handles the standard perceive-act cycle with
    /// automatic reconnection:
    ///
    /// 1. Create agent instance (domain state survives reconnections)
    /// 2. Connect to EventBusBroker (publisher + subscriber clients)
    /// 3. Connect to GatewayServer via `RemoteGatewayClient`
    /// 4. Build `RemoteHandles` with tool_engine/memory/reasoning initialized
    /// 5. Run tick loop: receive events -> handle_event -> tick -> dispatch
    /// 6. On disconnect: exponential backoff reconnection (1s → 30s cap)
    /// 7. Graceful shutdown on Ctrl+C (even during reconnection backoff)
    async fn run(&self, config: AgentConfig) -> anyhow::Result<()> {
        tracing::info!(
            "Agent {} starting (type: {:?})",
            config.agent_id,
            config.agent_type
        );

        // Create agent ONCE — domain state (last_dispatch, last_forecast, etc.)
        // survives reconnections.
        let mut agent = self.create_agent(&config).await?;
        tracing::info!("Agent {} created, starting tick loop", config.agent_id);

        let tick_interval = config.tick_interval();
        let mut consecutive_errors = 0u32;
        let mut reconnect_backoff = INITIAL_RECONNECT_BACKOFF;

        // Outer reconnection loop: on disconnect, wait and retry.
        loop {
            // Attempt to connect and build context + dispatcher.
            let (ctx, dispatcher, event_receiver) =
                match connect_and_build(&config).await {
                    Ok(x) => {
                        reconnect_backoff = INITIAL_RECONNECT_BACKOFF;
                        x
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Agent {} connect failed: {}. Retrying in {:?}",
                            config.agent_id,
                            e,
                            reconnect_backoff
                        );
                        // Allow Ctrl+C during backoff.
                        tokio::select! {
                            _ = signal::ctrl_c() => {
                                tracing::info!(
                                    "Agent {} received Ctrl+C during connect backoff, shutting down",
                                    config.agent_id
                                );
                                return Ok(());
                            }
                            _ = tokio::time::sleep(reconnect_backoff) => {}
                        }
                        reconnect_backoff = (reconnect_backoff * 2).min(MAX_RECONNECT_BACKOFF);
                        continue;
                    }
                };

            tracing::info!(
                "Agent {} connected to EventBus {} and Gateway {}",
                config.agent_id,
                config.eventbus_addr,
                config.gateway_addr
            );

            // Inner tick loop: runs until disconnect or shutdown.
            let loop_outcome = run_tick_loop(
                &config.agent_id,
                &mut agent,
                &ctx,
                &dispatcher,
                &event_receiver,
                tick_interval,
                &mut consecutive_errors,
            )
            .await;

            match loop_outcome {
                TickLoopOutcome::Shutdown => {
                    tracing::info!("Agent {} stopped", config.agent_id);
                    return Ok(());
                }
                TickLoopOutcome::Disconnected => {
                    tracing::warn!(
                        "Agent {} disconnected from EventBus, reconnecting in {:?}...",
                        config.agent_id,
                        reconnect_backoff
                    );
                    // Allow Ctrl+C during reconnection backoff.
                    tokio::select! {
                        _ = signal::ctrl_c() => {
                            tracing::info!(
                                "Agent {} received Ctrl+C during reconnect backoff, shutting down",
                                config.agent_id
                            );
                            return Ok(());
                        }
                        _ = tokio::time::sleep(reconnect_backoff) => {}
                    }
                    reconnect_backoff = (reconnect_backoff * 2).min(MAX_RECONNECT_BACKOFF);
                }
            }
        }
    }
}

/// Outcome of the inner tick loop.
enum TickLoopOutcome {
    /// Ctrl+C received — agent should shut down.
    Shutdown,
    /// Event receiver closed — EventBusBroker disconnected, should reconnect.
    Disconnected,
}

/// Connect to EventBusBroker + GatewayServer and build the AgentContext +
/// ActionDispatcher + event receiver.
///
/// This function is called on initial startup and on each reconnection.
/// The agent instance is NOT recreated — only the connections and context
/// are rebuilt.
async fn connect_and_build(
    config: &AgentConfig,
) -> anyhow::Result<(
    Arc<AgentContext>,
    Arc<ActionDispatcher>,
    Arc<TokioMutex<Option<tokio::sync::mpsc::Receiver<eneros_eventbus::Event>>>>,
)> {
    // 1. Connect to EventBusBroker — publisher client
    let publisher_client = EventBusClient::connect_tcp(&config.eventbus_addr)
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "connect publisher to eventbus {} failed: {}",
                config.eventbus_addr,
                e
            )
        })?;
    let event_bus: Arc<dyn EventBusPublisher> =
        Arc::new(RemoteEventBusPublisher::new(publisher_client));

    // 2. Connect to EventBusBroker — subscriber client (separate connection)
    let mut subscriber_client = EventBusClient::connect_tcp(&config.eventbus_addr)
        .await
        .map_err(|e| anyhow::anyhow!("connect subscriber to eventbus failed: {}", e))?;
    let event_receiver = subscriber_client
        .subscribe(Some(EventFilter::default()))
        .await
        .map_err(|e| anyhow::anyhow!("subscribe failed: {}", e))?;
    let event_receiver = Arc::new(TokioMutex::new(Some(event_receiver)));

    // 3. Connect to GatewayServer
    let gateway_client: Arc<dyn GatewayClient> =
        Arc::new(RemoteGatewayClient::new(&config.gateway_addr));

    // 4. Build RemoteHandles — initialize tool_engine, memory, and reasoning
    //    so agents have full capabilities when running as independent processes.
    //    Previously these were all None, causing agents like DispatchAgent to
    //    silently degrade (e.g., review_dispatch_with_reasoning returned empty).
    let network = Arc::new(RwLock::new(PowerNetwork::from_ieee14()));
    let system_state = Arc::new(RwLock::new(SystemOperatingState::Normal));
    let audit_trail = Arc::new(RwLock::new(Vec::new()));
    let tool_engine = Arc::new(tokio::sync::RwLock::new(ToolEngine::new()));
    let memory: Arc<dyn eneros_memory::AgentMemory> = Arc::new(InMemoryMemory::default());
    let reasoning: Arc<dyn eneros_reasoning::ReasoningEngine> = Arc::new(RuleBasedEngine::new());

    let remote_handles = RemoteHandles {
        event_bus: event_bus.clone(),
        gateway_client: gateway_client.clone(),
        event_receiver: event_receiver.clone(),
        message_store: None,
        tool_engine: Some(tool_engine),
        network,
        memory: Some(memory),
        reasoning: Some(reasoning),
        constraint_engine: None,
        system_state,
        audit_trail,
    };

    // 5. Build LocalContext
    //    trace_id 默认生成 UUID v4；Agent 进程作为独立 OS 进程运行时，
    //    每个 tick 循环会通过 tracing::Span 携带该 trace_id（T029-06）。
    let local_context = LocalContext {
        agent_id: config.agent_id.clone(),
        authority: config.authority,
        jurisdiction: config.jurisdiction.clone(),
        tick_interval: config.tick_interval(),
        last_seen_message_id: Arc::new(RwLock::new(0)),
        trace_id: uuid::Uuid::new_v4().to_string(),
    };

    // 6. Build AgentContext
    let ctx = Arc::new(AgentContext {
        local: local_context,
        remote: remote_handles,
    });

    // 7. Create ActionDispatcher
    let dispatcher = Arc::new(ActionDispatcher::new(
        event_bus.clone(),
        gateway_client.clone(),
    ));

    Ok((ctx, dispatcher, event_receiver))
}

/// Run the perceive-act tick loop until disconnect or shutdown.
///
/// Returns `TickLoopOutcome::Shutdown` if Ctrl+C was received, or
/// `TickLoopOutcome::Disconnected` if the event receiver was closed
/// (EventBusBroker disconnected).
///
/// 每次处理事件或 tick 时都会创建一个携带 `ctx.trace_id()` 的
/// `tracing::Span`，使所有日志都自动包含 trace_id（T029-06）。
async fn run_tick_loop(
    agent_id: &str,
    agent: &mut Box<dyn Agent>,
    ctx: &Arc<AgentContext>,
    dispatcher: &Arc<ActionDispatcher>,
    event_receiver: &Arc<TokioMutex<Option<tokio::sync::mpsc::Receiver<eneros_eventbus::Event>>>>,
    tick_interval: Duration,
    consecutive_errors: &mut u32,
) -> TickLoopOutcome {
    // 缓存 trace_id，避免每次循环都读取 RwLock。
    let trace_id = ctx.trace_id().to_string();

    loop {
        tokio::select! {
            // Check for shutdown signal
            _ = signal::ctrl_c() => {
                tracing::info!(
                    agent_id = %agent_id,
                    trace_id = %trace_id,
                    "Agent {} received Ctrl+C, shutting down",
                    agent_id
                );
                return TickLoopOutcome::Shutdown;
            }
            // Receive events from EventBusBroker
            event = async {
                let mut receiver = event_receiver.lock().await;
                if let Some(rx) = receiver.as_mut() {
                    rx.recv().await
                } else {
                    None
                }
            } => {
                match event {
                    Some(event) => {
                        *consecutive_errors = 0;
                        // 为本次事件处理创建 span，携带 trace_id（T029-06）。
                        let span = tracing::info_span!(
                            "agent.process_event",
                            agent_id = %agent_id,
                            trace_id = %trace_id,
                            event_type = ?event.event_type,
                        );
                        let _enter = span.enter();

                        match agent.handle_event(&event, ctx).await {
                            Ok(actions) => {
                                for action in actions {
                                    if let Err(e) = dispatcher.dispatch_with_trace(action, &trace_id).await {
                                        tracing::error!("dispatch error: {}", e);
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::error!("handle_event error: {}", e);
                                *consecutive_errors += 1;
                                if *consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                                    tracing::error!(
                                        "Agent {} reached {} consecutive errors, backing off {:?}",
                                        agent_id, *consecutive_errors, ERROR_BACKOFF
                                    );
                                    tokio::time::sleep(ERROR_BACKOFF).await;
                                    *consecutive_errors = 0;
                                }
                            }
                        }
                    }
                    None => {
                        // Event receiver closed — broker disconnected.
                        tracing::warn!(
                            agent_id = %agent_id,
                            trace_id = %trace_id,
                            "Agent {} event receiver closed",
                            agent_id
                        );
                        return TickLoopOutcome::Disconnected;
                    }
                }
            }
            // Tick timer
            _ = tokio::time::sleep(tick_interval) => {
                // 为本次 tick 创建 span，携带 trace_id（T029-06）。
                let span = tracing::info_span!(
                    "agent.tick",
                    agent_id = %agent_id,
                    trace_id = %trace_id,
                );
                let _enter = span.enter();

                match agent.tick(ctx).await {
                    Ok(actions) => {
                        *consecutive_errors = 0;
                        for action in actions {
                            if let Err(e) = dispatcher.dispatch_with_trace(action, &trace_id).await {
                                tracing::error!("dispatch error: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("tick error: {}", e);
                        *consecutive_errors += 1;
                        if *consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                            tracing::error!(
                                "Agent {} reached {} consecutive errors, backing off {:?}",
                                agent_id, *consecutive_errors, ERROR_BACKOFF
                            );
                            tokio::time::sleep(ERROR_BACKOFF).await;
                            *consecutive_errors = 0;
                        }
                    }
                }
            }
        }
    }
}

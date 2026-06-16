use std::sync::Arc;

use clap::{Parser, Subcommand};
use eneros_api::app::AppState;
use eneros_api::server::ApiServer;
use eneros_scada::{SimulatedDataSource, build_ieee14_scada_config, build_ieee14_snapshot_mappings};

// ---------------------------------------------------------------------------
// CLI definitions
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "eneros")]
#[command(about = "EnerOS - Power-Native Agent Operating System")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the EnerOS server (API + Agent orchestrator + SCADA pipeline)
    Run {
        /// Host address
        #[arg(short, long, default_value = "0.0.0.0")]
        host: String,
        /// Port number
        #[arg(short, long, default_value = "8080")]
        port: u16,
        /// Enable SCADA data pipeline
        #[arg(long)]
        with_scada: bool,
        /// Enable Agent orchestrator
        #[arg(long, default_value = "true")]
        with_agents: bool,
    },

    /// Show system status
    Status {
        /// Server address to query
        #[arg(short, long, default_value = "http://localhost:8080")]
        server: String,
    },

    /// Manage agents
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },

    /// Run power system analysis
    Analyze {
        #[command(subcommand)]
        command: AnalyzeCommand,
    },

    /// Run power flow calculation
    PowerFlow {
        /// Network case (ieee14, ieee30, ieee118)
        #[arg(short, long, default_value = "ieee14")]
        case: String,
        /// Maximum iterations
        #[arg(long, default_value = "50")]
        max_iterations: u32,
        /// Convergence tolerance
        #[arg(long, default_value = "1e-6")]
        tolerance: f64,
    },
}

#[derive(Subcommand)]
enum AgentCommand {
    /// List all registered agents
    List {
        /// Server address to query
        #[arg(short, long, default_value = "http://localhost:8080")]
        server: String,
    },
    /// Show details of a specific agent
    Inspect {
        /// Agent name or ID
        name: String,
        /// Server address to query
        #[arg(short, long, default_value = "http://localhost:8080")]
        server: String,
    },
}

#[derive(Subcommand)]
enum AnalyzeCommand {
    /// Run DC optimal power flow
    Opf {
        /// Network case
        #[arg(short, long, default_value = "ieee14")]
        case: String,
    },
    /// Run state estimation
    StateEstimation {
        /// Network case
        #[arg(short, long, default_value = "ieee14")]
        case: String,
    },
    /// Run short circuit analysis
    ShortCircuit {
        /// Fault bus ID
        #[arg(long)]
        bus: u64,
        /// Fault type (3p, slg, ll, dlg)
        #[arg(long, default_value = "3p")]
        fault_type: String,
    },
}

fn load_network(case: &str) -> anyhow::Result<eneros_network::PowerNetwork> {
    match case {
        "ieee14" => Ok(eneros_network::PowerNetwork::from_ieee14()),
        other => Err(anyhow::anyhow!(
            "Unknown network case '{}'. Available: ieee14",
            other
        )),
    }
}

async fn run_server(host: String, port: u16, _with_scada: bool, _with_agents: bool) -> anyhow::Result<()> {
    println!("EnerOS server starting on {}:{}", host, port);

    // 1. Create EventBus (shared across components)
    let event_bus = Arc::new(eneros_eventbus::EventBus::new(64));
    println!("  [Core] EventBus created");

    // 2. Create ConstraintEngine (EventBus wired; Projector added later after PowerNetwork creation)
    let constraint_engine = Arc::new(eneros_constraint::ConstraintEngine::with_event_bus(event_bus.clone()));
    println!("  [Constraint] Engine created (EventBus wired)");

    // 3. Create PowerNetwork from IEEE 14 bus data with shared ConstraintEngine
    let network = Arc::new(
        eneros_network::PowerNetwork::from_ieee14()
            .with_constraint_engine(constraint_engine.clone())
    );
    println!("  [Network] IEEE 14-bus network loaded ({} buses, {} branches)",
        network.bus_count(), network.branch_count());

    // 3. Create TimeSeriesEngine
    let ts_engine = Arc::new(eneros_timeseries::TimeSeriesEngine::new(10000));
    println!("  [TimeSeries] Engine created");

    // 4. Create ScadaCollector with SimulatedDataSource
    let data_source = Arc::new(SimulatedDataSource::new());
    let scada_config = build_ieee14_scada_config();
    let collector = Arc::new(eneros_scada::ScadaCollector::new(scada_config, data_source));
    println!("  [SCADA] Collector created with SimulatedDataSource");

    // 5. Create DataPipeline with collector and ts_engine
    let pipeline = Arc::new(
        eneros_scada::DataPipeline::new(collector.clone(), ts_engine.clone())
            .with_event_bus(event_bus.clone())
    );
    println!("  [SCADA] DataPipeline created");

    // 5b. Create DualScanGroup for fast/normal scan separation
    let dual_scan_group = eneros_scada::DualScanGroup::auto_classify(build_ieee14_scada_config().points);
    println!("  [SCADA] DualScanGroup: {} fast points, {} normal points",
        dual_scan_group.fast_points.len(), dual_scan_group.normal_points.len());

    // 6. Create SnapshotBuilder with IEEE 14 bus mappings
    let snapshot_builder = Arc::new(
        eneros_scada::SnapshotBuilder::new(build_ieee14_snapshot_mappings())
    );
    println!("  [SCADA] SnapshotBuilder created");

    // 7. Create SafetyGateway with PriorityCommandQueue
    let command_queue = Arc::new(eneros_gateway::SharedPriorityCommandQueue::new());
    let gateway = Arc::new(eneros_gateway::SafetyGateway::with_queue(100, command_queue.clone()));
    println!("  [Gateway] SafetyGateway created with PriorityCommandQueue");

    // 7b. Start RealtimeExecutor
    let rt_executor = gateway.start_executor()?;
    println!("  [Gateway] RealtimeExecutor started");

    // 7c. Start WatchdogTimer
    let watchdog = Arc::new(eneros_gateway::WatchdogTimer::new(
        std::time::Duration::from_millis(500)
    ));
    let _watchdog_handle = watchdog.start();
    println!("  [Gateway] WatchdogTimer started (500ms default timeout)");

    // 9. Create AgentOrchestrator with AgentContext
    let tool_engine = Arc::new(parking_lot::RwLock::new(eneros_tool::ToolEngine::new()));
    let network_rw = Arc::new(parking_lot::RwLock::new(eneros_network::PowerNetwork::from_ieee14()));
    let memory = Arc::new(eneros_memory::InMemoryMemory::default());
    let reasoning: Arc<dyn eneros_reasoning::ReasoningEngine> =
        if std::env::var("ENEROS_RIG_PROVIDER").is_ok() {
            #[cfg(feature = "rig")]
            {
                let config = eneros_reasoning::RigConfig::from_env();
                println!("  [Reasoning] Using rig engine: {} / {}", config.provider, config.model);
                let fallback = Arc::new(eneros_reasoning::RuleBasedEngine::new());
                Arc::new(eneros_reasoning::RigReasoningEngine::new(config, network_rw.clone()).with_fallback(fallback))
            }
            #[cfg(not(feature = "rig"))]
            {
                println!("  [Reasoning] rig feature not enabled, falling back to rule-based engine");
                Arc::new(eneros_reasoning::RuleBasedEngine::new())
            }
        } else {
            println!("  [Reasoning] Using rule-based engine (set ENEROS_RIG_PROVIDER to enable AI reasoning)");
            Arc::new(eneros_reasoning::RuleBasedEngine::new())
        };

    // 9b. Create ConstrainedDecisionPipeline (feasibility projection + constraint validation)
    let network_simulator = Arc::new(eneros_network::NetworkSimulatorAdapter::new(network_rw.clone()));
    let projector = Arc::new(eneros_constraint::projector::FeasibilityProjector::new(network_simulator));

    // 9c. Wire Projector into ConstraintEngine (now that it's created)
    constraint_engine.set_projector(projector.clone());
    println!("  [Constraint] Projector wired into ConstraintEngine");

    let pipeline_validator = Arc::new(eneros_gateway::constraint_validator::ConstraintAwareValidator::with_projector(
        constraint_engine.clone(),
        gateway.clone(),
        eneros_gateway::interlocking::InterlockingRuleEngine::new(),
        projector.clone(),
    ));
    let decision_pipeline = Arc::new(eneros_gateway::decision_pipeline::ConstrainedDecisionPipeline::new(
        projector,
        pipeline_validator,
        gateway.clone(),
    ));
    println!("  [Pipeline] ConstrainedDecisionPipeline created (projection + validation + execution)");

    // Phase 14: build the LLM feedback loop from the shared reasoning engine
    // *before* it is moved into the AgentContext. The loop re-prompts the
    // engine when the decision pipeline rejects an action, closing the
    // "LLM → projection → validation → re-reasoning" loop.
    let feedback_loop = Arc::new(eneros_reasoning::feedback::FeedbackLoop::with_default_iterations_shared(
        reasoning.clone(),
    ));
    println!("  [Pipeline] FeedbackLoop created (shared reasoning engine, max 2 retries)");

    let ctx = eneros_agent::AgentContext::new(
        event_bus.clone(),
        gateway.clone(),
        tool_engine,
        network_rw,
        memory,
        reasoning,
    );
    let mut orchestrator = eneros_agent::AgentOrchestrator::with_pipeline_and_feedback(
        ctx,
        decision_pipeline.clone(),
        feedback_loop,
    );
    println!("  [Agent] Orchestrator created with ConstrainedDecisionPipeline + FeedbackLoop");

    // 10. Register all 6 domain agents
    use eneros_agent::event_adapter::AgentEventHandler;
    use eneros_eventbus::event::EventType;

    // DispatchAgent
    let dispatch_agent = eneros_agent::DispatchAgent::new("dispatch-1", "DispatchAgent", vec![1]);
    orchestrator.register_agent(AgentEventHandler::new(
        Box::new(dispatch_agent),
        vec![EventType::ConstraintViolation, EventType::DataReceived],
    ));

    // OperationAgent
    let operation_agent = eneros_agent::OperationAgent::new("operation-1", "OperationAgent", vec![1]);
    orchestrator.register_agent(AgentEventHandler::new(
        Box::new(operation_agent),
        vec![EventType::ConstraintViolation, EventType::SystemAlarm],
    ));

    // SelfHealingAgent
    let self_healing_agent = eneros_agent::SelfHealingAgent::new("self-healing-1", "SelfHealingAgent", vec![1]);
    orchestrator.register_agent(AgentEventHandler::new_all_events(
        Box::new(self_healing_agent),
    ));

    // LoadForecastAgent
    let forecast_agent = eneros_agent::LoadForecastAgent::new(
        "forecast-1",
        eneros_core::Jurisdiction::for_zones(vec![1]),
        ts_engine.clone(),
    );
    orchestrator.register_agent(AgentEventHandler::new(
        Box::new(forecast_agent),
        vec![EventType::ConstraintViolation, EventType::DataReceived],
    ));

    // PlanningAgent
    let planning_agent = eneros_agent::PlanningAgent::new("planning-1", vec![1]);
    orchestrator.register_agent(AgentEventHandler::new(
        Box::new(planning_agent),
        vec![EventType::ConstraintViolation, EventType::DataReceived],
    ));

    // TradingAgent
    let trading_agent = eneros_agent::TradingAgent::new("trading-1", vec![1]);
    orchestrator.register_agent(AgentEventHandler::new(
        Box::new(trading_agent),
        vec![EventType::DataReceived, EventType::ConstraintViolation],
    ));

    println!("  [Agent] Registered 6 domain agents: Dispatch, Operation, SelfHealing, Forecast, Planning, Trading");

    // Wrap orchestrator in Arc for shared access
    let orchestrator = Arc::new(orchestrator);

    // 11. Create DataDrivenAgentLoop
    let state_machine = Arc::new(eneros_agent::SystemStateMachine::new());
    let dd_loop = Arc::new(
        eneros_agent::DataDrivenAgentLoop::new(
            pipeline.clone(),
            collector.clone(),
            snapshot_builder.clone(),
            orchestrator.clone(),
            state_machine,
        )
        .with_constraint_engine(constraint_engine.clone())
    );
    println!("  [Agent] DataDrivenAgentLoop created");

    // 12. Build AppState with all components injected
    let state = AppState::new()
        .with_network(network.clone())
        .with_constraint_engine(constraint_engine.clone())
        .with_ts_engine(ts_engine.clone())
        .with_scada_collector(collector.clone())
        .with_event_bus(event_bus.clone())
        .with_agent_orchestrator(orchestrator.clone())
        .with_data_pipeline(pipeline.clone())
        .with_snapshot_builder(snapshot_builder.clone())
        .with_data_driven_loop(dd_loop.clone())
        .with_decision_pipeline(decision_pipeline);

    // 13. Start SCADA pipeline background task
    let pipeline_handle = pipeline.start(1000);
    println!("  [SCADA] Background pipeline started (1s interval)");

    // 13b. Start DualScanGroup background tasks
    let dual_data_source = Arc::new(SimulatedDataSource::new());
    let dual_scan_handles = eneros_scada::start_dual_scan(
        &dual_scan_group,
        dual_data_source,
        ts_engine.clone(),
    );
    println!("  [SCADA] DualScanGroup started (fast={}ms, normal={}ms)",
        dual_scan_group.fast_interval.as_millis(), dual_scan_group.normal_interval.as_millis());

    // 14. Start DataDrivenAgentLoop background task
    let dd_loop_handle = dd_loop.start(2000);
    println!("  [Agent] DataDrivenAgentLoop started (2s cycle)");

    // 15. Start axum server with populated AppState
    let addr = format!("{}:{}", host, port)
        .parse::<std::net::SocketAddr>()
        .unwrap_or_else(|_| std::net::SocketAddr::from(([0, 0, 0, 0], port)));
    let server = ApiServer::with_state(state, addr);

    println!("EnerOS server running on {}:{}", host, port);
    println!("Press Ctrl+C to stop");

    // 16. Handle Ctrl+C gracefully
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let _ctrlc = tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        let _ = tx.send(());
    });

    // Start server and wait for Ctrl+C
    tokio::select! {
        result = server.start() => {
            result?;
        }
        _ = rx => {
            println!("\nShutting down EnerOS server...");
        }
    }

    // Cleanup background tasks
    pipeline_handle.abort();
    dd_loop_handle.abort();
    dual_scan_handles.abort();
    rt_executor.stop();
    watchdog.stop();

    println!("EnerOS server stopped.");
    Ok(())
}

async fn query_status(server: &str) -> anyhow::Result<()> {
    let client = eneros_api::ApiClient::new(server);

    // Health check first
    match client.health_check().await {
        Ok(true) => {},
        Ok(false) | Err(_) => {
            println!("EnerOS server not reachable at {}", server);
            return Ok(());
        }
    }

    // Fetch constraints and agents
    let constraints = match client.check_constraints().await {
        Ok(c) => c,
        Err(e) => {
            println!("Warning: Failed to fetch constraints: {}", e);
            Vec::new()
        }
    };
    let agents = match client.list_agents().await {
        Ok(a) => a,
        Err(e) => {
            println!("Warning: Failed to fetch agents: {}", e);
            eneros_api::types::AgentsResponse { agent_count: 0, agents: Vec::new() }
        }
    };

    println!("EnerOS System Status");
    println!("--------------------");
    println!("Server:          Running ({})", server);
    println!("Agents:          {} registered", agents.agent_count);
    if constraints.is_empty() {
        println!("Constraints:     No violations");
    } else {
        println!("Constraints:     {} violations", constraints.len());
    }
    println!("System state:    Normal");

    Ok(())
}

async fn agent_list(server: &str) -> anyhow::Result<()> {
    let client = eneros_api::ApiClient::new(server);

    let agents = client.list_agents().await?;

    println!("Registered Agents");
    println!("-----------------");
    println!("{:<20} {:<15} {:<12} {:<10}", "Name", "Type", "Authority", "Status");
    println!("{}", "-".repeat(60));
    for agent in &agents.agents {
        println!("{:<20} {:<15} {:<12} {:<10}", agent.name, agent.agent_type, agent.authority, agent.status);
    }

    Ok(())
}

async fn agent_inspect(name: &str, server: &str) -> anyhow::Result<()> {
    let client = eneros_api::ApiClient::new(server);

    let agents = client.list_agents().await?;
    let agent = agents.agents.iter().find(|a| a.name == name);

    println!("Agent Details");
    println!("-------------");
    match agent {
        Some(a) => {
            println!("Name:       {}", a.name);
            println!("Type:       {}", a.agent_type);
            println!("Authority:  {}", a.authority);
            println!("Status:     {}", a.status);
        }
        None => {
            println!("Agent '{}' not found.", name);
            println!("Registered agents:");
            for a in &agents.agents {
                println!("  - {}", a.name);
            }
        }
    }

    Ok(())
}

fn run_power_flow(case: &str, max_iterations: u32, tolerance: f64) -> anyhow::Result<()> {
    println!("Power Flow Analysis");
    println!("===================");
    println!("Case:            {}", case);
    println!("Max iterations:  {}", max_iterations);
    println!("Tolerance:       {:.e}", tolerance);
    println!();

    let network = load_network(case)?;
    let network = network.with_solver(max_iterations, tolerance);

    println!("Network: {} buses, {} branches", network.bus_count(), network.branch_count());
    println!();

    let result = network.solve()?;

    if result.converged {
        println!("Converged in {} iterations", result.iterations);
    } else {
        println!("Did NOT converge after {} iterations", result.iterations);
    }

    println!();
    println!("Total losses: {:.4} MW", result.total_losses);
    println!();

    // Bus voltage table
    println!("{:<8} {:>12} {:>12} {:>12} {:>12}",
             "Bus", "V (p.u.)", "theta (deg)", "P (MW)", "Q (MVar)");
    println!("{}", "-".repeat(58));
    for bus in &result.bus_results {
        println!("{:<8} {:>12.4} {:>12.4} {:>12.4} {:>12.4}",
                 bus.bus_id,
                 bus.voltage_magnitude,
                 bus.voltage_angle.to_degrees(),
                 bus.p_injection,
                 bus.q_injection);
    }

    Ok(())
}

fn run_opf(case: &str) -> anyhow::Result<()> {
    println!("DC Optimal Power Flow");
    println!("=====================");
    println!("Case: {}", case);
    println!();

    let _network = load_network(case)?;

    // Build OPF problem from IEEE 14-bus data
    let ieee_data = eneros_powerflow::ieee14();
    let problem = build_ieee14_opf_problem(&ieee_data);

    let solver = eneros_analysis::DcOpfSolver::new();
    let result = solver.solve(&problem)?;

    if result.converged {
        println!("OPF converged in {} iterations", result.iterations);
    } else {
        println!("OPF completed with {} warning(s) in {} iterations",
                 result.warnings.len(), result.iterations);
        for w in &result.warnings {
            println!("  Warning: {}", w);
        }
    }

    println!();
    println!("Total generation cost: ${:.2}", result.result.total_cost);
    println!();

    // Generation dispatch table
    println!("Generation Dispatch");
    println!("{:<10} {:>12}", "Gen ID", "P (MW)");
    println!("{}", "-".repeat(24));
    for (gen_id, p_mw) in &result.result.generation {
        println!("{:<10} {:>12.2}", gen_id, p_mw);
    }

    println!();
    // Nodal prices
    println!("Nodal Prices (LMP)");
    println!("{:<10} {:>12}", "Bus", "LMP ($/MWh)");
    println!("{}", "-".repeat(24));
    for (bus_id, price) in &result.result.nodal_prices {
        println!("{:<10} {:>12.2}", bus_id, price);
    }

    Ok(())
}

fn build_ieee14_opf_problem(data: &eneros_powerflow::Ieee14BusData) -> eneros_analysis::DcOpfProblem {
    use eneros_analysis::{GeneratorBid, BranchLimit};

    // IEEE 14-bus generators: buses 1, 2, 3, 6, 8
    let generators = vec![
        GeneratorBid { gen_id: 1, bus_id: 1, p_min: 0.0, p_max: 200.0, cost_a: 0.005, cost_b: 10.0, cost_c: 100.0 },
        GeneratorBid { gen_id: 2, bus_id: 2, p_min: 0.0, p_max: 150.0, cost_a: 0.01, cost_b: 15.0, cost_c: 80.0 },
        GeneratorBid { gen_id: 3, bus_id: 3, p_min: 0.0, p_max: 100.0, cost_a: 0.015, cost_b: 20.0, cost_c: 60.0 },
        GeneratorBid { gen_id: 4, bus_id: 6, p_min: 0.0, p_max: 80.0, cost_a: 0.02, cost_b: 25.0, cost_c: 40.0 },
        GeneratorBid { gen_id: 5, bus_id: 8, p_min: 0.0, p_max: 60.0, cost_a: 0.025, cost_b: 30.0, cost_c: 20.0 },
    ];

    let branches: Vec<BranchLimit> = data.branches.iter().enumerate().map(|(i, br)| {
        BranchLimit {
            branch_id: (i + 1) as u64,
            from_bus: br.from_bus as u64,
            to_bus: br.to_bus as u64,
            p_limit_mw: br.rate_mva,
            reactance_pu: br.x_pu,
        }
    }).collect();

    // Loads: buses with negative P (load buses)
    let loads: Vec<(u64, f64)> = data.buses.iter()
        .filter(|b| b.p_mw < 0.0)
        .map(|b| (b.bus_id as u64, -b.p_mw))
        .collect();

    eneros_analysis::DcOpfProblem {
        generators,
        branches,
        loads,
        slack_bus_id: 1,
    }
}

fn run_state_estimation(case: &str) -> anyhow::Result<()> {
    println!("State Estimation");
    println!("================");
    println!("Case: {}", case);
    println!();

    let network = load_network(case)?;

    // Run power flow first to get "true" values
    let pf_result = network.solve()?;

    // Create synthetic measurements from power flow results
    let measurements: Vec<eneros_analysis::Measurement> = pf_result.bus_results.iter()
        .flat_map(|bus| {
            vec![
                eneros_analysis::Measurement {
                    meas_type: eneros_analysis::MeasType::VoltageMagnitude,
                    element_id: bus.bus_id,
                    to_element_id: None,
                    value: bus.voltage_magnitude,
                    sigma: 0.005,
                },
                eneros_analysis::Measurement {
                    meas_type: eneros_analysis::MeasType::BusInjectionP,
                    element_id: bus.bus_id,
                    to_element_id: None,
                    value: bus.p_injection,
                    sigma: 0.05,
                },
            ]
        })
        .collect();

    let bus_count = pf_result.bus_results.len();
    let estimator = eneros_analysis::StateEstimator::default_estimator();
    let result = estimator.estimate(&measurements, bus_count, 0)?;

    if result.converged {
        println!("State estimation converged in {} iterations", result.iterations);
    } else {
        println!("State estimation did NOT converge after {} iterations", result.iterations);
    }

    println!();

    // Estimated voltages
    println!("Estimated Bus Voltages");
    println!("{:<8} {:>12} {:>12}", "Bus", "V (p.u.)", "theta (deg)");
    println!("{}", "-".repeat(36));
    for (bus_id, v, theta) in &result.result.bus_voltages {
        println!("{:<8} {:>12.4} {:>12.4}", bus_id, v, theta.to_degrees());
    }

    // Bad data detection
    if result.result.bad_data.is_empty() {
        println!();
        println!("No bad data detected.");
    } else {
        println!();
        println!("Bad data detected at {} measurement(s):", result.result.bad_data.len());
        for id in &result.result.bad_data {
            println!("  Element ID: {}", id);
        }
    }

    Ok(())
}

fn run_short_circuit(bus: u64, fault_type_str: &str) -> anyhow::Result<()> {
    println!("Short Circuit Analysis");
    println!("======================");
    println!("Fault bus:    {}", bus);
    println!("Fault type:   {}", fault_type_str);
    println!();

    let fault_type = match fault_type_str {
        "3p" => eneros_analysis::FaultType::ThreePhase,
        "slg" => eneros_analysis::FaultType::SingleLineGround,
        "ll" => eneros_analysis::FaultType::LineLine,
        "dlg" => eneros_analysis::FaultType::DoubleLineGround,
        other => return Err(anyhow::anyhow!(
            "Unknown fault type '{}'. Available: 3p, slg, ll, dlg", other
        )),
    };

    let network = eneros_network::PowerNetwork::from_ieee14();

    // Run power flow for pre-fault voltages
    let pf_result = network.solve()?;

    // Build Z-bus from Y-bus (invert Y-bus matrix)
    let ybus = network.ybus();
    let n = ybus.size();
    let z_bus = invert_ybus_to_zbus(ybus, n);

    // Pre-fault voltages as complex numbers
    let prefault_voltages: Vec<num_complex::Complex64> = pf_result.bus_results.iter()
        .map(|b| num_complex::Complex64::from_polar(b.voltage_magnitude, b.voltage_angle))
        .collect();

    let fault = eneros_analysis::FaultSpec {
        bus_id: bus,
        fault_type,
        fault_impedance: num_complex::Complex64::new(0.0, 0.0),
    };

    // For asymmetric faults, provide sequence impedances
    let seq_z = eneros_analysis::SequenceImpedance {
        z1: num_complex::Complex64::new(0.01, 0.1),
        z2: num_complex::Complex64::new(0.01, 0.1),
        z0: num_complex::Complex64::new(0.03, 0.3),
    };

    let analyzer = eneros_analysis::ShortCircuitAnalyzer::new();
    let result = analyzer.analyze(&fault, &z_bus, &prefault_voltages, Some(&seq_z))?;

    println!("Fault Current: {:.4} angle {:.2} deg kA",
             result.fault_current_ka.norm(),
             result.fault_current_ka.arg().to_degrees());
    println!();

    // Bus voltages during fault
    println!("Bus Voltages During Fault");
    println!("{:<8} {:>12} {:>12}", "Bus", "|V| (p.u.)", "angle (deg)");
    println!("{}", "-".repeat(36));
    for (bus_id, v) in &result.bus_voltages {
        println!("{:<8} {:>12.4} {:>12.2}", bus_id, v.norm(), v.arg().to_degrees());
    }

    Ok(())
}

/// Invert the Y-Bus admittance matrix to get the Z-Bus impedance matrix
#[allow(clippy::needless_range_loop)]
fn invert_ybus_to_zbus(ybus: &eneros_powerflow::YBusMatrix, n: usize) -> ndarray::Array2<num_complex::Complex64> {
    // Build complex Y-Bus matrix as Vec<Vec<Complex64>>
    let mut y_matrix = vec![vec![num_complex::Complex64::new(0.0, 0.0); n]; n];
    for i in 0..n {
        for j in 0..n {
            let (g, b) = ybus.get(i, j);
            y_matrix[i][j] = num_complex::Complex64::new(g, b);
        }
    }

    // Invert using shared linalg utility
    eneros_core::invert_complex_matrix(&y_matrix)
        .map(|inv| {
            let mut result = ndarray::Array2::<num_complex::Complex64>::zeros((n, n));
            for i in 0..n {
                for j in 0..n {
                    result[[i, j]] = inv[i][j];
                }
            }
            result
        })
        .unwrap_or_else(|| ndarray::Array2::from_elem((n, n), num_complex::Complex64::new(0.0, 0.0)))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Run { host, port, with_scada, with_agents } => {
            run_server(host, port, with_scada, with_agents).await?;
        }
        Commands::Status { server } => {
            query_status(&server).await?;
        }
        Commands::Agent { command } => {
            match command {
                AgentCommand::List { server } => {
                    agent_list(&server).await?;
                }
                AgentCommand::Inspect { name, server } => {
                    agent_inspect(&name, &server).await?;
                }
            }
        }
        Commands::Analyze { command } => {
            match command {
                AnalyzeCommand::Opf { case } => {
                    run_opf(&case)?;
                }
                AnalyzeCommand::StateEstimation { case } => {
                    run_state_estimation(&case)?;
                }
                AnalyzeCommand::ShortCircuit { bus, fault_type } => {
                    run_short_circuit(bus, &fault_type)?;
                }
            }
        }
        Commands::PowerFlow { case, max_iterations, tolerance } => {
            run_power_flow(&case, max_iterations, tolerance)?;
        }
    }

    Ok(())
}

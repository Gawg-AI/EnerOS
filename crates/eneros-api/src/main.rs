use std::sync::Arc;

use clap::{Parser, Subcommand};
use eneros_api::app::AppState;
use eneros_api::server::ApiServer;
use eneros_core::config::EnerOSConfig;
use eneros_runtime::scada::{
    build_ieee14_ioa_mapping, build_ieee14_scada_config, build_ieee14_snapshot_mappings,
    Iec104DataSource, SimulatedDataSource,
};

// tracing-subscriber prelude 提供 SubscriberExt::with() 等扩展 trait
use tracing_subscriber::prelude::*;

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
        #[arg(long, default_value = "0.0.0.0")]
        host: String,
        /// Port number
        #[arg(short, long, default_value = "8080")]
        port: u16,
        /// Path to the configuration file (default: ./eneros.toml)
        #[arg(short, long)]
        config: Option<String>,
        /// Enable SCADA data pipeline
        #[arg(long)]
        with_scada: bool,
        /// Enable Agent orchestrator
        #[arg(long, default_value = "true")]
        with_agents: bool,
        /// Enable structured JSON logging (v0.7.0 — deferred from v0.6.0 S3)
        #[arg(long)]
        json_log: bool,
        /// Path to TLS certificate file (PEM). When set, enables TLS (v0.7.0).
        #[arg(long)]
        tls_cert: Option<String>,
        /// Path to TLS private key file (PEM). When set, enables TLS (v0.7.0).
        #[arg(long)]
        tls_key: Option<String>,
        /// OTLP gRPC endpoint for OpenTelemetry trace export (v0.29.0 — T029-18)
        /// Overrides config [observability] otel_endpoint and env OTEL_EXPORTER_OTLP_ENDPOINT
        /// Example: --otel-endpoint http://localhost:4317
        #[arg(long)]
        otel_endpoint: Option<String>,
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

fn load_network(case: &str) -> anyhow::Result<eneros_runtime::network::PowerNetwork> {
    match case {
        "ieee14" => Ok(eneros_runtime::network::PowerNetwork::from_ieee14()),
        other => Err(anyhow::anyhow!(
            "Unknown network case '{}'. Available: ieee14",
            other
        )),
    }
}

/// Build a `PowerNetwork` from the configured source.
///
/// Supported sources:
/// - `ieee14` — built-in IEEE 14-bus test case (default)
/// - `cnpower` — load from cnpower equipment database via `eneros-bridge`
///   (requires the Python bridge server to be running)
/// - `cim` — load from CIM/CGMES profile (future, not yet implemented)
fn build_network_from_config(
    network_cfg: &eneros_core::config::NetworkConfig,
    constraint_engine: Arc<eneros_runtime::constraint::ConstraintEngine>,
) -> anyhow::Result<eneros_runtime::network::PowerNetwork> {
    let network = match network_cfg.source.as_str() {
        "ieee14" => {
            println!("  [Network] Loading IEEE 14-bus test case");
            eneros_runtime::network::PowerNetwork::from_ieee14()
        }
        "cnpower" => {
            println!("  [Network] Loading from cnpower equipment database");
            // The cnpower loader requires a running Python bridge server.
            // We construct the loader and attempt to build the full network;
            // if the bridge is unavailable, we fall back to IEEE 14 with a
            // warning so the server still starts.
            match build_cnpower_network(network_cfg) {
                Ok(n) => n,
                Err(e) => {
                    println!(
                        "  [Network] WARNING: cnpower load failed ({}); falling back to IEEE 14",
                        e
                    );
                    eneros_runtime::network::PowerNetwork::from_ieee14()
                }
            }
        }
        "cim" => {
            println!("  [Network] Loading from CIM/CGMES profile");
            match build_cim_network(network_cfg) {
                Ok(n) => n,
                Err(e) => {
                    println!(
                        "  [Network] WARNING: CIM load failed ({}); falling back to IEEE 14",
                        e
                    );
                    eneros_runtime::network::PowerNetwork::from_ieee14()
                }
            }
        }
        other => {
            println!(
                "  [Network] Unknown source '{}'; falling back to IEEE 14",
                other
            );
            eneros_runtime::network::PowerNetwork::from_ieee14()
        }
    };

    let network = network.with_constraint_engine(constraint_engine);
    println!(
        "  [Network] Loaded ({} buses, {} branches)",
        network.bus_count(),
        network.branch_count()
    );

    Ok(network)
}

/// Build a `PowerNetwork` from the cnpower equipment database via the
/// `eneros-bridge` Python bridge.
///
/// This calls the bridge's `build_full_network` command, which returns a
/// `NetworkTopologyData` (buses, branches, shunts, power-flow results). We
/// then convert that topology into a `PowerNetwork` by:
///
/// 1. Building a sorted bus ID → index map.
/// 2. Converting each `TopologyBranch` to per-unit (r, x, b, tap) using the
///    branch's voltage base and the system `base_mva`.
/// 3. Constructing the Y-Bus matrix from branches, then adding shunt
///    admittances.
/// 4. Assembling `p_spec`, `q_spec`, `bus_types`, and `v_initial` from bus
///    data.
/// 5. Building a `GeneratorSpec` table from buses with non-zero generation.
fn build_cnpower_network(
    network_cfg: &eneros_core::config::NetworkConfig,
) -> anyhow::Result<eneros_runtime::network::PowerNetwork> {
    use eneros_runtime::bridge::equipment_bridge::CnpowerEquipmentLoader;
    use eneros_runtime::bridge::topology_types::NetworkTopologyData;

    let mut loader = CnpowerEquipmentLoader::new();
    loader
        .start_server()
        .map_err(|e| anyhow::anyhow!("bridge start failed: {}", e))?;

    let assets = if let Some(ref path) = network_cfg.path {
        serde_json::from_str(&std::fs::read_to_string(path)?)
            .unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    let topo: NetworkTopologyData = loader
        .build_full_network(assets)
        .map_err(|e| anyhow::anyhow!("build_full_network failed: {}", e))?;

    println!(
        "  [Network] cnpower returned {} buses, {} branches, {} shunts (converged={})",
        topo.bus_count, topo.branch_count, topo.shunts.len(), topo.converged
    );

    topology_data_to_power_network(&topo)
}

/// Convert `NetworkTopologyData` (from cnpower/pandapower bridge) into a
/// `PowerNetwork` suitable for power-flow and What-If analysis.
///
/// This is the core topology → PowerNetwork conversion. It handles:
/// - Lines: r/x/b in physical units (Ohm/km, nF/km) → per-unit
/// - Transformers: vk_percent/vkr_percent → per-unit r/x on system base
/// - Shunts: p_mw/q_mvar → per-unit admittance
/// - Bus types: Slack/PV/PQ → corresponding `BusType`
/// - Generators: buses with p_gen_mw > 0 become `GeneratorSpec` entries
fn topology_data_to_power_network(
    topo: &eneros_runtime::bridge::topology_types::NetworkTopologyData,
) -> anyhow::Result<eneros_runtime::network::PowerNetwork> {
    use eneros_core::ElementId;
    use eneros_powerflow::{BusTypeNR, YBusMatrix};
    use std::collections::HashMap;

    let base_mva = if topo.base_mva > 0.0 {
        topo.base_mva
    } else {
        100.0
    };

    if topo.buses.is_empty() {
        return Err(anyhow::anyhow!("topology has no buses"));
    }

    // ── 1. Build bus ID → index map (sorted by bus ID) ────────────────────
    let mut bus_ids: Vec<i64> = topo.buses.iter().map(|b| b.id).collect();
    bus_ids.sort();
    bus_ids.dedup();
    let bus_map: HashMap<ElementId, usize> = bus_ids
        .iter()
        .enumerate()
        .map(|(idx, &id)| (id as ElementId, idx))
        .collect();
    let n = bus_ids.len();

    // Build a quick lookup: bus_id → TopologyBus
    let bus_lookup: HashMap<i64, &eneros_runtime::bridge::topology_types::TopologyBus> =
        topo.buses.iter().map(|b| (b.id, b)).collect();

    // ── 2. Convert branches to per-unit (from, to, r, x, b, tap) ──────────
    let branches: Vec<(ElementId, ElementId, f64, f64, f64, f64)> = topo
        .branches
        .iter()
        .filter(|b| b.in_service)
        .map(|br| {
            let from = br.from_bus as ElementId;
            let to = br.to_bus as ElementId;

            // Determine voltage base (kV) from the from-bus
            let vn_kv = bus_lookup
                .get(&br.from_bus)
                .map(|b| b.vn_kv)
                .unwrap_or(110.0);
            let z_base = vn_kv * vn_kv / base_mva; // Ohms

            let (r_pu, x_pu, b_pu, tap) = if br.branch_type == "line" {
                let length = br.length_km.unwrap_or(1.0);
                let r_ohm = br.r_ohm_per_km.unwrap_or(0.0) * length;
                let x_ohm = br.x_ohm_per_km.unwrap_or(0.0) * length;
                let c_nf = br.c_nf_per_km.unwrap_or(0.0) * length;
                // Charging susceptance in Siemens: 2*pi*f*C (f=50Hz)
                let b_siemens = 2.0 * std::f64::consts::PI * 50.0 * c_nf * 1e-9;
                let r_pu = r_ohm / z_base;
                let x_pu = x_ohm / z_base;
                let b_pu = b_siemens * z_base;
                (r_pu, x_pu, b_pu, 1.0)
            } else {
                // Transformer: vk_percent / vkr_percent → r, x on system base
                let sn_mva = br.sn_mva.unwrap_or(base_mva);
                let vk = br.vk_percent.unwrap_or(10.0) / 100.0;
                let vkr = br.vkr_percent.unwrap_or(0.5) / 100.0;
                let r_pu = vkr * (base_mva / sn_mva);
                let z_pu = vk * (base_mva / sn_mva);
                let x_pu = (z_pu * z_pu - r_pu * r_pu).max(0.0).sqrt();
                // Tap ratio: if tap_pos is present, assume tap_step = 1.25%
                let tap = br
                    .tap_pos
                    .map(|pos| 1.0 + 0.0125 * pos as f64)
                    .unwrap_or(1.0);
                (r_pu, x_pu, 0.0, tap)
            };

            (from, to, r_pu, x_pu, b_pu, tap)
        })
        .collect();

    // ── 3. Build Y-Bus from branches, then add shunt admittances ──────────
    let mut ybus = YBusMatrix::from_branches(&branches, &bus_map);

    for shunt in &topo.shunts {
        if let Some(&idx) = bus_map.get(&(shunt.bus as ElementId)) {
            // Shunt power → per-unit admittance (V=1.0 pu):
            //   g = P / base_mva, b = Q / base_mva
            let g_pu = shunt.p_mw / base_mva;
            let b_pu = shunt.q_mvar / base_mva;
            ybus.add_shunt(idx, g_pu, b_pu);
        }
    }

    // ── 4. Build p_spec, q_spec, bus_types, v_initial ─────────────────────
    let mut p_spec = vec![0.0; n];
    let mut q_spec = vec![0.0; n];
    let mut bus_types = vec![BusTypeNR::PQ; n];
    let mut v_initial = vec![1.0; n];

    for (&bus_id, &idx) in &bus_map {
        if let Some(bus) = bus_lookup.get(&(bus_id as i64)) {
            let p_gen = bus.p_gen_mw.unwrap_or(0.0);
            let q_gen = bus.q_gen_mvar.unwrap_or(0.0);
            let p_load = bus.p_load_mw.unwrap_or(0.0);
            let q_load = bus.q_load_mvar.unwrap_or(0.0);
            p_spec[idx] = (p_gen - p_load) / base_mva;
            q_spec[idx] = (q_gen - q_load) / base_mva;
            bus_types[idx] = match bus.bus_type.as_str() {
                "Slack" | "slack" | "SLACK" => BusTypeNR::Slack,
                "PV" | "pv" => BusTypeNR::PV,
                _ => BusTypeNR::PQ,
            };
            v_initial[idx] = bus.vm_pu.unwrap_or(1.0);
        }
    }

    // ── 5. Build generator table from buses with generation ───────────────
    let generators: Vec<eneros_runtime::network::GeneratorSpec> = topo
        .buses
        .iter()
        .filter(|b| b.p_gen_mw.unwrap_or(0.0).abs() > 1e-6 || b.bus_type == "Slack")
        .enumerate()
        .map(|(gen_idx, bus)| eneros_runtime::network::GeneratorSpec {
            gen_id: (gen_idx + 1) as ElementId,
            bus_id: bus.id as ElementId,
            p_min_mw: 0.0,
            p_max_mw: bus.p_gen_mw.unwrap_or(0.0).abs() * 2.0 + 10.0,
            p_gen_mw: bus.p_gen_mw.unwrap_or(0.0),
            p_load_mw: bus.p_load_mw.unwrap_or(0.0),
        })
        .collect();

    // ── 6. Build zone map (single default zone with all buses) ────────────
    let all_buses: Vec<ElementId> = bus_ids.iter().map(|&id| id as ElementId).collect();
    let mut zone_map = HashMap::new();
    zone_map.insert(0u32, all_buses);

    let branch_ids: Vec<ElementId> = (1..=branches.len() as ElementId).collect();

    let network = eneros_runtime::network::PowerNetwork::new(
        ybus,
        p_spec,
        q_spec,
        bus_types,
        branches,
        bus_map,
    )
    .with_initial_voltages(v_initial);

    // The PowerNetwork::new() constructor doesn't expose generators/zone_map
    // setters, so we use the public API and accept that loaded networks have
    // empty generator tables. The power-flow solver still works because it
    // uses p_spec/q_spec directly. For What-If agent actions targeting
    // generators, users should use ieee14 (which has full GeneratorSpec data)
    // or extend the config to provide generator limits.
    let _ = (generators, zone_map, branch_ids); // suppress unused warnings

    Ok(network)
}

/// Build a `PowerNetwork` from a CIM/CGMES XML file.
///
/// CIM (Common Information Model, IEC 61968/61970) is the standard data
/// exchange format for power system models. This loader:
///
/// 1. Reads the XML file from `network_cfg.path`.
/// 2. Parses it with `eneros_runtime::network::parse_cim()` into a `CimModel`.
/// 3. Converts to `PowerNetwork` via `eneros_runtime::network::cim_to_power_network()`.
fn build_cim_network(
    network_cfg: &eneros_core::config::NetworkConfig,
) -> anyhow::Result<eneros_runtime::network::PowerNetwork> {
    let path = network_cfg
        .path
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("CIM loader requires network.path to be set"))?;

    println!("  [Network] Parsing CIM file: {}", path);
    let xml = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read CIM file '{}': {}", path, e))?;
    let model = eneros_runtime::network::parse_cim(&xml)
        .map_err(|e| anyhow::anyhow!("CIM parse error: {}", e))?;

    println!(
        "  [Network] CIM parsed: {} busbar sections, {} lines, {} transformers, {} generators, {} loads",
        model.busbar_sections.len(),
        model.ac_line_segments.len(),
        model.power_transformers.len(),
        model.synchronous_machines.len(),
        model.energy_consumers.len(),
    );

    let network = eneros_runtime::network::cim_to_power_network(&model)
        .map_err(|e| anyhow::anyhow!("CIM → PowerNetwork conversion error: {}", e))?;

    println!(
        "  [Network] CIM converted: {} buses, {} branches",
        network.bus_count(),
        network.branch_count(),
    );

    Ok(network)
}

/// Build the SCADA data source from configuration.
///
/// - `simulated` — `SimulatedDataSource` with IEEE 14 data (default)
/// - `iec104` — `Iec104DataSource` connected to a real IEC 104 server
fn build_data_source_from_config(
    scada_cfg: &eneros_core::config::ScadaSourceConfig,
) -> Arc<dyn eneros_runtime::scada::DataSource> {
    match scada_cfg.source.as_str() {
        "iec104" => {
            println!(
                "  [SCADA] Using IEC 104 data source (server={}, asdu={})",
                scada_cfg.iec104_addr, scada_cfg.iec104_asdu
            );
            use eneros_runtime::device::adapters::iec104::client::Iec104Config;
            let iec_config = Iec104Config {
                remote_addr: scada_cfg.iec104_addr.clone(),
                asdu_address: scada_cfg.iec104_asdu,
                ..Default::default()
            };
            let client = Arc::new(eneros_runtime::device::adapters::iec104::client::Iec104Client::new(
                iec_config,
            ));
            let mapping = build_ieee14_ioa_mapping();
            Arc::new(Iec104DataSource::new(client, mapping))
        }
        _ => {
            println!("  [SCADA] Using SimulatedDataSource (IEEE 14 data)");
            Arc::new(SimulatedDataSource::new())
        }
    }
}

/// Build a `DeviceManager` from the configured device connections.
///
/// Each `[[devices]]` entry in `eneros.toml` becomes a registered device
/// with the appropriate protocol adapter. Devices that fail to connect are
/// logged but do not prevent the server from starting — the gateway falls
/// back to `LoggingExecutor` for commands targeting unregistered devices.
async fn build_device_manager(
    devices: &[eneros_core::config::DeviceConnectionConfig],
) -> Arc<eneros_runtime::device::DeviceManager> {
    use eneros_runtime::device::adapter::{ConnectionConfig, DeviceInfo, ProtocolConfig};
    use eneros_runtime::device::protocol::ProtocolType;

    let dm = Arc::new(eneros_runtime::device::DeviceManager::new());

    for dev in devices {
        let (adapter, protocol_type, protocol_config) = match dev.protocol.as_str() {
            "iec104" => {
                use eneros_runtime::device::adapters::iec104::Iec104Adapter;
                let common_address = dev
                    .params
                    .get("asdu_address")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(1) as u16;
                let adapter = Box::new(Iec104Adapter::new(&dev.device_id));
                (
                    adapter as Box<dyn eneros_runtime::device::adapter::ProtocolAdapter>,
                    ProtocolType::Iec104,
                    ProtocolConfig::Iec104 {
                        common_address,
                        ioa_size: 3,
                    },
                )
            }
            "modbus" => {
                use eneros_runtime::device::adapters::ModbusTcpAdapter;
                let slave_id = dev
                    .params
                    .get("slave_id")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(1) as u8;
                let baud_rate = dev
                    .params
                    .get("baud_rate")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32);
                let adapter = Box::new(ModbusTcpAdapter::new(&dev.device_id));
                (
                    adapter as Box<dyn eneros_runtime::device::adapter::ProtocolAdapter>,
                    ProtocolType::Modbus,
                    ProtocolConfig::Modbus {
                        slave_id,
                        baud_rate,
                    },
                )
            }
            "iec61850" => {
                use eneros_runtime::device::adapters::Iec61850Adapter;
                let adapter = Box::new(Iec61850Adapter::new(&dev.device_id));
                let logical_devices = dev
                    .params
                    .get("logical_devices")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                (
                    adapter as Box<dyn eneros_runtime::device::adapter::ProtocolAdapter>,
                    ProtocolType::Iec61850,
                    ProtocolConfig::Iec61850 { logical_devices },
                )
            }
            "mqtt" => {
                use eneros_runtime::device::adapters::MqttAdapter;
                let adapter = Box::new(MqttAdapter::new(&dev.device_id));
                (
                    adapter as Box<dyn eneros_runtime::device::adapter::ProtocolAdapter>,
                    ProtocolType::Mqtt,
                    ProtocolConfig::Mqtt {
                        client_id: dev.device_id.clone(),
                        topics: Vec::new(),
                    },
                )
            }
            other => {
                println!(
                    "  [Device] WARNING: unknown protocol '{}' for device '{}'; skipping",
                    other, dev.device_id
                );
                continue;
            }
        };

        let config = ConnectionConfig {
            host: dev.host.clone(),
            port: dev.port,
            timeout_ms: 5000,
            credentials: None,
            protocol_config,
        };
        let info = DeviceInfo {
            device_id: dev.device_id.clone(),
            name: dev.device_id.clone(),
            protocol: protocol_type,
            manufacturer: "unknown".into(),
            model: "unknown".into(),
            firmware_version: "0.0.0".into(),
            ip_address: dev.host.clone(),
            port: dev.port,
            capabilities: vec!["read".into(), "write".into()],
        };

        dm.register_device(&dev.device_id, adapter, config, info).await;
        println!(
            "  [Device] Registered '{}' ({}@{}:{})",
            dev.device_id, dev.protocol, dev.host, dev.port
        );

        // Attempt to connect; failures are non-fatal.
        match dm.connect(&dev.device_id).await {
            Ok(()) => println!("  [Device] Connected '{}'", dev.device_id),
            Err(e) => println!(
                "  [Device] WARNING: '{}' connect failed ({}); commands will fail until connected",
                dev.device_id, e
            ),
        }
    }

    dm
}

/// Build the command executor for the SafetyGateway.
///
/// If at least one device is configured and connected, returns a
/// `DeviceCommandExecutor` backed by the `DeviceManager`. Otherwise returns
/// a `LoggingExecutor` so the server still runs in simulation mode.
fn build_command_executor(
    dm: &Arc<eneros_runtime::device::DeviceManager>,
    devices_configured: usize,
) -> Arc<dyn eneros_runtime::gateway::CommandExecutor> {
    if devices_configured > 0 {
        println!(
            "  [Gateway] Using DeviceCommandExecutor ({} device(s) configured)",
            devices_configured
        );
        Arc::new(eneros_runtime::gateway::DeviceCommandExecutor::new(dm.clone()))
    } else {
        println!("  [Gateway] Using LoggingExecutor (no devices configured)");
        Arc::new(eneros_runtime::gateway::LoggingExecutor)
    }
}

/// Compute the in-memory retention capacity for the TimeSeriesEngine from the
/// `[timeseries]` config (Task 3: 时序配置接线).
///
/// Formula: `retention_days * 86400 * 1000 / sampling_interval_ms`, clamped to
/// a 10M-point upper bound to protect memory. Minimums are enforced: at least
/// 1 day and 100ms sampling interval, so a misconfigured `eneros.toml` cannot
/// produce a zero/nonsensical capacity.
fn compute_retention_capacity(retention_days: u32, sampling_interval_ms: u64) -> usize {
    let sampling_interval_ms = sampling_interval_ms.max(100); // 最小 100ms
    let retention_days = retention_days.max(1); // 最小 1 天
    (retention_days as usize * 86400 * 1000 / sampling_interval_ms as usize).min(10_000_000) // 上限 1000 万点
}

#[allow(clippy::too_many_arguments)]
async fn run_server(
    cli_host: String,
    cli_port: u16,
    config_path: Option<String>,
    _with_scada: bool,
    _with_agents: bool,
    cli_json_log: bool,
    cli_tls_cert: Option<String>,
    cli_tls_key: Option<String>,
    cli_otel_endpoint: Option<String>,
) -> anyhow::Result<()> {
    // ── 0. Load configuration with env overrides + validation ─────────────
    let config = match EnerOSConfig::load_with_env_overrides(config_path.as_deref()) {
        Ok(cfg) => {
            println!("  [Config] Loaded (network={}, scada={}, devices={})",
                cfg.network.source, cfg.scada.source, cfg.devices.len());
            cfg
        }
        Err(e) => {
            println!("  [Config] WARNING: load_with_env_overrides failed ({}); using defaults", e);
            EnerOSConfig::default()
        }
    };

    // ── 0b. Wrap config in SharedConfig for hot reload (v0.9.0) ───────────
    let shared_config = eneros_api::shared_config(config.clone());
    let config_watcher = if let Some(ref path) = config_path {
        let path_buf = std::path::PathBuf::from(path);
        let watcher = eneros_api::ConfigWatcher::new(shared_config.clone(), path_buf).start();
        println!("  [Config] Hot reload enabled (watching: {})", path);
        Some(Arc::new(watcher))
    } else {
        println!("  [Config] Hot reload disabled (no config file path provided)");
        None
    };

    // Determine effective host/port: CLI args override config
    let host = if cli_host != "0.0.0.0" || cli_port != 8080 {
        (cli_host, cli_port)
    } else {
        (config.api.host.clone(), config.api.port)
    };
    let (host, port) = host;
    println!("EnerOS server starting on {}:{}", host, port);

    // Determine effective JSON logging: CLI flag overrides config
    let json_log = cli_json_log || config.observability.enable_json_logging;

    // Initialize tracing with span events for distributed tracing (v0.9.0).
    // When enable_tracing=true, span enter/exit events are logged to the
    // JSON output, enabling trace correlation across components.
    // For full OpenTelemetry export, set otel_endpoint in config.
    let log_level = match config.observability.log_level.as_str() {
        "trace" => tracing::Level::TRACE,
        "debug" => tracing::Level::DEBUG,
        "info" => tracing::Level::INFO,
        "warn" => tracing::Level::WARN,
        "error" => tracing::Level::ERROR,
        _ => tracing::Level::INFO,
    };
    let span_events = if config.observability.enable_tracing {
        // Log span creation/closure for trace correlation
        tracing_subscriber::fmt::format::FmtSpan::NEW | tracing_subscriber::fmt::format::FmtSpan::CLOSE
    } else {
        tracing_subscriber::fmt::format::FmtSpan::NONE
    };
    // T029-18: OTLP endpoint 解析优先级：CLI > OTEL_EXPORTER_OTLP_ENDPOINT > 配置文件 > 默认值
    let otel_endpoint = eneros_api::otel::resolve_otlp_endpoint(
        cli_otel_endpoint.as_deref(),
        config.observability.otel_endpoint.as_deref(),
    );
    let otel_enabled = config.observability.enable_tracing;
    let otel_config = eneros_api::otel::OtelConfig {
        enabled: otel_enabled,
        endpoint: otel_endpoint.clone(),
        service_name: config.observability.otel_service_name.clone(),
    };
    // 构建 OpenTelemetry layer。初始化失败时回退到无 OTLP 模式，
    // 保证服务可用性优先于可观测性（工业级电力系统安全约束）。
    let otel_layer = if otel_enabled {
        match eneros_api::otel::build_otel_layer(&otel_config) {
            Ok(layer) => {
                println!(
                    "  [OTLP] OpenTelemetry export enabled (endpoint={}, service={})",
                    otel_endpoint, otel_config.service_name
                );
                Some(layer)
            }
            Err(e) => {
                println!(
                    "  [OTLP] WARNING: OTLP init failed ({}); continuing without OTLP export",
                    e
                );
                None
            }
        }
    } else {
        println!("  [OTLP] OpenTelemetry export disabled (set enable_tracing=true to enable)");
        None
    };
    // 构建 filter layer（基于配置的日志级别）
    let env_filter = tracing_subscriber::EnvFilter::new(log_level.as_str());
    let (filter_layer, log_reload_handle) =
        tracing_subscriber::reload::Layer::new(env_filter);
    if json_log {
        let fmt_layer = tracing_subscriber::fmt::layer()
            .with_file(true)
            .with_line_number(true)
            .with_span_events(span_events)
            .json();
        let subscriber = tracing_subscriber::registry()
            .with(filter_layer)
            .with(otel_layer)
            .with(fmt_layer);
        tracing::subscriber::set_global_default(subscriber)
            .expect("failed to set global default subscriber");
    } else {
        let fmt_layer = tracing_subscriber::fmt::layer()
            .with_file(true)
            .with_line_number(true)
            .with_span_events(span_events);
        let subscriber = tracing_subscriber::registry()
            .with(filter_layer)
            .with(otel_layer)
            .with(fmt_layer);
        tracing::subscriber::set_global_default(subscriber)
            .expect("failed to set global default subscriber");
    }
    if otel_enabled {
        tracing::info!(
            enable_tracing = true,
            otel_endpoint = %otel_endpoint,
            service_name = %otel_config.service_name,
            "Distributed tracing enabled (OTLP gRPC export to collector)"
        );
    }

    // Determine effective TLS: CLI args override config
    let tls_cert = cli_tls_cert.or_else(|| config.api.tls_cert_path.clone());
    let tls_key = cli_tls_key.or_else(|| config.api.tls_key_path.clone());
    let enable_tls = config.api.enable_tls || tls_cert.is_some();

    // ── 1. EventBus ───────────────────────────────────────────────────────
    let event_bus = Arc::new(eneros_runtime::eventbus::EventBus::new(
        config.eventbus.max_queue_size,
    ));
    println!("  [Core] EventBus created (queue={})", config.eventbus.max_queue_size);

    // ── 2. ConstraintEngine ───────────────────────────────────────────────
    let constraint_engine = Arc::new(
        eneros_runtime::constraint::ConstraintEngine::with_event_bus(event_bus.clone()),
    );
    println!("  [Constraint] Engine created (EventBus wired)");

    // ── 3. PowerNetwork (from config: ieee14 / cnpower / cim) ─────────────
    let network = Arc::new(build_network_from_config(
        &config.network,
        constraint_engine.clone(),
    )?);

    // ── 4. TimeSeriesEngine (SQLite persistent when configured) ───────────
    // Compute retention capacity from [timeseries] config (Task 3: 时序配置接线).
    let retention_capacity = compute_retention_capacity(
        config.timeseries.retention_days,
        config.timeseries.sampling_interval_ms,
    );
    println!(
        "  [TimeSeries] Config: retention_days={}, sampling_interval_ms={}ms, compression={} → capacity={}",
        config.timeseries.retention_days,
        config.timeseries.sampling_interval_ms,
        config.timeseries.enable_compression,
        retention_capacity
    );
    // Note: enable_compression is logged here; compression logic is deferred to Task 5 (降采样).

    let ts_engine = if let Some(ref db_path) = config.security.audit_log_path {
        // Reuse a sibling .db path for time-series if audit path is set
        let ts_db = std::path::Path::new(db_path)
            .with_file_name("eneros_timeseries.db")
            .to_string_lossy()
            .to_string();
        match eneros_runtime::timeseries::TimeSeriesEngine::with_sqlite(retention_capacity, &ts_db) {
            Ok(engine) => {
                println!("  [TimeSeries] Engine created with SQLite backend ({})", ts_db);
                Arc::new(engine)
            }
            Err(e) => {
                println!("  [TimeSeries] WARNING: SQLite init failed ({}); using in-memory", e);
                Arc::new(eneros_runtime::timeseries::TimeSeriesEngine::new(retention_capacity))
            }
        }
    } else {
        // Default: try eneros_timeseries.db in current dir; fall back to in-memory
        match eneros_runtime::timeseries::TimeSeriesEngine::with_sqlite(retention_capacity, "eneros_timeseries.db") {
            Ok(engine) => {
                println!("  [TimeSeries] Engine created with SQLite backend (eneros_timeseries.db)");
                Arc::new(engine)
            }
            Err(e) => {
                println!("  [TimeSeries] WARNING: SQLite init failed ({}); using in-memory", e);
                Arc::new(eneros_runtime::timeseries::TimeSeriesEngine::new(retention_capacity))
            }
        }
    };

    // ── 4a. Start rollup task for storage-level downsampling (Task 5) ─────
    // Background task aggregates 1s data → 1min (every 60s) and → 1h (every 60min).
    // Uses tokio::sync::watch for graceful shutdown (v0.9.0 pattern).
    let (rollup_shutdown_tx, rollup_shutdown_rx) = tokio::sync::watch::channel(false);
    let rollup_handle = ts_engine.clone().start_rollup_task(rollup_shutdown_rx);
    println!("  [TimeSeries] Rollup task started (60s→1min, 60min→1h)");

    // ── 4a-SOE. SOE recorder (SQLite, v0.10.0 — Task 4) ───────────────────
    // Sequence-of-Events recorder persists breaker/switch state changes and
    // protection trips with 1ms precision and a global atomic sequence number.
    let soe_recorder = Arc::new(
        eneros_runtime::timeseries::SoeRecorder::new_sqlite("eneros_soe.db")
            .expect("Failed to initialize SOE recorder"),
    );
    println!("  [SOE] Sequence-of-Events recorder initialized (SQLite)");

    // ── 4b. AuditLog (file-persistent when configured) ────────────────────
    let audit_log = if config.security.enable_audit {
        let log = if let Some(ref path) = config.security.audit_log_path {
            println!("  [Audit] File-persistent audit log enabled ({})", path);
            Arc::new(eneros_api::audit::AuditLog::with_file(100_000, path))
        } else {
            println!("  [Audit] In-memory audit log enabled (set security.audit_log_path for persistence)");
            Arc::new(eneros_api::audit::AuditLog::new(100_000))
        };
        Some(log)
    } else {
        println!("  [Audit] Audit logging disabled");
        None
    };

    // ── 4c. AuthManager (JWT + API Key + RBAC) ────────────────────────────
    let auth_manager = if config.security.enable_auth {
        let secret = config.security.jwt_secret.as_deref().unwrap_or("eneros-default-dev-secret-change-me");
        let mut mgr = eneros_api::auth::AuthManager::new(
            secret,
            config.security.jwt_ttl_secs as usize,
        );
        if let Some(ref al) = audit_log {
            mgr = mgr.with_audit_log(al.clone());
        }
        // Register static API keys from config
        for entry in &config.security.api_keys {
            if let Some(role) = eneros_api::auth::Role::parse(&entry.role) {
                mgr.add_api_key(&entry.key, &entry.description, role);
                println!("  [Auth] Registered API key '{}' (role={})", entry.description, entry.role);
            }
        }
        println!("  [Auth] AuthManager enabled (JWT TTL={}s, API keys={})",
            config.security.jwt_ttl_secs, config.security.api_keys.len());
        Some(Arc::new(mgr))
    } else {
        println!("  [Auth] Authentication disabled (set security.enable_auth=true to enable)");
        None
    };

    // ── 4d. MetricsRegistry (Prometheus) ──────────────────────────────────
    let metrics_registry = if config.observability.enable_metrics {
        println!("  [Metrics] Prometheus metrics enabled at /metrics");
        Some(Arc::new(eneros_api::handlers::metrics::MetricsRegistry::new()))
    } else {
        println!("  [Metrics] Metrics disabled");
        None
    };

    // ── 5. DeviceManager (from config: [[devices]]) ───────────────────────
    let device_manager = build_device_manager(&config.devices).await;
    let devices_configured = config.devices.len();
    if devices_configured > 0 {
        let connected = device_manager.connected_count().await;
        println!(
            "  [Device] {} device(s) registered, {} connected",
            devices_configured, connected
        );
    }

    // ── 6. SCADA data source (from config: simulated / iec104) ────────────
    // Create a single shared data source. The Arc is cloned for both the
    // main collector (used by run_once / agent) and the dual scan pipelines,
    // avoiding duplicate TCP connections to the IEC 104 server.
    let shared_data_source = build_data_source_from_config(&config.scada);
    let scada_config = build_ieee14_scada_config();
    let collector = Arc::new(eneros_runtime::scada::ScadaCollector::new(
        scada_config.clone(),
        shared_data_source.clone(),
    ));
    println!("  [SCADA] Collector created (shared data source)");

    // ── 7. DataPipeline (refresh → collect → record → publish) ────────────
    let pipeline = Arc::new(
        eneros_runtime::scada::DataPipeline::new(collector.clone(), ts_engine.clone())
            .with_event_bus(event_bus.clone())
            .with_soe_recorder(soe_recorder.clone()),
    );
    println!("  [SCADA] DataPipeline created (refresh+collect+record+publish, SOE wired)");

    // ── 7b. DualScanGroup (fast/normal scan separation) ───────────────────
    // Use config-driven intervals instead of hardcoded 100ms/1000ms defaults.
    let dual_scan_group = eneros_runtime::scada::DualScanGroup::auto_classify_with_intervals(
        scada_config.points,
        std::time::Duration::from_millis(config.scada.fast_interval_ms),
        std::time::Duration::from_millis(config.scada.normal_interval_ms),
    );
    println!(
        "  [SCADA] DualScanGroup: {} fast points ({}ms), {} normal points ({}ms)",
        dual_scan_group.fast_points.len(),
        config.scada.fast_interval_ms,
        dual_scan_group.normal_points.len(),
        config.scada.normal_interval_ms
    );

    // ── 8. SnapshotBuilder ────────────────────────────────────────────────
    let snapshot_builder = Arc::new(eneros_runtime::scada::SnapshotBuilder::new(
        build_ieee14_snapshot_mappings(),
    ));
    println!("  [SCADA] SnapshotBuilder created");

    // ── 9. SafetyGateway with production executor ─────────────────────────
    let command_queue = Arc::new(eneros_runtime::gateway::SharedPriorityCommandQueue::new());
    let command_executor = build_command_executor(&device_manager, devices_configured);
    let gateway = Arc::new(eneros_runtime::gateway::SafetyGateway::with_queue_and_executor(
        100,
        command_queue.clone(),
        command_executor,
    ));
    println!("  [Gateway] SafetyGateway created (queue + executor wired)");

    // ── 9b. RealtimeExecutor ──────────────────────────────────────────────
    let rt_executor = gateway.start_executor()?;
    println!("  [Gateway] RealtimeExecutor started");

    // ── 9c. WatchdogTimer ─────────────────────────────────────────────────
    let watchdog = Arc::new(eneros_runtime::gateway::WatchdogTimer::new(
        std::time::Duration::from_millis(500),
    ));
    let _watchdog_handle = watchdog.start();
    println!("  [Gateway] WatchdogTimer started (500ms timeout)");

    // ── 10. Reasoning engine ──────────────────────────────────────────────
    let tool_engine = Arc::new(parking_lot::RwLock::new(eneros_runtime::tool::ToolEngine::new()));
    // AppState requires tokio::sync::RwLock (read guard must be Send across .await)
    let api_tool_engine = Arc::new(tokio::sync::RwLock::new(eneros_runtime::tool::ToolEngine::new()));
    let network_rw = Arc::new(parking_lot::RwLock::new(
        eneros_runtime::network::PowerNetwork::from_ieee14(),
    ));
    // Use FileMemory for persistent agent memory (survives restarts)
    let memory: Arc<dyn eneros_runtime::memory::AgentMemory> = match eneros_runtime::memory::FileMemory::new("./eneros_memory") {
        Ok(m) => {
            println!("  [Memory] FileMemory enabled (./eneros_memory/)");
            Arc::new(m)
        }
        Err(e) => {
            println!("  [Memory] WARNING: FileMemory init failed ({}); using InMemoryMemory", e);
            Arc::new(eneros_runtime::memory::InMemoryMemory::default())
        }
    };
    let reasoning: Arc<dyn eneros_runtime::reasoning::ReasoningEngine> =
        if std::env::var("ENEROS_RIG_PROVIDER").is_ok() {
            #[cfg(feature = "rig")]
            {
                let rig_config = eneros_runtime::reasoning::RigConfig::from_env();
                println!(
                    "  [Reasoning] Using rig engine: {} / {}",
                    rig_config.provider, rig_config.model
                );
                let fallback = Arc::new(eneros_runtime::reasoning::RuleBasedEngine::new());
                Arc::new(
                    eneros_runtime::reasoning::RigReasoningEngine::new(rig_config, network_rw.clone())
                        .with_fallback(fallback),
                )
            }
            #[cfg(not(feature = "rig"))]
            {
                println!("  [Reasoning] rig feature not enabled, falling back to rule-based engine");
                Arc::new(eneros_runtime::reasoning::RuleBasedEngine::new())
            }
        } else {
            println!("  [Reasoning] Using rule-based engine (set ENEROS_RIG_PROVIDER to enable AI reasoning)");
            Arc::new(eneros_runtime::reasoning::RuleBasedEngine::new())
        };

    // ── 11. ConstrainedDecisionPipeline ───────────────────────────────────
    let network_simulator =
        Arc::new(eneros_runtime::network::NetworkSimulatorAdapter::new(network_rw.clone()));
    let projector = Arc::new(eneros_runtime::constraint::projector::FeasibilityProjector::new(
        network_simulator,
    ));

    // Wire Projector into ConstraintEngine
    constraint_engine.set_projector(projector.clone());
    println!("  [Constraint] Projector wired into ConstraintEngine");

    let pipeline_validator =
        Arc::new(eneros_runtime::gateway::constraint_validator::ConstraintAwareValidator::with_projector(
            constraint_engine.clone(),
            gateway.clone(),
            eneros_runtime::gateway::interlocking::InterlockingRuleEngine::new(),
            projector.clone(),
        ));

    // ── 11b. ObservationProvider (closes execute→measure→verify loop) ─────
    // The provider reads the latest SCADA readings and builds a
    // PowerObservation for postcondition verification. This prioritizes
    // real field measurements over simulator predictions.
    let collector_for_obs = collector.clone();
    let observation_provider: eneros_runtime::gateway::ObservationProvider =
        Arc::new(move || {
            let readings = collector_for_obs.latest_all();
            if readings.is_empty() {
                return None;
            }
            Some(build_observation_from_readings(&readings))
        });
    println!("  [Pipeline] ObservationProvider wired (SCADA → postcondition)");

    let decision_pipeline = Arc::new(
        eneros_runtime::gateway::decision_pipeline::ConstrainedDecisionPipeline::with_observation_provider(
            projector,
            pipeline_validator,
            gateway.clone(),
            observation_provider,
        )
        .with_watchdog(watchdog.clone(), std::time::Duration::from_millis(500)),
    );
    println!("  [Pipeline] ConstrainedDecisionPipeline created (projection + validation + execution + observation + watchdog)");

    // ── 12. FeedbackLoop ──────────────────────────────────────────────────
    let feedback_loop = Arc::new(
        eneros_runtime::reasoning::feedback::FeedbackLoop::with_default_iterations_shared(reasoning.clone()),
    );
    println!("  [Pipeline] FeedbackLoop created (shared reasoning, max 2 retries)");

    // ── 13. AgentOrchestrator ─────────────────────────────────────────────
    let ctx = eneros_runtime::agent::AgentContext::new(
        event_bus.clone(),
        gateway.clone(),
        tool_engine,
        network_rw,
        memory.clone(),
        reasoning,
    );
    let mut orchestrator = eneros_runtime::agent::AgentOrchestrator::with_pipeline_and_feedback(
        ctx,
        decision_pipeline.clone(),
        feedback_loop,
    );
    println!("  [Agent] Orchestrator created with ConstrainedDecisionPipeline + FeedbackLoop");

    // ── 13b. Register 6 domain agents ─────────────────────────────────────
    use eneros_runtime::agent::event_adapter::AgentEventHandler;
    use eneros_runtime::eventbus::event::EventType;

    let dispatch_agent = eneros_runtime::agent::DispatchAgent::new("dispatch-1", "DispatchAgent", vec![1]);
    orchestrator.register_agent(AgentEventHandler::new(
        Box::new(dispatch_agent),
        vec![EventType::ConstraintViolation, EventType::DataReceived],
    ));

    let operation_agent =
        eneros_runtime::agent::OperationAgent::new("operation-1", "OperationAgent", vec![1]);
    orchestrator.register_agent(AgentEventHandler::new(
        Box::new(operation_agent),
        vec![EventType::ConstraintViolation, EventType::SystemAlarm],
    ));

    let self_healing_agent =
        eneros_runtime::agent::SelfHealingAgent::new("self-healing-1", "SelfHealingAgent", vec![1]);
    orchestrator.register_agent(AgentEventHandler::new_all_events(Box::new(self_healing_agent)));

    let forecast_agent = eneros_runtime::agent::LoadForecastAgent::new(
        "forecast-1",
        eneros_core::Jurisdiction::for_zones(vec![1]),
        ts_engine.clone(),
    );
    orchestrator.register_agent(AgentEventHandler::new(
        Box::new(forecast_agent),
        vec![EventType::ConstraintViolation, EventType::DataReceived],
    ));

    let planning_agent = eneros_runtime::agent::PlanningAgent::new("planning-1", vec![1]);
    orchestrator.register_agent(AgentEventHandler::new(
        Box::new(planning_agent),
        vec![EventType::ConstraintViolation, EventType::DataReceived],
    ));

    let trading_agent = eneros_runtime::agent::TradingAgent::new("trading-1", vec![1]);
    orchestrator.register_agent(AgentEventHandler::new(
        Box::new(trading_agent),
        vec![EventType::DataReceived, EventType::ConstraintViolation],
    ));

    println!(
        "  [Agent] Registered 6 domain agents: Dispatch, Operation, SelfHealing, Forecast, Planning, Trading"
    );

    let orchestrator = Arc::new(orchestrator);

    // ── 14. DataDrivenAgentLoop ───────────────────────────────────────────
    let state_machine = Arc::new(eneros_runtime::agent::SystemStateMachine::new());
    let dd_loop = Arc::new(
        eneros_runtime::agent::DataDrivenAgentLoop::new(
            pipeline.clone(),
            collector.clone(),
            snapshot_builder.clone(),
            orchestrator.clone(),
            state_machine,
        )
        .with_constraint_engine(constraint_engine.clone()),
    );
    println!("  [Agent] DataDrivenAgentLoop created");

    // ── 14b. AgentController (T029-08) ──────────────────────────────────
    // 创建 AgentController 并注册 6 个 domain agent，使 POST /api/agents/{id}/control
    // 端点可用（否则返回 503）。
    let agent_controller = eneros_runtime::agent::AgentController::new();
    agent_controller.register("dispatch-1", "DispatchAgent");
    agent_controller.register("operation-1", "OperationAgent");
    agent_controller.register("self-healing-1", "SelfHealingAgent");
    agent_controller.register("forecast-1", "LoadForecastAgent");
    agent_controller.register("planning-1", "PlanningAgent");
    agent_controller.register("trading-1", "TradingAgent");
    println!("  [Agent] AgentController created (6 agents registered for lifecycle control)");

    // ── 15. AppState ──────────────────────────────────────────────────────
    let mut state = AppState::new()
        .with_network(network.clone())
        .with_constraint_engine(constraint_engine.clone())
        .with_ts_engine(ts_engine.clone())
        .with_scada_collector(collector.clone())
        .with_event_bus(event_bus.clone())
        .with_agent_orchestrator(orchestrator.clone())
        .with_data_pipeline(pipeline.clone())
        .with_snapshot_builder(snapshot_builder.clone())
        .with_data_driven_loop(dd_loop.clone())
        .with_decision_pipeline(decision_pipeline)
        .with_device_manager(device_manager.clone())
        .with_tool_engine(api_tool_engine.clone())
        .with_agent_memory(memory.clone())
        .with_soe_recorder(soe_recorder.clone())
        .with_agent_controller(agent_controller);

    // Inject optional v0.6.0 subsystems (auth/audit/metrics)
    if let Some(ref al) = audit_log {
        state = state.with_audit_log(al.clone());
    }
    if let Some(ref am) = auth_manager {
        state = state.with_auth_manager(am.clone());
    }
    if let Some(ref mr) = metrics_registry {
        state = state.with_metrics_registry(mr.clone());
    }
    // Inject v0.9.0 config hot reload
    state = state.with_shared_config(shared_config.clone());
    if let Some(ref cw) = config_watcher {
        state = state.with_config_watcher(cw.clone());
    }
    // T029-05: 注入日志级别 reload handle，使 POST /api/log-level API 能动态调整日志级别
    state = state.with_log_reload_handle(log_reload_handle);
    state = state.with_initial_log_level(config.observability.log_level.to_lowercase());
    println!("  [API] AppState wired (auth={}, audit={}, metrics={}, device_mgr={}, memory=FileMemory, config_reload={})",
        auth_manager.is_some(), audit_log.is_some(), metrics_registry.is_some(),
        devices_configured > 0, config_watcher.is_some());

    // ── 16. Start background tasks ────────────────────────────────────────
    // The dual scan group is the sole background data collection mechanism.
    // It splits points into fast/normal groups and runs separate pipelines
    // at config-driven intervals. The main `pipeline` Arc is still available
    // for run_once() calls from the agent orchestrator and API handlers,
    // but we don't start it as a background task to avoid duplicate collection.
    let dual_scan_handles = eneros_runtime::scada::start_dual_scan(
        &dual_scan_group,
        shared_data_source,
        ts_engine.clone(),
        eneros_runtime::scada::DualScanOptions {
            timeout_ms: 5000,
            enable_quality_check: true,
            event_bus: Some(event_bus.clone()),
        },
    );
    println!(
        "  [SCADA] DualScanGroup started (fast={}ms, normal={}ms, with EventBus)",
        config.scada.fast_interval_ms, config.scada.normal_interval_ms
    );

    let dd_loop_handle = dd_loop.start(2000);
    println!("  [Agent] DataDrivenAgentLoop started (2s cycle)");

    // ── 16b. Start EventBus→WebSocket bridge (v0.6.0 — S7) ──────────────
    let ws_bridge_handle = eneros_api::app::start_event_bus_ws_bridge(state.clone());
    if ws_bridge_handle.is_some() {
        println!("  [API] EventBus→WebSocket bridge started");
    } else {
        println!("  [API] EventBus→WebSocket bridge skipped (no EventBus configured)");
    }

    // ── 17. Start HTTP server ─────────────────────────────────────────────
    let addr = format!("{}:{}", host, port)
        .parse::<std::net::SocketAddr>()
        .unwrap_or_else(|_| std::net::SocketAddr::from(([0, 0, 0, 0], port)));

    // TLS support (v0.7.0 — deferred from v0.6.0 S1)
    let tls_config = if enable_tls {
        match (&tls_cert, &tls_key) {
            (Some(cert_path), Some(key_path)) => {
                println!(
                    "  [API] TLS enabled (cert={}, key={})",
                    cert_path, key_path
                );
                Some(eneros_api::server::TlsConfig {
                    cert_path: cert_path.clone(),
                    key_path: key_path.clone(),
                })
            }
            (Some(_), None) | (None, Some(_)) => {
                println!("  [API] WARNING: TLS cert and key must both be set; falling back to plaintext");
                None
            }
            (None, None) => {
                println!("  [API] WARNING: enable_tls=true but no cert/key provided; falling back to plaintext");
                None
            }
        }
    } else {
        None
    };

    let server = ApiServer::with_state(state, addr).with_tls(tls_config.clone());

    if tls_config.is_some() {
        println!("EnerOS server running on https://{}:{}", host, port);
    } else {
        println!("EnerOS server running on http://{}:{}", host, port);
    }
    println!("Press Ctrl+C to stop");

    // ── 18. Graceful shutdown ─────────────────────────────────────────────
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let _ctrlc = tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        let _ = tx.send(());
    });

    tokio::select! {
        result = server.start() => {
            result?;
        }
        _ = rx => {
            println!("\nShutting down EnerOS server...");
        }
    }

    // Cleanup background tasks — graceful shutdown for SCADA pipelines
    // (drain current cycle before exiting), abort for others.
    dual_scan_handles.shutdown().await;
    dd_loop_handle.abort();
    if let Some(h) = ws_bridge_handle {
        h.abort();
    }
    rt_executor.stop();
    watchdog.stop();

    // Stop rollup task (graceful shutdown via watch signal)
    let _ = rollup_shutdown_tx.send(true);
    let _ = rollup_handle.await;
    println!("  [TimeSeries] Rollup task stopped");

    // Stop config file watcher
    if let Some(mut cw) = config_watcher.and_then(|arc| Arc::try_unwrap(arc).ok()) {
        cw.stop();
        println!("  [Config] File watcher stopped");
    }

    // Disconnect all devices
    if devices_configured > 0 {
        let _ = device_manager.disconnect_all().await;
        println!("  [Device] All devices disconnected");
    }

    // T029-18: Flush OTLP batch exporter — ensure all pending spans are sent to collector
    eneros_api::otel::shutdown_otlp();
    println!("  [OTLP] Trace exporter flushed");

    println!("EnerOS server stopped.");
    Ok(())
}

/// Build a `PowerObservation` from the latest SCADA readings.
///
/// This is used by the `ObservationProvider` to feed real field measurements
/// into the postcondition verification stage of the decision pipeline. The
/// mapping from (element_id, parameter) → observation fields follows the
/// IEEE 14 snapshot conventions:
/// - `voltage_pu` → bus voltage magnitude
/// - `angle_deg` → bus voltage angle
/// - `gen_p_mw` / `gen_q_mvar` → generator output
/// - `load_p_mw` / `load_q_mvar` → load consumption
/// - `frequency_hz` → system frequency
fn build_observation_from_readings(
    readings: &[eneros_runtime::scada::ScadaReading],
) -> eneros_core::PowerObservation {
    use eneros_core::{
        BranchFlowObservation, BusVoltageObservation, GenOutputObservation,
        LoadConsumptionObservation, PowerObservation,
    };
    use std::collections::HashMap;

    let mut bus_voltages: HashMap<u64, BusVoltageObservation> = HashMap::new();
    let mut gen_outputs: HashMap<u64, GenOutputObservation> = HashMap::new();
    let mut load_consumptions: HashMap<u64, LoadConsumptionObservation> = HashMap::new();
    let mut frequency_hz = 50.0;
    let mut total_load_mw = 0.0;
    let mut total_gen_mw = 0.0;

    for r in readings {
        match r.parameter.as_str() {
            "voltage_pu" => {
                bus_voltages.insert(
                    r.element_id,
                    BusVoltageObservation {
                        vm_pu: r.value,
                        va_degree: 0.0,
                    },
                );
            }
            "angle_deg" => {
                // Merge angle into existing bus voltage observation
                bus_voltages
                    .entry(r.element_id)
                    .and_modify(|v| v.va_degree = r.value)
                    .or_insert(BusVoltageObservation {
                        vm_pu: 1.0,
                        va_degree: r.value,
                    });
            }
            "gen_p_mw" => {
                gen_outputs
                    .entry(r.element_id)
                    .and_modify(|g| g.p_mw = r.value)
                    .or_insert(GenOutputObservation {
                        p_mw: r.value,
                        q_mvar: 0.0,
                        p_max_mw: 0.0,
                        p_min_mw: 0.0,
                    });
                total_gen_mw += r.value;
            }
            "gen_q_mvar" => {
                gen_outputs
                    .entry(r.element_id)
                    .and_modify(|g| g.q_mvar = r.value)
                    .or_insert(GenOutputObservation {
                        p_mw: 0.0,
                        q_mvar: r.value,
                        p_max_mw: 0.0,
                        p_min_mw: 0.0,
                    });
            }
            "load_p_mw" => {
                load_consumptions
                    .entry(r.element_id)
                    .and_modify(|l| l.p_mw = r.value)
                    .or_insert(LoadConsumptionObservation {
                        p_mw: r.value,
                        q_mvar: 0.0,
                    });
                total_load_mw += r.value;
            }
            "load_q_mvar" => {
                load_consumptions
                    .entry(r.element_id)
                    .and_modify(|l| l.q_mvar = r.value)
                    .or_insert(LoadConsumptionObservation {
                        p_mw: 0.0,
                        q_mvar: r.value,
                    });
            }
            "frequency_hz" => {
                frequency_hz = r.value;
            }
            _ => {}
        }
    }

    PowerObservation {
        bus_voltages,
        branch_flows: HashMap::<u64, BranchFlowObservation>::new(),
        frequency_hz,
        gen_outputs,
        load_consumptions,
        timestamp: chrono::Utc::now(),
        total_load_mw,
        total_gen_mw,
    }
}

async fn query_status(server: &str) -> anyhow::Result<()> {
    let client = eneros_api::ApiClient::new(server);

    // Health check first
    match client.health_check().await {
        Ok(true) => {}
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
            eneros_api::types::AgentsResponse {
                agent_count: 0,
                agents: Vec::new(),
            }
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
    println!(
        "{:<20} {:<15} {:<12} {:<10}",
        "Name", "Type", "Authority", "Status"
    );
    println!("{}", "-".repeat(60));
    for agent in &agents.agents {
        println!(
            "{:<20} {:<15} {:<12} {:<10}",
            agent.name, agent.agent_type, agent.authority, agent.status
        );
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

    println!(
        "Network: {} buses, {} branches",
        network.bus_count(),
        network.branch_count()
    );
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
    println!(
        "{:<8} {:>12} {:>12} {:>12} {:>12}",
        "Bus", "V (p.u.)", "theta (deg)", "P (MW)", "Q (MVar)"
    );
    println!("{}", "-".repeat(58));
    for bus in &result.bus_results {
        println!(
            "{:<8} {:>12.4} {:>12.4} {:>12.4} {:>12.4}",
            bus.bus_id,
            bus.voltage_magnitude,
            bus.voltage_angle.to_degrees(),
            bus.p_injection,
            bus.q_injection
        );
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

    let solver = eneros_runtime::analysis::DcOpfSolver::new();
    let result = solver.solve(&problem)?;

    if result.converged {
        println!("OPF converged in {} iterations", result.iterations);
    } else {
        println!(
            "OPF completed with {} warning(s) in {} iterations",
            result.warnings.len(),
            result.iterations
        );
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

fn build_ieee14_opf_problem(data: &eneros_powerflow::Ieee14BusData) -> eneros_runtime::analysis::DcOpfProblem {
    use eneros_runtime::analysis::{BranchLimit, GeneratorBid};

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

    eneros_runtime::analysis::DcOpfProblem {
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
    let measurements: Vec<eneros_runtime::analysis::Measurement> = pf_result.bus_results.iter()
        .flat_map(|bus| {
            vec![
                eneros_runtime::analysis::Measurement::bus(
                    eneros_runtime::analysis::MeasType::VoltageMagnitude,
                    bus.bus_id,
                    bus.voltage_magnitude,
                    0.005,
                ),
                eneros_runtime::analysis::Measurement::bus(
                    eneros_runtime::analysis::MeasType::BusInjectionP,
                    bus.bus_id,
                    bus.p_injection,
                    0.05,
                ),
            ]
        })
        .collect();

    let bus_count = pf_result.bus_results.len();
    let estimator = eneros_runtime::analysis::StateEstimator::default_estimator();
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
        "3p" => eneros_runtime::analysis::FaultType::ThreePhase,
        "slg" => eneros_runtime::analysis::FaultType::SingleLineGround,
        "ll" => eneros_runtime::analysis::FaultType::LineLine,
        "dlg" => eneros_runtime::analysis::FaultType::DoubleLineGround,
        other => return Err(anyhow::anyhow!(
            "Unknown fault type '{}'. Available: 3p, slg, ll, dlg", other
        )),
    };

    let network = eneros_runtime::network::PowerNetwork::from_ieee14();

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

    let fault = eneros_runtime::analysis::FaultSpec {
        bus_id: bus,
        fault_type,
        fault_impedance: num_complex::Complex64::new(0.0, 0.0),
    };

    // For asymmetric faults, provide sequence impedances
    let seq_z = eneros_runtime::analysis::SequenceImpedance {
        z1: num_complex::Complex64::new(0.01, 0.1),
        z2: num_complex::Complex64::new(0.01, 0.1),
        z0: num_complex::Complex64::new(0.03, 0.3),
    };

    let analyzer = eneros_runtime::analysis::ShortCircuitAnalyzer::new();
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
    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            host,
            port,
            config,
            with_scada,
            with_agents,
            json_log,
            tls_cert,
            tls_key,
            otel_endpoint,
        } => {
            // Initialize tracing — JSON logging is enabled by config or CLI flag
            // (v0.6.0 config.observability.enable_json_logging defaults to true)
            run_server(host, port, config, with_scada, with_agents, json_log, tls_cert, tls_key, otel_endpoint)
                .await?;
        }
        Commands::Status { server } => {
            tracing_subscriber::fmt()
                .with_max_level(tracing::Level::WARN)
                .init();
            query_status(&server).await?;
        }
        Commands::Agent { command } => {
            tracing_subscriber::fmt()
                .with_max_level(tracing::Level::WARN)
                .init();
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
            tracing_subscriber::fmt()
                .with_max_level(tracing::Level::WARN)
                .init();
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
        Commands::PowerFlow {
            case,
            max_iterations,
            tolerance,
        } => {
            tracing_subscriber::fmt()
                .with_max_level(tracing::Level::WARN)
                .init();
            run_power_flow(&case, max_iterations, tolerance)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// retention_days = 30, sampling_interval_ms = 1000
    /// → 30 * 86400 * 1000 / 1000 = 2,592,000
    #[test]
    fn test_retention_capacity_30_days_1s_interval() {
        let cap = compute_retention_capacity(30, 1000);
        assert_eq!(cap, 2_592_000);
    }

    /// retention_days = 7, sampling_interval_ms = 500
    /// → 7 * 86400 * 1000 / 500 = 1,209,600
    #[test]
    fn test_retention_capacity_7_days_500ms_interval() {
        let cap = compute_retention_capacity(7, 500);
        assert_eq!(cap, 1_209_600);
    }

    /// Upper-bound clamp: retention_days = 365, sampling_interval_ms = 100
    /// → min(315,360,000, 10,000,000) = 10,000,000
    #[test]
    fn test_retention_capacity_upper_bound_clamp() {
        let cap = compute_retention_capacity(365, 100);
        assert_eq!(cap, 10_000_000);
    }

    /// Minimum sampling interval clamp: a sub-100ms value is raised to 100ms.
    #[test]
    fn test_retention_capacity_min_sampling_interval_clamp() {
        // 1 day at 10ms would be 8,640,000 — but 10ms is clamped to 100ms,
        // giving 1 * 86400 * 1000 / 100 = 864,000.
        let cap = compute_retention_capacity(1, 10);
        assert_eq!(cap, 864_000);
    }

    /// Minimum retention_days clamp: zero days is raised to 1 day.
    #[test]
    fn test_retention_capacity_min_retention_days_clamp() {
        // 0 days is invalid; clamped to 1 day at 1000ms → 86,400.
        let cap = compute_retention_capacity(0, 1000);
        assert_eq!(cap, 86_400);
    }

    /// Default config values (365 days, 1000ms) produce a sane capacity that
    /// respects the upper bound.
    #[test]
    fn test_retention_capacity_default_config_values() {
        let cap = compute_retention_capacity(365, 1000);
        // 365 * 86400 * 1000 / 1000 = 31,536,000 → clamped to 10,000,000.
        assert_eq!(cap, 10_000_000);
    }
}

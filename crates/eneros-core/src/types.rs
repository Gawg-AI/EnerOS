use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Unique identifier for power system elements
pub type ElementId = u64;

/// Zone identifier for network partitioning
pub type ZoneId = u32;

/// Timestamp in milliseconds since epoch
pub type TimestampMs = i64;

/// Bus type in power flow calculation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BusType {
    /// PQ bus (load bus) - voltage magnitude and angle unknown
    PQ,
    /// PV bus (generator bus) - voltage magnitude known, angle unknown
    PV,
    /// Slack bus (reference bus) - voltage magnitude and angle known
    Slack,
}

/// Branch type in power system
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BranchType {
    /// Transmission line
    Line,
    /// Transformer
    Transformer,
    /// DC link
    DcLink,
}

/// Equipment type classification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EquipmentType {
    /// Synchronous generator
    SynchronousGenerator,
    /// Asynchronous generator
    AsynchronousGenerator,
    /// Photovoltaic inverter
    PhotovoltaicInverter,
    /// Wind turbine converter
    WindTurbineConverter,
    /// Two-winding transformer
    TwoWindingTransformer,
    /// Three-winding transformer
    ThreeWindingTransformer,
    /// Autotransformer
    Autotransformer,
    /// Overhead transmission line
    OverheadLine,
    /// Underground cable
    UndergroundCable,
    /// Constant power load
    ConstantPowerLoad,
    /// Constant impedance load
    ConstantImpedanceLoad,
    /// Motor load
    MotorLoad,
    /// Capacitor bank
    CapacitorBank,
    /// Reactor
    Reactor,
    /// SVC (Static Var Compensator)
    Svc,
    /// SVG (Static Var Generator)
    Svg,
    /// Circuit breaker
    CircuitBreaker,
    /// Disconnector
    Disconnector,
    /// Load break switch
    LoadBreakSwitch,
}

/// Severity level for constraint violations
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum SeverityLevel {
    /// Informational - near limit, preventive reminder
    Info,
    /// Minor - out of normal range, needs attention
    Minor,
    /// Major - violates safety criteria, needs immediate handling
    Major,
    /// Critical - may cause system collapse or blackout
    Critical,
}

/// Power system state snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PowerSystemState {
    /// Timestamp of the state
    pub timestamp: DateTime<Utc>,
    /// Bus voltages (magnitude in p.u., angle in radians)
    pub bus_voltages: Vec<BusVoltage>,
    /// Branch power flows (MW, MVar)
    pub branch_flows: Vec<BranchFlow>,
    /// Generation output (MW, MVar)
    pub generation: Vec<GenOutput>,
    /// Load consumption (MW, MVar)
    pub loads: Vec<LoadConsumption>,
    /// System frequency (Hz)
    pub frequency: f64,
    /// Total system losses (MW)
    pub total_losses: f64,
}

/// Bus voltage measurement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusVoltage {
    pub bus_id: ElementId,
    pub voltage_magnitude: f64,
    pub voltage_angle: f64,
    pub voltage_kv: f64,
}

/// Branch power flow measurement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchFlow {
    pub branch_id: ElementId,
    pub from_bus: ElementId,
    pub to_bus: ElementId,
    pub active_power_mw: f64,
    pub reactive_power_mvar: f64,
    pub current_ka: f64,
    pub loading_percent: f64,
}

/// Generator output measurement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenOutput {
    pub gen_id: ElementId,
    pub bus_id: ElementId,
    pub active_power_mw: f64,
    pub reactive_power_mvar: f64,
    pub voltage_setpoint: f64,
    pub status: bool,
}

/// Load consumption measurement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadConsumption {
    pub load_id: ElementId,
    pub bus_id: ElementId,
    pub active_power_mw: f64,
    pub reactive_power_mvar: f64,
    pub status: bool,
}

/// Branch electrical parameters for topology changes
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BranchParams {
    /// Resistance in per-unit
    pub r: f64,
    /// Reactance in per-unit
    pub x: f64,
    /// Susceptance in per-unit
    pub b: f64,
    /// Rate in MVA
    pub rate_mva: f64,
    /// Branch name
    pub name: Option<String>,
    /// Branch type
    pub branch_type: BranchType,
    /// From bus ID
    pub from_bus: ElementId,
    /// To bus ID
    pub to_bus: ElementId,
}

/// Topology change event
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TopologyChange {
    /// Switch state changed
    SwitchToggle {
        switch_id: ElementId,
        closed: bool,
    },
    /// Branch added with electrical parameters
    BranchAdded {
        branch_id: ElementId,
        params: BranchParams,
    },
    /// Branch removed
    BranchRemoved {
        branch_id: ElementId,
    },
    /// Bus added
    BusAdded {
        bus_id: ElementId,
    },
    /// Bus removed
    BusRemoved {
        bus_id: ElementId,
    },
}

impl Default for PowerSystemState {
    fn default() -> Self {
        Self {
            timestamp: Utc::now(),
            bus_voltages: Vec::new(),
            branch_flows: Vec::new(),
            generation: Vec::new(),
            loads: Vec::new(),
            frequency: 50.0,
            total_losses: 0.0,
        }
    }
}

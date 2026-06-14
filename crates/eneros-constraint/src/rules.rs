use eneros_core::{ElementId, SeverityLevel};

/// Constraint type classification
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstraintType {
    Voltage,
    Thermal,
    Frequency,
    Stability,
    N1,
}

/// Constraint category
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstraintCategory {
    Normal,
    Alert,
    Emergency,
    Extreme,
}

/// Response strategy for constraint violations
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponseStrategy {
    /// Log and notify
    Alarm,
    /// Limit output/adjust flow
    Degradation,
    /// Trip breaker/generator
    Trip,
    /// Load shedding/system separation
    Emergency,
}

/// Power system constraint definition
#[derive(Debug, Clone)]
pub struct Constraint {
    /// Unique constraint identifier
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Constraint type
    pub constraint_type: ConstraintType,
    /// Constraint category
    pub category: ConstraintCategory,
    /// Element type this constraint applies to
    pub element_type: String,
    /// Element IDs this constraint applies to
    pub element_ids: Vec<ElementId>,
    /// Parameter being monitored
    pub parameter: String,
    /// Minimum limit value
    pub limit_min: f64,
    /// Maximum limit value
    pub limit_max: f64,
    /// Severity level
    pub severity: SeverityLevel,
    /// Response strategy
    pub response_strategy: ResponseStrategy,
    /// Check interval in milliseconds
    pub check_interval_ms: u64,
    /// Whether constraint is enabled
    pub enabled: bool,
}

impl Constraint {
    /// Create a new constraint
    pub fn new(
        id: String,
        name: String,
        constraint_type: ConstraintType,
        limit_min: f64,
        limit_max: f64,
    ) -> Self {
        Self {
            id,
            name,
            constraint_type,
            category: ConstraintCategory::Normal,
            element_type: String::new(),
            element_ids: Vec::new(),
            parameter: String::new(),
            limit_min,
            limit_max,
            severity: SeverityLevel::Minor,
            response_strategy: ResponseStrategy::Alarm,
            check_interval_ms: 1000,
            enabled: true,
        }
    }
}

/// N-1 contingency analysis result for a single branch outage
#[derive(Debug, Clone)]
pub struct N1Result {
    /// Branch ID that was removed
    pub branch_id: ElementId,
    /// Whether the post-contingency system converges
    pub converged: bool,
    /// Voltage violations after contingency
    pub voltage_violations: Vec<N1Violation>,
    /// Thermal violations after contingency
    pub thermal_violations: Vec<N1Violation>,
    /// Overall severity of this contingency
    pub severity: SeverityLevel,
}

/// Single violation found during N-1 analysis
#[derive(Debug, Clone)]
pub struct N1Violation {
    /// Element ID (bus or branch)
    pub element_id: ElementId,
    /// Violation type
    pub violation_type: N1ViolationType,
    /// Actual value
    pub actual_value: f64,
    /// Limit value
    pub limit_value: f64,
}

/// N-1 violation type
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum N1ViolationType {
    /// Bus voltage below minimum
    LowVoltage,
    /// Bus voltage above maximum
    HighVoltage,
    /// Branch loading exceeds thermal limit
    Overload,
}

/// Voltage stability analysis result
#[derive(Debug, Clone)]
pub struct StabilityResult {
    /// Voltage stability margin per bus (lower = closer to instability)
    pub voltage_margins: Vec<VoltageMargin>,
    /// Buses with critically low stability margin
    pub critical_buses: Vec<ElementId>,
    /// Overall system stability status
    pub stable: bool,
}

/// Voltage stability margin for a single bus
#[derive(Debug, Clone)]
pub struct VoltageMargin {
    /// Bus ID
    pub bus_id: ElementId,
    /// Voltage magnitude (p.u.)
    pub voltage_pu: f64,
    /// Stability margin indicator (1.0 = healthy, 0.0 = collapse)
    pub margin: f64,
}

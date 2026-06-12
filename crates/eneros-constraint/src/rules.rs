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

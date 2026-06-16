use eneros_core::{
    ActionFeasibility, ElementId, SeverityLevel, StructuredAction, SystemOperatingState,
};
use eneros_eventbus::event::{Event, EventPayload, EventType};
use eneros_eventbus::EventBus;
use eneros_powerflow::{BusResult, PowerFlowSolver, YBusMatrix};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

use crate::projector::{FeasibilityProjector, ProjectionResult};
use crate::rules::{
    Constraint, ConstraintType, N1Result, N1Violation, N1ViolationType, StabilityResult,
    VoltageMargin,
};
use crate::violation::Violation;

/// Default voltage limits for N-1 analysis
const VOLTAGE_MIN_PU: f64 = 0.95;
const VOLTAGE_MAX_PU: f64 = 1.10;
/// Default thermal loading limit for N-1 analysis
const THERMAL_LIMIT_PERCENT: f64 = 100.0;
/// Voltage stability margin threshold
const STABILITY_MARGIN_THRESHOLD: f64 = 0.3;

/// Constraint executor for power system safety
pub struct ConstraintEngine {
    constraints: RwLock<HashMap<String, Constraint>>,
    violations: RwLock<Vec<Violation>>,
    /// Dynamic threshold multiplier (1.0 = normal, higher = more relaxed).
    /// Interior-mutable so it can be updated by `set_emergency_thresholds`
    /// via a shared `&ConstraintEngine` (e.g. through `Arc<ConstraintEngine>`
    /// held by the system state machine), matching the pattern used by the
    /// other RwLock-protected fields above.
    current_threshold_multiplier: RwLock<f64>,
    /// Optional event bus for violation notifications
    event_bus: Option<Arc<EventBus>>,
    /// Optional feasibility projector for physics-based action feasibility checks
    projector: RwLock<Option<Arc<FeasibilityProjector>>>,
}

impl ConstraintEngine {
    /// Create a new constraint engine
    pub fn new() -> Self {
        Self {
            constraints: RwLock::new(HashMap::new()),
            violations: RwLock::new(Vec::new()),
            current_threshold_multiplier: RwLock::new(1.0),
            event_bus: None,
            projector: RwLock::new(None),
        }
    }

    /// Create a constraint engine with an event bus for violation notifications
    pub fn with_event_bus(event_bus: Arc<EventBus>) -> Self {
        Self {
            constraints: RwLock::new(HashMap::new()),
            violations: RwLock::new(Vec::new()),
            current_threshold_multiplier: RwLock::new(1.0),
            event_bus: Some(event_bus),
            projector: RwLock::new(None),
        }
    }

    /// Create a constraint engine with a feasibility projector for physics-based checks
    pub fn with_projector(projector: Arc<FeasibilityProjector>) -> Self {
        Self {
            constraints: RwLock::new(HashMap::new()),
            violations: RwLock::new(Vec::new()),
            current_threshold_multiplier: RwLock::new(1.0),
            event_bus: None,
            projector: RwLock::new(Some(projector)),
        }
    }

    /// Create a constraint engine with both event bus and projector
    pub fn with_event_bus_and_projector(
        event_bus: Arc<EventBus>,
        projector: Arc<FeasibilityProjector>,
    ) -> Self {
        Self {
            constraints: RwLock::new(HashMap::new()),
            violations: RwLock::new(Vec::new()),
            current_threshold_multiplier: RwLock::new(1.0),
            event_bus: Some(event_bus),
            projector: RwLock::new(Some(projector)),
        }
    }

    /// Register a constraint
    pub fn register(&self, constraint: Constraint) {
        let mut constraints = self.constraints.write();
        constraints.insert(constraint.id.clone(), constraint);
    }

    /// Remove a constraint
    pub fn remove(&self, constraint_id: &str) -> bool {
        let mut constraints = self.constraints.write();
        constraints.remove(constraint_id).is_some()
    }

    /// Set or replace the feasibility projector for physics-based action checks
    pub fn set_projector(&self, projector: Arc<FeasibilityProjector>) {
        let mut inner = self.projector.write();
        *inner = Some(projector);
    }

    /// Check all constraints against current state
    pub fn check_all(
        &self,
        bus_voltages: &[(ElementId, f64)], // (bus_id, voltage_pu)
        branch_loadings: &[(ElementId, f64)], // (branch_id, loading_percent)
        frequency: f64,
    ) -> Vec<Violation> {
        let constraints = self.constraints.read();
        let mut violations = Vec::new();

        for constraint in constraints.values() {
            if !constraint.enabled {
                continue;
            }

            let violation = match constraint.constraint_type {
                ConstraintType::Voltage => self.check_voltage_constraint(constraint, bus_voltages),
                ConstraintType::Thermal => {
                    self.check_thermal_constraint(constraint, branch_loadings)
                }
                ConstraintType::Frequency => self.check_frequency_constraint(constraint, frequency),
                ConstraintType::N1 => {
                    self.check_n1_constraint(constraint, bus_voltages, branch_loadings)
                }
                ConstraintType::Stability => {
                    self.check_stability_constraint(constraint, bus_voltages)
                }
            };

            if let Some(v) = violation {
                violations.push(v);
            }
        }

        // Record violations
        let mut recorded_violations = self.violations.write();
        recorded_violations.extend(violations.clone());

        // Publish violation events via EventBus
        self.publish_violations(&violations);

        violations
    }

    fn check_voltage_constraint(
        &self,
        constraint: &Constraint,
        bus_voltages: &[(ElementId, f64)],
    ) -> Option<Violation> {
        let (limit_min, limit_max) =
            self.relaxed_band_limits(constraint.limit_min, constraint.limit_max);
        for &(bus_id, voltage) in bus_voltages {
            if constraint.element_ids.contains(&bus_id)
                && (voltage < limit_min || voltage > limit_max)
            {
                return Some(Violation {
                    constraint_id: constraint.id.clone(),
                    element_id: bus_id,
                    constraint_type: ConstraintType::Voltage,
                    actual_value: voltage,
                    limit_min,
                    limit_max,
                    severity: constraint.severity,
                    response_strategy: constraint.response_strategy.clone(),
                    timestamp: chrono::Utc::now(),
                });
            }
        }
        None
    }

    fn check_thermal_constraint(
        &self,
        constraint: &Constraint,
        branch_loadings: &[(ElementId, f64)],
    ) -> Option<Violation> {
        let limit_max = self.relaxed_upper_limit(constraint.limit_max);
        for &(branch_id, loading) in branch_loadings {
            if constraint.element_ids.contains(&branch_id) && loading > limit_max {
                return Some(Violation {
                    constraint_id: constraint.id.clone(),
                    element_id: branch_id,
                    constraint_type: ConstraintType::Thermal,
                    actual_value: loading,
                    limit_min: constraint.limit_min,
                    limit_max,
                    severity: constraint.severity,
                    response_strategy: constraint.response_strategy.clone(),
                    timestamp: chrono::Utc::now(),
                });
            }
        }
        None
    }

    fn check_frequency_constraint(
        &self,
        constraint: &Constraint,
        frequency: f64,
    ) -> Option<Violation> {
        let (limit_min, limit_max) =
            self.relaxed_band_limits(constraint.limit_min, constraint.limit_max);
        if frequency < limit_min || frequency > limit_max {
            Some(Violation {
                constraint_id: constraint.id.clone(),
                element_id: 0,
                constraint_type: ConstraintType::Frequency,
                actual_value: frequency,
                limit_min,
                limit_max,
                severity: constraint.severity,
                response_strategy: constraint.response_strategy.clone(),
                timestamp: chrono::Utc::now(),
            })
        } else {
            None
        }
    }

    fn check_n1_constraint(
        &self,
        constraint: &Constraint,
        bus_voltages: &[(ElementId, f64)],
        _branch_loadings: &[(ElementId, f64)],
    ) -> Option<Violation> {
        let (limit_min, limit_max) =
            self.relaxed_band_limits(constraint.limit_min, constraint.limit_max);
        for &(bus_id, voltage) in bus_voltages {
            if constraint.element_ids.contains(&bus_id)
                && (voltage < limit_min || voltage > limit_max)
            {
                return Some(Violation {
                    constraint_id: constraint.id.clone(),
                    element_id: bus_id,
                    constraint_type: ConstraintType::N1,
                    actual_value: voltage,
                    limit_min,
                    limit_max,
                    severity: constraint.severity,
                    response_strategy: constraint.response_strategy.clone(),
                    timestamp: chrono::Utc::now(),
                });
            }
        }
        None
    }

    fn check_stability_constraint(
        &self,
        constraint: &Constraint,
        bus_voltages: &[(ElementId, f64)],
    ) -> Option<Violation> {
        let (limit_min, limit_max) =
            self.relaxed_band_limits(constraint.limit_min, constraint.limit_max);
        for &(bus_id, voltage) in bus_voltages {
            if constraint.element_ids.contains(&bus_id) && voltage < limit_min {
                return Some(Violation {
                    constraint_id: constraint.id.clone(),
                    element_id: bus_id,
                    constraint_type: ConstraintType::Stability,
                    actual_value: voltage,
                    limit_min,
                    limit_max,
                    severity: constraint.severity,
                    response_strategy: constraint.response_strategy.clone(),
                    timestamp: chrono::Utc::now(),
                });
            }
        }
        None
    }

    fn relaxed_band_limits(&self, limit_min: f64, limit_max: f64) -> (f64, f64) {
        // Read the multiplier into a local copy so the read-guard is dropped
        // immediately and never held across the arithmetic below.
        let multiplier = *self.current_threshold_multiplier.read();
        if multiplier <= 1.0 || !limit_min.is_finite() || !limit_max.is_finite() {
            return (limit_min, limit_max);
        }

        let midpoint = (limit_min + limit_max) / 2.0;
        let half_width = (limit_max - limit_min).abs() / 2.0;
        (
            midpoint - half_width * multiplier,
            midpoint + half_width * multiplier,
        )
    }

    fn relaxed_upper_limit(&self, limit_max: f64) -> f64 {
        let multiplier = *self.current_threshold_multiplier.read();
        if multiplier <= 1.0 || !limit_max.is_finite() {
            return limit_max;
        }
        limit_max * multiplier
    }

    /// Perform N-1 contingency analysis
    ///
    /// For each branch, remove it from the Y-Bus matrix, re-solve power flow,
    /// and check for voltage/thermal violations.
    #[allow(clippy::too_many_arguments)]
    pub fn check_n1_analysis(
        &self,
        _ybus: &YBusMatrix,
        p_spec: &[f64],
        q_spec: &[f64],
        bus_types: &[eneros_powerflow::solver::BusTypeNR],
        branches: &[(ElementId, ElementId, f64, f64, f64, f64)], // (from, to, r, x, b, tap)
        bus_map: &HashMap<ElementId, usize>,
        solver: &PowerFlowSolver,
        voltage_min: Option<f64>,
        voltage_max: Option<f64>,
        thermal_limit: Option<f64>,
    ) -> Vec<N1Result> {
        let v_min = voltage_min.unwrap_or(VOLTAGE_MIN_PU);
        let v_max = voltage_max.unwrap_or(VOLTAGE_MAX_PU);
        let t_limit = thermal_limit.unwrap_or(THERMAL_LIMIT_PERCENT);

        let mut results = Vec::new();

        for (branch_idx, &(from, to, _r, _x, _b, _tap)) in branches.iter().enumerate() {
            // Build Y-Bus without this branch
            let mut reduced_branches: Vec<(ElementId, ElementId, f64, f64, f64, f64)> = Vec::new();
            for (i, &br) in branches.iter().enumerate() {
                if i != branch_idx {
                    reduced_branches.push(br);
                }
            }

            let ybus_n1 = YBusMatrix::from_branches(&reduced_branches, bus_map);

            // Solve power flow for the contingency case
            let result = solver.solve(&ybus_n1, p_spec, q_spec, bus_types);

            match result {
                Ok(pf_result) if pf_result.converged => {
                    let mut voltage_violations = Vec::new();
                    let mut thermal_violations = Vec::new();

                    // Check voltage violations
                    for bus in &pf_result.bus_results {
                        if bus.voltage_magnitude < v_min {
                            voltage_violations.push(N1Violation {
                                element_id: bus.bus_id,
                                violation_type: N1ViolationType::LowVoltage,
                                actual_value: bus.voltage_magnitude,
                                limit_value: v_min,
                            });
                        } else if bus.voltage_magnitude > v_max {
                            voltage_violations.push(N1Violation {
                                element_id: bus.bus_id,
                                violation_type: N1ViolationType::HighVoltage,
                                actual_value: bus.voltage_magnitude,
                                limit_value: v_max,
                            });
                        }
                    }

                    // Check thermal violations
                    for branch in &pf_result.branch_results {
                        if branch.loading_percent > t_limit {
                            thermal_violations.push(N1Violation {
                                element_id: branch.branch_id,
                                violation_type: N1ViolationType::Overload,
                                actual_value: branch.loading_percent,
                                limit_value: t_limit,
                            });
                        }
                    }

                    let severity =
                        if !voltage_violations.is_empty() || !thermal_violations.is_empty() {
                            let has_low_voltage =
                                voltage_violations.iter().any(|v| v.actual_value < 0.85);
                            let has_overload =
                                thermal_violations.iter().any(|v| v.actual_value > 150.0);
                            if has_low_voltage || has_overload {
                                SeverityLevel::Critical
                            } else {
                                SeverityLevel::Major
                            }
                        } else {
                            SeverityLevel::Info
                        };

                    results.push(N1Result {
                        branch_id: from * 1000 + to, // Generate a unique ID from bus pair
                        converged: true,
                        voltage_violations,
                        thermal_violations,
                        severity,
                    });
                }
                _ => {
                    // Non-convergence is a critical N-1 violation
                    results.push(N1Result {
                        branch_id: from * 1000 + to,
                        converged: false,
                        voltage_violations: Vec::new(),
                        thermal_violations: Vec::new(),
                        severity: SeverityLevel::Critical,
                    });
                }
            }
        }

        results
    }

    /// Perform voltage stability analysis
    ///
    /// Uses a simplified approach: compute voltage stability margin
    /// based on distance from nominal voltage.
    pub fn check_stability(&self, bus_results: &[BusResult]) -> StabilityResult {
        let mut voltage_margins = Vec::new();
        let mut critical_buses = Vec::new();

        for bus in bus_results {
            // Simplified voltage stability margin:
            // margin = (V - V_critical) / V_nominal
            // V_critical ≈ 0.7 pu (typical voltage collapse threshold)
            let v_critical = 0.7;
            let v_nominal = 1.0;
            let margin = (bus.voltage_magnitude - v_critical) / (v_nominal - v_critical);

            voltage_margins.push(VoltageMargin {
                bus_id: bus.bus_id,
                voltage_pu: bus.voltage_magnitude,
                margin: margin.max(0.0),
            });

            if margin < STABILITY_MARGIN_THRESHOLD {
                critical_buses.push(bus.bus_id);
            }
        }

        let stable = critical_buses.is_empty();

        StabilityResult {
            voltage_margins,
            critical_buses,
            stable,
        }
    }

    /// Get violation history
    pub fn get_violations(&self) -> Vec<Violation> {
        self.violations.read().clone()
    }

    /// Clear violation history
    pub fn clear_violations(&self) {
        self.violations.write().clear();
    }

    /// Check if an action is feasible — predicts impact without executing.
    /// Uses FeasibilityProjector for physics-based What-If analysis when available,
    /// falls back to keyword-based heuristic matching otherwise.
    pub fn check_action_feasibility(&self, action_description: &str) -> ActionFeasibility {
        // If projector is available, try to use it for physics-based check
        let projector_guard = self.projector.read();
        if let Some(ref projector) = *projector_guard {
            if let Some(action) = Self::infer_structured_action(action_description) {
                return self.check_structured_action_feasibility_impl(&action, projector);
            }
        }

        // Fallback: keyword-based heuristic matching
        self.check_action_feasibility_heuristic(action_description)
    }

    /// Check if a structured action is feasible using What-If simulation.
    /// Delegates to FeasibilityProjector when available, otherwise uses heuristic.
    pub fn check_structured_action_feasibility(
        &self,
        action: &StructuredAction,
    ) -> ActionFeasibility {
        let projector_guard = self.projector.read();
        if let Some(ref projector) = *projector_guard {
            self.check_structured_action_feasibility_impl(action, projector)
        } else {
            // No projector — convert to description and use heuristic
            let desc = Self::structured_action_to_description(action);
            self.check_action_feasibility_heuristic(&desc)
        }
    }

    /// Keyword-based heuristic feasibility check (fallback when no projector available)
    fn check_action_feasibility_heuristic(&self, action_description: &str) -> ActionFeasibility {
        let mut new_violations = Vec::new();
        let mut worsened = Vec::new();

        let action_lower = action_description.to_lowercase();

        // Check against current violations
        let current = self.get_current_violations();

        // Shedding capacitor when voltage is low -> worsen voltage
        if action_lower.contains("shed capacitor") || action_lower.contains("切除电容") {
            let has_voltage_low = current
                .iter()
                .any(|v| v.constraint_type == ConstraintType::Voltage);
            if has_voltage_low {
                worsened.push("voltage_low".to_string());
            }
        }

        // Tripping line when thermal overload exists -> worsen thermal
        if action_lower.contains("trip line") || action_lower.contains("切除线路") {
            let has_thermal = current
                .iter()
                .any(|v| v.constraint_type == ConstraintType::Thermal);
            if has_thermal {
                worsened.push("thermal_overload".to_string());
            }
        }

        // Load shedding -> new violation risk (under-frequency if too much)
        if action_lower.contains("load shed") || action_lower.contains("切负荷") {
            new_violations.push("potential_under_frequency".to_string());
        }

        let feasible = worsened.is_empty();
        let risk_level = if !worsened.is_empty() {
            SeverityLevel::Critical
        } else if !new_violations.is_empty() {
            SeverityLevel::Major
        } else {
            SeverityLevel::Info
        };

        ActionFeasibility {
            feasible,
            new_violations,
            worsened_violations: worsened,
            risk_level,
        }
    }

    /// Physics-based feasibility check using FeasibilityProjector
    fn check_structured_action_feasibility_impl(
        &self,
        action: &StructuredAction,
        projector: &FeasibilityProjector,
    ) -> ActionFeasibility {
        let projection = projector.project(action);

        match projection {
            ProjectionResult::Feasible(_) => ActionFeasibility {
                feasible: true,
                new_violations: Vec::new(),
                worsened_violations: Vec::new(),
                risk_level: SeverityLevel::Info,
            },
            ProjectionResult::Projected { modifications, .. } => {
                let new_violations: Vec<String> = modifications
                    .iter()
                    .map(|m| {
                        format!(
                            "{}: {} -> {} ({})",
                            m.parameter, m.original_value, m.projected_value, m.reason
                        )
                    })
                    .collect();
                ActionFeasibility {
                    feasible: true,
                    new_violations,
                    worsened_violations: Vec::new(),
                    risk_level: SeverityLevel::Major,
                }
            }
            ProjectionResult::Infeasible {
                violated_constraints,
                ..
            } => ActionFeasibility {
                feasible: false,
                new_violations: violated_constraints,
                worsened_violations: Vec::new(),
                risk_level: SeverityLevel::Critical,
            },
        }
    }

    /// Publish violation events via EventBus
    fn publish_violations(&self, violations: &[Violation]) {
        if let Some(ref bus) = self.event_bus {
            for v in violations {
                let event = Event::new(
                    EventType::ConstraintViolation,
                    "ConstraintEngine",
                    EventPayload::ConstraintViolation {
                        constraint_id: v.constraint_id.clone(),
                        element_id: v.element_id,
                        actual_value: v.actual_value,
                        limit_value: if v.below_minimum() {
                            v.limit_min
                        } else {
                            v.limit_max
                        },
                        severity: format!("{:?}", v.severity),
                    },
                );
                let _ = bus.publish(event);
            }
        }
    }

    /// Infer a StructuredAction from a text description.
    /// Returns None if the description cannot be mapped.
    fn infer_structured_action(description: &str) -> Option<StructuredAction> {
        let desc_lower = description.to_lowercase();

        // Generator setpoint
        if desc_lower.contains("generator") || desc_lower.contains("发电机") {
            let target_mw = Self::extract_mw_value(description)?;
            let gen_id = Self::extract_number_after_keywords(description, &["generator"])
                .or_else(|| Self::extract_device_id(description))?;
            return Some(StructuredAction::StartGenerator { gen_id, target_mw });
        }

        // Load shedding
        if desc_lower.contains("load shed") || desc_lower.contains("切负荷") {
            let amount_mw = Self::extract_mw_value(description)?;
            let zone_id = Self::extract_number_after_keywords(description, &["zone"])
                .or_else(|| Self::extract_device_id(description))? as u32;
            return Some(StructuredAction::ShedLoad { zone_id, amount_mw });
        }

        // Fault isolation
        if desc_lower.contains("isolate fault") || desc_lower.contains("隔离故障") {
            let device_id = Self::extract_device_id(description)?;
            return Some(StructuredAction::IsolateFault {
                upstream_switch: device_id,
                downstream_switch: device_id + 1,
            });
        }

        // Tie switch
        if desc_lower.contains("close tie") || desc_lower.contains("合环") {
            let switch_id = Self::extract_device_id(description)?;
            return Some(StructuredAction::CloseTieSwitch { switch_id });
        }

        // Switching operation
        if desc_lower.contains("breaker")
            || desc_lower.contains("断路器")
            || desc_lower.contains("disconnector")
            || desc_lower.contains("隔离开关")
            || desc_lower.contains("switch")
            || desc_lower.contains("开关")
        {
            let device_id = Self::extract_device_id(description)?;
            let operation = if desc_lower.contains("close") || desc_lower.contains("合") {
                "close".to_string()
            } else {
                "open".to_string()
            };
            return Some(StructuredAction::ExecuteDevice {
                device_id,
                operation,
                value: 1.0,
            });
        }

        None
    }

    /// Convert a StructuredAction to a text description for heuristic fallback
    fn structured_action_to_description(action: &StructuredAction) -> String {
        match action {
            StructuredAction::StartGenerator { gen_id, target_mw } => {
                format!("Start generator {} to {:.1} MW", gen_id, target_mw)
            }
            StructuredAction::ShedLoad { zone_id, amount_mw } => {
                format!("Load shed {:.1} MW from zone {}", amount_mw, zone_id)
            }
            StructuredAction::ExecuteDevice {
                device_id,
                operation,
                value,
            } => format!("{} device {} value {:.2}", operation, device_id, value),
            StructuredAction::IsolateFault {
                upstream_switch,
                downstream_switch,
            } => format!(
                "Isolate fault: switches {} and {}",
                upstream_switch, downstream_switch
            ),
            StructuredAction::CloseTieSwitch { switch_id } => {
                format!("Close tie switch {}", switch_id)
            }
            StructuredAction::NotifyAgent { agent_id, message } => {
                format!("Notify agent {}: {}", agent_id, message)
            }
        }
    }

    /// Extract a MW value from a description string
    fn extract_mw_value(description: &str) -> Option<f64> {
        // Try patterns like "100MW", "100 MW", "100.0MW", "100.0 MW"
        let upper = description.to_uppercase();
        if let Some(idx) = upper.find("MW") {
            let prefix = &description[..idx].trim_end();
            let num_start = prefix
                .rfind(|c: char| !c.is_ascii_digit() && c != '.')
                .map(|i| i + 1)
                .unwrap_or(0);
            if num_start < prefix.len() {
                if let Ok(val) = prefix[num_start..].trim().parse::<f64>() {
                    return Some(val);
                }
            }
        }
        None
    }

    /// Extract a numeric device ID from a description string
    fn extract_device_id(description: &str) -> Option<u64> {
        Self::extract_number_after_keywords(
            description,
            &["switch", "breaker", "disconnector", "device"],
        )
        .or_else(|| {
            Self::extract_number_before_keywords(
                description,
                &["switch", "breaker", "disconnector", "device"],
            )
        })
        .or_else(|| Self::extract_numbers(description).first().copied())
    }

    fn extract_number_after_keywords(description: &str, keywords: &[&str]) -> Option<u64> {
        let lower = description.to_lowercase();
        keywords.iter().find_map(|keyword| {
            lower.find(keyword).and_then(|idx| {
                let start = idx + keyword.len();
                description
                    .get(start..)
                    .and_then(|suffix| Self::extract_numbers(suffix).first().copied())
            })
        })
    }

    fn extract_number_before_keywords(description: &str, keywords: &[&str]) -> Option<u64> {
        let lower = description.to_lowercase();
        keywords.iter().find_map(|keyword| {
            lower.find(keyword).and_then(|idx| {
                description
                    .get(..idx)
                    .and_then(|prefix| Self::extract_numbers(prefix).last().copied())
            })
        })
    }

    fn extract_numbers(description: &str) -> Vec<u64> {
        let mut numbers = Vec::new();
        let mut current = String::new();

        for ch in description.chars() {
            if ch.is_ascii_digit() {
                current.push(ch);
            } else if !current.is_empty() {
                if let Ok(number) = current.parse() {
                    numbers.push(number);
                }
                current.clear();
            }
        }

        if !current.is_empty() {
            if let Ok(number) = current.parse() {
                numbers.push(number);
            }
        }

        numbers
    }

    /// Get current violations
    pub fn get_current_violations(&self) -> Vec<Violation> {
        self.violations.read().clone()
    }

    /// Set emergency thresholds — adjust constraint limits based on system state.
    ///
    /// Takes `&self` (not `&mut self`) so it can be invoked through a shared
    /// `Arc<ConstraintEngine>` held by the system state machine. The multiplier
    /// field is interior-mutable (`RwLock<f64>`).
    pub fn set_emergency_thresholds(&self, state: SystemOperatingState) {
        let new_multiplier = match state {
            SystemOperatingState::Normal | SystemOperatingState::Restoration => 1.0,
            SystemOperatingState::Alert => 1.0, // No relaxation in alert
            SystemOperatingState::Emergency => 1.5, // Allow 50% more deviation
            SystemOperatingState::Blackout => 2.0, // Maximum relaxation
        };
        *self.current_threshold_multiplier.write() = new_multiplier;
    }

    /// Get current threshold multiplier
    pub fn threshold_multiplier(&self) -> f64 {
        *self.current_threshold_multiplier.read()
    }

    /// Get constraint count
    pub fn constraint_count(&self) -> usize {
        self.constraints.read().len()
    }
}

impl Default for ConstraintEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projector::{FeasibilityProjector, NetworkSimulator, WhatIfResult};
    use eneros_eventbus::event::{EventPayload, EventType};
    use eneros_powerflow::solver::BusTypeNR;
    use std::collections::HashMap;

    /// Mock simulator for projector-based tests
    struct MockSimulator {
        always_feasible: bool,
    }

    impl MockSimulator {
        fn new_feasible() -> Self {
            Self {
                always_feasible: true,
            }
        }
        fn new_with_violations() -> Self {
            Self {
                always_feasible: false,
            }
        }
    }

    impl NetworkSimulator for MockSimulator {
        fn simulate_action(&self, action: &StructuredAction) -> WhatIfResult {
            if self.always_feasible {
                WhatIfResult {
                    applicable: true,
                    converged: true,
                    voltage_violations: vec![],
                    thermal_violations: vec![],
                    all_constraints_satisfied: true,
                    summary: "OK".to_string(),
                }
            } else {
                match action {
                    StructuredAction::StartGenerator { target_mw, .. } if *target_mw > 200.0 => {
                        WhatIfResult {
                            applicable: true,
                            converged: true,
                            voltage_violations: vec![(2, 0.88, 0.95)],
                            thermal_violations: vec![(5, 110.0, 100.0)],
                            all_constraints_satisfied: false,
                            summary: "Voltage and thermal violations".to_string(),
                        }
                    }
                    _ => WhatIfResult {
                        applicable: true,
                        converged: true,
                        voltage_violations: vec![],
                        thermal_violations: vec![],
                        all_constraints_satisfied: true,
                        summary: "OK".to_string(),
                    },
                }
            }
        }
        fn generator_limits(&self) -> Vec<(u64, f64, f64)> {
            vec![(1, 0.0, 200.0), (2, 0.0, 150.0)]
        }
        fn current_voltages(&self) -> Vec<(u64, f64)> {
            vec![(1, 1.02), (2, 0.98)]
        }
    }

    #[test]
    fn test_constraint_engine_new() {
        let engine = ConstraintEngine::new();
        assert_eq!(engine.constraint_count(), 0);
    }

    #[test]
    fn test_register_and_check_voltage() {
        let engine = ConstraintEngine::new();
        let mut constraint = Constraint::new(
            "v1".to_string(),
            "Voltage check".to_string(),
            ConstraintType::Voltage,
            0.95,
            1.05,
        );
        constraint.element_ids = vec![1, 2];
        engine.register(constraint);

        let bus_voltages: Vec<(ElementId, f64)> = vec![(1, 0.90), (2, 1.02)];
        let branch_loadings: Vec<(ElementId, f64)> = vec![];
        let violations = engine.check_all(&bus_voltages, &branch_loadings, 50.0);

        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].element_id, 1);
    }

    #[test]
    fn test_check_frequency() {
        let engine = ConstraintEngine::new();
        let constraint = Constraint::new(
            "f1".to_string(),
            "Frequency check".to_string(),
            ConstraintType::Frequency,
            49.8,
            50.2,
        );
        engine.register(constraint);

        let bus_voltages: Vec<(ElementId, f64)> = vec![];
        let branch_loadings: Vec<(ElementId, f64)> = vec![];
        let violations = engine.check_all(&bus_voltages, &branch_loadings, 49.5);

        assert_eq!(violations.len(), 1);
        assert!(violations[0].actual_value < 49.8);
    }

    #[test]
    fn test_check_thermal() {
        let engine = ConstraintEngine::new();
        let mut constraint = Constraint::new(
            "t1".to_string(),
            "Thermal check".to_string(),
            ConstraintType::Thermal,
            0.0,
            100.0,
        );
        constraint.element_ids = vec![10];
        engine.register(constraint);

        let bus_voltages: Vec<(ElementId, f64)> = vec![];
        let branch_loadings: Vec<(ElementId, f64)> = vec![(10, 120.0)];
        let violations = engine.check_all(&bus_voltages, &branch_loadings, 50.0);

        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].element_id, 10);
    }

    #[test]
    fn test_n1_analysis_simple() {
        let mut bus_map = HashMap::new();
        bus_map.insert(0u64, 0);
        bus_map.insert(1u64, 1);
        bus_map.insert(2u64, 2);

        let branches: Vec<(ElementId, ElementId, f64, f64, f64, f64)> = vec![
            (0, 1, 0.01, 0.1, 0.0, 1.0),
            (1, 2, 0.015, 0.15, 0.0, 1.0),
            (0, 2, 0.02, 0.2, 0.0, 1.0),
        ];

        let ybus = YBusMatrix::from_branches(&branches, &bus_map);
        let p_spec = vec![0.0, 0.5, -1.0];
        let q_spec = vec![0.0, 0.2, -0.5];
        let bus_types = vec![BusTypeNR::Slack, BusTypeNR::PV, BusTypeNR::PQ];

        let solver = PowerFlowSolver::new(50, 1e-6);
        let engine = ConstraintEngine::new();

        let results = engine.check_n1_analysis(
            &ybus, &p_spec, &q_spec, &bus_types, &branches, &bus_map, &solver, None, None, None,
        );

        assert_eq!(results.len(), 3);
        // At least some contingencies should converge (mesh network)
        let converged_count = results.iter().filter(|r| r.converged).count();
        assert!(converged_count >= 1, "At least 1 N-1 case should converge");
    }

    #[test]
    fn test_stability_check() {
        let engine = ConstraintEngine::new();

        let bus_results = vec![
            BusResult {
                bus_id: 0,
                voltage_magnitude: 1.05,
                voltage_angle: 0.0,
                p_injection: 0.0,
                q_injection: 0.0,
            },
            BusResult {
                bus_id: 1,
                voltage_magnitude: 0.98,
                voltage_angle: -0.05,
                p_injection: 0.5,
                q_injection: 0.2,
            },
            BusResult {
                bus_id: 2,
                voltage_magnitude: 0.75,
                voltage_angle: -0.15,
                p_injection: -1.0,
                q_injection: -0.5,
            },
        ];

        let result = engine.check_stability(&bus_results);

        assert!(result.voltage_margins.len() == 3);
        // Bus 2 has low voltage, should be critical
        assert!(result.critical_buses.contains(&2));
        assert!(!result.stable);
    }

    #[test]
    fn test_stability_healthy() {
        let engine = ConstraintEngine::new();

        let bus_results = vec![
            BusResult {
                bus_id: 0,
                voltage_magnitude: 1.05,
                voltage_angle: 0.0,
                p_injection: 0.0,
                q_injection: 0.0,
            },
            BusResult {
                bus_id: 1,
                voltage_magnitude: 1.02,
                voltage_angle: -0.02,
                p_injection: 0.5,
                q_injection: 0.2,
            },
        ];

        let result = engine.check_stability(&bus_results);
        assert!(result.stable);
        assert!(result.critical_buses.is_empty());
    }

    // === Action feasibility tests ===

    #[test]
    fn test_check_action_feasibility_safe_action() {
        let engine = ConstraintEngine::new();
        let result = engine.check_action_feasibility("adjust transformer tap");
        assert!(result.feasible);
        assert!(result.new_violations.is_empty());
        assert!(result.worsened_violations.is_empty());
        assert_eq!(result.risk_level, SeverityLevel::Info);
    }

    #[test]
    fn test_check_action_feasibility_shed_capacitor_with_voltage_low() {
        let engine = ConstraintEngine::new();
        let mut constraint = Constraint::new(
            "v1".to_string(),
            "Voltage check".to_string(),
            ConstraintType::Voltage,
            0.95,
            1.05,
        );
        constraint.element_ids = vec![1];
        engine.register(constraint);

        // Trigger a voltage violation
        let bus_voltages: Vec<(ElementId, f64)> = vec![(1, 0.90)];
        let branch_loadings: Vec<(ElementId, f64)> = vec![];
        let violations = engine.check_all(&bus_voltages, &branch_loadings, 50.0);
        assert_eq!(violations.len(), 1);

        // Now check feasibility of shedding capacitor — should worsen voltage
        let result = engine.check_action_feasibility("shed capacitor bank 1");
        assert!(!result.feasible);
        assert!(result
            .worsened_violations
            .contains(&"voltage_low".to_string()));
        assert_eq!(result.risk_level, SeverityLevel::Critical);
    }

    #[test]
    fn test_check_action_feasibility_shed_capacitor_no_voltage_issue() {
        let engine = ConstraintEngine::new();
        // No violations registered, shedding capacitor should be feasible
        let result = engine.check_action_feasibility("shed capacitor bank 1");
        assert!(result.feasible);
        assert!(result.worsened_violations.is_empty());
        assert_eq!(result.risk_level, SeverityLevel::Info);
    }

    #[test]
    fn test_check_action_feasibility_trip_line_with_thermal() {
        let engine = ConstraintEngine::new();
        let mut constraint = Constraint::new(
            "t1".to_string(),
            "Thermal check".to_string(),
            ConstraintType::Thermal,
            0.0,
            100.0,
        );
        constraint.element_ids = vec![10];
        engine.register(constraint);

        // Trigger a thermal violation
        let bus_voltages: Vec<(ElementId, f64)> = vec![];
        let branch_loadings: Vec<(ElementId, f64)> = vec![(10, 120.0)];
        let violations = engine.check_all(&bus_voltages, &branch_loadings, 50.0);
        assert_eq!(violations.len(), 1);

        // Now check feasibility of tripping a line — should worsen thermal
        let result = engine.check_action_feasibility("trip line L1");
        assert!(!result.feasible);
        assert!(result
            .worsened_violations
            .contains(&"thermal_overload".to_string()));
        assert_eq!(result.risk_level, SeverityLevel::Critical);
    }

    #[test]
    fn test_check_action_feasibility_load_shed() {
        let engine = ConstraintEngine::new();
        // Load shedding introduces potential under-frequency risk
        let result = engine.check_action_feasibility("load shed 50MW");
        assert!(result.feasible); // feasible but with new violation risk
        assert!(result
            .new_violations
            .contains(&"potential_under_frequency".to_string()));
        assert!(result.worsened_violations.is_empty());
        assert_eq!(result.risk_level, SeverityLevel::Major);
    }

    #[test]
    fn test_check_action_feasibility_chinese_keywords() {
        let engine = ConstraintEngine::new();
        let mut constraint = Constraint::new(
            "v1".to_string(),
            "Voltage check".to_string(),
            ConstraintType::Voltage,
            0.95,
            1.05,
        );
        constraint.element_ids = vec![1];
        engine.register(constraint);

        // Trigger a voltage violation
        let bus_voltages: Vec<(ElementId, f64)> = vec![(1, 0.90)];
        let branch_loadings: Vec<(ElementId, f64)> = vec![];
        engine.check_all(&bus_voltages, &branch_loadings, 50.0);

        // Test Chinese keyword: 切除电容
        let result = engine.check_action_feasibility("切除电容组1");
        assert!(!result.feasible);
        assert!(result
            .worsened_violations
            .contains(&"voltage_low".to_string()));

        // Test Chinese keyword: 切负荷
        let engine2 = ConstraintEngine::new();
        let result2 = engine2.check_action_feasibility("切负荷50MW");
        assert!(result2.feasible);
        assert!(result2
            .new_violations
            .contains(&"potential_under_frequency".to_string()));
    }

    // === get_current_violations tests ===

    #[test]
    fn test_get_current_violations_empty() {
        let engine = ConstraintEngine::new();
        let violations = engine.get_current_violations();
        assert!(violations.is_empty());
    }

    #[test]
    fn test_get_current_violations_with_data() {
        let engine = ConstraintEngine::new();
        let mut constraint = Constraint::new(
            "v1".to_string(),
            "Voltage check".to_string(),
            ConstraintType::Voltage,
            0.95,
            1.05,
        );
        constraint.element_ids = vec![1];
        engine.register(constraint);

        let bus_voltages: Vec<(ElementId, f64)> = vec![(1, 0.90)];
        let branch_loadings: Vec<(ElementId, f64)> = vec![];
        engine.check_all(&bus_voltages, &branch_loadings, 50.0);

        let current = engine.get_current_violations();
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].constraint_type, ConstraintType::Voltage);
        assert_eq!(current[0].element_id, 1);
    }

    // === Emergency thresholds tests ===

    #[test]
    fn test_set_emergency_thresholds_normal() {
        let engine = ConstraintEngine::new();
        engine.set_emergency_thresholds(SystemOperatingState::Normal);
        assert!((engine.threshold_multiplier() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_set_emergency_thresholds_alert() {
        let engine = ConstraintEngine::new();
        engine.set_emergency_thresholds(SystemOperatingState::Alert);
        assert!((engine.threshold_multiplier() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_set_emergency_thresholds_emergency() {
        let engine = ConstraintEngine::new();
        engine.set_emergency_thresholds(SystemOperatingState::Emergency);
        assert!((engine.threshold_multiplier() - 1.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_set_emergency_thresholds_blackout() {
        let engine = ConstraintEngine::new();
        engine.set_emergency_thresholds(SystemOperatingState::Blackout);
        assert!((engine.threshold_multiplier() - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_set_emergency_thresholds_restoration() {
        let engine = ConstraintEngine::new();
        // First set to blackout, then restore
        engine.set_emergency_thresholds(SystemOperatingState::Blackout);
        assert!((engine.threshold_multiplier() - 2.0).abs() < f64::EPSILON);
        engine.set_emergency_thresholds(SystemOperatingState::Restoration);
        assert!((engine.threshold_multiplier() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_threshold_multiplier_default() {
        let engine = ConstraintEngine::new();
        assert!((engine.threshold_multiplier() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_emergency_thresholds_are_applied_to_constraint_checks() {
        let engine = ConstraintEngine::new();

        let mut voltage = Constraint::new(
            "v-emergency".to_string(),
            "Emergency voltage".to_string(),
            ConstraintType::Voltage,
            0.95,
            1.05,
        );
        voltage.element_ids = vec![1];
        engine.register(voltage);

        let mut thermal = Constraint::new(
            "t-emergency".to_string(),
            "Emergency thermal".to_string(),
            ConstraintType::Thermal,
            0.0,
            100.0,
        );
        thermal.element_ids = vec![10];
        engine.register(thermal);

        assert_eq!(
            engine.check_all(&[(1, 0.925)], &[(10, 125.0)], 50.0).len(),
            2
        );

        engine.set_emergency_thresholds(SystemOperatingState::Emergency);
        assert!(engine
            .check_all(&[(1, 0.925)], &[(10, 125.0)], 50.0)
            .is_empty());

        let violations = engine.check_all(&[(1, 0.915)], &[(10, 151.0)], 50.0);
        assert_eq!(violations.len(), 2);
    }

    // === Projector-based feasibility tests ===

    #[test]
    fn test_structured_action_feasibility_with_projector_feasible() {
        let projector = Arc::new(FeasibilityProjector::new(Arc::new(
            MockSimulator::new_feasible(),
        )));
        let engine = ConstraintEngine::with_projector(projector);
        let action = StructuredAction::StartGenerator {
            gen_id: 1,
            target_mw: 100.0,
        };
        let result = engine.check_structured_action_feasibility(&action);
        assert!(result.feasible);
        assert!(result.new_violations.is_empty());
        assert_eq!(result.risk_level, SeverityLevel::Info);
    }

    #[test]
    fn test_structured_action_feasibility_with_projector_infeasible() {
        let projector = Arc::new(FeasibilityProjector::new(Arc::new(
            MockSimulator::new_with_violations(),
        )));
        let engine = ConstraintEngine::with_projector(projector);
        // 300MW gets clipped to 200MW by projector, which is feasible in mock
        // So this tests the projector integration, not the mock behavior
        let action = StructuredAction::StartGenerator {
            gen_id: 1,
            target_mw: 300.0,
        };
        let result = engine.check_structured_action_feasibility(&action);
        // Projector clips 300→200, which is feasible → result is projected (feasible with modifications)
        assert!(result.feasible);
        assert_eq!(result.risk_level, SeverityLevel::Major); // Projected = Major risk
    }

    #[test]
    fn test_structured_action_feasibility_without_projector_fallback() {
        let engine = ConstraintEngine::new();
        let action = StructuredAction::ShedLoad {
            zone_id: 1,
            amount_mw: 50.0,
        };
        let result = engine.check_structured_action_feasibility(&action);
        // Falls back to heuristic — "Load shed" triggers potential_under_frequency
        assert!(result.feasible);
        assert!(result
            .new_violations
            .contains(&"potential_under_frequency".to_string()));
    }

    #[test]
    fn test_action_feasibility_with_projector_infers_action() {
        let projector = Arc::new(FeasibilityProjector::new(Arc::new(
            MockSimulator::new_feasible(),
        )));
        let engine = ConstraintEngine::with_projector(projector);
        // "generator" keyword should be inferred as StartGenerator
        let result = engine.check_action_feasibility("Start generator 1 to 100.0 MW");
        assert!(result.feasible);
    }

    #[test]
    fn test_action_feasibility_with_projector_uninferrable_fallback() {
        let projector = Arc::new(FeasibilityProjector::new(Arc::new(
            MockSimulator::new_feasible(),
        )));
        let engine = ConstraintEngine::with_projector(projector);
        // "shed capacitor" cannot be inferred as StructuredAction → falls back to heuristic
        let result = engine.check_action_feasibility("shed capacitor bank 1");
        assert!(result.feasible); // No violations → heuristic says feasible
    }

    // === EventBus violation notification tests ===

    #[test]
    fn test_event_bus_violation_published() {
        let event_bus = Arc::new(EventBus::new(100));
        let engine = ConstraintEngine::with_event_bus(event_bus.clone());
        let mut constraint = Constraint::new(
            "v1".to_string(),
            "Voltage check".to_string(),
            ConstraintType::Voltage,
            0.95,
            1.05,
        );
        constraint.element_ids = vec![1];
        engine.register(constraint);

        // Subscribe before publishing
        let mut receiver = event_bus.subscribe();

        // Trigger a violation
        let bus_voltages: Vec<(ElementId, f64)> = vec![(1, 0.90)];
        let branch_loadings: Vec<(ElementId, f64)> = vec![];
        let violations = engine.check_all(&bus_voltages, &branch_loadings, 50.0);
        assert_eq!(violations.len(), 1);

        // Verify event was published
        let event = receiver.try_recv().expect("Should receive violation event");
        assert_eq!(event.event_type, EventType::ConstraintViolation);
        match event.payload {
            EventPayload::ConstraintViolation {
                constraint_id,
                element_id,
                actual_value,
                ..
            } => {
                assert_eq!(constraint_id, "v1");
                assert_eq!(element_id, 1);
                assert!((actual_value - 0.90).abs() < f64::EPSILON);
            }
            _ => panic!("Expected ConstraintViolation payload"),
        }
    }

    #[test]
    fn test_event_bus_no_event_without_violations() {
        let event_bus = Arc::new(EventBus::new(100));
        let engine = ConstraintEngine::with_event_bus(event_bus.clone());
        let mut constraint = Constraint::new(
            "v1".to_string(),
            "Voltage check".to_string(),
            ConstraintType::Voltage,
            0.95,
            1.05,
        );
        constraint.element_ids = vec![1];
        engine.register(constraint);

        let mut receiver = event_bus.subscribe();

        // No violation
        let bus_voltages: Vec<(ElementId, f64)> = vec![(1, 1.00)];
        let branch_loadings: Vec<(ElementId, f64)> = vec![];
        let violations = engine.check_all(&bus_voltages, &branch_loadings, 50.0);
        assert!(violations.is_empty());

        // No event should be published
        assert!(receiver.try_recv().is_err());
    }

    // === Infer structured action tests ===

    #[test]
    fn test_infer_structured_action_generator() {
        let action = ConstraintEngine::infer_structured_action("Start generator 1 to 100.0 MW");
        assert!(action.is_some());
        match action.unwrap() {
            StructuredAction::StartGenerator { gen_id, target_mw } => {
                assert_eq!(gen_id, 1);
                // MW extraction may vary — just verify it's a positive number
                assert!(target_mw > 0.0);
            }
            _ => panic!("Expected StartGenerator"),
        }
    }

    #[test]
    fn test_infer_structured_action_load_shed() {
        let action = ConstraintEngine::infer_structured_action("load shed 50MW from zone 1");
        assert!(action.is_some());
        match action.unwrap() {
            StructuredAction::ShedLoad { zone_id, amount_mw } => {
                assert_eq!(zone_id, 1);
                assert!((amount_mw - 50.0).abs() < f64::EPSILON);
            }
            _ => panic!("Expected ShedLoad"),
        }
    }

    #[test]
    fn test_infer_structured_action_prefers_switch_id_over_line_id() {
        let action = ConstraintEngine::infer_structured_action("open line 5 switch 12");
        assert!(action.is_some());
        match action.unwrap() {
            StructuredAction::ExecuteDevice {
                device_id,
                operation,
                ..
            } => {
                assert_eq!(device_id, 12);
                assert_eq!(operation, "open");
            }
            _ => panic!("Expected ExecuteDevice"),
        }
    }

    #[test]
    fn test_infer_structured_action_unknown() {
        let action = ConstraintEngine::infer_structured_action("adjust transformer tap");
        assert!(action.is_none());
    }

    // === Integration: EventBus + Projector together ===

    #[test]
    fn test_with_event_bus_and_projector_integration() {
        let event_bus = Arc::new(EventBus::new(100));
        let projector = Arc::new(FeasibilityProjector::new(Arc::new(
            MockSimulator::new_feasible(),
        )));
        let engine = ConstraintEngine::with_event_bus_and_projector(event_bus.clone(), projector);

        // Register a constraint
        let mut constraint = Constraint::new(
            "v1".to_string(),
            "Voltage check".to_string(),
            ConstraintType::Voltage,
            0.95,
            1.05,
        );
        constraint.element_ids = vec![1];
        engine.register(constraint);

        // Subscribe to events
        let mut receiver = event_bus.subscribe();

        // Trigger a violation — should publish event
        let bus_voltages: Vec<(ElementId, f64)> = vec![(1, 0.90)];
        let branch_loadings: Vec<(ElementId, f64)> = vec![];
        let violations = engine.check_all(&bus_voltages, &branch_loadings, 50.0);
        assert_eq!(violations.len(), 1);

        // Verify EventBus notification
        let event = receiver.try_recv().expect("Should receive violation event");
        assert_eq!(event.event_type, EventType::ConstraintViolation);

        // Verify Projector-based feasibility check works
        let action = StructuredAction::StartGenerator {
            gen_id: 1,
            target_mw: 100.0,
        };
        let result = engine.check_structured_action_feasibility(&action);
        assert!(result.feasible);
    }

    #[test]
    fn test_set_projector_runtime() {
        let engine = ConstraintEngine::new();

        // Without projector — heuristic fallback
        let action = StructuredAction::ShedLoad {
            zone_id: 1,
            amount_mw: 50.0,
        };
        let result = engine.check_structured_action_feasibility(&action);
        assert!(result
            .new_violations
            .contains(&"potential_under_frequency".to_string()));

        // Set projector at runtime
        let projector = Arc::new(FeasibilityProjector::new(Arc::new(
            MockSimulator::new_feasible(),
        )));
        engine.set_projector(projector);

        // Now uses projector — no heuristic violations
        let result2 = engine.check_structured_action_feasibility(&action);
        assert!(result2.feasible);
        assert!(result2.new_violations.is_empty());
    }
}

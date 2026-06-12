use eneros_core::ElementId;

/// Power flow calculation result
#[derive(Debug, Clone)]
pub struct PowerFlowResult {
    /// Whether the calculation converged
    pub converged: bool,
    /// Number of iterations
    pub iterations: u32,
    /// Bus results
    pub bus_results: Vec<BusResult>,
    /// Branch results
    pub branch_results: Vec<BranchResult>,
    /// Total system losses (MW)
    pub total_losses: f64,
}

/// Bus calculation result
#[derive(Debug, Clone)]
pub struct BusResult {
    /// Bus ID
    pub bus_id: ElementId,
    /// Voltage magnitude (p.u.)
    pub voltage_magnitude: f64,
    /// Voltage angle (radians)
    pub voltage_angle: f64,
    /// Active power injection (MW)
    pub p_injection: f64,
    /// Reactive power injection (MVar)
    pub q_injection: f64,
}

/// Branch calculation result
#[derive(Debug, Clone)]
pub struct BranchResult {
    /// Branch ID
    pub branch_id: ElementId,
    /// From bus ID
    pub from_bus: ElementId,
    /// To bus ID
    pub to_bus: ElementId,
    /// Active power flow from (MW)
    pub p_from: f64,
    /// Reactive power flow from (MVar)
    pub q_from: f64,
    /// Active power flow to (MW)
    pub p_to: f64,
    /// Reactive power flow to (MVar)
    pub q_to: f64,
    /// Power loss (MW)
    pub loss_mw: f64,
    /// Reactive loss (MVar)
    pub loss_mvar: f64,
    /// Loading percentage
    pub loading_percent: f64,
}

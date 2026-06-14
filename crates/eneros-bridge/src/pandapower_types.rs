use serde::{Deserialize, Serialize};

/// Result of a pandapower power flow calculation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PandapowerResult {
    /// Whether the power flow converged
    pub converged: bool,
    /// Bus results
    pub buses: Vec<BusResult>,
    /// Line results
    pub lines: Vec<LineResult>,
    /// Transformer results
    pub trafos: Vec<TrafoResult>,
    /// Total active power loss (MW)
    pub total_loss_mw: f64,
    /// Total reactive power loss (MVar)
    pub total_loss_mvar: f64,
}

/// Bus power flow result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusResult {
    /// Bus ID
    pub id: i64,
    /// Voltage magnitude (pu)
    pub vm_pu: Option<f64>,
    /// Voltage angle (degrees)
    pub va_degree: Option<f64>,
}

/// Line power flow result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineResult {
    /// Line ID
    pub id: i64,
    /// Active power from bus (MW)
    pub p_from_mw: Option<f64>,
    /// Reactive power from bus (MVar)
    pub q_from_mvar: Option<f64>,
    /// Active power to bus (MW)
    pub p_to_mw: Option<f64>,
    /// Reactive power to bus (MVar)
    pub q_to_mvar: Option<f64>,
    /// Active power loss (MW)
    pub pl_mw: Option<f64>,
    /// Reactive power loss (MVar)
    pub ql_mvar: Option<f64>,
}

/// Transformer power flow result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrafoResult {
    /// Transformer ID
    pub id: i64,
    /// Active power HV side (MW)
    pub p_hv_mw: Option<f64>,
    /// Reactive power HV side (MVar)
    pub q_hv_mvar: Option<f64>,
    /// Active power LV side (MW)
    pub p_lv_mw: Option<f64>,
    /// Reactive power LV side (MVar)
    pub q_lv_mvar: Option<f64>,
    /// Active power loss (MW)
    pub pl_mw: Option<f64>,
    /// Reactive power loss (MVar)
    pub ql_mvar: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_pandapower_result() {
        let json = r#"{
            "converged": true,
            "buses": [
                {"id": 0, "vm_pu": 1.05, "va_degree": 0.0},
                {"id": 1, "vm_pu": 0.98, "va_degree": -2.5}
            ],
            "lines": [
                {"id": 0, "p_from_mw": 10.5, "q_from_mvar": 2.1, "p_to_mw": -10.3, "q_to_mvar": -2.0, "pl_mw": 0.2, "ql_mvar": 0.1}
            ],
            "trafos": [
                {"id": 0, "p_hv_mw": 50.0, "q_hv_mvar": 10.0, "p_lv_mw": -49.5, "q_lv_mvar": -9.8, "pl_mw": 0.5, "ql_mvar": 0.2}
            ],
            "total_loss_mw": 0.7,
            "total_loss_mvar": 0.3
        }"#;

        let result: PandapowerResult = serde_json::from_str(json).unwrap();
        assert!(result.converged);
        assert_eq!(result.buses.len(), 2);
        assert_eq!(result.lines.len(), 1);
        assert_eq!(result.trafos.len(), 1);
        assert!((result.total_loss_mw - 0.7).abs() < 0.001);
    }
}

use serde::{Deserialize, Serialize};

/// Full network topology data from pandapower/cnpower
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkTopologyData {
    /// Whether the power flow converged
    pub converged: bool,
    /// Base MVA
    pub base_mva: f64,
    /// Bus data
    pub buses: Vec<TopologyBus>,
    /// Branch data (lines + transformers)
    pub branches: Vec<TopologyBranch>,
    /// Shunt elements
    pub shunts: Vec<TopologyShunt>,
    /// Bus count
    pub bus_count: usize,
    /// Branch count
    pub branch_count: usize,
}

/// Bus in the network topology
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyBus {
    /// Bus ID
    pub id: i64,
    /// Bus name
    pub name: String,
    /// Nominal voltage (kV)
    pub vn_kv: f64,
    /// Bus type: "Slack", "PV", or "PQ"
    #[serde(rename = "type")]
    pub bus_type: String,
    /// Active generation (MW), if any
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p_gen_mw: Option<f64>,
    /// Reactive generation (MVar), if any
    #[serde(skip_serializing_if = "Option::is_none")]
    pub q_gen_mvar: Option<f64>,
    /// Active load (MW), if any
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p_load_mw: Option<f64>,
    /// Reactive load (MVar), if any
    #[serde(skip_serializing_if = "Option::is_none")]
    pub q_load_mvar: Option<f64>,
    /// Voltage magnitude from power flow (pu)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vm_pu: Option<f64>,
    /// Voltage angle from power flow (degrees)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub va_degree: Option<f64>,
}

/// Branch in the network topology (line or transformer)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyBranch {
    /// Branch ID
    pub id: i64,
    /// Branch type: "line" or "trafo"
    #[serde(rename = "type")]
    pub branch_type: String,
    /// From bus ID (HV bus for transformers)
    pub from_bus: i64,
    /// To bus ID (LV bus for transformers)
    pub to_bus: i64,
    /// Length in km (lines only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub length_km: Option<f64>,
    /// Resistance per km (Ohm/km, lines only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r_ohm_per_km: Option<f64>,
    /// Reactance per km (Ohm/km, lines only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x_ohm_per_km: Option<f64>,
    /// Capacitance per km (nF/km, lines only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub c_nf_per_km: Option<f64>,
    /// Maximum current (kA, lines only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_i_ka: Option<f64>,
    /// Rated power (MVA, transformers only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sn_mva: Option<f64>,
    /// Short circuit voltage (% , transformers only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vk_percent: Option<f64>,
    /// Resistive part of short circuit voltage (%, transformers only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vkr_percent: Option<f64>,
    /// Tap position (transformers only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tap_pos: Option<i64>,
    /// Whether the branch is in service
    pub in_service: bool,
}

/// Shunt element in the network topology
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyShunt {
    /// Shunt ID
    pub id: i64,
    /// Connected bus ID
    pub bus: i64,
    /// Reactive power (MVar)
    pub q_mvar: f64,
    /// Active power (MW)
    #[serde(default)]
    pub p_mw: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_network_topology() {
        let json = r#"{
            "converged": true,
            "base_mva": 1.0,
            "buses": [
                {"id": 0, "name": "Bus0", "vn_kv": 110.0, "type": "Slack", "vm_pu": 1.05, "va_degree": 0.0},
                {"id": 1, "name": "Bus1", "vn_kv": 110.0, "type": "PQ", "p_load_mw": 10.0, "q_load_mvar": 2.0, "vm_pu": 0.98, "va_degree": -2.5},
                {"id": 2, "name": "Bus2", "vn_kv": 10.0, "type": "PV", "p_gen_mw": 50.0, "q_gen_mvar": 10.0, "vm_pu": 1.02, "va_degree": -1.0}
            ],
            "branches": [
                {"id": 0, "type": "line", "from_bus": 0, "to_bus": 1, "length_km": 10.0, "r_ohm_per_km": 0.1, "x_ohm_per_km": 0.4, "c_nf_per_km": 10.0, "max_i_ka": 1.0, "in_service": true},
                {"id": 10000, "type": "trafo", "from_bus": 0, "to_bus": 2, "sn_mva": 40.0, "vk_percent": 10.0, "vkr_percent": 0.5, "tap_pos": 0, "in_service": true}
            ],
            "shunts": [
                {"id": 0, "bus": 1, "q_mvar": -5.0, "p_mw": 0.0}
            ],
            "bus_count": 3,
            "branch_count": 2
        }"#;

        let data: NetworkTopologyData = serde_json::from_str(json).unwrap();
        assert!(data.converged);
        assert_eq!(data.buses.len(), 3);
        assert_eq!(data.branches.len(), 2);
        assert_eq!(data.shunts.len(), 1);
        assert_eq!(data.buses[0].bus_type, "Slack");
        assert_eq!(data.buses[1].bus_type, "PQ");
        assert_eq!(data.buses[2].bus_type, "PV");
        assert_eq!(data.branches[0].branch_type, "line");
        assert_eq!(data.branches[1].branch_type, "trafo");
    }
}

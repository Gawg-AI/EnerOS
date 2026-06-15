use axum::extract::State;
use axum::Json;

use eneros_powerflow::ieee14;

use crate::app::AppState;
use crate::types::{ApiResponse, BranchData, BusData, TopologyDataResponse};

/// GET /api/topology
pub async fn topology_handler(
    State(state): State<AppState>,
) -> Json<ApiResponse<TopologyDataResponse>> {
    // If a topology engine is available, use its statistics
    if let Some(engine) = &state.topology_engine {
        let stats = engine.statistics();
        // TopologyEngine doesn't expose bus/branch details directly,
        // so return summary info
        let response = TopologyDataResponse {
            buses: Vec::new(),
            branches: Vec::new(),
            zones: vec![0; stats.zone_count],
        };
        return Json(ApiResponse::success(response));
    }

    // Default: return IEEE 14 bus topology data
    let data = ieee14();
    let mut zones = std::collections::HashSet::new();

    let buses: Vec<BusData> = data
        .buses
        .iter()
        .map(|b| {
            zones.insert(0u64); // IEEE 14 has a single zone by default
            BusData {
                id: b.bus_id as u64,
                name: format!("Bus {}", b.bus_id),
                zone_id: 0,
                voltage_kv: 138.0, // IEEE 14 is 138 kV nominal
            }
        })
        .collect();

    let branches: Vec<BranchData> = data
        .branches
        .iter()
        .enumerate()
        .map(|(i, b)| BranchData {
            id: i as u64,
            from_bus: b.from_bus as u64,
            to_bus: b.to_bus as u64,
            reactance: b.x_pu,
        })
        .collect();

    let response = TopologyDataResponse {
        buses,
        branches,
        zones: zones.into_iter().collect(),
    };

    Json(ApiResponse::success(response))
}

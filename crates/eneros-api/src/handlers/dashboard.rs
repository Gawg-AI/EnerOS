use std::collections::HashMap;
use axum::extract::State;
use axum::response::Html;
use axum::Json;
use chrono::Utc;

use eneros_dashboard::{
    topology_svg::{self, BusSvgData, BranchSvgData, TopologySvgConfig},
    flow_heatmap::{self, BusFlowData, BranchFlowData, FlowHeatmapConfig},
    agent_panel::{AgentDisplay, AgentPanelData},
    data_panel::{ReadingDisplay, DataPanelData},
    full_page,
};

use crate::app::AppState;
use crate::types::{ApiResponse, TopologySvgResponse, FlowHeatmapResponse};

/// Build SVG data from the PowerNetwork if available
fn build_svg_data_from_network(state: &AppState) -> (Vec<BusSvgData>, Vec<BranchSvgData>) {
    if state.network.is_some() {
        // When a PowerNetwork is available, use IEEE 14 bus data as the topology source
        // (the PowerNetwork is built from this data)
        let data = eneros_powerflow::ieee14();
        let buses: Vec<BusSvgData> = data.buses.iter().map(|b| {
            BusSvgData {
                id: b.bus_id as u64,
                name: format!("Bus {}", b.bus_id),
                x: 0.0, // will be set by circular_layout
                y: 0.0,
                zone_id: 0,
                voltage_level: format!("{:.0}kV", 138.0),
            }
        }).collect();
        let branches: Vec<BranchSvgData> = data.branches.iter().enumerate().map(|(i, b)| {
            BranchSvgData {
                id: i as u64,
                from_bus: b.from_bus as u64,
                to_bus: b.to_bus as u64,
                status: true,
            }
        }).collect();
        return (buses, branches);
    }

    // Default: empty
    (Vec::new(), Vec::new())
}

/// Build agent panel data from orchestrator if available
fn build_agent_panel_data(state: &AppState) -> AgentPanelData {
    if let Some(orchestrator) = &state.agent_orchestrator {
        let registered = orchestrator.registered_agents();
        let agents: Vec<AgentDisplay> = registered.iter().map(|(name, agent_type, authority)| {
            let type_str = match agent_type {
                eneros_agent::AgentType::Dispatcher => "Dispatcher",
                eneros_agent::AgentType::Operator => "Operator",
                eneros_agent::AgentType::Planner => "Planner",
                eneros_agent::AgentType::Trader => "Trader",
                eneros_agent::AgentType::Custom(ref s) => s,
            };
            let auth_str = match authority {
                eneros_core::AuthorityLevel::Emergency => "Emergency",
                eneros_core::AuthorityLevel::Supervisor => "Supervisor",
                eneros_core::AuthorityLevel::Operator => "Operator",
                eneros_core::AuthorityLevel::Observer => "Observer",
            };
            AgentDisplay {
                name: name.clone(),
                agent_type: type_str.to_string(),
                authority: auth_str.to_string(),
                status: "active".to_string(),
                last_action: None,
                last_action_time: None,
            }
        }).collect();
        let active_count = agents.iter().filter(|a| a.status == "active").count();
        return AgentPanelData {
            total_count: agents.len(),
            active_count,
            agents,
        };
    }

    // Default: empty
    AgentPanelData {
        agents: Vec::new(),
        total_count: 0,
        active_count: 0,
    }
}

/// Build data panel from SCADA collector if available
fn build_data_panel_data(state: &AppState) -> DataPanelData {
    if let Some(collector) = &state.scada_collector {
        let readings = collector.latest_all();
        let displays: Vec<ReadingDisplay> = readings.iter().map(|r| {
            ReadingDisplay {
                element_id: r.element_id,
                parameter: r.parameter.clone(),
                value: r.value,
                unit: "p.u.".to_string(),
                quality: format!("{:?}", r.quality),
            }
        }).collect();
        return DataPanelData {
            readings: displays,
            timestamp: Utc::now().to_rfc3339(),
        };
    }

    DataPanelData {
        readings: Vec::new(),
        timestamp: Utc::now().to_rfc3339(),
    }
}

/// GET / — serve the main dashboard HTML page
pub async fn dashboard_handler(
    State(state): State<AppState>,
) -> Html<String> {
    let (buses, branches) = build_svg_data_from_network(&state);
    let config = TopologySvgConfig::default();

    let layout_buses = topology_svg::circular_layout(&buses, &config);
    let svg = topology_svg::generate_topology_svg(&layout_buses, &branches, &config);

    let agent_data = build_agent_panel_data(&state);
    let data_panel = build_data_panel_data(&state);

    let page = full_page::generate_dashboard_page(&svg, &agent_data, &data_panel);
    Html(page)
}

/// GET /api/dashboard/topology-svg — return topology SVG
pub async fn topology_svg_handler(
    State(state): State<AppState>,
) -> Json<ApiResponse<TopologySvgResponse>> {
    let (buses, branches) = build_svg_data_from_network(&state);
    let bus_count = buses.len();
    let branch_count = branches.len();

    let config = TopologySvgConfig::default();
    let layout_buses = topology_svg::circular_layout(&buses, &config);
    let svg = topology_svg::generate_topology_svg(&layout_buses, &branches, &config);

    let response = TopologySvgResponse {
        svg,
        bus_count,
        branch_count,
    };
    Json(ApiResponse::success(response))
}

/// GET /api/dashboard/flow-heatmap — return flow heatmap data as JSON
pub async fn flow_heatmap_handler(
    State(state): State<AppState>,
) -> Json<ApiResponse<FlowHeatmapResponse>> {
    if let Some(network) = &state.network {
        match network.solve() {
            Ok(pf_result) => {
                let buses: Vec<BusFlowData> = pf_result.bus_results.iter().map(|b| {
                    BusFlowData {
                        id: b.bus_id,
                        v_pu: b.voltage_magnitude,
                    }
                }).collect();

                let branches: Vec<BranchFlowData> = pf_result.branch_results.iter().map(|b| {
                    BranchFlowData {
                        id: b.branch_id,
                        from_bus: b.from_bus,
                        to_bus: b.to_bus,
                        loading_percent: b.loading_percent,
                    }
                }).collect();

                let config = FlowHeatmapConfig::default();
                let overlay = flow_heatmap::generate_flow_overlay(&buses, &branches, &config);

                let response = FlowHeatmapResponse {
                    bus_colors: overlay.bus_colors,
                    branch_widths: overlay.branch_widths,
                    branch_colors: overlay.branch_colors,
                };
                return Json(ApiResponse::success(response));
            }
            Err(e) => {
                return Json(ApiResponse::error(format!("Power flow failed for heatmap: {}", e)));
            }
        }
    }

    // No network available — return empty data
    let response = FlowHeatmapResponse {
        bus_colors: HashMap::new(),
        branch_widths: HashMap::new(),
        branch_colors: HashMap::new(),
    };
    Json(ApiResponse::success(response))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppState;

    #[test]
    fn test_build_svg_data_empty_state() {
        let state = AppState::new();
        let (buses, branches) = build_svg_data_from_network(&state);
        assert!(buses.is_empty());
        assert!(branches.is_empty());
    }

    #[test]
    fn test_build_agent_panel_empty_state() {
        let state = AppState::new();
        let data = build_agent_panel_data(&state);
        assert_eq!(data.total_count, 0);
        assert!(data.agents.is_empty());
    }

    #[test]
    fn test_build_data_panel_empty_state() {
        let state = AppState::new();
        let data = build_data_panel_data(&state);
        assert!(data.readings.is_empty());
    }

    #[test]
    fn test_topology_svg_response_serialization() {
        let response = TopologySvgResponse {
            svg: "<svg></svg>".to_string(),
            bus_count: 14,
            branch_count: 20,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"bus_count\":14"));
        assert!(json.contains("\"branch_count\":20"));
    }

    #[test]
    fn test_flow_heatmap_response_serialization() {
        let mut bus_colors = HashMap::new();
        bus_colors.insert(1u64, "#00ff00".to_string());
        let response = FlowHeatmapResponse {
            bus_colors,
            branch_widths: HashMap::new(),
            branch_colors: HashMap::new(),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("bus_colors"));
    }
}

use axum::extract::State;
use axum::Json;

use crate::app::AppState;
use crate::types::{AgentInfo, AgentsResponse, ApiResponse};

/// Known agent types in the EnerOS system (fallback when no orchestrator is available)
const KNOWN_AGENT_TYPES: &[(&str, &str, &str)] = &[
    ("DispatchAgent", "Dispatcher", "System"),
    ("OperationAgent", "Operator", "Zone"),
    ("SelfHealingAgent", "Operator", "Zone"),
    ("PlanningAgent", "Planner", "System"),
    ("TradingAgent", "Trader", "Market"),
];

/// GET /api/agents
#[utoipa::path(
    get,
    path = "/api/agents",
    responses(
        (status = 200, description = "已注册的 Agent 列表", body = AgentsResponse),
    )
)]
/// Agent 列表查询 handler (T029-18: 添加 tracing span 用于 OTLP 导出)
#[tracing::instrument(skip(state), fields(endpoint = "/api/agents"))]
pub async fn agents_handler(State(state): State<AppState>) -> Json<ApiResponse<AgentsResponse>> {
    // If agent_orchestrator is available, query registered agents
    if let Some(orchestrator) = &state.agent_orchestrator {
        let registered = orchestrator.registered_agents();
        let agents: Vec<AgentInfo> = registered
            .iter()
            .map(|(name, agent_type, authority)| {
                let type_str = match agent_type {
                    eneros_runtime::agent::AgentType::Dispatcher => "Dispatcher",
                    eneros_runtime::agent::AgentType::Operator => "Operator",
                    eneros_runtime::agent::AgentType::Planner => "Planner",
                    eneros_runtime::agent::AgentType::Trader => "Trader",
                    eneros_runtime::agent::AgentType::Custom(ref s) => s,
                };
                let auth_str = match authority {
                    eneros_core::AuthorityLevel::Emergency => "Emergency",
                    eneros_core::AuthorityLevel::Supervisor => "Supervisor",
                    eneros_core::AuthorityLevel::Operator => "Operator",
                    eneros_core::AuthorityLevel::Observer => "Observer",
                };
                AgentInfo {
                    name: name.clone(),
                    agent_type: type_str.to_string(),
                    authority: auth_str.to_string(),
                    status: "active".to_string(),
                }
            })
            .collect();

        let response = AgentsResponse {
            agent_count: agents.len(),
            agents,
        };
        return Json(ApiResponse::success(response));
    }

    // Fallback: return placeholder list based on known agent types
    let agents: Vec<AgentInfo> = KNOWN_AGENT_TYPES
        .iter()
        .map(|(name, agent_type, authority)| AgentInfo {
            name: name.to_string(),
            agent_type: agent_type.to_string(),
            authority: authority.to_string(),
            status: "available".to_string(),
        })
        .collect();

    let response = AgentsResponse {
        agent_count: agents.len(),
        agents,
    };

    Json(ApiResponse::success(response))
}

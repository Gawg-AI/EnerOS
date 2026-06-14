use serde::{Deserialize, Serialize};

/// Agent display data for the status panel
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDisplay {
    pub name: String,
    pub agent_type: String,
    pub authority: String,
    pub status: String,
    pub last_action: Option<String>,
    pub last_action_time: Option<String>,
}

/// Agent panel data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentPanelData {
    pub agents: Vec<AgentDisplay>,
    pub total_count: usize,
    pub active_count: usize,
}

fn status_color(status: &str) -> &'static str {
    match status.to_lowercase().as_str() {
        "active" => "#00cc00",
        "idle" => "#888888",
        "error" => "#cc0000",
        _ => "#cccccc",
    }

}

/// Generate an HTML fragment for the agent status panel.
pub fn generate_agent_panel_html(data: &AgentPanelData) -> String {
    let mut html = String::new();

    html.push_str("<div class=\"agent-panel\">\n");
    html.push_str(&format!(
        "  <h3>Agent Status ({} total, {} active)</h3>\n",
        data.total_count, data.active_count
    ));
    html.push_str("  <table class=\"data-table\">\n");
    html.push_str("    <thead><tr><th>Name</th><th>Type</th><th>Authority</th><th>Status</th><th>Last Action</th></tr></thead>\n");
    html.push_str("    <tbody>\n");

    for agent in &data.agents {
        let color = status_color(&agent.status);
        let last_action = agent
            .last_action
            .as_deref()
            .unwrap_or("—");
        html.push_str(&format!(
            "      <tr><td>{}</td><td>{}</td><td>{}</td><td><span class=\"status-dot\" style=\"color:{}\">●</span> {}</td><td>{}</td></tr>\n",
            agent.name, agent.agent_type, agent.authority, color, agent.status, last_action
        ));
    }

    html.push_str("    </tbody>\n");
    html.push_str("  </table>\n");
    html.push_str("</div>");

    html
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_agent_panel_html() {
        let data = AgentPanelData {
            agents: vec![
                AgentDisplay {
                    name: "DispatchAgent".to_string(),
                    agent_type: "Dispatch".to_string(),
                    authority: "High".to_string(),
                    status: "active".to_string(),
                    last_action: Some("Re-dispatched gen".to_string()),
                    last_action_time: Some("2025-01-01T12:00:00Z".to_string()),
                },
                AgentDisplay {
                    name: "ForecastAgent".to_string(),
                    agent_type: "Forecast".to_string(),
                    authority: "Medium".to_string(),
                    status: "idle".to_string(),
                    last_action: None,
                    last_action_time: None,
                },
            ],
            total_count: 2,
            active_count: 1,
        };
        let html = generate_agent_panel_html(&data);

        assert!(html.contains("Agent Status"));
        assert!(html.contains("2 total"));
        assert!(html.contains("1 active"));
        assert!(html.contains("DispatchAgent"));
        assert!(html.contains("ForecastAgent"));
        assert!(html.contains("active"));
        assert!(html.contains("idle"));
        assert!(html.contains("<table"));
        assert!(html.contains("</table>"));
        assert!(html.contains("<thead>"));
        assert!(html.contains("<tbody>"));
    }

    #[test]
    fn test_status_color_active() {
        assert_eq!(status_color("active"), "#00cc00");
    }

    #[test]
    fn test_status_color_idle() {
        assert_eq!(status_color("idle"), "#888888");
    }

    #[test]
    fn test_status_color_error() {
        assert_eq!(status_color("error"), "#cc0000");
    }

    #[test]
    fn test_status_color_unknown() {
        assert_eq!(status_color("unknown"), "#cccccc");
    }

    #[test]
    fn test_empty_agents() {
        let data = AgentPanelData {
            agents: vec![],
            total_count: 0,
            active_count: 0,
        };
        let html = generate_agent_panel_html(&data);
        assert!(html.contains("0 total"));
        assert!(html.contains("0 active"));
    }
}

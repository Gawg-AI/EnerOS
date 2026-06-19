use crate::agent_panel::AgentPanelData;
use crate::assets;
use crate::data_panel::DataPanelData;

/// Generate a complete HTML dashboard page combining all components.
pub fn generate_dashboard_page(
    topology_svg: &str,
    flow_heatmap_svg: &str,
    agent_data: &AgentPanelData,
    data_panel: &DataPanelData,
) -> String {
    let agent_html = crate::agent_panel::generate_agent_panel_html(agent_data);
    let data_html = crate::data_panel::generate_data_panel_html(data_panel);
    let css = assets::get_style_css();
    let js = assets::get_app_js();

    format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>EnerOS Dashboard</title>
  <style>{css}</style>
</head>
<body>
  <header>
    <h1>EnerOS Dashboard</h1>
    <span id="connection-status" class="status-connected">Connected</span>
  </header>
  <main class="dashboard-grid">
    <section class="panel" id="topology-panel">
      <h2>Topology</h2>
      <div id="topology-svg-container" class="svg-container">{topology_svg}</div>
    </section>
    <section class="panel" id="flow-panel">
      <h2>Power Flow Heatmap</h2>
      <div id="flow-svg-container" class="svg-container">{flow_heatmap_svg}</div>
    </section>
    <section class="panel" id="agent-panel">
      <h2>Agent Status</h2>
      <div id="agent-content">{agent_html}</div>
    </section>
    <section class="panel" id="data-panel">
      <h2>Real-Time Data</h2>
      <div id="data-content">{data_html}</div>
    </section>
  </main>
  <script>{js}</script>
</body>
</html>"##,
        css = css,
        topology_svg = topology_svg,
        flow_heatmap_svg = flow_heatmap_svg,
        agent_html = agent_html,
        data_html = data_html,
        js = js,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_panel::AgentDisplay;
    use crate::data_panel::ReadingDisplay;

    #[test]
    fn test_generate_dashboard_page() {
        let topology_svg = "<svg><circle cx=\"100\" cy=\"100\" r=\"10\"/></svg>";
        let flow_svg = "<svg><circle cx=\"50\" cy=\"50\" r=\"5\"/></svg>";
        let agent_data = AgentPanelData {
            agents: vec![AgentDisplay {
                name: "TestAgent".to_string(),
                agent_type: "Test".to_string(),
                authority: "Low".to_string(),
                status: "active".to_string(),
                last_action: Some("did something".to_string()),
                last_action_time: None,
            }],
            total_count: 1,
            active_count: 1,
        };
        let data_panel = DataPanelData {
            readings: vec![ReadingDisplay {
                element_id: 1,
                parameter: "Voltage".to_string(),
                value: 1.05,
                unit: "p.u.".to_string(),
                quality: "Good".to_string(),
            }],
            timestamp: "2025-01-01T12:00:00Z".to_string(),
        };

        let page = generate_dashboard_page(topology_svg, flow_svg, &agent_data, &data_panel);

        assert!(page.contains("<!DOCTYPE html>"));
        assert!(page.contains("EnerOS Dashboard"));
        assert!(page.contains("<svg>"));
        assert!(page.contains("TestAgent"));
        assert!(page.contains("Voltage"));
        assert!(page.contains("<style>"));
        assert!(page.contains("<script>"));
        assert!(page.contains("dashboard-grid"));
    }

    #[test]
    fn test_dashboard_page_contains_all_panels() {
        let topology_svg = "<svg></svg>";
        let flow_svg = "<svg></svg>";
        let agent_data = AgentPanelData {
            agents: vec![],
            total_count: 0,
            active_count: 0,
        };
        let data_panel = DataPanelData {
            readings: vec![],
            timestamp: "now".to_string(),
        };

        let page = generate_dashboard_page(topology_svg, flow_svg, &agent_data, &data_panel);

        assert!(page.contains("topology-panel"));
        assert!(page.contains("flow-panel"));
        assert!(page.contains("agent-panel"));
        assert!(page.contains("data-panel"));
    }
}

/// Return the main dashboard HTML page as a static string.
pub fn get_index_html() -> &'static str {
    INDEX_HTML
}

/// Return the CSS styles as a static string.
pub fn get_style_css() -> &'static str {
    STYLE_CSS
}

/// Return the JavaScript for WebSocket and dynamic updates as a static string.
pub fn get_app_js() -> &'static str {
    APP_JS
}

static INDEX_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>EnerOS Dashboard</title>
  <link rel="stylesheet" href="/style.css">
</head>
<body>
  <header>
    <h1>EnerOS Dashboard</h1>
    <span id="connection-status" class="status-disconnected">Disconnected</span>
  </header>
  <main class="dashboard-grid">
    <section class="panel" id="topology-panel">
      <h2>Topology</h2>
      <div id="topology-svg-container" class="svg-container"></div>
    </section>
    <section class="panel" id="flow-panel">
      <h2>Power Flow Heatmap</h2>
      <div id="flow-svg-container" class="svg-container"></div>
    </section>
    <section class="panel" id="agent-panel">
      <h2>Agent Status</h2>
      <div id="agent-content"></div>
    </section>
    <section class="panel" id="data-panel">
      <h2>Real-Time Data</h2>
      <div id="data-content"></div>
    </section>
  </main>
  <script src="/app.js"></script>
</body>
</html>"##;

static STYLE_CSS: &str = r##"/* EnerOS Dashboard - Dark Power Industry Theme */
* {
  margin: 0;
  padding: 0;
  box-sizing: border-box;
}

body {
  font-family: 'Segoe UI', Tahoma, Geneva, Verdana, sans-serif;
  background-color: #0d1117;
  color: #c9d1d9;
  min-height: 100vh;
}

header {
  background-color: #161b22;
  padding: 16px 24px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  border-bottom: 1px solid #30363d;
}

header h1 {
  font-size: 1.5rem;
  color: #58a6ff;
}

#connection-status {
  font-size: 0.85rem;
  padding: 4px 12px;
  border-radius: 12px;
}

.status-connected {
  background-color: #0d419d;
  color: #58a6ff;
}

.status-disconnected {
  background-color: #490202;
  color: #f85149;
}

.dashboard-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  grid-template-rows: 1fr 1fr;
  gap: 16px;
  padding: 16px;
  height: calc(100vh - 70px);
}

.panel {
  background-color: #161b22;
  border: 1px solid #30363d;
  border-radius: 8px;
  padding: 16px;
  overflow: auto;
}

.panel h2 {
  font-size: 1.1rem;
  color: #58a6ff;
  margin-bottom: 12px;
  border-bottom: 1px solid #30363d;
  padding-bottom: 8px;
}

.svg-container {
  width: 100%;
  height: calc(100% - 40px);
  display: flex;
  align-items: center;
  justify-content: center;
}

.svg-container svg {
  max-width: 100%;
  max-height: 100%;
}

.data-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 0.85rem;
}

.data-table thead th {
  text-align: left;
  padding: 8px 6px;
  border-bottom: 2px solid #30363d;
  color: #8b949e;
  font-weight: 600;
}

.data-table tbody td {
  padding: 6px;
  border-bottom: 1px solid #21262d;
}

.data-table tbody tr:hover {
  background-color: #1c2128;
}

.status-dot {
  font-size: 0.75rem;
}

.agent-panel h3,
.data-panel h3 {
  font-size: 0.95rem;
  color: #c9d1d9;
  margin-bottom: 8px;
}

@media (max-width: 768px) {
  .dashboard-grid {
    grid-template-columns: 1fr;
    grid-template-rows: auto;
    height: auto;
  }
}"##;

static APP_JS: &str = r##"(function() {
  'use strict';

  var ws = null;
  var reconnectDelay = 1000;

  function connectWebSocket() {
    var protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
    var wsUrl = protocol + '//' + location.host + '/ws';
    ws = new WebSocket(wsUrl);

    ws.onopen = function() {
      document.getElementById('connection-status').textContent = 'Connected';
      document.getElementById('connection-status').className = 'status-connected';
      reconnectDelay = 1000;
    };

    ws.onclose = function() {
      document.getElementById('connection-status').textContent = 'Disconnected';
      document.getElementById('connection-status').className = 'status-disconnected';
      setTimeout(function() {
        reconnectDelay = Math.min(reconnectDelay * 2, 30000);
        connectWebSocket();
      }, reconnectDelay);
    };

    ws.onerror = function() {
      ws.close();
    };

    ws.onmessage = function(event) {
      try {
        var msg = JSON.parse(event.data);
        handleMessage(msg);
      } catch(e) {
        console.error('Failed to parse WebSocket message:', e);
      }
    };
  }

  function handleMessage(msg) {
    if (msg.type === 'topology') {
      document.getElementById('topology-svg-container').innerHTML = msg.svg || '';
    } else if (msg.type === 'flow') {
      document.getElementById('flow-svg-container').innerHTML = msg.svg || '';
    } else if (msg.type === 'agents') {
      document.getElementById('agent-content').innerHTML = msg.html || '';
    } else if (msg.type === 'data') {
      document.getElementById('data-content').innerHTML = msg.html || '';
    } else if (msg.type === 'event') {
      // EventBus → WS bridge events (v0.6.0 S7)
      // Refresh the relevant panel based on event_type
      var eventType = msg.event_type || '';
      if (eventType === 'DataReceived' || eventType === 'DeviceConnected' || eventType === 'DeviceDisconnected') {
        // SCADA data changed — refresh data panel
        fetchJSON('/api/scada/latest', function(data) {
          if (data.data && data.data.readings) {
            renderScadaData(data.data.readings);
          }
        });
      } else if (eventType === 'ConstraintViolation' || eventType === 'SystemAlarm') {
        // Constraint/alarm event — refresh agents and flow heatmap
        fetchJSON('/api/agents', function(data) {
          if (data.data && data.data.agents) {
            renderAgents(data.data.agents);
          }
        });
        fetchJSON('/api/dashboard/flow-heatmap', function(data) {
          if (data.data) {
            applyFlowOverlay(data.data);
          }
        });
        // Show alarm banner
        showAlarmBanner(msg);
      } else if (eventType === 'AgentDecision') {
        // Agent made a decision — refresh agent panel
        fetchJSON('/api/agents', function(data) {
          if (data.data && data.data.agents) {
            renderAgents(data.data.agents);
          }
        });
      }
    }
  }

  function showAlarmBanner(msg) {
    var banner = document.getElementById('alarm-banner');
    if (!banner) {
      banner = document.createElement('div');
      banner.id = 'alarm-banner';
      banner.style.cssText = 'position:fixed;top:0;left:0;right:0;background:#cc0000;color:#fff;padding:8px;text-align:center;z-index:9999;font-weight:bold;';
      document.body.insertBefore(banner, document.body.firstChild);
    }
    var detail = msg.payload ? (msg.payload.message || msg.payload.Message || JSON.stringify(msg.payload)) : '';
    banner.innerHTML = '⚠ ' + (msg.event_type || 'Alarm') + ': ' + detail;
    banner.style.display = 'block';
    setTimeout(function() { banner.style.display = 'none'; }, 10000);
  }

  function fetchJSON(url, callback) {
    fetch(url)
      .then(function(res) { return res.json(); })
      .then(function(data) { callback(data); })
      .catch(function(err) { console.error('Fetch error:', url, err); });
  }

  function refreshData() {
    fetchJSON('/api/dashboard/topology-svg', function(data) {
      if (data.data && data.data.svg) {
        document.getElementById('topology-svg-container').innerHTML = data.data.svg;
      }
    });
    fetchJSON('/api/dashboard/flow-heatmap', function(data) {
      if (data.data) {
        // Flow heatmap returns colors/widths, not SVG.
        // Apply overlay to the existing topology SVG.
        applyFlowOverlay(data.data);
      }
    });
    fetchJSON('/api/agents', function(data) {
      if (data.data && data.data.agents) {
        renderAgents(data.data.agents);
      }
    });
    fetchJSON('/api/scada/latest', function(data) {
      if (data.data && data.data.readings) {
        renderScadaData(data.data.readings);
      }
    });
  }

  function applyFlowOverlay(flowData) {
    // Update branch colors/widths and bus colors via CSS on existing SVG
    if (flowData.branch_colors) {
      Object.keys(flowData.branch_colors).forEach(function(id) {
        var el = document.querySelector('[data-branch-id="' + id + '"]');
        if (el) { el.setAttribute('stroke', flowData.branch_colors[id]); }
      });
    }
    if (flowData.branch_widths) {
      Object.keys(flowData.branch_widths).forEach(function(id) {
        var el = document.querySelector('[data-branch-id="' + id + '"]');
        if (el) { el.setAttribute('stroke-width', flowData.branch_widths[id]); }
      });
    }
    if (flowData.bus_colors) {
      Object.keys(flowData.bus_colors).forEach(function(id) {
        var el = document.querySelector('[data-bus-id="' + id + '"]');
        if (el) { el.setAttribute('fill', flowData.bus_colors[id]); }
      });
    }
  }

  function renderAgents(agents) {
    var html = '<div class="agent-panel"><h3>Agents (' + agents.length + ')</h3>';
    html += '<table class="data-table"><thead><tr><th>Name</th><th>Type</th><th>Authority</th><th>Status</th></tr></thead><tbody>';
    agents.forEach(function(a) {
      html += '<tr><td>' + (a.name || '') + '</td><td>' + (a.agent_type || '') + '</td><td>' + (a.authority || '') + '</td><td>' + (a.status || '') + '</td></tr>';
    });
    html += '</tbody></table></div>';
    document.getElementById('agent-content').innerHTML = html;
  }

  function renderScadaData(readings) {
    var html = '<div class="data-panel"><h3>Real-Time Data</h3>';
    html += '<table class="data-table"><thead><tr><th>Element</th><th>Parameter</th><th>Value</th><th>Unit</th><th>Quality</th></tr></thead><tbody>';
    readings.forEach(function(r) {
      var color = r.quality === 'Good' ? '#00cc00' : (r.quality === 'Uncertain' ? '#cccc00' : '#cc0000');
      html += '<tr><td>' + r.element_id + '</td><td>' + (r.parameter || '') + '</td><td>' + (r.value != null ? r.value.toFixed(3) : '') + '</td><td>' + (r.unit || '') + '</td><td><span style="color:' + color + '">\u25CF</span> ' + (r.quality || '') + '</td></tr>';
    });
    html += '</tbody></table></div>';
    document.getElementById('data-content').innerHTML = html;
  }

  connectWebSocket();
  refreshData();
  setInterval(refreshData, 5000);
})();
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_index_html() {
        let html = get_index_html();
        assert!(html.contains("EnerOS Dashboard"));
        assert!(html.contains("topology-panel"));
        assert!(html.contains("flow-panel"));
        assert!(html.contains("agent-panel"));
        assert!(html.contains("data-panel"));
        assert!(html.contains("<!DOCTYPE html>"));
    }

    #[test]
    fn test_get_style_css() {
        let css = get_style_css();
        assert!(css.contains("dashboard-grid"));
        assert!(css.contains("data-table"));
        assert!(css.contains("#0d1117")); // dark background
    }

    #[test]
    fn test_get_app_js() {
        let js = get_app_js();
        assert!(js.contains("WebSocket"));
        assert!(js.contains("/api/"));
        assert!(js.contains("setInterval"));
        assert!(js.contains("5000"));
    }
}

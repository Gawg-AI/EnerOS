//! Agent 决策验证场景。
//!
//! 验证 Agent 控制和查询端点：
//! - `GET /api/agents` 返回 agent 列表
//! - `POST /api/agents/{id}/control { action: "status" }` 返回 agent 状态

use anyhow::{Context, Result};

use crate::cluster::TestCluster;

/// 验证 `/api/agents` 返回 agent 列表。
pub async fn agents_list(cluster: &TestCluster) -> Result<()> {
    let url = format!("{}/api/agents", cluster.api_endpoint());
    let resp = reqwest::get(&url)
        .await
        .context("failed to GET /api/agents")?;
    assert!(
        resp.status().is_success(),
        "agents list failed: status={}",
        resp.status()
    );

    let body: serde_json::Value = resp.json().await.context("failed to parse agents JSON")?;
    let data = body.get("data").context("missing 'data' field")?;
    let agents = data
        .get("agents")
        .and_then(|v| v.as_array())
        .context("missing 'agents' array")?;
    assert!(
        !agents.is_empty(),
        "expected at least 1 agent"
    );
    Ok(())
}

/// 验证 `POST /api/agents/{id}/control` 查询 agent 状态。
pub async fn agent_status_control(cluster: &TestCluster) -> Result<()> {
    // 先获取 agent 列表，取第一个 agent 的 name 作为 ID
    let list_url = format!("{}/api/agents", cluster.api_endpoint());
    let resp = reqwest::get(&list_url)
        .await
        .context("failed to GET /api/agents")?;
    let body: serde_json::Value = resp.json().await.context("failed to parse agents JSON")?;
    let agents = body
        .get("data")
        .and_then(|d| d.get("agents"))
        .and_then(|a| a.as_array())
        .context("missing agents array")?;

    let agent_name = agents
        .first()
        .and_then(|a| a.get("name"))
        .and_then(|n| n.as_str())
        .context("no agent found to control")?;

    // 发送 status 控制命令
    let control_url = format!("{}/api/agents/{}/control", cluster.api_endpoint(), agent_name);
    let client = reqwest::Client::new();
    let resp = client
        .post(&control_url)
        .json(&serde_json::json!({ "action": "status" }))
        .send()
        .await
        .context("failed to POST /api/agents/{id}/control")?;

    assert!(
        resp.status().is_success(),
        "agent control failed: status={}",
        resp.status()
    );

    let body: serde_json::Value = resp.json().await.context("failed to parse control JSON")?;
    // 响应应包含 agent_id 和 current_state
    assert!(
        body.get("agent_id").is_some() || body.get("data").is_some(),
        "agent control response missing agent_id or data"
    );
    Ok(())
}

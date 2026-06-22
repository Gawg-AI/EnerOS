//! 单节点启动验证场景。
//!
//! 验证集群启动后核心 API 端点可用：
//! - `/health` 返回 200
//! - `/api/agents` 返回已注册 agent 列表
//! - `/api/topology` 返回电网拓扑

use anyhow::{Context, Result};

use crate::cluster::TestCluster;

/// 验证 `/health` 端点返回 200。
pub async fn health_check(cluster: &TestCluster) -> Result<()> {
    let url = format!("{}/health", cluster.api_endpoint());
    let resp = reqwest::get(&url)
        .await
        .context("failed to GET /health")?;
    assert!(
        resp.status().is_success(),
        "health check failed: status={}",
        resp.status()
    );
    Ok(())
}

/// 验证 `/api/agents` 返回已注册 agent 列表。
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
    // API 返回 { success: true, data: { agent_count: N, agents: [...] } }
    let data = body.get("data").context("missing 'data' field in agents response")?;
    let agent_count = data
        .get("agent_count")
        .and_then(|v| v.as_u64())
        .context("missing 'agent_count' field")?;
    assert!(
        agent_count > 0,
        "expected at least 1 registered agent, got {}",
        agent_count
    );
    Ok(())
}

/// 验证 `/api/topology` 返回电网拓扑。
pub async fn topology(cluster: &TestCluster) -> Result<()> {
    let url = format!("{}/api/topology", cluster.api_endpoint());
    let resp = reqwest::get(&url)
        .await
        .context("failed to GET /api/topology")?;
    assert!(
        resp.status().is_success(),
        "topology failed: status={}",
        resp.status()
    );

    let body: serde_json::Value = resp.json().await.context("failed to parse topology JSON")?;
    // 拓扑响应应包含 bus 或 branch 信息
    let data = body.get("data").context("missing 'data' field in topology response")?;
    assert!(
        data.is_object(),
        "topology data should be an object"
    );
    Ok(())
}

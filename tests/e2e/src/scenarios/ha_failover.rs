//! HA 故障切换验证场景（简化版）。
//!
//! 完整 HA 需要多节点集群，此处简化为配置验证 + 状态查询：
//! - 验证 `/health` 返回 200（集群正常运行）
//! - 验证 `/api/agents` 返回 agent 列表（Agent 子系统就绪）
//!
//! 这验证了 HA 配置加载和心跳启动的基础路径。

use anyhow::{Context, Result};

use crate::cluster::TestCluster;

/// 验证 HA 配置加载和基础状态查询。
pub async fn ha_config_and_status(cluster: &TestCluster) -> Result<()> {
    // 1. 验证集群健康（HA 心跳基础）
    let health_url = format!("{}/health", cluster.api_endpoint());
    let resp = reqwest::get(&health_url)
        .await
        .context("failed to GET /health")?;
    assert!(
        resp.status().is_success(),
        "HA health check failed: status={}",
        resp.status()
    );

    // 2. 验证 Agent 子系统就绪（HA 切换的基础）
    let agents_url = format!("{}/api/agents", cluster.api_endpoint());
    let resp = reqwest::get(&agents_url)
        .await
        .context("failed to GET /api/agents")?;
    assert!(
        resp.status().is_success(),
        "HA agents query failed: status={}",
        resp.status()
    );

    let body: serde_json::Value = resp.json().await.context("failed to parse agents JSON")?;
    let data = body.get("data").context("missing 'data' field")?;
    let agent_count = data
        .get("agent_count")
        .and_then(|v| v.as_u64())
        .context("missing 'agent_count'")?;
    assert!(
        agent_count > 0,
        "HA failover requires at least 1 agent, got {}",
        agent_count
    );
    Ok(())
}

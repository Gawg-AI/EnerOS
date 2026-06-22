//! 命令下发验证场景。
//!
//! 验证结构化命令下发和审计日志查询：
//! - `POST /api/actions/structured` 下发结构化命令
//! - `GET /api/audit` 查询审计日志

use anyhow::{Context, Result};

use crate::cluster::TestCluster;

/// 验证 `POST /api/actions/structured` 下发结构化命令。
///
/// 使用 `NotifyAgent` 动作类型，它不需要真实设备连接，
/// 适合端到端验证命令管线的连通性。
pub async fn structured_action(cluster: &TestCluster) -> Result<()> {
    let url = format!("{}/api/actions/structured", cluster.api_endpoint());
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "action": {
                "NotifyAgent": {
                    "agent_id": "dispatch-1",
                    "message": "e2e test notification"
                }
            },
            "authority": "Operator",
            "system_state": "Normal"
        }))
        .send()
        .await
        .context("failed to POST /api/actions/structured")?;

    assert!(
        resp.status().is_success(),
        "structured action failed: status={}",
        resp.status()
    );

    let body: serde_json::Value = resp.json().await.context("failed to parse action JSON")?;
    // 响应应包含 executed 或 verdict 字段
    assert!(
        body.get("executed").is_some()
            || body.get("data").and_then(|d| d.get("executed")).is_some(),
        "structured action response missing 'executed' field"
    );
    Ok(())
}

/// 验证 `GET /api/audit` 查询审计日志。
pub async fn audit_query(cluster: &TestCluster) -> Result<()> {
    let url = format!("{}/api/audit", cluster.api_endpoint());
    let resp = reqwest::get(&url)
        .await
        .context("failed to GET /api/audit")?;
    assert!(
        resp.status().is_success(),
        "audit query failed: status={}",
        resp.status()
    );

    let body: serde_json::Value = resp.json().await.context("failed to parse audit JSON")?;
    // 审计响应应是有效 JSON 对象
    assert!(
        body.is_object(),
        "audit response should be a JSON object"
    );
    Ok(())
}

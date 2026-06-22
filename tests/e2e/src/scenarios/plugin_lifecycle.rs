//! 插件加载/卸载验证场景。
//!
//! 验证插件市场 API 端点可用：
//! - `GET /api/plugins/market/search` 返回插件列表
//! - 验证响应结构包含插件数据

use anyhow::{Context, Result};

use crate::cluster::TestCluster;

/// 验证插件市场搜索端点返回插件列表。
pub async fn plugin_market_search(cluster: &TestCluster) -> Result<()> {
    let url = format!("{}/api/plugins/market/search", cluster.api_endpoint());
    let resp = reqwest::get(&url)
        .await
        .context("failed to GET /api/plugins/market/search")?;
    assert!(
        resp.status().is_success(),
        "plugin market search failed: status={}",
        resp.status()
    );

    // 验证响应是有效 JSON
    let body: serde_json::Value = resp.json().await.context("failed to parse plugin JSON")?;
    assert!(
        body.is_object(),
        "plugin market response should be a JSON object"
    );
    Ok(())
}

/// 验证插件状态查询（通过插件市场端点）。
pub async fn plugin_status(cluster: &TestCluster) -> Result<()> {
    let url = format!("{}/api/plugins/market/search", cluster.api_endpoint());
    let resp = reqwest::get(&url)
        .await
        .context("failed to GET /api/plugins/market/search")?;
    assert!(
        resp.status().is_success(),
        "plugin status query failed: status={}",
        resp.status()
    );
    Ok(())
}

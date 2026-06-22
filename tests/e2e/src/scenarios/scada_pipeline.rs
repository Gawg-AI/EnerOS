//! SCADA 采集验证场景。
//!
//! 验证 SCADA 数据管线端到端可用：
//! - `GET /api/scada/latest` 返回最新采集数据
//! - 验证响应包含测点读数

use anyhow::{Context, Result};

use crate::cluster::TestCluster;

/// 验证 `/api/scada/latest` 返回最新 SCADA 数据。
pub async fn scada_latest(cluster: &TestCluster) -> Result<()> {
    let url = format!("{}/api/scada/latest", cluster.api_endpoint());
    let resp = reqwest::get(&url)
        .await
        .context("failed to GET /api/scada/latest")?;
    assert!(
        resp.status().is_success(),
        "scada latest failed: status={}",
        resp.status()
    );

    let body: serde_json::Value = resp.json().await.context("failed to parse scada JSON")?;
    let data = body.get("data").context("missing 'data' field in scada response")?;
    // 响应应包含 readings 数组
    let readings = data
        .get("readings")
        .context("missing 'readings' field in scada response")?;
    assert!(
        readings.is_array(),
        "scada readings should be an array"
    );
    Ok(())
}

/// 验证 SCADA 测点列表（通过 latest 端点的 readings 结构）。
pub async fn scada_points(cluster: &TestCluster) -> Result<()> {
    let url = format!("{}/api/scada/latest", cluster.api_endpoint());
    let resp = reqwest::get(&url)
        .await
        .context("failed to GET /api/scada/latest")?;
    assert!(
        resp.status().is_success(),
        "scada points query failed: status={}",
        resp.status()
    );

    let body: serde_json::Value = resp.json().await.context("failed to parse scada JSON")?;
    let data = body.get("data").context("missing 'data' field")?;
    let readings = data
        .get("readings")
        .and_then(|v| v.as_array())
        .context("missing 'readings' array")?;
    // 每个 reading 应包含 element_id 和 parameter 字段
    for reading in readings {
        assert!(
            reading.get("element_id").is_some(),
            "scada reading missing 'element_id'"
        );
        assert!(
            reading.get("parameter").is_some(),
            "scada reading missing 'parameter'"
        );
    }
    Ok(())
}

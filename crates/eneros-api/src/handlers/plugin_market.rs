//! 插件市场 API handlers (v0.28.0 — Task 17).
//!
//! 提供远程插件市场的搜索与安装端点：
//! - `GET /api/plugins/market/search?q=xxx` — 搜索远程插件市场
//! - `POST /api/plugins/market/install` — 安装插件（下载到本地缓存）

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use eneros_runtime::plugin::PluginIndexEntry;

use crate::app::AppState;

/// `GET /api/plugins/market/search` 查询参数
#[derive(Debug, Deserialize, IntoParams)]
pub struct MarketSearchQuery {
    /// 搜索关键词（匹配插件名称、描述、作者）
    pub q: String,
}

/// `GET /api/plugins/market/search` 响应体中的单个结果条目
///
/// 镜像 `eneros_runtime::plugin::PluginIndexEntry`，添加 `repo` 字段标识来源仓库。
#[derive(Debug, Serialize, ToSchema)]
pub struct MarketSearchResultEntry {
    /// 来源仓库名称
    pub repo: String,
    /// 插件名称
    pub name: String,
    /// 版本（语义化版本）
    pub version: String,
    /// 描述
    pub description: String,
    /// 作者
    pub author: String,
    /// 插件类型（protocol / agent / analysis）
    pub plugin_type: String,
    /// 下载 URL
    pub download_url: String,
    /// 文件校验和（SHA-256）
    pub checksum: String,
    /// 签名 URL
    pub signature_url: String,
}

impl From<(String, PluginIndexEntry)> for MarketSearchResultEntry {
    fn from((repo, entry): (String, PluginIndexEntry)) -> Self {
        Self {
            repo,
            name: entry.name,
            version: entry.version,
            description: entry.description,
            author: entry.author,
            plugin_type: entry.plugin_type,
            download_url: entry.download_url,
            checksum: entry.checksum,
            signature_url: entry.signature_url,
        }
    }
}

/// `GET /api/plugins/market/search` 响应体
#[derive(Debug, Serialize, ToSchema)]
pub struct MarketSearchResponse {
    /// 搜索结果列表
    pub results: Vec<MarketSearchResultEntry>,
}

/// `POST /api/plugins/market/install` 请求体
#[derive(Debug, Deserialize, ToSchema)]
pub struct MarketInstallRequest {
    /// 插件名称
    pub name: String,
    /// 插件版本
    pub version: String,
}

/// `POST /api/plugins/market/install` 响应体
#[derive(Debug, Serialize, ToSchema)]
pub struct MarketInstallResponse {
    /// 是否安装成功
    pub installed: bool,
    /// 本地缓存路径
    pub path: String,
}

/// `GET /api/plugins/market/search` — 搜索远程插件市场。
///
/// 在已加载的仓库索引中搜索插件（匹配名称、描述、作者，不区分大小写）。
/// 若未配置插件市场客户端或未加载索引，返回友好错误。
#[utoipa::path(
    get,
    tag = "plugin_market",
    path = "/api/plugins/market/search",
    params(MarketSearchQuery),
    responses(
        (status = 200, description = "搜索结果", body = MarketSearchResponse),
        (status = 503, description = "插件市场客户端未配置"),
    )
)]
pub async fn search_handler(
    State(state): State<AppState>,
    Query(params): Query<MarketSearchQuery>,
) -> axum::response::Response {
    let client_arc = match &state.plugin_market_client {
        Some(c) => c.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "results": [],
                    "error": "plugin market client not configured",
                })),
            )
                .into_response();
        }
    };

    let client = client_arc.read().await;
    let result = client.search(&params.q);
    let results: Vec<MarketSearchResultEntry> = result
        .entries
        .into_iter()
        .map(MarketSearchResultEntry::from)
        .collect();

    let response = MarketSearchResponse { results };
    (StatusCode::OK, Json(response)).into_response()
}

/// `POST /api/plugins/market/install` — 安装插件（下载到本地缓存）。
///
/// 在已加载的仓库索引中查找指定插件并下载到本地缓存目录。
/// 若未配置插件市场客户端、未加载索引或插件不存在，返回友好错误。
#[utoipa::path(
    post,
    tag = "plugin_market",
    path = "/api/plugins/market/install",
    request_body = MarketInstallRequest,
    responses(
        (status = 200, description = "安装结果", body = MarketInstallResponse),
        (status = 404, description = "插件未在索引中找到"),
        (status = 503, description = "插件市场客户端未配置"),
        (status = 500, description = "下载失败"),
    )
)]
pub async fn install_handler(
    State(state): State<AppState>,
    Json(req): Json<MarketInstallRequest>,
) -> axum::response::Response {
    let client_arc = match &state.plugin_market_client {
        Some(c) => c.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "installed": false,
                    "path": "",
                    "error": "plugin market client not configured",
                })),
            )
                .into_response();
        }
    };

    // 在所有已加载仓库中查找匹配的插件
    let client = client_arc.read().await;
    let plugins = client.list_plugins();
    let found = plugins
        .iter()
        .find(|(_, p)| p.name == req.name && p.version == req.version);

    let repo_name = match found {
        Some((repo, _)) => repo.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "installed": false,
                    "path": "",
                    "error": format!("plugin {}@{} not found in any loaded repo index", req.name, req.version),
                })),
            )
                .into_response();
        }
    };

    // 下载插件到本地缓存
    match client.download(&repo_name, &req.name, &req.version) {
        Ok(result) => {
            let response = MarketInstallResponse {
                installed: true,
                path: result.local_path.to_string_lossy().to_string(),
            };
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "installed": false,
                "path": "",
                "error": format!("download failed: {}", e),
            })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppState;
    use std::sync::Arc;

    /// 测试用仓库索引 TOML
    const SAMPLE_INDEX_TOML: &str = r#"
repo_name = "official"

[[plugins]]
name = "iec103-adapter"
version = "1.0.0"
description = "IEC 103 协议适配器"
author = "EnerOS Team"
plugin_type = "protocol"
download_url = "https://plugins.eneros.io/iec103-adapter-1.0.0.so"
checksum = "sha256:abc123"
signature_url = "https://plugins.eneros.io/iec103-adapter-1.0.0.sig"

[[plugins]]
name = "custom-strategy"
version = "0.9.0"
description = "自定义策略 Agent"
author = "ThirdParty"
plugin_type = "agent"
download_url = "https://plugins.eneros.io/custom-strategy-0.9.0.so"
checksum = "sha256:def456"
signature_url = "https://plugins.eneros.io/custom-strategy-0.9.0.sig"
"#;

    /// 构建一个已加载索引的 AppState（用于测试）
    fn app_state_with_market() -> AppState {
        let mut client = eneros_runtime::plugin::PluginMarketClient::with_defaults();
        client
            .load_repo_index("official", SAMPLE_INDEX_TOML)
            .expect("加载索引应成功");
        // 使用临时目录作为缓存目录，避免污染用户主目录
        let temp_dir = std::env::temp_dir().join("eneros_api_test_plugins_cache");
        let _ = std::fs::create_dir_all(&temp_dir);
        // 重新创建客户端，使用临时缓存目录
        let config = eneros_runtime::plugin::MarketConfig {
            repos: vec![eneros_runtime::plugin::RepoConfig {
                name: "official".to_string(),
                url: "https://plugins.eneros.io/index.toml".to_string(),
                enabled: true,
                priority: 100,
            }],
            cache_dir: temp_dir.to_string_lossy().to_string(),
            cache_limit_bytes: 512 * 1024 * 1024,
        };
        let mut client = eneros_runtime::plugin::PluginMarketClient::new(config);
        client
            .load_repo_index("official", SAMPLE_INDEX_TOML)
            .expect("加载索引应成功");
        AppState::new().with_plugin_market_client(Arc::new(tokio::sync::RwLock::new(client)))
    }

    #[test]
    fn test_market_search_result_entry_from() {
        let entry = PluginIndexEntry {
            name: "test-plugin".to_string(),
            version: "1.0.0".to_string(),
            description: "测试插件".to_string(),
            author: "Tester".to_string(),
            plugin_type: "protocol".to_string(),
            download_url: "https://example.com/test.so".to_string(),
            checksum: "sha256:abc".to_string(),
            signature_url: "https://example.com/test.sig".to_string(),
        };
        let dto = MarketSearchResultEntry::from(("official".to_string(), entry));
        assert_eq!(dto.repo, "official");
        assert_eq!(dto.name, "test-plugin");
        assert_eq!(dto.version, "1.0.0");
    }

    #[test]
    fn test_market_install_request_deserialization() {
        let json = r#"{"name":"iec103-adapter","version":"1.0.0"}"#;
        let req: MarketInstallRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "iec103-adapter");
        assert_eq!(req.version, "1.0.0");
    }

    #[tokio::test]
    async fn test_search_handler_no_client() {
        let state = AppState::new();
        let response = search_handler(
            State(state),
            Query(MarketSearchQuery {
                q: "test".to_string(),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn test_plugin_market_search() {
        let state = app_state_with_market();
        let response = search_handler(
            State(state),
            Query(MarketSearchQuery {
                q: "iec103".to_string(),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        let results = json["results"].as_array().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["name"], "iec103-adapter");
        assert_eq!(results[0]["repo"], "official");
    }

    #[tokio::test]
    async fn test_plugin_market_install() {
        let state = app_state_with_market();
        let req = MarketInstallRequest {
            name: "iec103-adapter".to_string(),
            version: "1.0.0".to_string(),
        };
        let response = install_handler(State(state), Json(req)).await;
        assert_eq!(response.status(), StatusCode::OK);

        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(json["installed"], true);
        assert!(json["path"].as_str().unwrap().contains("iec103-adapter-1.0.0"));
    }

    #[tokio::test]
    async fn test_install_handler_not_found() {
        let state = app_state_with_market();
        let req = MarketInstallRequest {
            name: "nonexistent".to_string(),
            version: "0.0.0".to_string(),
        };
        let response = install_handler(State(state), Json(req)).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}

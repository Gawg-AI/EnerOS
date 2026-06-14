use anyhow::Result;
use reqwest::Client;

use super::types::*;

/// API client for EnerOS server
pub struct ApiClient {
    base_url: String,
    client: Client,
}

impl ApiClient {
    /// Create a new API client pointing at the given base URL
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client: Client::new(),
        }
    }

    /// GET /api/topology
    pub async fn query_topology(&self) -> Result<TopologyDataResponse> {
        let url = format!("{}/api/topology", self.base_url);
        let resp = self.client.get(&url).send().await?;
        let data: ApiResponse<TopologyDataResponse> = resp.json().await?;
        match data.data {
            Some(d) => Ok(d),
            None => anyhow::bail!("API error: {}", data.error.unwrap_or_else(|| "unknown".to_string())),
        }
    }

    /// POST /api/power-flow
    pub async fn run_power_flow(&self) -> Result<PowerFlowResponse> {
        let url = format!("{}/api/power-flow", self.base_url);
        let body = serde_json::json!({});
        let resp = self.client.post(&url).json(&body).send().await?;
        let data: ApiResponse<PowerFlowResponse> = resp.json().await?;
        match data.data {
            Some(d) => Ok(d),
            None => anyhow::bail!("API error: {}", data.error.unwrap_or_else(|| "unknown".to_string())),
        }
    }

    /// GET /api/constraints
    pub async fn check_constraints(&self) -> Result<Vec<ConstraintViolationResponse>> {
        let url = format!("{}/api/constraints", self.base_url);
        let resp = self.client.get(&url).send().await?;
        let data: ApiResponse<Vec<ConstraintViolationResponse>> = resp.json().await?;
        match data.data {
            Some(d) => Ok(d),
            None => anyhow::bail!("API error: {}", data.error.unwrap_or_else(|| "unknown".to_string())),
        }
    }

    /// GET /api/agents
    pub async fn list_agents(&self) -> Result<AgentsResponse> {
        let url = format!("{}/api/agents", self.base_url);
        let resp = self.client.get(&url).send().await?;
        let data: ApiResponse<AgentsResponse> = resp.json().await?;
        match data.data {
            Some(d) => Ok(d),
            None => anyhow::bail!("API error: {}", data.error.unwrap_or_else(|| "unknown".to_string())),
        }
    }

    /// GET /api/scada/latest
    pub async fn get_scada_latest(&self) -> Result<ScadaLatestResponse> {
        let url = format!("{}/api/scada/latest", self.base_url);
        let resp = self.client.get(&url).send().await?;
        let data: ApiResponse<ScadaLatestResponse> = resp.json().await?;
        match data.data {
            Some(d) => Ok(d),
            None => anyhow::bail!("API error: {}", data.error.unwrap_or_else(|| "unknown".to_string())),
        }
    }

    /// GET /health — returns true if server is reachable
    pub async fn health_check(&self) -> Result<bool> {
        let url = format!("{}/health", self.base_url);
        let resp = self.client.get(&url).send().await?;
        if resp.status().is_success() {
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

impl Default for ApiClient {
    fn default() -> Self {
        Self::new("http://localhost:8080")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_client_creation() {
        let client = ApiClient::new("http://localhost:8080");
        assert_eq!(client.base_url, "http://localhost:8080");
    }

    #[test]
    fn test_api_client_trims_trailing_slash() {
        let client = ApiClient::new("http://localhost:8080/");
        assert_eq!(client.base_url, "http://localhost:8080");
    }

    #[test]
    fn test_api_client_default() {
        let client = ApiClient::default();
        assert_eq!(client.base_url, "http://localhost:8080");
    }

    #[test]
    fn test_api_client_custom_url() {
        let client = ApiClient::new("http://192.168.1.100:9090");
        assert_eq!(client.base_url, "http://192.168.1.100:9090");
    }
}

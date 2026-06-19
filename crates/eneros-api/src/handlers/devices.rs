//! Devices API handlers (v0.6.0 — S4).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Serialize;

use crate::app::AppState;

/// Device summary in the list response.
#[derive(Debug, Serialize)]
pub struct DeviceSummary {
    pub device_id: String,
    pub protocol: String,
    pub connected: bool,
    pub connection_state: String,
}

/// Device health response.
#[derive(Debug, Serialize)]
pub struct DeviceHealth {
    pub device_id: String,
    pub connected: bool,
    pub connection_state: String,
    pub statistics: Option<serde_json::Value>,
}

/// `GET /api/devices` — list all registered devices.
pub async fn list_handler(
    State(state): State<AppState>,
) -> axum::response::Response {
    let device_manager = match &state.device_manager {
        Some(m) => m,
        None => return (StatusCode::SERVICE_UNAVAILABLE, "device manager not configured").into_response(),
    };

    let device_ids = device_manager.device_ids().await;
    let mut devices = Vec::with_capacity(device_ids.len());

    for id in &device_ids {
        let connected = device_manager.is_connected(id).await;
        let conn_state = device_manager.connection_state(id).await;
        let info = device_manager.device_info(id).await;
        devices.push(DeviceSummary {
            device_id: id.clone(),
            protocol: info.map(|i| format!("{:?}", i.protocol)).unwrap_or_else(|| "unknown".to_string()),
            connected,
            connection_state: conn_state
                .map(|s| format!("{:?}", s))
                .unwrap_or_else(|| "Unknown".to_string()),
        });
    }

    (StatusCode::OK, Json(serde_json::json!({"devices": devices}))).into_response()
}

/// `GET /api/devices/{id}/health` — get device health status.
pub async fn health_handler(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
) -> axum::response::Response {
    let device_manager = match &state.device_manager {
        Some(m) => m,
        None => return (StatusCode::SERVICE_UNAVAILABLE, "device manager not configured").into_response(),
    };

    let connected = device_manager.is_connected(&device_id).await;
    let conn_state = device_manager.connection_state(&device_id).await;
    let stats = device_manager.statistics(&device_id).await;

    if conn_state.is_none() && stats.is_none() {
        return (StatusCode::NOT_FOUND, "device not found").into_response();
    }

    let health = DeviceHealth {
        device_id: device_id.clone(),
        connected,
        connection_state: conn_state
            .map(|s| format!("{:?}", s))
            .unwrap_or_else(|| "Unknown".to_string()),
        statistics: stats.map(|s| serde_json::to_value(s).unwrap_or(serde_json::json!({}))),
    };

    (StatusCode::OK, Json(health)).into_response()
}

/// `POST /api/devices/{id}/connect` — connect a device.
pub async fn connect_handler(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
) -> axum::response::Response {
    let device_manager = match &state.device_manager {
        Some(m) => m,
        None => return (StatusCode::SERVICE_UNAVAILABLE, "device manager not configured").into_response(),
    };

    match device_manager.connect(&device_id).await {
        Ok(_) => (StatusCode::OK, Json(serde_json::json!({"device_id": device_id, "action": "connect", "result": "success"}))).into_response(),
        Err(e) => {
            tracing::warn!("device connect failed: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, format!("connect failed: {}", e)).into_response()
        }
    }
}

/// `POST /api/devices/{id}/disconnect` — disconnect a device.
pub async fn disconnect_handler(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
) -> axum::response::Response {
    let device_manager = match &state.device_manager {
        Some(m) => m,
        None => return (StatusCode::SERVICE_UNAVAILABLE, "device manager not configured").into_response(),
    };

    match device_manager.disconnect(&device_id).await {
        Ok(_) => (StatusCode::OK, Json(serde_json::json!({"device_id": device_id, "action": "disconnect", "result": "success"}))).into_response(),
        Err(e) => {
            tracing::warn!("device disconnect failed: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, format!("disconnect failed: {}", e)).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_summary_serialization() {
        let summary = DeviceSummary {
            device_id: "rtu-1".to_string(),
            protocol: "Iec104".to_string(),
            connected: true,
            connection_state: "Active".to_string(),
        };
        let json = serde_json::to_string(&summary).unwrap();
        assert!(json.contains("\"device_id\":\"rtu-1\""));
        assert!(json.contains("\"connected\":true"));
    }

    #[test]
    fn test_device_health_serialization() {
        let health = DeviceHealth {
            device_id: "rtu-1".to_string(),
            connected: false,
            connection_state: "Disconnected".to_string(),
            statistics: None,
        };
        let json = serde_json::to_string(&health).unwrap();
        assert!(json.contains("\"connected\":false"));
    }
}

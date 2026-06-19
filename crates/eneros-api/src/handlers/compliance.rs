//! Device compliance check API handler (v0.7.0 — deferred from v0.6.0 S4).
//!
//! Exposes the `ComplianceChecker` from `eneros-constraint` via
//! `POST /api/compliance/check`. Clients submit an equipment spec and
//! operating conditions, and receive a list of `ComplianceFinding`s.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::app::AppState;
use eneros_constraint::compliance::{
    ComplianceChecker, ComplianceFinding, EquipmentSpec, OperatingConditions,
};

/// Request body for `POST /api/compliance/check`.
#[derive(Debug, Deserialize)]
pub struct ComplianceRequest {
    /// Equipment specification.
    pub spec: EquipmentSpec,
    /// Current operating conditions.
    pub operating: OperatingConditions,
}

/// Response body for `POST /api/compliance/check`.
#[derive(Debug, Serialize)]
pub struct ComplianceResponse {
    /// All compliance findings.
    pub findings: Vec<ComplianceFinding>,
    /// Whether all checks passed.
    pub all_passed: bool,
    /// Number of failed checks.
    pub failed_count: usize,
    /// Number of inconclusive checks.
    pub inconclusive_count: usize,
}

/// `POST /api/compliance/check` — check equipment compliance against GB/T standards.
pub async fn check_handler(
    State(_state): State<AppState>,
    Json(req): Json<ComplianceRequest>,
) -> axum::response::Response {
    let findings = ComplianceChecker::check_all(&req.spec, &req.operating);

    let failed_count = findings
        .iter()
        .filter(|f| {
            matches!(
                f.status,
                eneros_constraint::compliance::ComplianceStatus::Failed(_)
            )
        })
        .count();

    let inconclusive_count = findings
        .iter()
        .filter(|f| {
            matches!(
                f.status,
                eneros_constraint::compliance::ComplianceStatus::Inconclusive(_)
            )
        })
        .count();

    let all_passed = failed_count == 0 && inconclusive_count == 0;

    let response = ComplianceResponse {
        findings,
        all_passed,
        failed_count,
        inconclusive_count,
    };

    (StatusCode::OK, Json(response)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compliance_request_deserialize() {
        let json = r#"{
            "spec": {
                "equipment_type": "transformer",
                "rated_capacity": 10.0,
                "rated_voltage_kv": 35.0,
                "normal_loading_limit_percent": 85.0
            },
            "operating": {
                "loading_percent": 75.0,
                "voltage_pu": 1.0
            }
        }"#;
        let req: ComplianceRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.spec.equipment_type, "transformer");
        assert_eq!(req.spec.rated_capacity, Some(10.0));
        assert_eq!(req.operating.loading_percent, Some(75.0));
    }
}

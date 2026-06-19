//! System-level validation API handler (v0.7.0 — deferred from v0.6.0 S4).
//!
//! Exposes the `ValidationRuleEngine` from `eneros-constraint` via
//! `POST /api/validation/check`. Clients submit a `SystemStateSnapshot`
//! and receive a list of `ValidationFinding`s.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::app::AppState;
use eneros_constraint::validation_rules::{
    SystemStateSnapshot, ValidationFinding, ValidationRuleEngine, ValidationSummary,
};

/// Request body for `POST /api/validation/check`.
#[derive(Debug, Deserialize)]
pub struct ValidationRequest {
    /// The system state snapshot to validate.
    pub state: SystemStateSnapshot,
    /// Optional: only run specific checks (e.g., ["voltage", "frequency"]).
    /// If omitted, all checks are run.
    #[serde(default)]
    pub checks: Vec<String>,
}

/// Response body for `POST /api/validation/check`.
#[derive(Debug, Serialize)]
pub struct ValidationResponse {
    /// All findings from the validation checks.
    pub findings: Vec<ValidationFinding>,
    /// Summary of the validation results.
    pub summary: ValidationSummary,
}

/// `POST /api/validation/check` — run system-level validation rules.
pub async fn check_handler(
    State(_state): State<AppState>,
    Json(req): Json<ValidationRequest>,
) -> axum::response::Response {
    let engine = ValidationRuleEngine::new();

    let findings = if req.checks.is_empty() {
        engine.validate_all(&req.state)
    } else {
        let mut all_findings = Vec::new();
        for check in &req.checks {
            let found = match check.as_str() {
                "voltage" => engine.check_voltage_deviation(&req.state),
                "frequency" => engine.check_frequency_deviation(&req.state),
                "harmonics" => engine.check_harmonics(&req.state),
                "flicker" => engine.check_flicker(&req.state),
                "n1" | "n-1" => engine.check_n1_security(&req.state),
                "short_circuit" | "short-circuit" => engine.check_short_circuit_capacity(&req.state),
                "fault_clearing" | "fault-clearing" => engine.check_fault_clearing_time(&req.state),
                _ => Vec::new(),
            };
            all_findings.extend(found);
        }
        all_findings
    };

    let summary = ValidationRuleEngine::summarize(&findings);

    let response = ValidationResponse { findings, summary };

    (StatusCode::OK, Json(response)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validation_request_deserialize() {
        let json = r#"{"state": {"buses": [], "frequency": null, "contingencies": [], "short_circuits": []}, "checks": ["voltage"]}"#;
        let req: ValidationRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.checks.len(), 1);
        assert_eq!(req.checks[0], "voltage");
    }

    #[test]
    fn test_validation_request_defaults() {
        let json = r#"{"state": {"buses": [], "frequency": null, "contingencies": [], "short_circuits": []}}"#;
        let req: ValidationRequest = serde_json::from_str(json).unwrap();
        assert!(req.checks.is_empty());
    }
}

//! Distribution network planning evaluation API handler (v0.7.0 — deferred from v0.6.0 S4).
//!
//! Exposes the `PlanningEvaluator` from `eneros-analysis` via
//! `POST /api/planning/evaluate`. Clients submit supply area classification,
//! voltage level, and current operating metrics, and receive candidate
//! expansion plans plus limit information.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::app::AppState;
use eneros_analysis::planning::{
    CandidatePlan, LoadingLimits, PlanningEvaluator, SupplyRadius, SupplyAreaClass, VoltageLimits,
};

/// Request body for `POST /api/planning/evaluate`.
#[derive(Debug, Deserialize)]
pub struct PlanningRequest {
    /// Supply area classification (A/B/C/D/E).
    pub area_class: SupplyAreaClass,
    /// Rated voltage level (kV).
    pub voltage_kv: f64,
    /// Current transformer loading (%).
    pub current_loading_percent: f64,
    /// Current voltage deviation (%).
    pub voltage_deviation_percent: f64,
    /// PV penetration rate (%).
    #[serde(default)]
    pub pv_penetration_percent: f64,
    /// EV penetration rate (%).
    #[serde(default)]
    pub ev_penetration_percent: f64,
}

/// Response body for `POST /api/planning/evaluate`.
#[derive(Debug, Serialize)]
pub struct PlanningResponse {
    /// Applicable voltage limits.
    pub voltage_limits: VoltageLimits,
    /// Applicable transformer loading limits.
    pub transformer_loading_limits: LoadingLimits,
    /// Applicable line loading limits.
    pub line_loading_limits: LoadingLimits,
    /// Applicable supply radius.
    pub supply_radius: SupplyRadius,
    /// Generated candidate plans.
    pub candidate_plans: Vec<CandidatePlan>,
    /// N-1 pass rate requirement (%).
    pub n1_pass_rate_percent: f64,
    /// Reliability RS-1 requirement (%).
    pub reliability_rs1_percent: f64,
}

/// `POST /api/planning/evaluate` — evaluate distribution network planning.
pub async fn evaluate_handler(
    State(_state): State<AppState>,
    Json(req): Json<PlanningRequest>,
) -> axum::response::Response {
    let evaluator = PlanningEvaluator::new(req.area_class, req.voltage_kv);

    let voltage_limits = evaluator.voltage_limits();
    let transformer_loading_limits = evaluator.transformer_loading_limits();
    let line_loading_limits = evaluator.line_loading_limits();
    let supply_radius = evaluator.supply_radius();

    let candidate_plans = evaluator.generate_candidates(
        req.current_loading_percent,
        req.voltage_deviation_percent,
        req.pv_penetration_percent,
        req.ev_penetration_percent,
    );

    let n1_pass_rate_percent = req.area_class.n1_pass_rate_percent();
    let reliability_rs1_percent = req.area_class.reliability_rs1_percent();

    let response = PlanningResponse {
        voltage_limits,
        transformer_loading_limits,
        line_loading_limits,
        supply_radius,
        candidate_plans,
        n1_pass_rate_percent,
        reliability_rs1_percent,
    };

    (StatusCode::OK, Json(response)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_planning_request_deserialize() {
        let json = r#"{
            "area_class": "A",
            "voltage_kv": 10.0,
            "current_loading_percent": 75.0,
            "voltage_deviation_percent": 3.0,
            "pv_penetration_percent": 20.0,
            "ev_penetration_percent": 5.0
        }"#;
        let req: PlanningRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.area_class, SupplyAreaClass::A);
        assert_eq!(req.voltage_kv, 10.0);
        assert_eq!(req.current_loading_percent, 75.0);
    }

    #[test]
    fn test_planning_request_defaults() {
        let json = r#"{
            "area_class": "B",
            "voltage_kv": 35.0,
            "current_loading_percent": 50.0,
            "voltage_deviation_percent": 2.0
        }"#;
        let req: PlanningRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.pv_penetration_percent, 0.0);
        assert_eq!(req.ev_penetration_percent, 0.0);
    }
}

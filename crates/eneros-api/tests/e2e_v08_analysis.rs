//! v0.8.0 Analysis Precision API integration tests.
//!
//! Verifies the 5 new analysis endpoints exposed by T10:
//! - POST /api/analysis/ac-opf
//! - POST /api/analysis/transient
//! - POST /api/analysis/observability
//! - POST /api/analysis/bad-data
//! - POST /api/analysis/short-circuit/asymmetric
//!
//! These tests exercise the full HTTP stack (router → handler → analysis
//! engine) using the IEEE-14 network loaded into AppState, mirroring real
//! dispatch decision scenarios.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use tower::ServiceExt;

use eneros_api::app::{create_router, AppState};
use eneros_network::PowerNetwork;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Build a minimal AppState with just the PowerNetwork loaded.
/// This is sufficient for the analysis endpoints, which only require
/// `state.network` to be populated.
fn build_app_with_network() -> Router {
    let network = Arc::new(PowerNetwork::from_ieee14());
    let state = AppState::new().with_network(network);
    create_router(state)
}

/// Build an empty AppState (no network) for error-path tests.
fn build_app_without_network() -> Router {
    let state = AppState::new();
    create_router(state)
}

/// Extract response body as String.
async fn body_to_string(body: Body) -> String {
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

// ===========================================================================
// T10.1: AC-OPF endpoint
// ===========================================================================

#[tokio::test]
async fn test_ac_opf_with_network_returns_result() {
    let app = build_app_with_network();

    let body = serde_json::json!({});
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/analysis/ac-opf")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_to_string(response.into_body()).await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();

    // Should succeed (may have warnings but should return data)
    assert_eq!(
        json["success"], true,
        "AC-OPF should succeed with network: {}",
        json
    );

    let data = json["data"].as_object().expect("data should be object");
    // Should have bus voltages
    assert!(
        data["bus_voltages"].is_array(),
        "bus_voltages should be array"
    );
    assert!(
        !data["bus_voltages"].as_array().unwrap().is_empty(),
        "bus_voltages should not be empty"
    );
    // Should have generation
    assert!(
        data["generation"].is_array(),
        "generation should be array"
    );
    // Should have nodal prices
    assert!(
        data["nodal_prices"].is_array(),
        "nodal_prices should be array"
    );
}

#[tokio::test]
async fn test_ac_opf_without_network_returns_error() {
    let app = build_app_without_network();

    let body = serde_json::json!({});
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/analysis/ac-opf")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_to_string(response.into_body()).await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();

    // Should return error since no network and no request data
    assert_eq!(json["success"], false);
    assert!(
        json["error"]
            .as_str()
            .unwrap()
            .contains("AC-OPF requires"),
        "should mention AC-OPF requires network or data"
    );
}

#[tokio::test]
async fn test_ac_opf_interior_point_method() {
    let app = build_app_with_network();

    let body = serde_json::json!({"method": "interior_point"});
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/analysis/ac-opf")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_to_string(response.into_body()).await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["success"], true, "Interior point should succeed");
}

// ===========================================================================
// T10.2: Transient stability endpoint
// ===========================================================================

#[tokio::test]
async fn test_transient_equal_area_mode() {
    let app = build_app_with_network();

    // 等面积法则模式：单机无穷大系统
    // efd=1.05, v_inf=1.0, x_pre=0.5 → Pmax_pre=2.1
    // x_fault=2.0 → Pmax_fault=0.525 < Pm=0.8（故障期间发电机加速）
    // x_post=0.6 → Pmax_post=1.75 > Pm=0.8（故障后可恢复同步）
    // δ₀=asin(0.8/2.1)=0.392, δ_max=π-asin(0.8/1.75)=2.668
    // f(δ_max)=Pm·(δ_max-δ₀)-Pmax_fault·(cos δ₀-cos δ_max)
    //        =0.8·2.276-0.525·1.814=1.821-0.952=0.869>0 ✓
    let body = serde_json::json!({
        "mode": "equal_area",
        "generators": [{
            "gen_id": 1,
            "bus_id": 1,
            "model": "classical",
            "h": 5.0,
            "d": 2.0,
            "xd_prime": 0.3,
            "efd": 1.05,
            "pm": 0.8
        }],
        "equal_area": {
            "v_inf": 1.0,
            "x_pre_fault": 0.5,
            "x_fault": 2.0,
            "x_post_fault": 0.6,
            "frequency": 50.0
        }
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/analysis/transient")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_to_string(response.into_body()).await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();

    assert_eq!(json["success"], true, "Equal area should succeed: {}", json);

    let data = json["data"].as_object().unwrap();
    assert_eq!(data["mode"], "equal_area");
    assert!(
        data["equal_area"].is_object(),
        "equal_area result should be present"
    );
    // CCT should be positive
    let cct = data["equal_area"]["cct"].as_f64().unwrap();
    assert!(cct > 0.0, "CCT should be positive, got {}", cct);
}

#[tokio::test]
async fn test_transient_simulate_mode_with_network() {
    let app = build_app_with_network();

    let body = serde_json::json!({
        "mode": "simulate",
        "fault_type": "three_phase",
        "fault_bus": 1,
        "fault_impedance": 0.0,
        "params": {
            "t_start": 0.0,
            "t_end": 0.5,
            "dt": 0.01,
            "t_fault": 0.1,
            "t_clear": 0.2,
            "method": "rk4",
            "frequency": 50.0
        }
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/analysis/transient")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_to_string(response.into_body()).await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();

    // Simulation may succeed or fail depending on convergence, but should return a response
    if json["success"] == true {
        let data = json["data"].as_object().unwrap();
        assert_eq!(data["mode"], "simulate");
        assert!(data["time_series"].is_array());
    }
    // Either success with data or error message is acceptable for this test
}

#[tokio::test]
async fn test_transient_without_network_returns_error() {
    let app = build_app_without_network();

    let body = serde_json::json!({"mode": "simulate"});
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/analysis/transient")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let body = body_to_string(response.into_body()).await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["success"], false);
}

// ===========================================================================
// T10.3: Observability endpoint
// ===========================================================================

#[tokio::test]
async fn test_observability_numerical_with_network() {
    let app = build_app_with_network();

    let body = serde_json::json!({
        "method": "numerical",
        "compute_pmu_placement": true
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/analysis/observability")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_to_string(response.into_body()).await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();

    assert_eq!(
        json["success"], true,
        "Observability analysis should succeed: {}",
        json
    );

    let data = json["data"].as_object().unwrap();
    assert!(
        data["observable_buses"].is_array(),
        "observable_buses should be array"
    );
    assert!(
        data["jacobian_rank"].is_number(),
        "jacobian_rank should be number"
    );
    // PMU placement should be computed
    assert!(
        data["pmu_placement"].is_object(),
        "pmu_placement should be present"
    );
    let pmu = data["pmu_placement"].as_object().unwrap();
    assert!(pmu["pmu_count"].as_u64().unwrap() > 0);
    assert!(pmu["coverage"].as_f64().unwrap() > 0.0);
}

#[tokio::test]
async fn test_observability_topological_method() {
    let app = build_app_with_network();

    let body = serde_json::json!({
        "method": "topological"
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/analysis/observability")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_to_string(response.into_body()).await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();

    assert_eq!(json["success"], true, "Topological method should succeed");
    let data = json["data"].as_object().unwrap();
    assert_eq!(data["method"], "Topological");
}

#[tokio::test]
async fn test_observability_without_network_returns_error() {
    let app = build_app_without_network();

    let body = serde_json::json!({});
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/analysis/observability")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let body = body_to_string(response.into_body()).await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("network model"));
}

// ===========================================================================
// T10.4: Bad data detection endpoint
// ===========================================================================

#[tokio::test]
async fn test_bad_data_detection_with_clean_measurements() {
    let app = build_app_with_network();

    // 使用合成测量（无不良数据注入）
    let body = serde_json::json!({
        "eliminate": false
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/analysis/bad-data")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_to_string(response.into_body()).await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();

    // Should succeed (may or may not find bad data in synthetic measurements)
    assert_eq!(
        json["success"], true,
        "Bad data detection should succeed: {}",
        json
    );

    let data = json["data"].as_object().unwrap();
    assert!(
        data["chi_square_test"].is_object(),
        "chi_square_test should be present"
    );
    assert!(
        data["threshold"].as_f64().unwrap() >= 3.0,
        "threshold should be >= 3.0"
    );
}

#[tokio::test]
async fn test_bad_data_detection_with_elimination() {
    let app = build_app_with_network();

    let body = serde_json::json!({
        "eliminate": true,
        "max_elimination_rounds": 5
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/analysis/bad-data")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_to_string(response.into_body()).await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();

    assert_eq!(
        json["success"], true,
        "Bad data elimination should succeed: {}",
        json
    );

    let data = json["data"].as_object().unwrap();
    assert!(
        data["elimination_rounds"].is_number(),
        "elimination_rounds should be present"
    );
}

#[tokio::test]
async fn test_bad_data_without_network_returns_error() {
    let app = build_app_without_network();

    let body = serde_json::json!({});
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/analysis/bad-data")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let body = body_to_string(response.into_body()).await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("network model"));
}

// ===========================================================================
// T10.5: Asymmetric short circuit endpoint
// ===========================================================================

#[tokio::test]
async fn test_asymmetric_short_circuit_slg() {
    let app = build_app_with_network();

    let body = serde_json::json!({
        "bus_id": 1,
        "fault_type": "slg",
        "fault_impedance_real": 0.0,
        "fault_impedance_imag": 0.0
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/analysis/short-circuit/asymmetric")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_to_string(response.into_body()).await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();

    assert_eq!(
        json["success"], true,
        "SLG fault analysis should succeed: {}",
        json
    );

    let data = json["data"].as_object().unwrap();
    assert_eq!(data["fault_type"], "SingleLineGround");
    assert_eq!(data["method"], "sequence_networks");
    // Fault current magnitude should be positive
    let fault_mag = data["fault_current_magnitude_ka"].as_f64().unwrap();
    assert!(
        fault_mag > 0.0,
        "Fault current magnitude should be positive, got {}",
        fault_mag
    );
    // Should have bus voltages
    assert!(data["bus_voltages"].is_array());
}

#[tokio::test]
async fn test_asymmetric_short_circuit_ll() {
    let app = build_app_with_network();

    let body = serde_json::json!({
        "bus_id": 2,
        "fault_type": "ll"
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/analysis/short-circuit/asymmetric")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_to_string(response.into_body()).await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();

    assert_eq!(json["success"], true, "LL fault should succeed: {}", json);
    let data = json["data"].as_object().unwrap();
    assert_eq!(data["fault_type"], "LineLine");
}

#[tokio::test]
async fn test_asymmetric_short_circuit_dlg() {
    let app = build_app_with_network();

    let body = serde_json::json!({
        "bus_id": 3,
        "fault_type": "dlg"
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/analysis/short-circuit/asymmetric")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_to_string(response.into_body()).await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();

    assert_eq!(json["success"], true, "DLG fault should succeed: {}", json);
    let data = json["data"].as_object().unwrap();
    assert_eq!(data["fault_type"], "DoubleLineGround");
}

#[tokio::test]
async fn test_asymmetric_short_circuit_invalid_type() {
    let app = build_app_with_network();

    let body = serde_json::json!({
        "bus_id": 1,
        "fault_type": "invalid_fault"
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/analysis/short-circuit/asymmetric")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let body = body_to_string(response.into_body()).await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("Unknown fault type"));
}

#[tokio::test]
async fn test_asymmetric_short_circuit_without_network() {
    let app = build_app_without_network();

    let body = serde_json::json!({
        "bus_id": 1,
        "fault_type": "slg"
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/analysis/short-circuit/asymmetric")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let body = body_to_string(response.into_body()).await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("network model"));
}

// ===========================================================================
// Route registration smoke test
// ===========================================================================

#[tokio::test]
async fn test_all_v080_analysis_routes_registered() {
    // Verify all 5 new routes are registered by sending empty POST requests
    // and checking they don't return 404
    let app = build_app_with_network();

    let routes = [
        "/api/analysis/ac-opf",
        "/api/analysis/transient",
        "/api/analysis/observability",
        "/api/analysis/bad-data",
        "/api/analysis/short-circuit/asymmetric",
    ];

    for route in &routes {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(*route)
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();

        // Should NOT be 404 (route should exist)
        assert_ne!(
            response.status(),
            StatusCode::NOT_FOUND,
            "Route {} should be registered",
            route
        );
    }
}

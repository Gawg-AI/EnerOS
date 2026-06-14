//! End-to-end integration tests for the EnerOS Python Bridge.
//!
//! These tests require cnpower and pandapower to be installed.
//! They are ignored by default to avoid failures in CI environments
//! where Python dependencies are not available.
//!
//! Run with: cargo test -p eneros-bridge --test bridge_e2e -- --ignored --test-threads=1
//!
//! Note: --test-threads=1 is required because all tests share the same port (8321)
//! and cannot run in parallel.

use eneros_bridge::bridge_client::BridgeClient;
use eneros_bridge::pandapower_types::PandapowerResult;
use eneros_bridge::topology_types::NetworkTopologyData;
use std::collections::HashMap;

/// Helper to create a BridgeClient and start the server
fn start_bridge() -> BridgeClient {
    let mut client = BridgeClient::new();
    client.start().expect("Failed to start Python bridge server");
    client
}

/// Minimal network assets that work with build_pandapower_net().
/// Includes ext_grids so power flow has a slack reference bus.
fn minimal_assets() -> serde_json::Value {
    serde_json::json!({
        "buses": [
            {"id": 0, "vn_kv": 10.0},
            {"id": 1, "vn_kv": 10.0}
        ],
        "ext_grids": [
            {"bus": 0, "vm_pu": 1.0}
        ],
        "lines": [
            {"from_bus": 0, "to_bus": 1, "length_km": 1.0, "std_type": "NAYY 4x50 SE"}
        ],
        "loads": [
            {"bus": 1, "p_mw": 0.1, "q_mvar": 0.05}
        ]
    })
}

#[test]
#[ignore] // Requires Python + cnpower + pandapower
fn test_bridge_health_check() {
    let mut client = start_bridge();

    let result = client.health_check();
    assert!(result.is_ok(), "Health check failed: {:?}", result);

    let health = result.unwrap();
    assert_eq!(health["status"], "ok");

    client.stop().unwrap();
}

#[test]
#[ignore]
fn test_list_transformers() {
    let mut client = start_bridge();

    let result: Vec<serde_json::Value> = client
        .call("list_transformers", HashMap::new())
        .expect("list_transformers failed");

    assert!(!result.is_empty(), "Transformer list should not be empty");

    // Verify first transformer has expected fields
    let first = &result[0];
    assert!(
        first.get("sn_kva").is_some() || first.get("_category").is_some(),
        "Transformer should have sn_kva or _category field"
    );

    client.stop().unwrap();
}

#[test]
#[ignore]
fn test_list_cables() {
    let mut client = start_bridge();

    let result: Vec<serde_json::Value> = client
        .call("list_cables", HashMap::new())
        .expect("list_cables failed");

    assert!(!result.is_empty(), "Cable list should not be empty");

    client.stop().unwrap();
}

#[test]
#[ignore]
fn test_list_standards() {
    let mut client = start_bridge();

    let result: serde_json::Value = client
        .call("list_standards", HashMap::new())
        .expect("list_standards failed");

    // Standards should return some data (dict or list)
    assert!(
        result.is_object() || result.is_array(),
        "Standards should be object or array"
    );

    client.stop().unwrap();
}

#[test]
#[ignore]
fn test_build_network() {
    let mut client = start_bridge();

    let assets = minimal_assets();

    let mut params = HashMap::new();
    params.insert("assets".to_string(), assets);
    params.insert("run_powerflow".to_string(), serde_json::Value::Bool(true));

    let result: serde_json::Value = client
        .call("build_network", params)
        .expect("build_network failed");

    // Should return network statistics
    assert!(result.get("bus_count").is_some(), "Should have bus_count");
    assert!(result.get("line_count").is_some(), "Should have line_count");

    client.stop().unwrap();
}

#[test]
#[ignore]
fn test_run_powerflow() {
    let mut client = start_bridge();

    let assets = minimal_assets();

    let mut params = HashMap::new();
    params.insert("assets".to_string(), assets);

    let result: PandapowerResult = client
        .call("run_powerflow", params)
        .expect("run_powerflow failed");

    // Verify result structure
    assert!(result.converged, "Power flow should converge");
    assert!(!result.buses.is_empty(), "Should have bus results");
    assert!(
        result.buses[0].vm_pu.is_some(),
        "Bus 0 should have voltage"
    );

    client.stop().unwrap();
}

#[test]
#[ignore]
fn test_build_full_network() {
    let mut client = start_bridge();

    let assets = minimal_assets();

    let mut params = HashMap::new();
    params.insert("assets".to_string(), assets);

    let result: NetworkTopologyData = client
        .call("build_full_network", params)
        .expect("build_full_network failed");

    // Verify topology structure
    assert!(result.converged, "Power flow should converge");
    assert!(!result.buses.is_empty(), "Should have buses");
    assert!(!result.branches.is_empty(), "Should have branches");
    assert_eq!(result.bus_count, result.buses.len());
    assert_eq!(result.branch_count, result.branches.len());

    // Verify bus types
    let slack_buses: Vec<_> = result
        .buses
        .iter()
        .filter(|b| b.bus_type == "Slack")
        .collect();
    assert!(
        !slack_buses.is_empty(),
        "Should have at least one Slack bus"
    );

    client.stop().unwrap();
}

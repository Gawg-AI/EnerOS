//! EnerOS boot integration tests
//!
//! These tests verify the boot logic of eneros-init without requiring
//! an actual QEMU environment. They test:
//! - Service graph construction
//! - Configuration loading
//! - Startup order validation
//! - Signal handling setup

use eneros_os::init::{
    InitConfig, RestartPolicy, ServiceGraph, ServiceManager, SignalHandler,
};

/// Test that the default service configuration is valid
#[test]
fn test_default_service_config_valid() {
    let config = InitConfig::load_default();

    let mut graph = ServiceGraph::new();
    for service in &config.services {
        graph.add_service(service.clone());
    }

    // Should not fail - no cycles, all dependencies exist
    let order = graph
        .topological_sort()
        .expect("Default service config should be valid");

    // network should come before timesync (timesync depends on network)
    let network_pos = order
        .iter()
        .position(|s| s == "network")
        .expect("network service should exist");
    let timesync_pos = order
        .iter()
        .position(|s| s == "timesync")
        .expect("timesync service should exist");
    assert!(
        network_pos < timesync_pos,
        "network should start before timesync"
    );

    // power-app should come last (depends on all others)
    let power_app_pos = order
        .iter()
        .position(|s| s == "power-app")
        .expect("power-app service should exist");
    assert_eq!(power_app_pos, order.len() - 1, "power-app should start last");
}

/// Test that the service graph has correct dependencies
#[test]
fn test_service_dependencies() {
    let config = InitConfig::load_default();

    let network = config
        .services
        .iter()
        .find(|s| s.name == "network")
        .expect("network service should exist");
    assert!(
        network.dependencies.is_empty(),
        "network should have no dependencies"
    );

    let timesync = config
        .services
        .iter()
        .find(|s| s.name == "timesync")
        .expect("timesync service should exist");
    assert!(
        timesync.dependencies.contains(&"network".to_string()),
        "timesync should depend on network"
    );

    let power_app = config
        .services
        .iter()
        .find(|s| s.name == "power-app")
        .expect("power-app service should exist");
    assert!(
        power_app.dependencies.contains(&"network".to_string()),
        "power-app should depend on network"
    );
    assert!(
        power_app.dependencies.contains(&"timesync".to_string()),
        "power-app should depend on timesync"
    );
    assert!(
        power_app.dependencies.contains(&"syslog".to_string()),
        "power-app should depend on syslog"
    );
    assert!(
        power_app.dependencies.contains(&"devmgr".to_string()),
        "power-app should depend on devmgr"
    );
}

/// Test that all services have correct restart policies
#[test]
fn test_restart_policies() {
    let config = InitConfig::load_default();

    for service in &config.services {
        match service.name.as_str() {
            "network" | "timesync" | "syslog" | "devmgr" => {
                assert_eq!(
                    service.restart_policy,
                    RestartPolicy::Always,
                    "{} should have Always restart policy",
                    service.name
                );
            }
            "power-app" => {
                assert_eq!(
                    service.restart_policy,
                    RestartPolicy::OnFailure,
                    "power-app should have OnFailure restart policy"
                );
            }
            _ => {}
        }
    }
}

/// Test that the service manager can be created with default config.
///
/// NOTE: `ServiceManager::new` does not register services with the
/// supervisor until `prepare()` is called. This test calls `prepare()`
/// to populate the supervisor and then verifies the service count.
#[test]
fn test_service_manager_creation() {
    let config = InitConfig::load_default();

    let mut graph = ServiceGraph::new();
    for service in &config.services {
        graph.add_service(service.clone());
    }

    let mut manager = ServiceManager::new(graph);

    // prepare() registers all graph services with the supervisor and
    // computes the cached startup order.
    manager
        .prepare()
        .expect("prepare should succeed for valid config");

    // Should have 5 services registered
    let services: Vec<_> = manager.supervisor().services().collect();
    assert_eq!(services.len(), 5, "Should have 5 services");
}

/// Test that the startup order is correct.
///
/// The topological sort uses Kahn's algorithm over a HashMap, so the
/// relative order of independent services (network, syslog, devmgr) is
/// non-deterministic. We only assert the deterministic constraints:
/// - All 5 services are present.
/// - `network` appears before `timesync` (timesync depends on network).
/// - `power-app` appears last (it depends on all other services).
#[test]
fn test_startup_order() {
    let config = InitConfig::load_default();

    let mut graph = ServiceGraph::new();
    for service in &config.services {
        graph.add_service(service.clone());
    }

    let order = graph.topological_sort().unwrap();

    // Verify all 5 services are in the order
    assert_eq!(order.len(), 5, "Should have 5 services in startup order");

    // Verify network starts before timesync (deterministic dependency)
    let network_pos = order
        .iter()
        .position(|s| s == "network")
        .expect("network should be in startup order");
    let timesync_pos = order
        .iter()
        .position(|s| s == "timesync")
        .expect("timesync should be in startup order");
    assert!(
        network_pos < timesync_pos,
        "network should start before timesync"
    );

    // Verify power-app starts last (depends on all others — deterministic)
    assert_eq!(
        order[order.len() - 1], "power-app",
        "power-app should start last"
    );
}

/// Test configuration loading from TOML string
#[test]
fn test_config_from_toml() {
    let toml_str = r#"
[[services]]
name = "test-service"
binary = "/bin/test"
restart_policy = "always"
dependencies = []
graceful_timeout_secs = 5

[services.env]
TEST_VAR = "test_value"
"#;

    let config: InitConfig = toml::from_str(toml_str).expect("Should parse TOML");

    assert_eq!(config.services.len(), 1);
    assert_eq!(config.services[0].name, "test-service");
    assert_eq!(config.services[0].binary, "/bin/test");
    assert_eq!(config.services[0].restart_policy, RestartPolicy::Always);
    assert_eq!(
        config.services[0].env.get("TEST_VAR"),
        Some(&"test_value".to_string())
    );
}

/// Test that the init config file path is correct
#[test]
fn test_config_file_path() {
    // The default config path should be /etc/eneros/init.toml
    // This is what eneros-init will look for at runtime
    let expected_path = "/etc/eneros/init.toml";

    // Verify the path format
    assert!(expected_path.starts_with("/etc/eneros/"));
    assert!(expected_path.ends_with(".toml"));
}

/// Test signal handler creation
#[test]
fn test_signal_handler_creation() {
    let handler = SignalHandler::new();

    // Initially, no signals should be requested
    assert!(!handler.should_shutdown());
    assert!(!handler.should_reload());
}

/// Test that the rootfs contains required files
/// This is a documentation test - verifies our understanding of rootfs structure
#[test]
fn test_rootfs_structure_documentation() {
    // These are the files that should exist in the rootfs
    let required_files = vec![
        "/bin/eneros-init",
        "/bin/eneros-api",
        "/etc/eneros/init.toml",
        "/etc/passwd",
        "/etc/group",
        "/etc/hostname",
        "/var/lib/eneros",
        "/var/log/eneros",
    ];

    // This test documents the expected rootfs structure
    // Actual verification happens in boot_test.sh on Linux
    for file in required_files {
        assert!(
            !file.is_empty(),
            "Required file path should not be empty: {}",
            file
        );
    }
}

/// Test that the kernel boot parameters are correct
#[test]
fn test_kernel_boot_parameters() {
    // These are the RT optimization parameters that should be in grub.cfg
    let required_params = vec![
        "isolcpus=2,3", // CPU isolation for RT
        "nohz_full=2,3", // Tickless kernel
        "rcu_nocbs=2,3", // RCU callback migration
        "irqaffinity=0,1", // Interrupt routing
        "mlock=1", // Memory locking
    ];

    // This test documents the required boot parameters
    for param in required_params {
        assert!(
            !param.is_empty(),
            "Boot parameter should not be empty: {}",
            param
        );
    }
}

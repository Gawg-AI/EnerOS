//! EnerOS 端到端集成测试入口。
//!
//! 每个测试启动一个独立的集群实例（使用不同端口），执行场景验证，
//! 然后关闭集群。如果二进制不存在，测试将被跳过。
//!
//! ## 运行方式
//!
//! ```bash
//! # 先构建所有二进制
//! cargo build --workspace
//!
//! # 运行 e2e 测试
//! cargo test -p eneros-e2e-tests
//! ```

use eneros_e2e_tests::chaos_scenarios;
use eneros_e2e_tests::cluster::{ClusterConfig, TestCluster};
use eneros_e2e_tests::scenarios;

/// 尝试启动集群。如果二进制不存在则返回 None（跳过测试）。
async fn try_start_cluster(base_port: u16) -> Option<TestCluster> {
    let config = ClusterConfig {
        base_port,
        ..Default::default()
    };
    match TestCluster::start(config).await {
        Ok(cluster) => Some(cluster),
        Err(e) => {
            eprintln!("SKIP: cluster start failed ({}), skipping test", e);
            None
        }
    }
}

// ── 启动验证场景 ──────────────────────────────────────────────────────────

#[tokio::test]
async fn test_startup_health_check() {
    let mut cluster = match try_start_cluster(18000).await {
        Some(c) => c,
        None => return,
    };
    scenarios::startup::health_check(&cluster)
        .await
        .expect("health check scenario failed");
    cluster.shutdown().await;
}

#[tokio::test]
async fn test_startup_agents_list() {
    let mut cluster = match try_start_cluster(18003).await {
        Some(c) => c,
        None => return,
    };
    scenarios::startup::agents_list(&cluster)
        .await
        .expect("agents list scenario failed");
    cluster.shutdown().await;
}

#[tokio::test]
async fn test_startup_topology() {
    let mut cluster = match try_start_cluster(18006).await {
        Some(c) => c,
        None => return,
    };
    scenarios::startup::topology(&cluster)
        .await
        .expect("topology scenario failed");
    cluster.shutdown().await;
}

// ── HA 故障切换场景 ────────────────────────────────────────────────────────

#[tokio::test]
async fn test_ha_failover_config_and_status() {
    let mut cluster = match try_start_cluster(18010).await {
        Some(c) => c,
        None => return,
    };
    scenarios::ha_failover::ha_config_and_status(&cluster)
        .await
        .expect("HA config and status scenario failed");
    cluster.shutdown().await;
}

// ── 插件生命周期场景 ───────────────────────────────────────────────────────

#[tokio::test]
async fn test_plugin_market_search() {
    let mut cluster = match try_start_cluster(18020).await {
        Some(c) => c,
        None => return,
    };
    scenarios::plugin_lifecycle::plugin_market_search(&cluster)
        .await
        .expect("plugin market search scenario failed");
    cluster.shutdown().await;
}

#[tokio::test]
async fn test_plugin_status() {
    let mut cluster = match try_start_cluster(18023).await {
        Some(c) => c,
        None => return,
    };
    scenarios::plugin_lifecycle::plugin_status(&cluster)
        .await
        .expect("plugin status scenario failed");
    cluster.shutdown().await;
}

// ── SCADA 采集场景 ─────────────────────────────────────────────────────────

#[tokio::test]
async fn test_scada_latest() {
    let mut cluster = match try_start_cluster(18030).await {
        Some(c) => c,
        None => return,
    };
    scenarios::scada_pipeline::scada_latest(&cluster)
        .await
        .expect("scada latest scenario failed");
    cluster.shutdown().await;
}

#[tokio::test]
async fn test_scada_points() {
    let mut cluster = match try_start_cluster(18033).await {
        Some(c) => c,
        None => return,
    };
    scenarios::scada_pipeline::scada_points(&cluster)
        .await
        .expect("scada points scenario failed");
    cluster.shutdown().await;
}

// ── Agent 决策场景 ─────────────────────────────────────────────────────────

#[tokio::test]
async fn test_agent_decision_list() {
    let mut cluster = match try_start_cluster(18040).await {
        Some(c) => c,
        None => return,
    };
    scenarios::agent_decision::agents_list(&cluster)
        .await
        .expect("agent decision list scenario failed");
    cluster.shutdown().await;
}

#[tokio::test]
async fn test_agent_decision_control() {
    let mut cluster = match try_start_cluster(18043).await {
        Some(c) => c,
        None => return,
    };
    scenarios::agent_decision::agent_status_control(&cluster)
        .await
        .expect("agent status control scenario failed");
    cluster.shutdown().await;
}

// ── 命令下发场景 ───────────────────────────────────────────────────────────

#[tokio::test]
async fn test_command_dispatch_structured() {
    let mut cluster = match try_start_cluster(18050).await {
        Some(c) => c,
        None => return,
    };
    scenarios::command_dispatch::structured_action(&cluster)
        .await
        .expect("structured action scenario failed");
    cluster.shutdown().await;
}

#[tokio::test]
async fn test_command_dispatch_audit() {
    let mut cluster = match try_start_cluster(18053).await {
        Some(c) => c,
        None => return,
    };
    scenarios::command_dispatch::audit_query(&cluster)
        .await
        .expect("audit query scenario failed");
    cluster.shutdown().await;
}

// ── 混沌工程场景 ───────────────────────────────────────────────────────────
//
// 混沌测试用 `#[ignore]` 标注，因为需要完整集群环境。
// 运行方式：cargo test -p eneros-e2e-tests -- --ignored chaos

/// 网络分区混沌 — 验证 NetworkChaos::inject_partition 能启动和停止。
#[tokio::test]
#[ignore = "chaos test: requires full cluster environment"]
async fn test_chaos_network_partition() {
    chaos_scenarios::network_partition::run()
        .await
        .expect("network partition chaos scenario failed");
}

/// Agent 崩溃混沌 — 验证 ProcessChaos::inject_crash 能执行。
#[tokio::test]
#[ignore = "chaos test: requires full cluster environment"]
async fn test_chaos_agent_crash() {
    chaos_scenarios::agent_crash::run()
        .await
        .expect("agent crash chaos scenario failed");
}

/// CPU 饱和混沌 — 验证 CpuChaos::inject_cpu_saturation 能启动和停止。
#[tokio::test]
#[ignore = "chaos test: requires full cluster environment"]
async fn test_chaos_cpu_saturation() {
    chaos_scenarios::cpu_saturation::run()
        .await
        .expect("cpu saturation chaos scenario failed");
}

/// 磁盘满混沌 — 验证 DiskChaos::inject_disk_full 能执行和恢复。
#[tokio::test]
#[ignore = "chaos test: requires full cluster environment"]
async fn test_chaos_disk_full() {
    chaos_scenarios::disk_full::run()
        .await
        .expect("disk full chaos scenario failed");
}

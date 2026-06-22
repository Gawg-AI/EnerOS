//! 网络分区混沌场景。
//!
//! 完整版：注入网络分区 5s，验证 HA 脑裂检测触发，分区解除后系统恢复。
//! 简化版：验证 `NetworkChaos::inject_partition` 能启动和停止。

use std::time::Duration;

use anyhow::Result;
use eneros_test_utils::chaos::NetworkChaos;

/// 运行网络分区混沌场景（简化版）。
///
/// 1. 注入 5s 网络分区
/// 2. 等待 100ms 确认注入激活
/// 3. 停止注入，验证混沌解除后系统恢复正常
pub async fn run() -> Result<()> {
    let chaos = NetworkChaos::new();

    // 注入网络分区（5s，但会提前停止）
    let handle = chaos
        .inject_partition(Duration::from_secs(5))
        .await
        .expect("partition injection should succeed");

    // 确认注入激活
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 停止注入 — 模拟分区解除
    handle.stop().await;

    // 验证混沌解除后可再次注入（系统恢复正常）
    let handle2 = chaos
        .inject_partition(Duration::from_millis(50))
        .await
        .expect("should be able to inject again after recovery");
    handle2.stop().await;

    Ok(())
}

/// 完整版网络分区场景 — 需要运行中的集群。
///
/// TODO: 注入网络分区 5s，验证：
/// - HA 脑裂检测触发
/// - 分区期间写入被拒绝
/// - 分区解除后集群重新同步
#[allow(dead_code)]
pub async fn run_with_cluster(
    _cluster: &crate::cluster::TestCluster,
) -> Result<()> {
    // 完整版需要集群支持，暂未实现
    run().await
}

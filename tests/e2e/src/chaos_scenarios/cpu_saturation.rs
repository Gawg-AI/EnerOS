//! CPU 饱和混沌场景。
//!
//! 完整版：注入 80% CPU 饱和 5s，验证系统降级但可用，混沌解除后恢复。
//! 简化版：验证 `CpuChaos::inject_cpu_saturation` 能启动和停止。

use std::time::Duration;

use anyhow::Result;
use eneros_test_utils::chaos::CpuChaos;

/// 运行 CPU 饱和混沌场景（简化版）。
///
/// 1. 注入 80% CPU 饱和 5s
/// 2. 等待 100ms 确认注入激活
/// 3. 停止注入，验证混沌解除后恢复正常
pub async fn run() -> Result<()> {
    let chaos = CpuChaos::new();

    // 注入 80% CPU 饱和 5s（但会提前停止）
    let handle = chaos
        .inject_cpu_saturation(80, Duration::from_secs(5))
        .await
        .expect("cpu saturation injection should succeed");

    // 确认注入激活
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 停止注入
    handle.stop().await;

    // 验证混沌解除后可再次注入
    let handle2 = chaos
        .inject_cpu_saturation(50, Duration::from_millis(50))
        .await
        .expect("should be able to inject again after recovery");
    handle2.stop().await;

    Ok(())
}

/// 完整版 CPU 饱和场景 — 需要运行中的集群。
///
/// TODO: 注入 80% CPU 饱和 5s，验证：
/// - 系统响应延迟增加但服务可用
/// - SCADA 采集不中断
/// - 混沌解除后延迟恢复正常
#[allow(dead_code)]
pub async fn run_with_cluster(
    _cluster: &crate::cluster::TestCluster,
) -> Result<()> {
    // 完整版需要集群支持，暂未实现
    run().await
}

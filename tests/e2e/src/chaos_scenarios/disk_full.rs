//! 磁盘满混沌场景。
//!
//! 完整版：注入磁盘满，验证日志降级写入，混沌解除后恢复。
//! 简化版：验证 `DiskChaos::inject_disk_full` 能执行和恢复。

use std::time::Duration;

use anyhow::Result;
use eneros_test_utils::chaos::DiskChaos;

/// 运行磁盘满混沌场景（简化版）。
///
/// 1. 注入磁盘满（创建 2MB 临时文件）
/// 2. 等待 100ms 确认注入激活
/// 3. 停止注入（临时文件删除），验证恢复正常
pub async fn run() -> Result<()> {
    let chaos = DiskChaos::new();

    // 注入磁盘满（2MB 临时文件）
    let handle = chaos
        .inject_disk_full(".", 2)
        .await
        .expect("disk full injection should succeed");

    // 确认注入激活
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 停止注入 — 临时文件应被删除
    handle.stop().await;

    // 验证混沌解除后可再次注入
    let handle2 = chaos
        .inject_disk_full(".", 1)
        .await
        .expect("should be able to inject again after recovery");
    handle2.stop().await;

    Ok(())
}

/// 完整版磁盘满场景 — 需要运行中的集群。
///
/// TODO: 注入磁盘满，验证：
/// - 日志降级写入（跳过 debug/info，仅保留 warn/error）
/// - SCADA 时序数据降级采样
/// - 混沌解除后恢复正常写入
#[allow(dead_code)]
pub async fn run_with_cluster(
    _cluster: &crate::cluster::TestCluster,
) -> Result<()> {
    // 完整版需要集群支持，暂未实现
    run().await
}

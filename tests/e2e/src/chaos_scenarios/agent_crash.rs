//! Agent 崩溃混沌场景。
//!
//! 完整版：注入进程崩溃，验证 AgentSupervisor 自动重启，重启后状态恢复。
//! 简化版：验证 `ProcessChaos::inject_crash` 能执行（对无效 PID 不 panic）。

use anyhow::Result;
use eneros_test_utils::chaos::ProcessChaos;

/// 运行 Agent 崩溃混沌场景（简化版）。
///
/// 1. 注入重启延迟
/// 2. 验证 `inject_crash` 对无效 PID 能安全执行
/// 3. 停止重启延迟，验证恢复正常
pub async fn run() -> Result<()> {
    let chaos = ProcessChaos::new();

    // 注入重启延迟
    let handle = chaos
        .inject_restart_delay(100)
        .await
        .expect("restart delay injection should succeed");

    // 验证 inject_crash 能安全执行（使用无效 PID，预期失败但不 panic）
    // PID 0xFFFFFFFF 极不可能存在
    let _ = chaos.inject_crash(u32::MAX).await;

    // 停止重启延迟
    handle.stop().await;

    // 验证恢复后可再次注入
    let handle2 = chaos
        .inject_restart_delay(50)
        .await
        .expect("should be able to inject again after recovery");
    handle2.stop().await;

    Ok(())
}

/// 完整版 Agent 崩溃场景 — 需要运行中的集群。
///
/// TODO: 注入进程崩溃，验证：
/// - AgentSupervisor 检测到 Agent 退出
/// - 自动重启 Agent
/// - 重启后状态恢复（从持久化存储）
#[allow(dead_code)]
pub async fn run_with_cluster(
    _cluster: &crate::cluster::TestCluster,
) -> Result<()> {
    // 完整版需要集群支持，暂未实现
    run().await
}

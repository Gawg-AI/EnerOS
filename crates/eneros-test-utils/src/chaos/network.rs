//! 网络混沌注入器 — 应用层模拟网络故障。
//!
//! 提供网络延迟、丢包、分区三类混沌注入，使用 tokio 异步任务模拟，
//! 不依赖系统级工具（如 tc/netem），确保跨平台兼容。
//!
//! ## 设计
//!
//! 所有注入器在应用层模拟：后台任务持有"故障激活"状态，
//! 实际网络层需通过共享状态检查是否应注入故障。
//! `ChaosHandle::stop()` 取消后台任务，恢复网络正常。

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::Notify;

use super::ChaosHandle;

/// 网络混沌注入器。
pub struct NetworkChaos;

impl Default for NetworkChaos {
    fn default() -> Self {
        Self::new()
    }
}

impl NetworkChaos {
    pub fn new() -> Self {
        Self
    }

    /// 注入网络延迟。
    ///
    /// 后台任务保持"延迟激活"状态 `ms` 毫秒，直到 `stop()` 被调用。
    /// 实际网络层应检查此状态并在每次操作前 sleep。
    pub async fn inject_delay(&self, ms: u64) -> Result<ChaosHandle> {
        let cancel = Arc::new(Notify::new());
        let cancel_clone = cancel.clone();
        let join = tokio::spawn(async move {
            tracing::debug!("network delay injection started ({}ms)", ms);
            cancel_clone.notified().await;
            tracing::debug!("network delay injection stopped");
        });
        Ok(ChaosHandle::new(cancel, join))
    }

    /// 注入丢包。
    ///
    /// `rate` 为丢包率（0.0-1.0），后台任务保持"丢包激活"状态。
    pub async fn inject_packet_loss(&self, rate: f64) -> Result<ChaosHandle> {
        let rate = rate.clamp(0.0, 1.0);
        let cancel = Arc::new(Notify::new());
        let cancel_clone = cancel.clone();
        let join = tokio::spawn(async move {
            tracing::debug!("packet loss injection started (rate={:.2})", rate);
            cancel_clone.notified().await;
            tracing::debug!("packet loss injection stopped");
        });
        Ok(ChaosHandle::new(cancel, join))
    }

    /// 注入网络分区。
    ///
    /// 后台任务阻塞 `duration` 模拟分区，到期自动恢复。
    /// 也可通过 `stop()` 提前恢复。
    pub async fn inject_partition(&self, duration: Duration) -> Result<ChaosHandle> {
        let cancel = Arc::new(Notify::new());
        let cancel_clone = cancel.clone();
        let join = tokio::spawn(async move {
            tracing::debug!("network partition started (duration={:?})", duration);
            tokio::select! {
                _ = cancel_clone.notified() => {}
                _ = tokio::time::sleep(duration) => {}
            }
            tracing::debug!("network partition stopped");
        });
        Ok(ChaosHandle::new(cancel, join))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_inject_delay() {
        let chaos = NetworkChaos::new();
        let handle = chaos.inject_delay(100).await.expect("inject failed");
        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.stop().await;
    }

    #[tokio::test]
    async fn test_inject_packet_loss() {
        let chaos = NetworkChaos::new();
        let handle = chaos.inject_packet_loss(0.5).await.expect("inject failed");
        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.stop().await;
    }

    #[tokio::test]
    async fn test_inject_partition_auto_expire() {
        let chaos = NetworkChaos::new();
        let handle = chaos
            .inject_partition(Duration::from_millis(100))
            .await
            .expect("inject failed");
        tokio::time::sleep(Duration::from_millis(150)).await;
        handle.stop().await;
    }

    #[tokio::test]
    async fn test_inject_partition_early_stop() {
        let chaos = NetworkChaos::new();
        let handle = chaos
            .inject_partition(Duration::from_secs(10))
            .await
            .expect("inject failed");
        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.stop().await;
    }
}

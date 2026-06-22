//! 内存混沌注入器 — 应用层模拟内存压力。
//!
//! 通过分配大块内存并持有，模拟内存压力场景。
//! 后台任务可通过 `ChaosHandle::stop()` 释放内存。

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::Notify;

use super::ChaosHandle;

/// 内存混沌注入器。
pub struct MemoryChaos;

impl Default for MemoryChaos {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryChaos {
    pub fn new() -> Self {
        Self
    }

    /// 注入内存压力 — 分配并持有指定大小内存。
    ///
    /// `size_mb` 为分配大小（MB），`duration` 为持续时间。
    /// 到期或 `stop()` 后释放内存。
    pub async fn inject_memory_pressure(
        &self,
        size_mb: u64,
        duration: Duration,
    ) -> Result<ChaosHandle> {
        let cancel = Arc::new(Notify::new());
        let cancel_clone = cancel.clone();
        let join = tokio::spawn(async move {
            tracing::debug!("memory pressure started ({}MB, {:?})", size_mb, duration);
            let size = (size_mb as usize) * 1024 * 1024;
            let mut buffer = vec![0u8; size];
            // Touch all pages to ensure physical allocation
            for chunk in buffer.chunks_mut(4096) {
                chunk[0] = 1;
            }
            tokio::select! {
                _ = cancel_clone.notified() => {}
                _ = tokio::time::sleep(duration) => {}
            }
            buffer.clear();
            tracing::debug!("memory pressure stopped (buffer freed)");
        });

        Ok(ChaosHandle::new(cancel, join))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_inject_memory_pressure() {
        let chaos = MemoryChaos::new();
        let handle = chaos
            .inject_memory_pressure(1, Duration::from_millis(100))
            .await
            .expect("inject failed");
        handle.stop().await;
    }

    #[tokio::test]
    async fn test_inject_memory_pressure_auto_expire() {
        let chaos = MemoryChaos::new();
        let handle = chaos
            .inject_memory_pressure(1, Duration::from_millis(100))
            .await
            .expect("inject failed");
        tokio::time::sleep(Duration::from_millis(150)).await;
        handle.stop().await;
    }
}

//! CPU 混沌注入器 — 应用层模拟 CPU 饱和。
//!
//! 通过 busy loop 占用 CPU，支持按百分比和持续时间控制。
//! 后台任务可通过 `ChaosHandle::stop()` 取消。

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::Notify;

use super::ChaosHandle;

/// CPU 混沌注入器。
pub struct CpuChaos;

impl Default for CpuChaos {
    fn default() -> Self {
        Self::new()
    }
}

impl CpuChaos {
    pub fn new() -> Self {
        Self
    }

    /// 注入 CPU 饱和 — busy loop 占用指定百分比 CPU。
    ///
    /// `percent` 为 CPU 占比（0-100），`duration` 为持续时间。
    /// 每 10ms 一个周期：busy 占 `percent%`，idle 占剩余。
    /// `stop()` 提前终止。
    pub async fn inject_cpu_saturation(
        &self,
        percent: u8,
        duration: Duration,
    ) -> Result<ChaosHandle> {
        let percent = percent.min(100);
        let cancel = Arc::new(Notify::new());
        let cancel_clone = cancel.clone();
        let join = tokio::spawn(async move {
            tracing::debug!("cpu saturation started ({}%, {:?})", percent, duration);
            let start = Instant::now();
            let cycle = Duration::from_millis(10);
            let busy = Duration::from_millis((10 * percent as u64) / 100);
            let idle = cycle.checked_sub(busy).unwrap_or(Duration::ZERO);

            loop {
                if start.elapsed() >= duration {
                    break;
                }
                // Busy loop — spin to consume CPU
                let busy_start = Instant::now();
                while busy_start.elapsed() < busy {
                    std::hint::spin_loop();
                }
                // Idle — yield to allow cancellation
                if idle.is_zero() {
                    tokio::select! {
                        _ = cancel_clone.notified() => break,
                        _ = tokio::task::yield_now() => {}
                    }
                } else {
                    tokio::select! {
                        _ = cancel_clone.notified() => break,
                        _ = tokio::time::sleep(idle) => {}
                    }
                }
            }
            tracing::debug!("cpu saturation stopped");
        });

        Ok(ChaosHandle::new(cancel, join))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_inject_cpu_saturation_low() {
        let chaos = CpuChaos::new();
        let handle = chaos
            .inject_cpu_saturation(30, Duration::from_millis(100))
            .await
            .expect("inject failed");
        handle.stop().await;
    }

    #[tokio::test]
    async fn test_inject_cpu_saturation_full() {
        let chaos = CpuChaos::new();
        let handle = chaos
            .inject_cpu_saturation(100, Duration::from_millis(50))
            .await
            .expect("inject failed");
        handle.stop().await;
    }

    #[tokio::test]
    async fn test_inject_cpu_saturation_auto_expire() {
        let chaos = CpuChaos::new();
        let handle = chaos
            .inject_cpu_saturation(50, Duration::from_millis(100))
            .await
            .expect("inject failed");
        tokio::time::sleep(Duration::from_millis(150)).await;
        handle.stop().await;
    }
}

//! 进程混沌注入器 — 模拟进程崩溃和重启延迟。
//!
//! 进程崩溃通过系统命令终止指定 PID（Windows: taskkill, Linux: kill）。
//! 重启延迟通过后台任务模拟。跨平台兼容。

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::process::Command;
use tokio::sync::Notify;

use super::ChaosHandle;

/// 进程混沌注入器。
pub struct ProcessChaos;

impl Default for ProcessChaos {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessChaos {
    pub fn new() -> Self {
        Self
    }

    /// 注入进程崩溃 — 终止指定 PID 的进程。
    ///
    /// Windows 使用 `taskkill /PID <pid> /F`，Linux 使用 `kill -9 <pid>`。
    /// 这是即发即弃操作，不返回 `ChaosHandle`。
    pub async fn inject_crash(&self, pid: u32) -> Result<()> {
        let pid_str = pid.to_string();
        #[cfg(target_os = "windows")]
        {
            let output = Command::new("taskkill")
                .args(["/PID", &pid_str, "/F"])
                .output()
                .await
                .context("failed to execute taskkill")?;
            if !output.status.success() {
                tracing::warn!(
                    "taskkill returned non-zero: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
        #[cfg(unix)]
        {
            let output = Command::new("kill")
                .args(["-9", &pid_str])
                .output()
                .await
                .context("failed to execute kill")?;
            if !output.status.success() {
                tracing::warn!(
                    "kill returned non-zero: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
        #[cfg(not(any(target_os = "windows", unix)))]
        {
            let _ = pid_str;
            anyhow::bail!("process crash not supported on this platform");
        }
        Ok(())
    }

    /// 注入重启延迟 — 后台任务模拟重启等待时间。
    ///
    /// `delay_ms` 为延迟毫秒数。`stop()` 提前终止。
    pub async fn inject_restart_delay(&self, delay_ms: u64) -> Result<ChaosHandle> {
        let cancel = Arc::new(Notify::new());
        let cancel_clone = cancel.clone();
        let join = tokio::spawn(async move {
            tracing::debug!("restart delay started ({}ms)", delay_ms);
            tokio::select! {
                _ = cancel_clone.notified() => {}
                _ = tokio::time::sleep(Duration::from_millis(delay_ms)) => {}
            }
            tracing::debug!("restart delay stopped");
        });
        Ok(ChaosHandle::new(cancel, join))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_inject_restart_delay() {
        let chaos = ProcessChaos::new();
        let handle = chaos
            .inject_restart_delay(100)
            .await
            .expect("inject failed");
        handle.stop().await;
    }

    #[tokio::test]
    async fn test_inject_restart_delay_auto_expire() {
        let chaos = ProcessChaos::new();
        let handle = chaos
            .inject_restart_delay(50)
            .await
            .expect("inject failed");
        tokio::time::sleep(Duration::from_millis(100)).await;
        handle.stop().await;
    }

    #[tokio::test]
    async fn test_inject_crash_invalid_pid() {
        // 使用一个几乎肯定不存在的 PID，验证命令执行不 panic
        let chaos = ProcessChaos::new();
        // PID 0xFFFFFFFF 极不可能存在
        let _ = chaos.inject_crash(u32::MAX).await;
    }
}

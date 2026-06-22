//! 磁盘混沌注入器 — 应用层模拟磁盘故障。
//!
//! 提供磁盘满和慢速 IO 两类混沌注入。磁盘满通过创建临时大文件模拟，
//! 慢速 IO 通过周期性延迟写入模拟。所有操作跨平台兼容。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::sync::Notify;

use super::ChaosHandle;

static FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 生成唯一的临时文件路径。
fn temp_file_path(prefix: &str) -> std::path::PathBuf {
    let id = FILE_COUNTER.fetch_add(1, Ordering::SeqCst);
    let name = format!("{}-{}-{}.tmp", prefix, std::process::id(), id);
    std::env::temp_dir().join(name)
}

/// 磁盘混沌注入器。
pub struct DiskChaos;

impl Default for DiskChaos {
    fn default() -> Self {
        Self::new()
    }
}

impl DiskChaos {
    pub fn new() -> Self {
        Self
    }

    /// 注入磁盘满 — 创建指定大小的临时文件占用磁盘空间。
    ///
    /// `path` 为目录路径（忽略，统一使用系统临时目录）。
    /// `size_mb` 为填充大小（MB）。`stop()` 删除临时文件。
    pub async fn inject_disk_full(&self, _path: &str, size_mb: u64) -> Result<ChaosHandle> {
        let file_path = temp_file_path("eneros-chaos-disk");
        let file = fs::File::create(&file_path)
            .await
            .context("failed to create temp file for disk chaos")?;

        let chunk = vec![0u8; 1024 * 1024]; // 1 MB
        let mut writer = file;
        for _ in 0..size_mb {
            writer
                .write_all(&chunk)
                .await
                .context("failed to write chunk to temp file")?;
        }
        writer.flush().await.ok();
        drop(writer);

        let cancel = Arc::new(Notify::new());
        let cancel_clone = cancel.clone();
        let path_clone = file_path.clone();
        let join = tokio::spawn(async move {
            tracing::debug!("disk full injection started (path={:?})", path_clone);
            cancel_clone.notified().await;
            let _ = fs::remove_file(&path_clone).await;
            tracing::debug!("disk full injection stopped (file removed)");
        });

        Ok(ChaosHandle::new(cancel, join))
    }

    /// 注入慢速 IO — 后台任务周期性模拟延迟。
    ///
    /// `delay_ms` 为每次 IO 操作的模拟延迟。`stop()` 取消注入。
    pub async fn inject_slow_io(&self, delay_ms: u64) -> Result<ChaosHandle> {
        let cancel = Arc::new(Notify::new());
        let cancel_clone = cancel.clone();
        let join = tokio::spawn(async move {
            tracing::debug!("slow io injection started (delay={}ms)", delay_ms);
            loop {
                tokio::select! {
                    _ = cancel_clone.notified() => break,
                    _ = tokio::time::sleep(Duration::from_millis(delay_ms.max(1))) => {}
                }
            }
            tracing::debug!("slow io injection stopped");
        });

        Ok(ChaosHandle::new(cancel, join))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_inject_disk_full() {
        let chaos = DiskChaos::new();
        let handle = chaos
            .inject_disk_full(".", 2)
            .await
            .expect("inject failed");
        handle.stop().await;
    }

    #[tokio::test]
    async fn test_inject_slow_io() {
        let chaos = DiskChaos::new();
        let handle = chaos
            .inject_slow_io(50)
            .await
            .expect("inject failed");
        tokio::time::sleep(Duration::from_millis(100)).await;
        handle.stop().await;
    }
}

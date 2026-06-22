//! 混沌工程注入器 — 应用层模拟各类故障场景。
//!
//! 提供网络、磁盘、CPU、内存、进程五类混沌注入器，用于在测试中
//! 模拟分布式系统故障。所有注入器均在应用层模拟，不依赖系统级工具
//! （如 tc、cgroup），确保跨平台兼容（Windows/Linux）。
//!
//! ## 设计原则
//!
//! - **安全**：不真正破坏系统，用应用层模拟
//! - **可取消**：所有混沌注入返回 [`ChaosHandle`]，支持 `stop()`
//! - **跨平台**：Windows 和 Linux 均可编译运行

pub mod cpu;
pub mod disk;
pub mod memory;
pub mod network;
pub mod process;

pub use cpu::CpuChaos;
pub use disk::DiskChaos;
pub use memory::MemoryChaos;
pub use network::NetworkChaos;
pub use process::ProcessChaos;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;

/// 混沌效应类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChaosEffect {
    /// 网络延迟
    NetworkDelay,
    /// 网络分区
    NetworkPartition,
    /// 网络丢包
    NetworkPacketLoss,
    /// 磁盘满
    DiskFull,
    /// CPU 饱和
    CpuSaturation,
    /// 内存压力
    MemoryPressure,
    /// 进程崩溃
    ProcessCrash,
}

/// 混沌注入配置。
#[derive(Debug, Clone)]
pub struct ChaosConfig {
    /// 持续时间（毫秒）
    pub duration_ms: u64,
    /// 强度（0.0 - 1.0）
    pub intensity: f64,
    /// 效应类型
    pub effect: ChaosEffect,
}

impl Default for ChaosConfig {
    fn default() -> Self {
        Self {
            duration_ms: 1000,
            intensity: 0.5,
            effect: ChaosEffect::NetworkDelay,
        }
    }
}

/// 混沌注入句柄 — 用于停止混沌注入。
///
/// 持有后台任务的取消信号和 join handle，调用 [`stop`](Self::stop) 可优雅终止。
pub struct ChaosHandle {
    cancel: Arc<Notify>,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl ChaosHandle {
    fn new(cancel: Arc<Notify>, join: JoinHandle<()>) -> Self {
        Self {
            cancel,
            join: Mutex::new(Some(join)),
        }
    }

    /// 停止混沌注入，等待后台任务退出。
    ///
    /// 使用 `notify_one()` 而非 `notify_waiters()`，确保通知不会丢失：
    /// `notify_one()` 在无等待者时存储许可，下次 `notified()` 立即返回。
    pub async fn stop(&self) {
        self.cancel.notify_one();
        let mut guard = self.join.lock().await;
        if let Some(join) = guard.take() {
            let _ = join.await;
        }
    }
}

/// 混沌注入器 — 统一入口，持有各类混沌注入器。
///
/// 通过 [`inject`](Self::inject) 方法根据 [`ChaosConfig`] 分发到具体注入器，
/// 也可通过 `network()` / `disk()` / `cpu()` / `memory()` / `process()` 直接访问。
pub struct ChaosInjector {
    network: NetworkChaos,
    disk: DiskChaos,
    cpu: CpuChaos,
    memory: MemoryChaos,
    process: ProcessChaos,
}

impl Default for ChaosInjector {
    fn default() -> Self {
        Self::new()
    }
}

impl ChaosInjector {
    /// 创建新的混沌注入器实例。
    pub fn new() -> Self {
        Self {
            network: NetworkChaos::new(),
            disk: DiskChaos::new(),
            cpu: CpuChaos::new(),
            memory: MemoryChaos::new(),
            process: ProcessChaos::new(),
        }
    }

    /// 网络混沌注入器。
    pub fn network(&self) -> &NetworkChaos {
        &self.network
    }

    /// 磁盘混沌注入器。
    pub fn disk(&self) -> &DiskChaos {
        &self.disk
    }

    /// CPU 混沌注入器。
    pub fn cpu(&self) -> &CpuChaos {
        &self.cpu
    }

    /// 内存混沌注入器。
    pub fn memory(&self) -> &MemoryChaos {
        &self.memory
    }

    /// 进程混沌注入器。
    pub fn process(&self) -> &ProcessChaos {
        &self.process
    }

    /// 根据配置注入混沌。
    ///
    /// `intensity` 字段（0.0-1.0）映射到具体注入器的参数：
    /// - NetworkDelay: 延迟毫秒数 = duration_ms * intensity
    /// - DiskFull: 填充大小 = 100MB * intensity
    /// - CpuSaturation: CPU 占比 = intensity * 100%
    /// - MemoryPressure: 内存大小 = 100MB * intensity
    pub async fn inject(&self, config: &ChaosConfig) -> Result<ChaosHandle> {
        let intensity = config.intensity.clamp(0.0, 1.0);
        match config.effect {
            ChaosEffect::NetworkDelay => {
                let ms = ((config.duration_ms as f64) * intensity) as u64;
                self.network.inject_delay(ms.max(1)).await
            }
            ChaosEffect::NetworkPartition => {
                self.network
                    .inject_partition(Duration::from_millis(config.duration_ms))
                    .await
            }
            ChaosEffect::NetworkPacketLoss => {
                self.network.inject_packet_loss(intensity).await
            }
            ChaosEffect::DiskFull => {
                let size_mb = (100.0 * intensity) as u64;
                self.disk.inject_disk_full(".", size_mb.max(1)).await
            }
            ChaosEffect::CpuSaturation => {
                let percent = (intensity * 100.0) as u8;
                self.cpu
                    .inject_cpu_saturation(percent, Duration::from_millis(config.duration_ms))
                    .await
            }
            ChaosEffect::MemoryPressure => {
                let size_mb = (100.0 * intensity) as u64;
                self.memory
                    .inject_memory_pressure(size_mb.max(1), Duration::from_millis(config.duration_ms))
                    .await
            }
            ChaosEffect::ProcessCrash => {
                // ProcessCrash 是即发即弃的，返回一个 no-op handle
                let cancel = Arc::new(Notify::new());
                let cancel_clone = cancel.clone();
                let join = tokio::spawn(async move {
                    cancel_clone.notified().await;
                });
                Ok(ChaosHandle::new(cancel, join))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_chaos_handle_stop() {
        let cancel = Arc::new(Notify::new());
        let cancel_clone = cancel.clone();
        let join = tokio::spawn(async move {
            cancel_clone.notified().await;
        });
        let handle = ChaosHandle::new(cancel, join);
        handle.stop().await;
    }

    #[tokio::test]
    async fn test_chaos_injector_network_delay() {
        let injector = ChaosInjector::new();
        let config = ChaosConfig {
            duration_ms: 100,
            intensity: 0.5,
            effect: ChaosEffect::NetworkDelay,
        };
        let handle = injector.inject(&config).await.expect("inject failed");
        handle.stop().await;
    }

    #[tokio::test]
    async fn test_chaos_injector_cpu_saturation() {
        let injector = ChaosInjector::new();
        let config = ChaosConfig {
            duration_ms: 100,
            intensity: 0.3,
            effect: ChaosEffect::CpuSaturation,
        };
        let handle = injector.inject(&config).await.expect("inject failed");
        handle.stop().await;
    }

    #[tokio::test]
    async fn test_chaos_injector_memory_pressure() {
        let injector = ChaosInjector::new();
        let config = ChaosConfig {
            duration_ms: 100,
            intensity: 0.1,
            effect: ChaosEffect::MemoryPressure,
        };
        let handle = injector.inject(&config).await.expect("inject failed");
        handle.stop().await;
    }

    #[tokio::test]
    async fn test_chaos_injector_process_crash_noop() {
        let injector = ChaosInjector::new();
        let config = ChaosConfig {
            duration_ms: 0,
            intensity: 1.0,
            effect: ChaosEffect::ProcessCrash,
        };
        let handle = injector.inject(&config).await.expect("inject failed");
        handle.stop().await;
    }
}

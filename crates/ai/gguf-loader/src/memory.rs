//! 模型内存管理器（D5）.
//!
//! [`ModelMemoryManager`] 跟踪 CPU/GPU 侧已加载模型占用的内存与模型数量。
//! 使用普通 `u64`/`u32` 计数（单线程无需原子操作，D5），卸载时用
//! `saturating_sub` 防止下溢。设备归属通过 [`ComputeDevice::is_gpu`] 判定。

use eneros_llm_engine::device::ComputeDevice;

/// 内存使用统计（D5）.
#[derive(Debug, Clone, Default)]
pub struct MemoryStats {
    /// CPU 侧占用字节数.
    pub cpu_bytes: u64,
    /// GPU 侧占用字节数.
    pub gpu_bytes: u64,
    /// 已加载模型数量.
    pub model_count: u32,
}

/// 模型内存管理器（D5）.
pub struct ModelMemoryManager {
    stats: MemoryStats,
}

impl ModelMemoryManager {
    /// 构造空的管理器.
    pub fn new() -> Self {
        Self {
            stats: MemoryStats::default(),
        }
    }

    /// 记录一次模型加载（累加对应设备内存与模型计数）.
    pub fn record_load(&mut self, device: ComputeDevice, bytes: u64) {
        if device.is_gpu() {
            self.stats.gpu_bytes += bytes;
        } else {
            self.stats.cpu_bytes += bytes;
        }
        self.stats.model_count += 1;
    }

    /// 记录一次模型卸载（扣减对应设备内存与模型计数，saturating 防下溢）.
    pub fn record_unload(&mut self, device: ComputeDevice, bytes: u64) {
        if device.is_gpu() {
            self.stats.gpu_bytes = self.stats.gpu_bytes.saturating_sub(bytes);
        } else {
            self.stats.cpu_bytes = self.stats.cpu_bytes.saturating_sub(bytes);
        }
        if self.stats.model_count > 0 {
            self.stats.model_count -= 1;
        }
    }

    /// 返回当前内存统计的不可变引用.
    pub fn stats(&self) -> &MemoryStats {
        &self.stats
    }
}

impl Default for ModelMemoryManager {
    fn default() -> Self {
        Self::new()
    }
}

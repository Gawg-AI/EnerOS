//! KV Cache 元数据管理器（D4：元数据跟踪，非实际 GPU 分配）.
//!
//! `KvCacheManager` 跟踪每个推理请求的 KV Cache 大小（按 token 数估算），
//! 在预算超限时淘汰最旧条目。实际 GPU 内存分配由 llama.cpp 通过
//! `n_gpu_layers` 参数完成（v0.59.0 `ComputeDevice`），本模块仅做元数据管理。

use alloc::vec::Vec;

use crate::error::SchedulerError;

/// KV Cache 条目（元数据）.
#[derive(Debug, Clone)]
pub struct KvCacheEntry {
    /// 关联的请求 ID.
    pub request_id: u64,
    /// 上下文长度（token 数）.
    pub context_length: u32,
    /// 缓存大小（字节）.
    pub size_bytes: u64,
}

/// KV Cache 元数据管理器.
///
/// 按 512KB/token 估算缓存大小（D4），在 `max_cache_size` 预算内分配/释放/淘汰。
#[derive(Debug, Clone)]
pub struct KvCacheManager {
    entries: Vec<KvCacheEntry>,
    max_cache_size: u64,
    current_size: u64,
}

impl KvCacheManager {
    /// 创建 KV Cache 管理器.
    ///
    /// `max_cache_size` 为缓存预算上限（字节）。
    pub fn new(max_cache_size: u64) -> Self {
        Self {
            entries: Vec::new(),
            max_cache_size,
            current_size: 0,
        }
    }

    /// 计算指定上下文长度的缓存大小（512KB/token，D4）.
    pub fn calculate_cache_size(context_length: u32) -> u64 {
        (context_length as u64) * 512 * 1024
    }

    /// 分配 KV Cache 条目，返回缓存大小（字节）.
    ///
    /// 超预算时先淘汰最旧条目；淘汰后仍超预算则返回 `CacheExhausted`。
    pub fn allocate(
        &mut self,
        request_id: u64,
        context_length: u32,
    ) -> Result<u64, SchedulerError> {
        let size = Self::calculate_cache_size(context_length);

        // 超预算时淘汰最旧条目
        while self.current_size + size > self.max_cache_size && !self.entries.is_empty() {
            self.evict_oldest();
        }

        // 淘汰后仍超预算且无条目可淘汰，返回错误
        if self.current_size + size > self.max_cache_size {
            return Err(SchedulerError::CacheExhausted);
        }

        self.entries.push(KvCacheEntry {
            request_id,
            context_length,
            size_bytes: size,
        });
        self.current_size += size;
        Ok(size)
    }

    /// 释放指定请求 ID 的 KV Cache 条目，返回是否找到并释放.
    pub fn release(&mut self, request_id: u64) -> bool {
        if let Some(pos) = self.entries.iter().position(|e| e.request_id == request_id) {
            let entry = self.entries.remove(pos);
            self.current_size = self.current_size.saturating_sub(entry.size_bytes);
            true
        } else {
            false
        }
    }

    /// 淘汰最旧（首个）条目，返回是否淘汰成功.
    pub fn evict_oldest(&mut self) -> bool {
        if let Some(entry) = self.entries.first().cloned() {
            self.entries.remove(0);
            self.current_size = self.current_size.saturating_sub(entry.size_bytes);
            true
        } else {
            false
        }
    }

    /// 当前缓存使用量（字节）.
    pub fn current_size(&self) -> u64 {
        self.current_size
    }

    /// 缓存预算上限（字节）.
    pub fn max_size(&self) -> u64 {
        self.max_cache_size
    }

    /// 当前条目数.
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }
}

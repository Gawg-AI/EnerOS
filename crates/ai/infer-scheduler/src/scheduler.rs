//! LLM 推理调度器（D2 轮询式，D4 KV Cache 元数据跟踪，D6 now_ns 注入）.
//!
//! 单线程 no_std 轮询式调度器，管理推理请求队列、并发控制（≤2）、KV Cache 元数据。
//! 调用方通过 [`InferScheduler::tick`] 驱动调度：超时清理 → 优先级排序 → 派发执行。

use alloc::collections::VecDeque;
use alloc::vec::Vec;

use eneros_llm_engine::{ComputeDevice, LlmEngine};

use crate::cache::KvCacheManager;
use crate::error::SchedulerError;
use crate::request::InferRequest;
use crate::result::InferResult;
use crate::stats::SchedulerStats;

/// LLM 推理调度器.
///
/// 单线程 no_std 轮询式调度器，管理推理请求队列、并发控制（≤2）、KV Cache 元数据。
/// 调用方通过 [`tick`](Self::tick) 驱动调度：超时清理 → 优先级排序 → 派发执行。
pub struct InferScheduler {
    queue: VecDeque<InferRequest>,
    active_count: u8,
    max_concurrent: u8,
    kv_cache: KvCacheManager,
    stats: SchedulerStats,
    next_request_id: u64,
    device: ComputeDevice,
}

impl InferScheduler {
    /// 创建调度器.
    ///
    /// `max_concurrent` 应 ≤ 2（GPU VRAM 约束）。`max_cache_bytes` 为 KV Cache 预算上限。
    pub fn new(max_concurrent: u8, max_cache_bytes: u64, device: ComputeDevice) -> Self {
        Self {
            queue: VecDeque::new(),
            active_count: 0,
            max_concurrent,
            kv_cache: KvCacheManager::new(max_cache_bytes),
            stats: SchedulerStats::default(),
            next_request_id: 1,
            device,
        }
    }

    /// 提交请求到队列，返回调度器分配的请求 ID.
    ///
    /// 请求的 `id` 字段会被调度器分配的 ID 覆盖。
    pub fn submit(&mut self, mut req: InferRequest) -> u64 {
        req.id = self.next_request_id;
        self.next_request_id += 1;
        let id = req.id;
        self.queue.push_back(req);
        self.stats.record_submit();
        id
    }

    /// 轮询调度器：清理超时请求，按优先级派发最多 `max_concurrent` 个请求.
    ///
    /// 返回本次 tick 中完成的请求结果（成功/超时/失败）。
    pub fn tick(&mut self, now_ns: u64, engine: &mut dyn LlmEngine) -> Vec<InferResult> {
        let mut results = Vec::new();

        // 1. 分离超时请求，收集存活请求
        let mut active: Vec<InferRequest> = Vec::new();
        while let Some(req) = self.queue.pop_front() {
            if req.is_timed_out(now_ns) {
                self.stats.record_timeout();
                results.push(InferResult::failure(req.id, SchedulerError::Timeout));
            } else {
                active.push(req);
            }
        }

        // 2. 按优先级降序排序（Critical 优先）
        active.sort_by_key(|r| core::cmp::Reverse(r.priority));

        // 将排序后的请求放回队列
        for req in active {
            self.queue.push_back(req);
        }

        // 3. 派发最多 (max_concurrent - active_count) 个请求
        let slots = (self.max_concurrent as usize).saturating_sub(self.active_count as usize);
        for _ in 0..slots {
            let req = match self.queue.pop_front() {
                Some(r) => r,
                None => break,
            };

            // 分配 KV Cache（以 params.max_tokens 估算上下文长度）
            let context_length = req.params.max_tokens;
            match self.kv_cache.allocate(req.id, context_length) {
                Ok(_size) => {
                    self.active_count += 1;
                    // 执行推理
                    match engine.infer(&req.prompt, &req.params) {
                        Ok(output) => {
                            self.kv_cache.release(req.id);
                            self.active_count -= 1;
                            self.stats.record_complete();
                            results.push(InferResult::success(req.id, output));
                        }
                        Err(e) => {
                            self.kv_cache.release(req.id);
                            self.active_count -= 1;
                            self.stats.record_failure();
                            let err = SchedulerError::from(e);
                            results.push(InferResult::failure(req.id, err));
                        }
                    }
                }
                Err(err) => {
                    // KV Cache 耗尽
                    self.stats.record_failure();
                    results.push(InferResult::failure(req.id, err));
                }
            }
        }

        results
    }

    /// 当前队列长度.
    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }

    /// 当前活跃请求数.
    pub fn active_count(&self) -> u8 {
        self.active_count
    }

    /// 调度器统计.
    pub fn stats(&self) -> &SchedulerStats {
        &self.stats
    }

    /// KV Cache 管理器.
    pub fn kv_cache(&self) -> &KvCacheManager {
        &self.kv_cache
    }

    /// 计算设备.
    pub fn device(&self) -> ComputeDevice {
        self.device
    }

    /// 最大并发数.
    pub fn max_concurrent(&self) -> u8 {
        self.max_concurrent
    }
}

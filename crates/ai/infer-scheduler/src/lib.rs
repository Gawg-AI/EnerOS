//! EnerOS LLM 推理调度器（v0.62.0）.
//!
//! 双脑架构（LLM + Solver）的 LLM 是"感知者"，本 crate 提供推理请求的调度、
//! 并发控制（≤2，GPU VRAM 约束）与 KV Cache 元数据跟踪。单线程 no_std 轮询式
//! 调度器，调用方通过 `tick(now_ns, engine)` 驱动：超时清理 → 优先级排序 → 派发执行。
//!
//! # 核心类型
//!
//! - [`scheduler::InferScheduler`] — 推理调度器（D2 轮询式，D4 KV Cache 元数据，D6 now_ns 注入）
//! - [`request::InferRequest`] — 推理请求（优先级 / 时间戳 / 超时）
//! - [`result::InferResult`] — 推理结果（成功输出或错误）
//! - [`cache::KvCacheManager`] — KV Cache 元数据管理器（D4，512KB/token 估算）
//! - [`priority::RequestPriority`] — 请求优先级（Low / Normal / High / Critical）
//! - [`stats::SchedulerStats`] — 调度器统计（D5 普通 u64）
//! - [`error::SchedulerError`] — 调度器错误（D7 5 变体 + From<LlmError>）
//!
//! # 依赖关系（D11）
//!
//! 复用 v0.59.0 `eneros-llm-engine` 类型，不重定义：
//! - `eneros_llm_engine::LlmEngine`（推理引擎 trait）
//! - `eneros_llm_engine::MockEngine`（默认 Mock 引擎，测试用）
//! - `eneros_llm_engine::ComputeDevice`（Cpu / Cuda / Metal / Npu）
//! - `eneros_llm_engine::InferParams`（推理参数）
//! - `eneros_llm_engine::LlmError`（引擎错误，转换为 `SchedulerError`）
//!
//! 不依赖 v0.60.0 `eneros-gguf-loader` 或 v0.61.0 `eneros-model-deploy`。
//!
//! # 偏差声明（D1~D12）
//!
//! | 偏差 | 说明 |
//! |------|------|
//! | **D1** | no_std 合规：`alloc::collections::VecDeque` / `alloc::string::String` / `alloc::vec::Vec` 替代 `std::*`；`#![cfg_attr(not(test), no_std)]` + `extern crate alloc`；子模块不重复 no_std 声明 |
//! | **D2** | 轮询式 `tick(now_ns, engine) -> Vec<InferResult>` 替代 async callback（单线程 no_std 无真实并发） |
//! | **D3** | 无 feature 门控（KV Cache 为元数据跟踪，无 FFI；`Cargo.toml` 无 `[features]`） |
//! | **D4** | KV Cache 作为元数据跟踪器（`allocate` 返回 `size_bytes`，非 `*mut u8`；实际 GPU 分配由 llama.cpp 通过 `n_gpu_layers` 完成） |
//! | **D5** | `SchedulerStats` 用普通 `u64`，不使用 `AtomicU64`（单线程无需） |
//! | **D6** | `now_ns: u64` 注入（无 `MonotonicTime::now()`；`timeout_ns` 为 `u64` 纳秒） |
//! | **D7** | `SchedulerError` 5 变体（QueueFull / Timeout / CacheExhausted / Engine / NotScheduled）；派生 `Debug`/`Clone`，手动实现 `PartialEq`（`LlmError` 未派生），实现 `core::fmt::Display` + `From<LlmError>` |
//! | **D8** | `InferScheduler` **不**实现 `Drop`（队列由 Rust 所有权自动释放） |
//! | **D9** | crate 位置 `crates/ai/infer-scheduler/`（AI 子系统；项目规则 §2.3.1） |
//! | **D10** | 不声明 FFI，无 `unsafe` 块（纯安全 Rust） |
//! | **D11** | 复用 v0.59.0 类型（`LlmEngine` / `InferParams` / `LlmError` / `ComputeDevice`）；不依赖 v0.60.0 / v0.61.0 |
//! | **D12** | 无 Mock 后端（`KvCacheManager` 直接实例化，纯元数据管理） |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，可交叉编译到 `aarch64-unknown-none`。
//! 不引入任何 `std::*`，不调用 `panic!` / `todo!` / `unimplemented!`，无 `unsafe` 块。

#![cfg_attr(not(test), no_std)]
#![allow(dead_code)]

extern crate alloc;

pub mod cache;
pub mod error;
pub mod priority;
pub mod request;
pub mod result;
pub mod scheduler;
pub mod stats;

pub use cache::{KvCacheEntry, KvCacheManager};
pub use error::SchedulerError;
pub use priority::RequestPriority;
pub use request::InferRequest;
pub use result::InferResult;
pub use scheduler::InferScheduler;
pub use stats::SchedulerStats;

#[cfg(test)]
mod tests {
    //! 集成测试 T1~T15（覆盖 D1~D12 偏差声明）.
    //!
    //! 全部使用 v0.59.0 `MockEngine`（无 feature-gated 依赖）。

    use alloc::vec;

    use eneros_llm_engine::{ComputeDevice, InferParams, LlmEngine, MockEngine};

    use super::*;
    use crate::cache::KvCacheManager;
    use crate::error::SchedulerError;
    use crate::priority::RequestPriority;
    use crate::request::InferRequest;
    use crate::scheduler::InferScheduler;

    /// 1GB 缓存预算（测试默认值，足够容纳 max_tokens=128 的请求）.
    const TEST_CACHE_BUDGET: u64 = 1024 * 1024 * 1024;

    // ===== T1：InferRequest::new 默认值（priority=Normal, submitted_at_ns=0, timeout_ns=u64::MAX）=====
    #[test]
    fn test_t1_infer_request_defaults() {
        let req = InferRequest::new(42, "hello", InferParams::default());
        assert_eq!(req.id, 42);
        assert_eq!(req.prompt, "hello");
        assert_eq!(req.priority, RequestPriority::Normal);
        assert_eq!(req.submitted_at_ns, 0);
        assert_eq!(req.timeout_ns, u64::MAX);
    }

    // ===== T2：InferRequest builder（with_priority / with_timeout / with_timestamp）=====
    #[test]
    fn test_t2_infer_request_builder() {
        let req = InferRequest::new(1, "test", InferParams::default())
            .with_priority(RequestPriority::Critical)
            .with_timeout(1000)
            .with_timestamp(5000);
        assert_eq!(req.priority, RequestPriority::Critical);
        assert_eq!(req.timeout_ns, 1000);
        assert_eq!(req.submitted_at_ns, 5000);
    }

    // ===== T3：InferRequest::is_timed_out（submitted_at=0, timeout=1000）=====
    #[test]
    fn test_t3_infer_request_is_timed_out() {
        let req = InferRequest::new(1, "test", InferParams::default())
            .with_timeout(1000)
            .with_timestamp(0);
        // now=500: 500 - 0 = 500 <= 1000，未超时
        assert!(!req.is_timed_out(500));
        // now=2000: 2000 - 0 = 2000 > 1000，超时
        assert!(req.is_timed_out(2000));
    }

    // ===== T4：RequestPriority Ord 排序（Critical > High > Normal > Low）=====
    #[test]
    fn test_t4_request_priority_ord() {
        let mut priorities = vec![
            RequestPriority::Normal,
            RequestPriority::Critical,
            RequestPriority::Low,
            RequestPriority::High,
        ];
        priorities.sort();
        assert_eq!(
            priorities,
            vec![
                RequestPriority::Low,
                RequestPriority::Normal,
                RequestPriority::High,
                RequestPriority::Critical,
            ]
        );
        // 显式验证偏序关系
        assert!(RequestPriority::Critical > RequestPriority::High);
        assert!(RequestPriority::High > RequestPriority::Normal);
        assert!(RequestPriority::Normal > RequestPriority::Low);
    }

    // ===== T5：InferScheduler::new 初始状态（queue_len=0, active_count=0, stats 全 0）=====
    #[test]
    fn test_t5_scheduler_initial_state() {
        let sched = InferScheduler::new(2, TEST_CACHE_BUDGET, ComputeDevice::Cpu);
        assert_eq!(sched.queue_len(), 0);
        assert_eq!(sched.active_count(), 0);
        assert_eq!(sched.max_concurrent(), 2);
        assert_eq!(sched.device(), ComputeDevice::Cpu);
        assert_eq!(sched.stats().total_requests, 0);
        assert_eq!(sched.stats().completed_requests, 0);
        assert_eq!(sched.stats().timed_out_requests, 0);
        assert_eq!(sched.stats().failed_requests, 0);
        assert_eq!(sched.stats().cache_evictions, 0);
    }

    // ===== T6：submit 返回递增唯一 ID（1, 2, 3）=====
    #[test]
    fn test_t6_submit_incrementing_ids() {
        let mut sched = InferScheduler::new(2, TEST_CACHE_BUDGET, ComputeDevice::Cpu);
        let id1 = sched.submit(InferRequest::new(0, "a", InferParams::default()));
        let id2 = sched.submit(InferRequest::new(0, "b", InferParams::default()));
        let id3 = sched.submit(InferRequest::new(0, "c", InferParams::default()));
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(id3, 3);
        assert_eq!(sched.queue_len(), 3);
        assert_eq!(sched.stats().total_requests, 3);
    }

    // ===== T7：tick 空队列返回空 Vec =====
    #[test]
    fn test_t7_tick_empty_queue() {
        let mut engine = MockEngine::new(ComputeDevice::Cpu);
        engine.load_model("/test.gguf").unwrap();
        let mut sched = InferScheduler::new(2, TEST_CACHE_BUDGET, ComputeDevice::Cpu);
        let results = sched.tick(0, &mut engine);
        assert!(results.is_empty());
    }

    // ===== T8：tick 单请求 → 执行推理，返回 success，队列清空 =====
    #[test]
    fn test_t8_tick_single_request() {
        let mut engine = MockEngine::new(ComputeDevice::Cpu);
        engine.load_model("/test.gguf").unwrap();
        let mut sched = InferScheduler::new(2, TEST_CACHE_BUDGET, ComputeDevice::Cpu);
        let id = sched.submit(InferRequest::new(0, "hello", InferParams::default()));
        let results = sched.tick(0, &mut engine);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, id);
        assert!(results[0].result.is_ok());
        assert_eq!(results[0].result.as_ref().unwrap(), "mock: hello");
        assert_eq!(sched.queue_len(), 0);
        assert_eq!(sched.stats().completed_requests, 1);
    }

    // ===== T9：tick max_concurrent=2 且 3 请求 → 处理 2，队列剩 1 =====
    #[test]
    fn test_t9_tick_max_concurrent_limit() {
        let mut engine = MockEngine::new(ComputeDevice::Cpu);
        engine.load_model("/test.gguf").unwrap();
        let mut sched = InferScheduler::new(2, TEST_CACHE_BUDGET, ComputeDevice::Cpu);
        sched.submit(InferRequest::new(0, "a", InferParams::default()));
        sched.submit(InferRequest::new(0, "b", InferParams::default()));
        sched.submit(InferRequest::new(0, "c", InferParams::default()));
        let results = sched.tick(0, &mut engine);
        assert_eq!(results.len(), 2);
        assert_eq!(sched.queue_len(), 1);
        assert_eq!(sched.stats().completed_requests, 2);
    }

    // ===== T10：tick 超时请求 → 返回 Timeout 错误，stats.timed_out_requests=1 =====
    #[test]
    fn test_t10_tick_timed_out() {
        let mut engine = MockEngine::new(ComputeDevice::Cpu);
        engine.load_model("/test.gguf").unwrap();
        let mut sched = InferScheduler::new(2, TEST_CACHE_BUDGET, ComputeDevice::Cpu);
        let req = InferRequest::new(0, "timeout", InferParams::default())
            .with_timeout(1000)
            .with_timestamp(0);
        let id = sched.submit(req);
        // now=2000: 2000 - 0 = 2000 > 1000，超时
        let results = sched.tick(2000, &mut engine);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, id);
        assert!(matches!(results[0].result, Err(SchedulerError::Timeout)));
        assert_eq!(sched.stats().timed_out_requests, 1);
        assert_eq!(sched.queue_len(), 0);
    }

    // ===== T11：tick 优先级排序（先提交 Normal，后提交 High；tick 先执行 High）=====
    #[test]
    fn test_t11_tick_priority_sorting() {
        let mut engine = MockEngine::new(ComputeDevice::Cpu);
        engine.load_model("/test.gguf").unwrap();
        // max_concurrent=1：每次 tick 只处理 1 个，验证优先级排序
        let mut sched = InferScheduler::new(1, TEST_CACHE_BUDGET, ComputeDevice::Cpu);
        let _id_normal = sched.submit(
            InferRequest::new(0, "normal", InferParams::default())
                .with_priority(RequestPriority::Normal),
        );
        let id_high = sched.submit(
            InferRequest::new(0, "high", InferParams::default())
                .with_priority(RequestPriority::High),
        );
        let results = sched.tick(0, &mut engine);
        // High 优先级应先执行
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, id_high);
        assert_eq!(results[0].result.as_ref().unwrap(), "mock: high");
        // Normal 仍在队列中
        assert_eq!(sched.queue_len(), 1);
    }

    // ===== T12：KvCacheManager allocate + release（current_size 增减）=====
    #[test]
    fn test_t12_kv_cache_allocate_release() {
        let mut cache = KvCacheManager::new(TEST_CACHE_BUDGET);
        let size = cache.allocate(1, 10).unwrap();
        assert_eq!(size, 10 * 512 * 1024);
        assert_eq!(cache.current_size(), size);
        assert_eq!(cache.entry_count(), 1);

        // 释放存在的条目
        let released = cache.release(1);
        assert!(released);
        assert_eq!(cache.current_size(), 0);
        assert_eq!(cache.entry_count(), 0);

        // 释放不存在的条目
        let released2 = cache.release(999);
        assert!(!released2);
    }

    // ===== T13：KvCacheManager evict_oldest（2 条目超预算，最旧被淘汰）=====
    #[test]
    fn test_t13_kv_cache_evict_oldest() {
        // 预算 = 1 token 缓存大小（512KB）
        let mut cache = KvCacheManager::new(512 * 1024);

        // 第 1 次分配：恰好等于预算，成功
        cache.allocate(1, 1).unwrap();
        assert_eq!(cache.entry_count(), 1);

        // 第 2 次分配：超预算 → 淘汰 id=1，然后成功
        cache.allocate(2, 1).unwrap();
        assert_eq!(cache.entry_count(), 1);

        // id=1 已被淘汰（release 返回 false）
        assert!(!cache.release(1));
        // id=2 仍存在
        assert!(cache.release(2));
    }

    // ===== T14：KvCacheManager CacheExhausted（单条目大于 max_cache_size）=====
    #[test]
    fn test_t14_kv_cache_exhausted() {
        // 预算 = 1024 字节（远小于 1 token 的 512KB）
        let mut cache = KvCacheManager::new(1024);
        let result = cache.allocate(1, 1); // 512KB >> 1024
        assert!(matches!(result, Err(SchedulerError::CacheExhausted)));
        assert_eq!(cache.entry_count(), 0);
        assert_eq!(cache.current_size(), 0);
    }

    // ===== T15：SchedulerStats 完整流程（submit 3 → tick → 验证统计）=====
    #[test]
    fn test_t15_stats_full_flow() {
        let mut engine = MockEngine::new(ComputeDevice::Cpu);
        engine.load_model("/test.gguf").unwrap();
        let mut sched = InferScheduler::new(2, TEST_CACHE_BUDGET, ComputeDevice::Cpu);

        // 提交 3 个请求
        sched.submit(InferRequest::new(0, "a", InferParams::default()));
        sched.submit(InferRequest::new(0, "b", InferParams::default()));
        sched.submit(InferRequest::new(0, "c", InferParams::default()));
        assert_eq!(sched.stats().total_requests, 3);

        // tick：处理 2 个，剩 1 个在队列
        let results = sched.tick(0, &mut engine);
        assert_eq!(results.len(), 2);
        assert_eq!(sched.stats().completed_requests, 2);
        assert_eq!(sched.queue_len(), 1);
        assert_eq!(sched.stats().failed_requests, 0);
        assert_eq!(sched.stats().timed_out_requests, 0);

        // 验证所有结果均为成功
        for r in &results {
            assert!(r.result.is_ok());
        }

        // 第二次 tick：处理剩余 1 个
        let results2 = sched.tick(0, &mut engine);
        assert_eq!(results2.len(), 1);
        assert_eq!(sched.stats().completed_requests, 3);
        assert_eq!(sched.queue_len(), 0);
    }
}

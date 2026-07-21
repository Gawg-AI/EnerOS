# Checklist

## Workspace 同步
- [x] C1 根 `Cargo.toml` 版本号已更新为 `0.62.0`
- [x] C2 members 列表已添加 `crates/ai/infer-scheduler`
- [x] C3 `cargo metadata --format-version 1` 成功

## Crate 骨架
- [x] C4 `crates/ai/infer-scheduler/Cargo.toml` 存在，package name = `eneros-infer-scheduler`
- [x] C5 dependencies 仅包含 `eneros-llm-engine = { path = "../llm-engine" }`（D11，不依赖 v0.60.0/v0.61.0）
- [x] C6 **不声明** `[features]`（D3：无 feature 门控，纯 Rust）
- [x] C7 `src/lib.rs` 包含 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- [x] C8 `src/lib.rs` 包含 D1~D12 偏差声明表
- [x] C9 模块声明：error / request / result / priority / cache / stats / scheduler

## error.rs — SchedulerError
- [x] C10 `SchedulerError` 枚举包含 5 变体（QueueFull / Timeout / CacheExhausted / Engine(LlmError) / NotScheduled）
- [x] C11 派生 `Debug` + `Clone` + `PartialEq`
- [x] C12 实现 `core::fmt::Display`
- [x] C13 实现 `From<LlmError> for SchedulerError`（D7）

## priority.rs — RequestPriority
- [x] C14 `RequestPriority` 枚举：Low / Normal / High / Critical
- [x] C15 派生 `Debug` + `Clone` + `Copy` + `PartialEq` + `Eq` + `PartialOrd` + `Ord`
- [x] C16 实现 `Default`（`Normal`）
- [x] C17 `Ord` 排序：Critical > High > Normal > Low

## request.rs — InferRequest
- [x] C18 `InferRequest` 结构体：id / prompt / params / priority / submitted_at_ns / timeout_ns（D6）
- [x] C19 派生 `Debug` + `Clone`
- [x] C20 `InferRequest::new(id, prompt, params) -> Self`（默认 Normal, submitted_at_ns=0, timeout_ns=u64::MAX）
- [x] C21 `with_priority` / `with_timeout` / `with_timestamp` builder 方法
- [x] C22 `is_timed_out(&self, now_ns: u64) -> bool`（`now_ns.saturating_sub(submitted_at_ns) > timeout_ns`）
- [x] C23 单元测试：构造 / builder / is_timed_out

## result.rs — InferResult
- [x] C24 `InferResult` 结构体：id: u64 / result: Result<String, SchedulerError>
- [x] C25 派生 `Debug` + `Clone`
- [x] C26 `InferResult::new(id, result)` / `success(id, output)` / `failure(id, error)` 构造
- [x] C27 单元测试：success / failure 构造

## cache.rs — KvCacheManager
- [x] C28 `KvCacheEntry` 结构体：request_id / context_length / size_bytes
- [x] C29 `KvCacheManager` 结构体：entries / max_cache_size / current_size
- [x] C30 派生 `Debug` + `Clone`
- [x] C31 `KvCacheManager::new(max_cache_size: u64) -> Self`
- [x] C32 `allocate(request_id, context_length) -> Result<u64, SchedulerError>`（D4：返回 size_bytes，不返回指针）
- [x] C33 `release(request_id) -> bool`
- [x] C34 `current_size()` / `max_size()` / `entry_count()` 查询
- [x] C35 `calculate_cache_size(context_length) -> u64`（`context_length * 512 * 1024`，512KB/token）
- [x] C36 `evict_oldest() -> bool`（移除 entries[0]，`current_size` 用 `saturating_sub`）
- [x] C37 单元测试：allocate / release / evict_oldest / 耗尽

## stats.rs — SchedulerStats
- [x] C38 `SchedulerStats` 结构体：total_requests / completed_requests / timed_out_requests / failed_requests / cache_evictions（全 u64，D5）
- [x] C39 派生 `Debug` + `Clone` + `Default`（全 0）
- [x] C40 `record_submit` / `record_complete` / `record_timeout` / `record_failure` / `record_eviction` 方法
- [x] C41 单元测试：累加

## scheduler.rs — InferScheduler
- [x] C42 `InferScheduler` 结构体：queue / active_count / max_concurrent / kv_cache / stats / next_request_id / device
- [x] C43 派生 `Debug`
- [x] C44 `InferScheduler::new(max_concurrent, max_cache_bytes, device) -> Self`（max_concurrent ≤ 2）
- [x] C45 `submit(req) -> u64`（入队，stats.record_submit()）
- [x] C46 `tick(now_ns, engine: &mut dyn LlmEngine) -> Vec<InferResult>`（D2：poll-based，无 callback）
- [x] C47 tick 流程：丢弃超时 → 按 priority 降序排序 → 调度最多 max_concurrent-active_count 个 → allocate KV Cache → engine.infer → release KV Cache → 记录结果
- [x] C48 `queue_len()` / `active_count()` / `stats()` / `kv_cache()` 查询
- [x] C49 单元测试：空队列 / 单请求 / 并发限制 / 超时 / 优先级

## 集成测试（lib.rs）
- [x] C50 T1 InferRequest::new 默认值
- [x] C51 T2 InferRequest builder
- [x] C52 T3 InferRequest::is_timed_out
- [x] C53 T4 RequestPriority Ord 排序
- [x] C54 T5 InferScheduler::new 初始状态
- [x] C55 T6 submit 返回唯一递增 ID
- [x] C56 T7 tick 空队列返回空 Vec
- [x] C57 T8 tick 单请求执行成功
- [x] C58 T9 tick 并发限制（max_concurrent=2, 3 请求）
- [x] C59 T10 tick 超时请求被丢弃
- [x] C60 T11 tick 优先级排序（High 先于 Normal）
- [x] C61 T12 KvCacheManager allocate + release
- [x] C62 T13 KvCacheManager evict_oldest
- [x] C63 T14 KvCacheManager 缓存耗尽返回 CacheExhausted
- [x] C64 T15 SchedulerStats 完整流程累加
- [x] C65 `cargo test -p eneros-infer-scheduler` 15/15 通过

## 设计文档
- [x] C66 `docs/ai/infer-scheduler-design.md` 存在
- [x] C67 12 章节完整
- [x] C68 2 Mermaid 图（InferScheduler 类图 + tick 时序图）
- [x] C69 D1~D12 偏差声明表
- [x] C70 文档在 `docs/ai/` 下（符合目录规范）

## 版本同步
- [x] C71 `Makefile` 版本号 `0.62.0`
- [x] C72 `.github/workflows/ci.yml` 版本号 `0.62.0`
- [x] C73 `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-infer-scheduler`

## 构建校验（§2.4.2 C6~C11）
- [x] C74 `cargo metadata --format-version 1` 成功
- [x] C75 `cargo test -p eneros-infer-scheduler` 全部通过（15 tests）
- [x] C76 `cargo build -p eneros-infer-scheduler --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C77 `cargo fmt -p eneros-infer-scheduler -- --check` 通过
- [x] C78 `cargo clippy -p eneros-infer-scheduler --all-targets -- -D warnings` 无 warning
- [x] C79 `cargo deny check licenses bans sources` 通过

## no_std 合规
- [x] C80 无 `use std::*`（仅 `alloc::*` / `core::*`）
- [x] C81 无 `panic!` / `todo!` / `unimplemented!`
- [x] C82 子模块不重复 `#![cfg_attr(not(test), no_std)]`
- [x] C83 无 `unsafe` 块（D10：纯 safe Rust）

## 目录规范
- [x] C84 crate 在 `crates/ai/infer-scheduler/`（D9）
- [x] C85 跨 crate path 引用 `../llm-engine`（相对路径）
- [x] C86 文档在 `docs/ai/` 下
- [x] C87 无根目录 crate（除 `ci/`）
- [x] C88 无垃圾文件（`target/` / `*.elf` / `*.bin` 被忽略）

## 依赖复用（D11）
- [x] C89 复用 v0.59.0 `LlmEngine` trait（不重定义）
- [x] C90 复用 v0.59.0 `InferParams` / `LlmError` / `ComputeDevice`（不重定义）
- [x] C91 `From<LlmError> for SchedulerError` 转换实现
- [x] C92 **不依赖** v0.60.0（GgufLoader）或 v0.61.0（model-deploy）— 调度器与模型加载/部署解耦

## 简化设计验证（Karpathy 原则）
- [x] C93 无 callback（D2：poll-based `tick` 替代）
- [x] C94 无 FFI（D3/D4/D10：KV Cache 为元数据跟踪，不分配 GPU 内存）
- [x] C95 无 feature 门控（D3：纯 Rust，默认编译）
- [x] C96 无 `Drop` 实现（D8：队列自动释放）
- [x] C97 无 Mock 后端（D12：KvCacheManager 直接实例化）

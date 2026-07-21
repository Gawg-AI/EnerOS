# Tasks

- [x] Task 1: Workspace 同步
  - [x] 根 `Cargo.toml` 版本号 `0.61.0` → `0.62.0`
  - [x] members 添加 `crates/ai/infer-scheduler`
  - [x] 验证：`cargo metadata --format-version 1` 成功（待 crate 骨架创建后）

- [x] Task 2: 创建 `eneros-infer-scheduler` crate 骨架
  - [x] 新建 `crates/ai/infer-scheduler/Cargo.toml`，package name = `eneros-infer-scheduler`
  - [x] dependencies 添加 `eneros-llm-engine = { path = "../llm-engine" }`（D11 复用 v0.59.0 类型；不依赖 v0.60.0/v0.61.0）
  - [x] **不声明** `[features]`（D3：无 feature 门控，纯 Rust）
  - [x] no_std 配置：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
  - [x] 新建 `src/lib.rs`，模块声明：error / request / result / priority / cache / stats / scheduler
  - [x] lib.rs 包含 D1~D12 偏差声明表
  - [x] 验证：`cargo metadata --format-version 1` 成功

- [x] Task 3: 实现 `error.rs` — SchedulerError 错误类型
  - [x] `SchedulerError` 枚举：QueueFull / Timeout / CacheExhausted / Engine(LlmError) / NotScheduled
  - [x] 派生 `Debug` + `Clone` + `PartialEq`，实现 `core::fmt::Display`
  - [x] 实现 `From<LlmError> for SchedulerError`（D7）
  - [x] 验证：`cargo build -p eneros-infer-scheduler` 通过

- [x] Task 4: 实现 `priority.rs` — RequestPriority 优先级枚举
  - [x] `RequestPriority` 枚举：Low / Normal / High / Critical
  - [x] 派生 `Debug` + `Clone` + `Copy` + `PartialEq` + `Eq` + `PartialOrd` + `Ord`
  - [x] `Critical` 最高，`Low` 最低
  - [x] 实现 `Default`（`Normal`）
  - [x] 验证：编译通过

- [x] Task 5: 实现 `request.rs` — InferRequest 请求结构
  - [x] `InferRequest` 结构体：id: u64 / prompt: String / params: InferParams / priority: RequestPriority / submitted_at_ns: u64 / timeout_ns: u64
  - [x] 派生 `Debug` + `Clone`
  - [x] `InferRequest::new(id, prompt, params) -> Self`（默认 Normal 优先级，submitted_at_ns=0，timeout_ns=u64::MAX 表示永不超时）
  - [x] `with_priority(self, priority) -> Self`（builder）
  - [x] `with_timeout(self, timeout_ns: u64) -> Self`（builder）
  - [x] `with_timestamp(self, submitted_at_ns: u64) -> Self`（builder）
  - [x] `is_timed_out(&self, now_ns: u64) -> bool`（`now_ns.saturating_sub(submitted_at_ns) > timeout_ns`）
  - [x] 验证：单元测试 — 构造 / builder / is_timed_out

- [x] Task 6: 实现 `result.rs` — InferResult 结果结构
  - [x] `InferResult` 结构体：id: u64 / result: Result<String, SchedulerError>
  - [x] 派生 `Debug` + `Clone`
  - [x] `InferResult::new(id, result) -> Self`
  - [x] `InferResult::success(id, output: String) -> Self`
  - [x] `InferResult::failure(id, error: SchedulerError) -> Self`
  - [x] 验证：单元测试 — success / failure 构造

- [x] Task 7: 实现 `cache.rs` — KvCacheManager + KvCacheEntry
  - [x] `KvCacheEntry` 结构体：request_id: u64 / context_length: u32 / size_bytes: u64
  - [x] `KvCacheManager` 结构体：entries: Vec<KvCacheEntry> / max_cache_size: u64 / current_size: u64
  - [x] 派生 `Debug` + `Clone`
  - [x] `KvCacheManager::new(max_cache_size: u64) -> Self`（空 entries）
  - [x] `allocate(&mut self, request_id: u64, context_length: u32) -> Result<u64, SchedulerError>` — 返回 size_bytes（D4）
  - [x] `release(&mut self, request_id: u64) -> bool` — 移除条目，返回是否找到
  - [x] `current_size(&self) -> u64`
  - [x] `max_size(&self) -> u64`
  - [x] `entry_count(&self) -> usize`
  - [x] `calculate_cache_size(context_length: u32) -> u64` — `context_length as u64 * 512 * 1024`（512KB/token）
  - [x] `evict_oldest(&mut self) -> bool` — 移除 entries[0]，`current_size` 用 `saturating_sub`，返回是否回收
  - [x] 验证：单元测试 — allocate / release / evict_oldest / 耗尽

- [x] Task 8: 实现 `stats.rs` — SchedulerStats 统计
  - [x] `SchedulerStats` 结构体：total_requests / completed_requests / timed_out_requests / failed_requests / cache_evictions（全 u64）
  - [x] 派生 `Debug` + `Clone` + `Default`（全 0）
  - [x] `record_submit(&mut self)` — `total_requests += 1`
  - [x] `record_complete(&mut self)` — `completed_requests += 1`
  - [x] `record_timeout(&mut self)` — `timed_out_requests += 1`
  - [x] `record_failure(&mut self)` — `failed_requests += 1`
  - [x] `record_eviction(&mut self)` — `cache_evictions += 1`
  - [x] 验证：单元测试 — 累加

- [x] Task 9: 实现 `scheduler.rs` — InferScheduler 调度器
  - [x] `InferScheduler` 结构体：queue: VecDeque<InferRequest> / active_count: u8 / max_concurrent: u8 / kv_cache: KvCacheManager / stats: SchedulerStats / next_request_id: u64 / device: ComputeDevice
  - [x] 派生 `Debug`
  - [x] `InferScheduler::new(max_concurrent: u8, max_cache_bytes: u64, device: ComputeDevice) -> Self`（D6/D11）
  - [x] `submit(&mut self, req: InferRequest) -> u64` — 入队，`stats.record_submit()`，返回 req.id
  - [x] `tick(&mut self, now_ns: u64, engine: &mut dyn LlmEngine) -> Vec<InferResult>` — D2 轮询执行
  - [x] `queue_len(&self) -> usize`
  - [x] `active_count(&self) -> u8`
  - [x] `stats(&self) -> &SchedulerStats`
  - [x] `kv_cache(&self) -> &KvCacheManager`
  - [x] tick 流程：① 丢弃超时请求 ② 按 priority 降序排序队列 ③ 调度最多 `max_concurrent - active_count` 个 ④ 对每个请求：allocate KV Cache → engine.infer → release KV Cache → 记录结果 ⑤ 返回 Vec<InferResult>
  - [x] 验证：单元测试 — 空队列 / 单请求 / 多请求 / 超时 / 优先级

- [x] Task 10: 集成测试模块（lib.rs `#[cfg(test)] mod tests`）
  - [x] T1 InferRequest::new 默认值（priority=Normal, timeout_ns=u64::MAX）
  - [x] T2 InferRequest builder（with_priority / with_timeout / with_timestamp）
  - [x] T3 InferRequest::is_timed_out（超时返回 true，未超时返回 false）
  - [x] T4 RequestPriority Ord 排序（Critical > High > Normal > Low）
  - [x] T5 InferScheduler::new 初始状态（queue_len=0, active_count=0）
  - [x] T6 submit 返回唯一递增 ID
  - [x] T7 tick 空队列返回空 Vec
  - [x] T8 tick 单请求执行成功（MockEngine + InferResult::success）
  - [x] T9 tick 并发限制（max_concurrent=2, 3 请求 → 2 执行，1 留队列）
  - [x] T10 tick 超时请求被丢弃（stats.timed_out_requests += 1）
  - [x] T11 tick 优先级排序（High 先于 Normal 执行）
  - [x] T12 KvCacheManager allocate + release（current_size 增减）
  - [x] T13 KvCacheManager evict_oldest（超出 max_cache_size 触发回收）
  - [x] T14 KvCacheManager 缓存耗尽返回 CacheExhausted
  - [x] T15 SchedulerStats 完整流程累加（submit → tick → complete/timeout/failure）
  - [x] 验证：`cargo test -p eneros-infer-scheduler` 全部通过

- [x] Task 11: 设计文档 `docs/ai/infer-scheduler-design.md`
  - [x] 12 章节：版本目标 / 架构定位 / InferRequest / InferScheduler / tick 流程 / KvCacheManager / RequestPriority / SchedulerStats / 错误处理 / GPU 策略 / 内存预算 / 偏差声明
  - [x] 2 Mermaid 图：InferScheduler 类图 + tick 时序图
  - [x] D1~D12 偏差声明表
  - [x] 文档位置在 `docs/ai/` 下（复用 v0.59.0/v0.60.0/v0.61.0 创建的目录）

- [x] Task 12: 版本号同步 + gate.rs 注释更新
  - [x] `Makefile` 版本号 `0.61.0` → `0.62.0`
  - [x] `.github/workflows/ci.yml` 版本号 `0.61.0` → `0.62.0`
  - [x] `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-infer-scheduler` 说明
  - [x] 验证：`cargo build -p eneros-infer-scheduler` 通过

- [x] Task 13: 构建校验（§2.4.2 C6~C11）
  - [x] `cargo metadata --format-version 1` 成功
  - [x] `cargo test -p eneros-infer-scheduler` 全部通过（15 tests）
  - [x] `cargo build -p eneros-infer-scheduler --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 交叉编译通过
  - [x] `cargo fmt -p eneros-infer-scheduler -- --check` 格式通过
  - [x] `cargo clippy -p eneros-infer-scheduler --all-targets -- -D warnings` lint 通过
  - [x] `cargo deny check licenses bans sources` 安全扫描通过

- [x] Task 14: 更新 tasks.md + checklist.md 所有项 → [x]
  - [x] tasks.md 14 任务全部 [x]
  - [x] checklist.md 所有检查点全部 [x]

# Task Dependencies

- Task 2 → Task 1（crate 骨架需先于 metadata 验证）
- Task 3~8 → Task 2（各模块依赖 crate 骨架）
- Task 3（error）→ Task 4~9（各模块返回 SchedulerError）
- Task 4（priority）→ Task 5（request 使用 RequestPriority）
- Task 5（request）→ Task 9（scheduler 使用 InferRequest）
- Task 6（result）→ Task 9（scheduler 返回 InferResult）
- Task 7（cache）→ Task 9（scheduler 使用 KvCacheManager）
- Task 8（stats）→ Task 9（scheduler 使用 SchedulerStats）
- Task 9（scheduler）→ Task 10（集成测试依赖 scheduler）
- Task 10 → Task 3~9（集成测试依赖所有模块）
- Task 11 → Task 10（文档在测试通过后撰写）
- Task 12 → Task 11（版本同步在功能完成后）
- Task 13 → Task 12（构建校验在版本同步后）
- Task 14 → Task 13（更新文档在全部校验通过后）

# Parallelizable Work

- Task 3（error）+ Task 4（priority）+ Task 6（result）+ Task 7（cache）+ Task 8（stats）可并行（无相互依赖）
- Task 5（request）依赖 Task 3（error）+ Task 4（priority）
- Task 9（scheduler）依赖 Task 3~8 全部
- Task 10 → Task 9
- Task 11（设计文档）可与 Task 9~10 并行（独立工作）

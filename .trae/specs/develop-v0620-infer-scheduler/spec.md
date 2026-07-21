# v0.62.0 推理调度与并发控制 Spec

## Why

v0.61.0 完成了 7B INT4 量化模型部署验证（`QuantConfig7B` / `DeployVerifier`），v0.59.0 定义了 `LlmEngine` trait。LLM 推理是计算密集型操作，边缘设备资源有限，需要请求队列、并发控制（≤2）与 KV Cache 管理，避免 GPU 显存溢出与队列积压。本版本实现推理调度器，为 v0.63.0 Prompt 模板系统提供可控的推理执行层。

## What Changes

- 新增 crate `eneros-infer-scheduler`（位置：`crates/ai/infer-scheduler/`，子系统：ai）
- 新增 `InferScheduler` — 推理调度器（队列 + 并发限制 + tick 轮询执行）
- 新增 `InferRequest` — 推理请求结构（id / prompt / params / priority / submitted_at_ns / timeout_ns）
- 新增 `InferResult` — 推理执行结果（id + Result<String, SchedulerError>）
- 新增 `RequestPriority` — 请求优先级（Low / Normal / High / Critical）
- 新增 `KvCacheManager` — KV Cache 元数据管理器（跟踪缓存占用，触发回收）
- 新增 `KvCacheEntry` — KV Cache 条目（request_id / context_length / size_bytes）
- 新增 `SchedulerStats` — 调度器统计（total / completed / timed_out / failed / evictions）
- 新增 `SchedulerError` — 调度器错误类型（QueueFull / Timeout / CacheExhausted / Engine / NotScheduled）
- 复用 v0.59.0 类型：`LlmEngine` / `InferParams` / `LlmError` / `ComputeDevice`（D11，不重定义）
- 设计文档 `docs/ai/infer-scheduler-design.md`

## Impact

- Affected specs: v0.59.0（依赖其 `LlmEngine` / `InferParams` / `LlmError` / `ComputeDevice` 类型）
- Affected code: 新增 `crates/ai/infer-scheduler/`；根 `Cargo.toml` members 新增条目
- **无破坏性改动**：本版本仅新增 crate，不修改 v0.59.0 / v0.60.0 / v0.61.0 既有代码

## 偏差声明（D1~D12）

> 依据 Karpathy "Think Before Coding" 原则，逐条列出蓝图伪代码与实际 no_std / 项目约束的偏差。

### D1：no_std 合规 — `alloc::*` 替代 `std::*`

**蓝图**：`VecDeque<InferRequest>`、`String`、`Duration`、`MonotonicTime`，隐含 `std::collections::VecDeque`、`std::string::String`、`std::time::Duration`。

**实际**：本项目所有 Rust 代码必须 no_std（蓝图 §43.1）。

**决策**：
- 使用 `alloc::collections::VecDeque`、`alloc::string::String`、`alloc::vec::Vec`
- `lib.rs` 顶部 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- 子模块不重复 `#![cfg_attr(not(test), no_std)]`（继承 lib.rs）
- 禁用 `panic!` / `todo!` / `unimplemented!`

### D2：Poll-based `tick(now_ns)` 替代异步 callback — 单线程 no_std 无真实并发

**蓝图**：
```rust
callback: Option<Box<dyn FnOnce(Result<String, LlmError>)>>
fn try_dispatch(&mut self) { ... self.execute_request(req); }
fn execute_request(&mut self, req: InferRequest) { ... cb(result); ... self.try_dispatch(); }
```
隐含异步执行 + 回调通知。

**实际**：
1. 单线程 no_std RTOS 无 `std::thread`、无 `async/await` runtime
2. `execute_request` 同步调用 `engine.infer()`，回调在 `tick` 内立即触发
3. 回调在调度器内部执行增加耦合与测试复杂度（Karpathy: Simplicity First）

**决策**：
- `submit(req) -> u64` — 入队，返回请求 ID
- `tick(now_ns: u64, engine: &mut dyn LlmEngine) -> Vec<InferResult>` — 轮询执行，返回本轮完成结果
- `InferResult { id: u64, result: Result<String, SchedulerError> }` — 调用方自行分发
- **不使用 callback**：调用方 poll `tick()` 获取结果，更简单、更易测试、符合单线程模型

### D3：无 feature 门控 — KV Cache 为纯元数据跟踪，无 FFI

**蓝图**：`alloc_gpu_memory(size)` / `free_gpu_memory(ptr, size)` 是 FFI 调用，需要 `#[cfg(feature = "llama-cpp")]` 门控。

**实际**：见 D4 — KV Cache 是元数据跟踪，不分配 GPU 内存。因此**无需** feature 门控。

**决策**：
- `Cargo.toml` **不声明** `[features] llama-cpp = []`（与 v0.59.0/v0.60.0/v0.61.0 不同）
- 整个 crate 纯 Rust，默认编译，无外部 C 库依赖
- CI 环境完全可测（无模型文件、无 GPU 也能运行全部测试）

### D4：KV Cache 元数据跟踪 — 不直接分配 GPU 内存

**蓝图**：
```rust
pub fn allocate(&mut self, request_id: u64, context_length: u32) -> Result<*mut u8, LlmError> {
    let ptr = alloc_gpu_memory(size)?;
    ...
}
```
直接在 GPU 显存中分配 KV Cache。

**实际**：
1. llama.cpp 内部通过 `n_gpu_layers` 参数（v0.59.0 `ComputeDevice::n_gpu_layers()`）管理 KV Cache 的 GPU offload
2. Rust 侧重复分配 GPU 内存会导致双重管理 + 内存泄漏
3. 调度器只需**跟踪** KV Cache 占用（用于限额 + 回收决策），实际分配由 llama.cpp 负责

**决策**：
- `KvCacheManager` 是**元数据跟踪器**，非内存分配器
- `allocate(request_id, context_length) -> Result<u64, SchedulerError>` — 返回 `size_bytes`（不返回 `*mut u8`）
- `release(request_id)` — 移除条目，减少 `current_size`
- `evict_oldest()` — 标记最旧条目为待回收（实际回收由 llama.cpp 在下次推理时处理）
- 缓存大小估算公式：`context_length * 512 * 1024` 字节（512KB/token，7B INT4 KV Cache 近似值）

### D5：`SchedulerStats` 用普通 u64，不用 AtomicU64

**蓝图**：未明确，但调度器统计需要 `total_requests` / `completed_requests` 等。

**实际**：与 v0.56.0 D7 / v0.57.0 D7 / v0.58.0 D4 / v0.59.0 D5 / v0.60.0 D5 / v0.61.0 D5 一致，单线程无需原子操作。

**决策**：`SchedulerStats { total_requests: u64, completed_requests: u64, timed_out_requests: u64, failed_requests: u64, cache_evictions: u64 }`，普通 `u64`，派生 `Debug` / `Clone` / `Default`。

### D6：`now_ns: u64` 注入替代 `MonotonicTime::now()`

**蓝图**：`MonotonicTime::now() - req.timestamp > req.timeout`。

**实际**：no_std 无 `Instant::now()` / `SystemTime`。v0.12.0 RTC 提供时间源，但调度器不应直接依赖 RTC（解耦）。

**决策**：
- `InferRequest { submitted_at_ns: u64, timeout_ns: u64 }` — 提交方注入单调时钟纳秒
- `tick(now_ns: u64, ...)` — 调用方注入当前时间（来自 v0.12.0 RTC 或测试 mock）
- 超时判定：`now_ns.saturating_sub(req.submitted_at_ns) > req.timeout_ns`
- `Duration` → 改用 `u64` 纳秒（`core::time::Duration` 也可用，但 `u64` 更简单）

### D7：`SchedulerError` 错误类型 — 调度场景专用错误

**蓝图**：使用 `LlmError` 直接传递。

**实际**：调度器有独特的失败模式（队列满 / 超时 / 缓存耗尽），需独立错误类型。

**决策**：`SchedulerError` 枚举：
- `QueueFull` — 请求队列已满（达到上限）
- `Timeout` — 请求超时被丢弃
- `CacheExhausted` — KV Cache 耗尽且无法回收
- `Engine(LlmError)` — 引擎推理失败（包装 v0.59.0 错误）
- `NotScheduled` — 请求未提交就查询

派生 `Debug` + `Clone` + `PartialEq`，实现 `core::fmt::Display`。提供 `From<LlmError> for SchedulerError` 转换。

### D8：`InferScheduler` 不实现 `Drop` — 无需自动清理

**蓝图**：未定义 Drop 行为。

**实际**：调度器不持有 GPU 内存（D4），队列中的请求是值类型（非指针），无需 Drop 清理。

**决策**：`InferScheduler` **不实现** `Drop`。队列中未处理的请求在调度器销毁时自动释放（Rust 所有权机制）。

### D9：crate 位置 `crates/ai/infer-scheduler/`

**蓝图**：`infer-scheduler` 模块。

**实际**：项目规则 §2.3.1 要求所有 crate 放入 `crates/<subsystem>/`。推理调度属于 AI 子系统。

**决策**：crate 路径 `crates/ai/infer-scheduler/`，package name `eneros-infer-scheduler`。跨 crate 引用 v0.59.0：`path = "../llm-engine"`。

### D10：无 FFI 声明 — 纯 Rust 实现

**蓝图**：`alloc_gpu_memory` / `free_gpu_memory` 是 FFI 调用。

**实际**：见 D4 — KV Cache 是元数据跟踪，不分配 GPU 内存。调度器通过 `LlmEngine` trait 间接调用 llama.cpp（v0.59.0 已封装 FFI）。

**决策**：
- 不引入任何 `extern "C"` 声明
- 不使用 `unsafe` 块
- 整个 crate 纯 safe Rust

### D11：复用 v0.59.0 类型 — 不重定义

**蓝图**：`InferRequest { params: InferParams, callback: ... }`。

**实际**：v0.59.0 已定义 `InferParams`、`LlmEngine`、`LlmError`、`ComputeDevice`。重定义会导致类型不兼容。

**决策**：
- `eneros-infer-scheduler` 仅依赖 `eneros-llm-engine`（`path = "../llm-engine"`）
- `InferRequest` 使用 `params: InferParams`（来自 v0.59.0）
- `tick()` 接受 `engine: &mut dyn LlmEngine`（来自 v0.59.0 trait）
- `SchedulerError::Engine(LlmError)` 包装 v0.59.0 错误
- **不依赖** v0.60.0（GgufLoader）或 v0.61.0（model-deploy）— 调度器与模型加载/部署解耦

### D12：默认无 Mock 后端 — KV Cache 本身即纯元数据

**蓝图**：需要 Mock 后端用于测试。

**实际**：见 D3/D4 — KV Cache 是纯 Rust 元数据跟踪，无后端抽象，无需 Mock。

**决策**：
- `KvCacheManager` 直接实例化（`KvCacheManager::new(max_cache_bytes: u64)`）
- 无 `KvCacheBackend` trait，无 `MockKvCacheBackend`
- 测试直接使用真实 `KvCacheManager`（纯元数据，无副作用）

## ADDED Requirements

### Requirement: 推理调度器

系统 SHALL 提供 `InferScheduler` 结构体，管理推理请求队列、并发限制与 KV Cache，通过 `tick(now_ns, engine)` 轮询执行。

#### Scenario: 提交请求
- **WHEN** `scheduler.submit(req)` 被调用
- **THEN** 请求入队，返回唯一请求 ID，`stats.total_requests += 1`

#### Scenario: 轮询执行
- **WHEN** `scheduler.tick(now_ns, &mut engine)` 被调用
- **THEN** 丢弃超时请求（`now_ns - submitted_at_ns > timeout_ns`）
- **AND** 调度最多 `max_concurrent - active_count` 个请求
- **AND** 对每个请求调用 `engine.infer(prompt, params)`，分配并释放 KV Cache
- **AND** 返回本轮完成的 `Vec<InferResult>`

#### Scenario: 并发限制
- **WHEN** 队列有 5 个请求，`max_concurrent = 2`
- **THEN** 单次 `tick` 最多执行 2 个请求，剩余 3 个留在队列

### Requirement: 推理请求与优先级

系统 SHALL 提供 `InferRequest` 结构体与 `RequestPriority` 枚举，支持按优先级调度。

#### Scenario: 优先级排序
- **WHEN** 队列中有 High 与 Normal 优先级请求
- **THEN** `tick` 优先调度 High 请求（按 `Ord` 排序：Critical > High > Normal > Low）

### Requirement: KV Cache 管理

系统 SHALL 提供 `KvCacheManager`，跟踪推理请求的 KV Cache 占用，支持回收最旧条目。

#### Scenario: 分配缓存
- **WHEN** `kv_cache.allocate(request_id, context_length)` 被调用
- **THEN** 计算缓存大小 `context_length * 512 * 1024`
- **AND** 若 `current_size + size > max_cache_size`，回收最旧条目
- **AND** 添加条目，`current_size += size`
- **AND** 返回 `Ok(size_bytes)`

#### Scenario: 释放缓存
- **WHEN** `kv_cache.release(request_id)` 被调用
- **THEN** 移除对应条目，`current_size -= entry.size_bytes`（用 `saturating_sub` 防下溢）

#### Scenario: 缓存耗尽
- **WHEN** 回收后仍超出 `max_cache_size`
- **THEN** `allocate` 返回 `Err(SchedulerError::CacheExhausted)`

### Requirement: 调度器统计

系统 SHALL 提供 `SchedulerStats`，记录请求总数、完成数、超时数、失败数、缓存回收数。

#### Scenario: 统计累加
- **WHEN** 请求完成
- **THEN** `completed_requests += 1`
- **WHEN** 请求超时
- **THEN** `timed_out_requests += 1`
- **WHEN** 请求失败
- **THEN** `failed_requests += 1`
- **WHEN** KV Cache 回收
- **THEN** `cache_evictions += 1`

### Requirement: 错误类型

系统 SHALL 提供 `SchedulerError` 枚举，覆盖队列满、超时、缓存耗尽、引擎错误、未调度等场景，并实现 `From<LlmError>` 转换。

#### Scenario: 引擎错误包装
- **WHEN** `engine.infer()` 返回 `Err(LlmError::InferFailed)`
- **THEN** `InferResult.result` 为 `Err(SchedulerError::Engine(LlmError::InferFailed))`

# EnerOS v0.11.0 — 用户态堆分配器 Spec

> **版本**：v0.11.0（Phase 0 / P0-C 终点，堆能力闭环）
> **类型**：基础服务实现版本（用户态堆 + 配额 + OOM 策略）
> **前序依赖**：v0.10.0（内核堆，复用 BuddyAllocator）
> **后续版本**：v0.12.0（RTC 驱动 + 系统时钟服务）
> **蓝图依据**：`蓝图/phase0.md` §v0.11.0（第 2257–2437 行）
> **合规性**：蓝图 §43.1（no_std）、§43.2（非瓶颈版本：签名可编译，算法可简化）

---

## Why

用户态组件（Agent/RTOS）需要堆来使用标准集合类型（Vec/String/HashMap）。v0.10.0 实现了内核态堆，v0.11.0 将其扩展到用户态，增加配额机制防止单分区耗尽系统内存，并提供 OOM 策略。这是 P0-C 的终点，堆能力闭环。

---

## What Changes

- **新增** `user/heap/Cargo.toml`：新 crate `eneros-user-heap`，依赖 `eneros-heap` + `spin`
- **新增** `user/heap/src/lib.rs`（~200 行）：`UserHeap` 占位类型、`GlobalAlloc` 实现、`heap_init`/`set_quota`/`used`/`trigger_oom` 全局接口
- **新增** `user/heap/src/quota.rs`（~120 行）：`Quota` 配额结构体、`OomHandler` 类型、配额检查逻辑
- **修改** workspace 根 `Cargo.toml`：members 添加 `"user/heap"`，version `0.10.0` → `0.11.0`
- **修改** `.github/workflows/ci.yml`：版本标识 v0.10.0 → v0.11.0，cross-build 添加 user-heap crate
- **修改** `Makefile`：VERSION 0.10.0 → 0.11.0，添加 `user-heap-build`/`user-heap-test` 目标
- **修改** `ci/src/gate.rs`：注释更新（+ v0.11.0 user-heap）
- **新增** 文档：`docs/user-heap-design.md`、`docs/oom-policy.md`

---

## Impact

- **Affected specs**：v0.14.0（panic 框架需 OOM 处理）；v0.18.0（TCB 用 Box 分配）
- **Affected code**：
  - `user/heap/` 新增 2 个源文件（~320 行代码）
  - workspace `Cargo.toml`、CI 配置、Makefile、ci/gate.rs
- **不影响**：现有 kernel/runtime/board/sel4-sys/hello/hal/mm/heap crate 的功能行为
- **不影响**：v0.10.0 内核堆实现（回归兼容，仅依赖不修改）
- **不影响**：`heap` crate 的 `#[global_allocator]` 注册（两者独立，不同二进制使用不同分配器）

---

## ADDED Requirements

### Requirement: Quota 配额管理

系统 SHALL 提供 `Quota` 结构体，跟踪用户态堆的配额使用情况。

#### Scenario: Quota 初始化

- **WHEN** 调用 `Quota::new(limit)` 创建配额为 `limit` 字节的 Quota
- **THEN** `limit` = limit，`used` = 0

#### Scenario: 配额检查通过

- **WHEN** 调用 `quota.check(size)` 且 `used + size <= limit`
- **THEN** 返回 `true`

#### Scenario: 配额检查失败

- **WHEN** 调用 `quota.check(size)` 且 `used + size > limit`
- **THEN** 返回 `false`

#### Scenario: 配额无限制

- **WHEN** `limit` = 0（表示无限制）
- **THEN** `check(size)` 始终返回 `true`

#### Scenario: 增减已用

- **WHEN** 调用 `quota.add_used(size)`
- **THEN** `used` += size
- **WHEN** 调用 `quota.sub_used(size)`
- **THEN** `used` = saturating_sub(size)

### Requirement: OomHandler OOM 处理

系统 SHALL 提供 `OomHandler` 类型，支持自定义 OOM 处理函数。

#### Scenario: 默认 OOM handler

- **WHEN** 未设置 oom_handler，调用 `trigger_oom()`
- **THEN** 执行默认行为（panic "user heap OOM"）

#### Scenario: 自定义 OOM handler

- **WHEN** 设置 `oom_handler = Some(fn)`，调用 `trigger_oom()`
- **THEN** 调用自定义 handler 函数

#### Scenario: OomHandler 类型定义

```rust
pub type OomHandler = Option<fn() -> !>;
```

### Requirement: UserHeap GlobalAlloc 实现

系统 SHALL 提供 `UserHeap` 零字段占位类型，实现 `core::alloc::GlobalAlloc`，通过 `spin::Mutex<UserHeapInner>` 访问内部可变状态，复用 v0.10.0 的 `BuddyAllocator`。

#### Scenario: alloc 配额检查

- **WHEN** `GlobalAlloc::alloc(&self, layout)` 被调用
- **THEN** 检查 `quota.check(layout.size())`
- **AND** 若配额不足，调用 `trigger_oom()`，返回 `null_mut()`
- **AND** 若配额充足，调用 `buddy.alloc(layout.size())`
- **AND** 更新 `quota.add_used(size)`
- **AND** 返回块指针

#### Scenario: dealloc 归还

- **WHEN** `GlobalAlloc::dealloc(&self, ptr, layout)`
- **THEN** 调用 `buddy.dealloc(ptr, layout.size())`
- **AND** 更新 `quota.sub_used(size)`

#### Scenario: 堆未初始化

- **WHEN** `alloc` 时 `buddy.base` 为 null
- **THEN** 返回 `null_mut()`，不 panic

#### Scenario: OOM 处理

- **WHEN** `buddy.alloc` 返回 null（堆耗尽但配额未满）
- **THEN** 调用 `trigger_oom()`
- **AND** 返回 `null_mut()`

### Requirement: heap_init / set_quota / used / trigger_oom 全局接口

系统 SHALL 提供全局函数管理用户态堆生命周期。

#### Scenario: heap_init 初始化

- **WHEN** 调用 `heap_init(base, size)` 传入堆池基址和大小
- **THEN** 创建 `BuddyAllocator` 并 `init(base, size / PAGE_SIZE)`
- **THEN** 创建 `Quota::new(size)`（默认配额 = 堆大小）
- **THEN` 存入全局 `USER_HEAP` 静态变量

#### Scenario: set_quota 设置配额

- **WHEN** 调用 `set_quota(limit)`
- **THEN** 更新 `USER_HEAP` 的 `quota.limit` = limit
- **AND** 若 limit = 0，表示无限制

#### Scenario: used 查询已用

- **WHEN** 调用 `used()`
- **THEN** 返回当前 `quota.used`
- **AND** 若堆未初始化，返回 0

#### Scenario: trigger_oom 触发 OOM

- **WHEN** 调用 `trigger_oom()`
- **THEN** 若 `oom_handler` 为 `Some(f)`，调用 `f()`
- **AND** 若 `oom_handler` 为 `None`，panic "user heap OOM"

### Requirement: no_std 合规

所有代码 MUST 遵循蓝图 §43.1：`#![cfg_attr(not(test), no_std)]`，正式构建 no_std，测试构建链接 std。使用 `core::alloc::{GlobalAlloc, Layout}`、`core::ptr`、`spin::Mutex`、`eneros_heap::buddy::BuddyAllocator`，不使用 `std::*`。

### Requirement: 文档交付

系统 SHALL 交付两份文档：
1. `docs/user-heap-design.md`：《用户态堆设计》——架构概述、与内核堆的关系、配额机制、初始化序列、GlobalAlloc 集成
2. `docs/oom-policy.md`：《OOM 策略》——OOM 触发条件、handler 机制、默认行为、恢复策略

---

## MODIFIED Requirements

### Requirement: Workspace 版本

workspace 根 `Cargo.toml` 的 version 从 `0.10.0` 升级到 `0.11.0`，members 列表添加 `"user/heap"`。

### Requirement: CI 流水线版本

`.github/workflows/ci.yml` 的版本标识从 v0.10.0 升级到 v0.11.0，cross-build job 添加 "Build user-heap crate" 步骤。

### Requirement: Makefile 版本

`Makefile` 的 VERSION 从 0.10.0 升级到 0.11.0，添加 `user-heap-build`/`user-heap-test` 目标。

### Requirement: CI 门禁注释

`ci/src/gate.rs` 的注释更新，添加 "+ v0.11.0 user-heap" 说明。

---

## 设计决策（Design Decisions）

### D1: `user/heap/` 作为 workspace 成员

蓝图路径 `user/heap/src/`，在 workspace 中创建 `user/heap/` 目录。理由：
- 蓝图明确指定路径
- 建立 `user/` 命名空间，为未来用户态 crate（user/thread, user/ipc 等）铺路
- Cargo workspace 支持任意路径的成员（`members = ["user/heap"]`）
- 与内核态 `heap/` 区分清晰

### D2: 复用 `eneros-heap` 的 `BuddyAllocator`

`user/heap` 依赖 `eneros-heap` crate，直接使用 `BuddyAllocator`。理由：
- 蓝图 §1 "用户态堆复用算法"
- v0.10.0 的 `BuddyAllocator` 已是 public，字段和方法均可访问
- 避免代码重复
- 内核堆和用户态堆使用各自独立的 `BuddyAllocator` 实例（不同内存池）

### D3: 仅用 buddy，不用 slab

蓝图 `UserHeap` 结构体只含 `buddy: BuddyAllocator`，无 slab。理由：
- 蓝图未提 slab
- 用户态性能要求 < 500ns，buddy 的 O(log N) 分配足够
- 简化实现（Karpathy 简洁优先）
- 若性能不足，后续可加 slab

### D4: `#[alloc_error_handler]` 不放库中

蓝图代码含 `#[alloc_error_handler]`，但这是二进制级属性，不应在库 crate 中定义。理由：
- `#[alloc_error_handler]` 每个二进制只能有一个
- 库 crate 定义会与消费方二进制冲突
- 正确做法：库提供 `trigger_oom()` 函数，二进制定义 `#[alloc_error_handler]` 调用它
- 文档说明此设计

### D5: `spin::Mutex` 而非自实现自旋锁

蓝图 §5.4 提"用户态无 spin crate 需自实现自旋锁"。但本项目 workspace 中 `spin` 已是可用依赖。理由：
- v0.10.0 已引入 `spin = "0.9"` 作为 workspace 依赖
- `user/heap` 直接依赖 `spin` 即可，无需自实现
- 自实现自旋锁增加复杂度且易出错（Karpathy 简洁优先）
- `spin` crate 是 no_std 兼容的成熟库

### D6: Quota 为简单计数器

`Quota` 结构体仅含 `limit` 和 `used` 两个计数字段。理由：
- 蓝图 §9.5 "配额可配置"，简单计数器即可满足
- 不实现复杂的配额策略（如 per-type 限制、分级配额）
- OOM 触发条件：`used + size > limit`（limit=0 表示无限制）

### D7: 测试不注册 `#[global_allocator]`

测试使用 `cfg_attr(not(test), no_std)` 模式，`#[global_allocator]` 仅在 `#[cfg(not(test))]` 下注册。理由：
- 测试构建链接 std，std 有自己的全局分配器
- 直接调用 `GlobalAlloc::alloc/dealloc` 方法测试分配逻辑
- Vec/String/HashMap 集成测试需在独立二进制中进行（不在本 crate 测试范围）

---

## 非目标（Non-Goals）

- **不实现** per-thread heap（蓝图 §8.4，未来版本）
- **不实现** 多分区独立堆实例（蓝图 §9.7，未来版本）
- **不实现** `#[alloc_error_handler]`（属二进制 crate）
- **不实现** slab 分配器（蓝图未要求，buddy 足够）
- **不做** Vec/String/HashMap 独立二进制集成测试（延后到 userland 二进制集成）
- **不做** QEMU 运行时验证（延后到 kernel 集成）
- **不做**纳秒级性能基准测试（设计目标 < 500ns，CI 不强制测量）

---

## 风险与缓解

| 风险 | 等级 | 缓解 |
|------|------|------|
| 配额检查与 buddy 分配的原子性 | 中/高 | `spin::Mutex` 保护整个 alloc/dealloc 流程 |
| OOM handler 缺失导致死锁 | 中/高 | 默认 panic "user heap OOM" |
| `#[alloc_error_handler]` nightly 不稳定 | 低/中 | 不在库中定义，由二进制处理 |
| buddy 分配失败但配额未满 | 低/低 | 也触发 OOM handler |
| 测试中 `#[global_allocator]` 冲突 | 中/中 | `#[cfg(not(test))]` 门控注册 |

---

## 性能目标（设计目标，非 CI 强制）

- 用户态 alloc < 500ns（蓝图 §7.3）
- 配额检查 O(1)（简单计数器比较）

**验证策略**：配额检查为 O(1) 常数时间操作；buddy 分配为 O(log N)；纳秒级时序不在 CI 中测量。

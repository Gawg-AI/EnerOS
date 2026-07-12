# v0.18.0 线程/任务抽象 Spec

## Why

v0.17.0 完成多核内存一致性后，调度器（v0.16.0）仅持有 `Tid` 标识与每核运行队列，缺少真正的"线程"数据结构。线程是调度的基本单位，是 Agent/RTOS 控制循环的载体——没有 TCB 就无法做上下文切换、阻塞/唤醒、优先级抢占。

v0.18.0 在 `eneros-sched` crate 内引入 TCB（Thread Control Block）、五态状态机、ARM64 上下文切换与基本优先级调度，为 v0.19.0 分区调度器和后续 RTOS 控制循环提供数据结构基础。

## What Changes

- **新增** `crates/kernel/sched/src/tcb.rs`（~250 行）— `ThreadState` 枚举、`Tcb` 结构体、状态机转换 `transition()`、ARM64 栈帧初始化 `init_stack_frame()`
- **新增** `crates/kernel/sched/src/switch.rs`（~180 行）— `context_switch` naked 函数（aarch64 内联汇编）、`thread_switch` 包装
- **新增** `crates/kernel/sched/src/priority.rs`（~120 行）— 优先级比较、`select_next_by_priority` 选下一个线程
- **新增** 全局线程管理 API（在 `tcb.rs` 或 `lib.rs` 中）：`thread_create` / `thread_destroy` / `thread_block` / `thread_resume` / `thread_exit` / `thread_yield` / `thread_state`
- **修改** `crates/kernel/sched/src/lib.rs` — 添加 `pub mod tcb/switch/priority` 声明、`pub use` 导出、`extern crate alloc` 引入
- **修改** `crates/kernel/sched/Cargo.toml` — 版本 `0.16.0` → `0.18.0`
- **修改** 根 `Cargo.toml` — workspace 版本 `0.17.0` → `0.18.0`
- **修改** `Makefile` — `VERSION` `0.17.0` → `0.18.0`
- **修改** `.github/workflows/ci.yml` — 版本标识更新为 v0.18.0
- **修改** `ci/src/gate.rs` — 注释追加 v0.18.0
- **新增** `docs/smp/thread-abstraction-design.md` — 线程抽象设计文档
- **新增** `docs/smp/context-switch-guide.md` — ARM64 上下文切换说明

## Impact

- **Affected specs**: v0.16.0（多核调度器，复用其 `Tid`/`CoreMask`/`Spinlock`）、v0.19.0（分区调度器，将依赖 TCB）
- **Affected code**:
  - `crates/kernel/sched/` — 主要变更点
  - 根 `Cargo.toml`、`Makefile`、`.github/workflows/ci.yml`、`ci/src/gate.rs` — 版本同步
- **依赖关系**: v0.18.0 依赖 v0.11.0（用户态堆，`Box` 分配 TCB）与 v0.17.0（多核一致性，上下文切换内存屏障）
- **回归风险**: sched crate 原零外部依赖，v0.18.0 引入 `alloc` crate（Rust 内置，非外部依赖），不破坏 D2"零外部依赖"精神

## ADDED Requirements

### Requirement: TCB 数据结构与状态机

系统 SHALL 提供 `ThreadState` 枚举（`Ready`/`Running`/`Blocked`/`Suspended`/`Dead`）与 `Tcb` 结构体，包含 `tid`、`state`、`priority`、`stack`、`stack_top`、`stack_size`、`sp`、`pc`、`entry`、`partition` 字段。

#### Scenario: 合法状态转换
- **WHEN** `Tcb::transition(Ready → Running)` 被调用
- **THEN** 返回 `Ok(())` 且 `state` 更新为 `Running`

#### Scenario: 非法状态转换
- **WHEN** `Tcb::transition(Dead → Running)` 被调用
- **THEN** 返回 `Err("invalid transition")` 且 `state` 保持 `Dead`

#### Scenario: 合法转换集合
- **GIVEN** 转换规则表
- **THEN** 仅以下转换合法：`Ready↔Running`、`Running→Blocked`、`Blocked→Ready`、`Ready↔Suspended`、`Running→Dead`、`Ready→Dead`

### Requirement: ARM64 上下文切换

系统 SHALL 提供 `context_switch(from_sp, to_sp)` naked 函数，使用 `extern "C"` ABI，保存 callee-saved 寄存器（x19-x30）到当前栈，从目标栈恢复。

#### Scenario: 切换保存恢复
- **WHEN** `thread_switch(from, to)` 被调用
- **THEN** `from.sp` 被更新为当前 sp，`to.sp` 对应的栈被加载，callee-saved 寄存器恢复后 `ret` 到 `to.pc`

#### Scenario: 栈帧布局
- **GIVEN** `init_stack_frame(stack_top, entry)` 初始化栈帧
- **THEN** 栈帧大小 272 字节（31 个 64 位寄存器 + spsr_el1 + elr_el1），`x30`/`elr_el1` 设为入口地址，`spsr_el1` 设为 `0x3C5`（启用 IRQ）

### Requirement: 线程生命周期 API

系统 SHALL 提供以下全局 API：

- `thread_create(entry, stack_size, priority) -> Tid`
- `thread_destroy(tid)`
- `thread_block(tid)`
- `thread_resume(tid)`
- `thread_exit(tid) -> !`
- `thread_yield()`
- `thread_state(tid) -> ThreadState`

#### Scenario: 线程创建
- **WHEN** `thread_create(entry, 4096, 5)` 被调用
- **THEN** 分配 TCB + 4KB 栈，初始化栈帧，状态置 `Ready`，返回新 `Tid`

#### Scenario: 栈分配失败
- **WHEN** 堆分配失败
- **THEN** `thread_create` 返回 `Tid(0)`（无效 Tid）

#### Scenario: 线程销毁
- **WHEN** `thread_destroy(tid)` 被调用且 `tid` 状态非 `Running`
- **THEN** TCB 状态置 `Dead`，栈被回收

#### Scenario: 销毁 Running 线程
- **WHEN** `thread_destroy(tid)` 被调用且 `tid` 当前 `Running`
- **THEN** 返回错误（销毁 Running 线程需先切换）

### Requirement: 优先级调度

系统 SHALL 提供基于 `priority` 字段（0 最高）的线程选择：在多个 `Ready` 线程中选 `priority` 值最小者。

#### Scenario: 选最高优先级
- **GIVEN** Ready 队列含 `Tid(1, prio=5)` 与 `Tid(2, prio=2)`
- **THEN** `select_next_by_priority()` 返回 `Some(Tid(2))`

#### Scenario: 同优先级 FIFO
- **GIVEN** Ready 队列含两个 `prio=3` 线程，`Tid(1)` 先入队
- **THEN** `select_next_by_priority()` 返回 `Some(Tid(1))`（FIFO）

## MODIFIED Requirements

### Requirement: eneros-sched crate

v0.16.0 提供 `Scheduler`/`PerCoreRq`/`CoreMask`/`Tid` 数据结构与 `sched_init`/`enqueue`/`pick_next`/`balance_load` API。

v0.18.0 新增 `tcb`/`switch`/`priority` 模块，引入 `alloc` crate 以支持 `Box<Tcb>` 动态分配。`Tid`/`CoreMask`/`Spinlock` 类型复用现有定义，不重定义。

## Design Decisions

- **D1（Tid/CoreMask 复用）**：蓝图 §4.5 代码片段重新定义了 `Tid`，但 v0.16.0 的 `percore.rs` 已有 `pub struct Tid(pub u32)` 并 `pub use` 导出。v0.18.0 复用现有 `Tid`，不重定义，避免类型冲突。`CoreMask` 同理。
- **D2（alloc 引入）**：蓝图 §2 假设前提"线程栈由堆分配"。引入 `extern crate alloc;`（Rust 内置，非外部 crate 依赖），保持 sched crate 的"零外部依赖"精神（D2 of v0.16.0）。aarch64 实际运行时依赖 v0.10.0 `eneros-heap` 提供全局分配器，host 测试用 `std` 分配器。
- **D3（naked 函数 cfg gate）**：`context_switch` 与 `init_stack_frame` 含 aarch64 内联汇编，host 无法编译。用 `#[cfg(target_arch = "aarch64")]` gate，host 侧提供 stub 函数（返回错误或 panic）以便 crate 在 host 编译通过。
- **D4（全局线程表）**：用 `percore::Spinlock` 包裹固定大小数组 `[Option<Box<Tcb>>; MAX_THREADS]`（256 槽，复用 `MAX_THREADS` 常量）。`Tid` 即数组索引。无外部依赖，const 初始化。
- **D5（Tcb 不 impl Send/Sync）**：`Tcb` 含裸指针（`stack`/`stack_top`），不自动 `Send`/`Sync`。全局表的 `Spinlock` 提供 `Sync`，访问需 `unsafe` 标注。
- **D6（priority.rs 自行设计）**：蓝图未提供 priority.rs 代码，基于 `priority: u8`（0 最高）实现简单的优先级选择——遍历 Ready 线程选最小 `priority` 值，同优先级 FIFO。
- **D7（非瓶颈版本）**：v0.18.0 非蓝图标记的瓶颈版本（★），代码"骨架可用"——naked 函数与栈帧布局必须与 ARMv8 异常模型一致（蓝图 §8.1 高风险），但性能测试（< 2μs）可在 QEMU 实机后验证。
- **D8（文档位置）**：遵循新规则 §2.3.3，两份文档放 `docs/smp/`（与 v0.16.0 调度器文档保持一致）。
- **D9（测试策略）**：
  - 状态机转换：host 可测
  - `init_stack_frame`：aarch64 专属，host 用 stub 测试跳过
  - `context_switch`：aarch64 专属，host 无法测真实切换，仅测 API 签名编译
  - 全局线程 API：host 可测（用 `std` 全局分配器）
  - 优先级调度：host 可测
  - 单元测试覆盖率 ≥ 80%（蓝图 §6.1）

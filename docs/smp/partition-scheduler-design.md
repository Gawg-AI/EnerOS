# EnerOS 分区调度器设计

> 版本：v0.19.0 | 日期：2026-07-13 | 状态：已实现
> 蓝图依据：`phase0.md §v0.19.0`、`Power_Native_Agent_OS_Blueprint.md §4`（调度算法）、§6.3（性能要求）、§43.1（no_std 合规）、§43.2（非瓶颈版本，抖动 < 1ms 延后至 QEMU）
> 实现位置：`crates/kernel/sched/src/partition_sched.rs`、`crates/kernel/sched/src/wcet.rs`

## 1. 概述

EnerOS 分区调度器是 Phase 0 P0-F（调度器）的第三块拼图。v0.16.0 多核调度器
提供了 per-core 运行队列与核亲和性；v0.18.0 线程抽象引入了 TCB、状态机与
上下文切换；v0.19.0 在此之上引入 **ARINC 653 风格的时间触发分区调度**
（time-triggered partition scheduling），为混合关键性系统提供时间隔离。

### 1.1 为什么选择时间触发而非优先级抢占

电力场景中，RTOS 控制大区（10ms 控制周期）与 Agent Runtime（管理信息大区）
共存在同一硬件平台上。若采用纯优先级抢占调度，高优先级线程可能长时间独占
CPU，导致低优先级分区饿死，无法保证管理信息大区的响应时延。

时间触发分区调度的优势：

| 特性 | 优先级抢占 | 时间触发分区 |
|------|-----------|-------------|
| 隔离方式 | 优先级（弱） | 时间片（强） |
| 可预测性 | 难（依赖最坏响应分析） | 强（周期确定） |
| 验证难度 | 高（需全局调度分析） | 低（每分区独立分析） |
| 适合场景 | 通用 OS | 混合关键性系统 |
| 电力适配 | 一般 | 强（SCADA/EMS 周期确定） |

时间触发分区调度的核心思想：将 CPU 时间划分为 **主帧（Major Frame）**，
主帧内包含若干 **时间片（Slot）**，每个时间片分配给一个分区。主帧周期性
循环执行，保证每个分区在固定时间窗口内获得 CPU 资源。

### 1.2 本版本交付物

- **`partition_sched.rs`**（~350 行）— `PartitionId` 新类型、`PartitionSlot`、
  `MajorFrame`、`JitterStats` 结构体；`schedule_add / schedule_run / on_tick /
  schedule_stop` API；时间源注入接口。
- **`wcet.rs`**（~150 行）— `WCET_TABLE` 静态表、`wcet_set / wcet_estimate /
  check_partition_overrun` API。

本版本**不**包含的能力（明确延后）：

- 分区模式切换（冷启动/正常/停止）—— 未来版本
- 健康监控表（Health Monitor Table）—— 未来版本
- 分区间通信（IPC）—— v0.20.0
- 动态重配置（运行时修改 Major Frame）—— 未来版本

crate 顶层属性 `#![cfg_attr(not(test), no_std)]` 遵循蓝图 §43.1；本版本
**不引入任何外部依赖**（D1 决策），时间源通过函数指针注入。

## 2. 核心数据结构

### 2.1 PartitionId

```rust
// crates/kernel/sched/src/partition_sched.rs

/// 分区标识（新类型，D3 决策）
///
/// 与 Tid 一样是 u32 包装类型，但语义不同：
/// - Tid 标识线程
/// - PartitionId 标识分区（一组线程的集合）
///
/// 约定：
/// - PartitionId(0) = RTOS 控制大区
/// - PartitionId(1) = Agent Runtime（管理信息大区）
/// - PartitionId(2..) = 其他分区
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct PartitionId(pub u32);

impl PartitionId {
    pub const fn new(id: u32) -> Self {
        Self(id)
    }
    pub const fn raw(self) -> u32 {
        self.0
    }
}
```

设计要点（D3 决策）：

- **新类型模式**：避免 `PartitionId` 与 `Tid` 混淆（两者都是 `u32`）。
- **`const fn`**：支持 `const RTOS: PartitionId = PartitionId::new(0);` 常量定义。
- **`Ord` 派生**：便于排序与 BTreeMap 索引。

与 `Tcb.partition` 字段（v0.18.0 预留的 `u32`）的关系：本版本将 `u32` 语义
明确为 `PartitionId`，但为保持向后兼容，`Tcb.partition` 仍是 `u32` 而非
`PartitionId`（避免修改 v0.18.0 已稳定 API）。调度器内部用
`PartitionId(tcb.partition)` 构造。

### 2.2 PartitionSlot

```rust
/// 分区时间片
///
/// 描述主帧中一个时间窗口：在 `duration_ms` 时间内，CPU 归属 `partition`。
#[derive(Clone, Copy, Debug)]
pub struct PartitionSlot {
    /// 该时间片归属的分区
    pub partition: PartitionId,
    /// 时间片时长（毫秒）
    pub duration_ms: u64,
}

impl PartitionSlot {
    pub const fn new(partition: PartitionId, duration_ms: u64) -> Self {
        Self { partition, duration_ms }
    }
}
```

### 2.3 MajorFrame

```rust
/// 主帧最大 slot 数
pub const MAX_SLOTS: usize = 16;

/// 主帧（Major Frame）
///
/// 一个主帧包含最多 16 个时间片，按顺序循环执行。
/// 主帧总周期 = 所有 slot 的 duration_ms 之和。
pub struct MajorFrame {
    /// 时间片数组
    slots: [Option<PartitionSlot>; MAX_SLOTS],
    /// 已添加的 slot 数量
    count: usize,
    /// 主帧总周期（ms），由 add 时累加
    total_duration_ms: u64,
}

impl MajorFrame {
    pub const fn new() -> Self {
        Self {
            slots: [None; MAX_SLOTS],
            count: 0,
            total_duration_ms: 0,
        }
    }

    /// 添加一个时间片
    pub fn add(&mut self, slot: PartitionSlot) -> Result<(), &'static str> {
        if self.count >= MAX_SLOTS {
            return Err("major frame full (max 16 slots)");
        }
        self.slots[self.count] = Some(slot);
        self.count += 1;
        self.total_duration_ms = self.total_duration_ms.saturating_add(slot.duration_ms);
        Ok(())
    }

    pub fn slot(&self, index: usize) -> Option<PartitionSlot> {
        if index < self.count {
            self.slots[index]
        } else {
            None
        }
    }

    pub fn count(&self) -> usize {
        self.count
    }

    pub fn total_duration_ms(&self) -> u64 {
        self.total_duration_ms
    }
}
```

设计要点：

- **固定数组 16 槽**：避免动态分配；16 个 slot 足够覆盖典型电力场景
  （RTOS 5ms + Agent 20ms + RTOS 5ms = 3 个 slot）。
- **`Option<PartitionSlot>`**：未使用的槽位为 `None`，避免默认值歧义。
- **`total_duration_ms` 累加**：添加时维护，避免每次扫描数组求和。

## 3. 时间源注入机制

### 3.1 设计动机

`eneros-sched` crate 截至 v0.18.0 保持**零外部依赖**（仅依赖 `alloc` crate
与 v0.10.0 `eneros-heap`）。v0.19.0 需要时间源（读取当前时间）与定时器注册
能力，但直接依赖 `eneros-time`（v0.12.0）会引入跨子系统耦合，且
`eneros-time` 依赖 `eneros-hal`，形成 kernel→drivers→hal 的依赖链。

D1 决策：**采用函数指针注入**，由调用方（通常是 `eneros-runtime` 或板级
初始化代码）在启动时注入时间源与定时器注册函数，`eneros-sched` 本身不依赖
任何时间相关 crate。

优势：

| 方案 | 依赖 | 可测性 | 耦合度 |
|------|------|--------|--------|
| 直接依赖 eneros-time | 高 | 差（需 mock） | 高 |
| Trait 对象注入 | 中 | 中 | 中 |
| **函数指针注入（D1）** | **零** | **好（host 可注入 mock）** | **零** |

### 3.2 API

```rust
// crates/kernel/sched/src/partition_sched.rs
use core::sync::atomic::{AtomicBool, Ordering};

/// 时间源函数指针类型：返回当前纳秒时间戳
type TimeSource = fn() -> u64;

/// 定时器注册函数指针类型：在 `after_ns` 纳秒后触发 `callback`
/// 返回 true 表示注册成功
type TimerRegistrar = fn(after_ns: u64, callback: fn()) -> bool;

/// 注入的时间源（初始为 dummy，未注入时返回 0）
static TIME_SOURCE: spin::Once<TimeSource> = spin::Once::new();

/// 注入的定时器注册器
static TIMER_REGISTRAR: spin::Once<TimerRegistrar> = spin::Once::new();

/// 调度器是否已启动
static SCHED_RUNNING: AtomicBool = AtomicBool::new(false);

/// 注入时间源（初始化时调用一次）
pub fn set_time_source(f: TimeSource) {
    TIME_SOURCE.call_once(|| f);
}

/// 注入定时器注册器
pub fn set_timer_registrar(f: TimerRegistrar) {
    TIMER_REGISTRAR.call_once(|| f);
}

/// 获取当前纳秒时间戳（未注入时返回 0）
pub fn now_ns() -> u64 {
    match TIME_SOURCE.get() {
        Some(f) => f(),
        None => 0,
    }
}
```

设计要点：

- **`spin::Once`**：保证注入只发生一次，后续调用被忽略（防误覆盖）。
- **未注入时返回 0**：host 测试时不注入，`now_ns()` 返回 0，调度器仍可
  编译运行（抖动统计会失效，但不崩溃）。
- **`fn() -> u64` 裸函数指针**：不捕获环境，`Copy`，零开销。

### 3.3 使用示例

```rust
// 典型初始化流程（在 eneros-runtime 或板级 main 中）

use eneros_sched::partition_sched::*;

fn board_time_ns() -> u64 {
    // 读取硬件定时器计数器（CNTVCT_EL0 或 RTC）
    // 由 eneros-time 提供
    eneros_time::now_ns()
}

fn board_register_timer(after_ns: u64, callback: fn()) -> bool {
    // 设置硬件定时器中断，after_ns 后触发 callback
    eneros_time::schedule_oneshot(after_ns, callback)
}

fn init_partition_scheduler() {
    // 1. 注入时间源与定时器
    set_time_source(board_time_ns);
    set_timer_registrar(board_register_timer);

    // 2. 配置主帧
    let mut frame = MajorFrame::new();
    frame.add(PartitionSlot::new(PartitionId::new(0), 5)).unwrap();   // RTOS 5ms
    frame.add(PartitionSlot::new(PartitionId::new(1), 20)).unwrap();  // Agent 20ms
    frame.add(PartitionSlot::new(PartitionId::new(0), 5)).unwrap();   // RTOS 5ms

    // 3. 启动调度
    schedule_run(frame).expect("failed to start partition scheduler");
}
```

## 4. 分区调度流程

### 4.1 schedule_add

`MajorFrame::add`（见 §2.3）负责添加时间片。调度器内部通过全局
`SCHED_FRAME: Spinlock<MajorFrame>` 持有主帧。

```rust
use crate::percore::Spinlock;

pub static SCHED_FRAME: Spinlock<MajorFrame> = Spinlock::new(MajorFrame::new());

/// 向全局主帧添加时间片
pub fn schedule_add(slot: PartitionSlot) -> Result<(), &'static str> {
    if SCHED_RUNNING.load(Ordering::Acquire) {
        return Err("cannot add slot while scheduler running");
    }
    SCHED_FRAME.lock().add(slot)
}
```

### 4.2 schedule_run

启动调度：初始化抖动统计，注册首个定时器，标记运行状态（D7 决策返回 `Result`）。

```rust
/// 启动分区调度
///
/// 注册首个时间片的到期定时器，调度器开始周期运行。
///
/// # 返回
/// - Ok(())：调度器已启动
/// - Err("already running")：调度器已在运行
/// - Err("empty major frame")：主帧无时间片
/// - Err("timer registrar not set")：未注入定时器注册器
/// - Err("timer register failed")：定时器注册失败
pub fn schedule_run(frame: MajorFrame) -> Result<(), &'static str> {
    if SCHED_RUNNING.swap(true, Ordering::AcqRel) {
        return Err("already running");
    }
    if frame.count() == 0 {
        SCHED_RUNNING.store(false, Ordering::Release);
        return Err("empty major frame");
    }

    // 替换全局主帧
    *SCHED_FRAME.lock() = frame;

    // 重置 slot 索引与抖动统计
    {
        let mut state = SCHED_STATE.lock();
        state.current_slot = 0;
        state.frame_start_ns = now_ns();
        state.last_tick_ns = state.frame_start_ns;
        state.jitter = JitterStats::new();
        state.switch_count = 0;
    }

    // 注册首个定时器（第一个 slot 的 duration）
    let first_duration_ns = SCHED_FRAME.lock().slot(0)
        .ok_or("empty major frame")?
        .duration_ms * 1_000_000;

    let registrar = TIMER_REGISTRAR.get().ok_or("timer registrar not set")?;
    if !registrar(first_duration_ns, on_tick) {
        SCHED_RUNNING.store(false, Ordering::Release);
        return Err("timer register failed");
    }
    Ok(())
}
```

### 4.3 on_tick

定时器到期回调：计算抖动、记录、推进 slot、切换分区、注册下一个定时器。

```rust
/// 调度器内部状态
pub struct SchedState {
    pub current_slot: usize,
    pub frame_start_ns: u64,
    pub last_tick_ns: u64,
    pub jitter: JitterStats,
    pub switch_count: u64,
}

pub static SCHED_STATE: Spinlock<SchedState> = Spinlock::new(SchedState {
    current_slot: 0,
    frame_start_ns: 0,
    last_tick_ns: 0,
    jitter: JitterStats::new(),
    switch_count: 0,
});

/// 定时器回调（由硬件定时器中断调用）
///
/// D11 决策：本函数仅记录分区切换事件，不真正执行上下文切换
/// （真正的切换由 v0.18.0 thread_switch 完成，本函数只更新调度状态）。
fn on_tick() {
    if !SCHED_RUNNING.load(Ordering::Acquire) {
        return;
    }

    let now = now_ns();
    let (next_slot, next_duration_ns, expected_ns) = {
        let frame = SCHED_FRAME.lock();
        let mut state = SCHED_STATE.lock();

        // 1. 计算抖动 = 实际时间 - 期望时间
        let expected = state.last_tick_ns + frame.slot(state.current_slot)
            .map(|s| s.duration_ms * 1_000_000)
            .unwrap_or(0);
        let jitter_ns = now.saturating_sub(expected);
        state.jitter.record(jitter_ns);

        // 2. 推进 slot（循环回到 0）
        state.current_slot = (state.current_slot + 1) % frame.count();
        state.last_tick_ns = now;
        state.switch_count += 1;

        // 3. 若回到 slot 0，记录新主帧起点
        if state.current_slot == 0 {
            state.frame_start_ns = now;
        }

        let next = frame.slot(state.current_slot).unwrap();
        (state.current_slot, next.duration_ms * 1_000_000, now)
    };

    // 4. 切换分区（D11：仅记录，真正切换由调用方 thread_switch）
    let _ = next_slot;
    switch_partition();

    // 5. 注册下一个定时器
    if let Some(registrar) = TIMER_REGISTRAR.get() {
        let _ = registrar(next_duration_ns, on_tick);
    }
}
```

### 4.4 schedule_stop

```rust
/// 停止分区调度
///
/// 标记调度器停止，后续 on_tick 调用直接返回。
/// 已注册的定时器无法撤销（取决于硬件支持），可能再触发一次 on_tick。
pub fn schedule_stop() {
    SCHED_RUNNING.store(false, Ordering::Release);
}
```

### 4.5 JitterStats

```rust
/// 抖动统计（微秒精度）
///
/// 记录每次定时器回调的实际时间与期望时间的偏差。
#[derive(Clone, Copy, Debug)]
pub struct JitterStats {
    pub min_us: u64,
    pub max_us: u64,
    pub sum_us: u64,
    pub samples: u64,
}

impl JitterStats {
    pub const fn new() -> Self {
        Self { min_us: u64::MAX, max_us: 0, sum_us: 0, samples: 0 }
    }

    /// 记录一次抖动（输入为纳秒，内部转微秒）
    pub fn record(&mut self, jitter_ns: u64) {
        let jitter_us = jitter_ns / 1000;
        if jitter_us < self.min_us {
            self.min_us = jitter_us;
        }
        if jitter_us > self.max_us {
            self.max_us = jitter_us;
        }
        self.sum_us = self.sum_us.saturating_add(jitter_us);
        self.samples = self.samples.saturating_add(1);
    }

    pub fn avg_us(&self) -> u64 {
        if self.samples == 0 { 0 } else { self.sum_us / self.samples }
    }

    pub fn is_empty(&self) -> bool {
        self.samples == 0
    }
}
```

## 5. 与 v0.16.0 / v0.18.0 的关系

### 5.1 复用 v0.16.0 基础设施

| 组件 | 来源 | 复用方式 |
|------|------|---------|
| `Spinlock<T>` | `crates/kernel/sched/src/percore.rs` | `SCHED_FRAME`、`SCHED_STATE` 保护 |
| `Tid` | `crates/kernel/sched/src/percore.rs` | WCET 表索引 |
| `const fn new` 模式 | v0.16.0 Spinlock | `MajorFrame::new`、`JitterStats::new` |

### 5.2 复用 v0.18.0 TCB

- `Tcb.partition: u32` 字段在 v0.18.0 预留，本版本赋语义为 `PartitionId`。
- `switch_partition()`（§4.3 调用）内部按 `current_slot` 对应的 `PartitionId`
  过滤 `THREAD_TABLE` 中 `partition == id` 且 `state == Ready` 的线程，
  调用 v0.18.0 `select_next_by_priority` 的分区过滤变体。
- 上下文切换复用 v0.18.0 `thread_switch`，本版本不重新实现。

### 5.3 三版本协作架构

```text
v0.16.0 多核调度器        v0.18.0 线程抽象          v0.19.0 分区调度器
─────────────────        ───────────────          ───────────────
PerCoreRq                Tcb / ThreadState         MajorFrame / Slot
Tid / CoreMask           thread_create/destroy     PartitionId
Spinlock<T>              context_switch            on_tick / JitterStats
pick_next(core)          select_next_by_priority   schedule_run / stop
                         THREAD_TABLE              WCET_TABLE
                              │
                              ▼
                    partition_sched.rs
                    按 partition 过滤 TCB
                    调用 thread_switch
```

## 6. 内存安全

### 6.1 Spinlock + 内部可变性模式

`SCHED_FRAME` 与 `SCHED_STATE` 为 `Spinlock<T>`，复用 v0.16.0 实现
（D2 决策：用 Spinlock 而非 `static mut`）。

```rust
// ❌ 反模式（v0.19.0 禁止）
static mut SCHED_FRAME: MajorFrame = MajorFrame::new();
// 访问需 unsafe，且无并发保护

// ✅ 本版本采用
pub static SCHED_FRAME: Spinlock<MajorFrame> = Spinlock::new(MajorFrame::new());
// Spinlock::new 是 const fn（v0.16.0 提供），可在 static 上下文初始化
// 访问通过 .lock() 获取 MutexGuard，自动 unsafe Sync
```

### 6.2 `unsafe impl Sync` 的安全性论证

`Spinlock<T>` 内部用 `core::sync::atomic::AtomicBool` 实现互斥，`T` 通过
`UnsafeCell` 持有。`Spinlock<T>: Sync` 的安全性保证：

1. **互斥**：`AtomicBool` 的 `compare_exchange` 保证同一时刻只有一个核
   持有锁，对 `T` 的访问不会数据竞争。
2. **内存序**：`lock()` 用 `Acquire`，`unlock()` 用 `Release`，保证
   锁内修改对其他核可见。
3. **`T: Send`**：`T` 必须能跨核传递（`MajorFrame`、`SchedState` 仅含
   `Copy` 类型，自动 `Send`）。

因此 `Spinlock<MajorFrame>: Sync` 是安全的，无需手动 `unsafe impl`。

### 6.3 裸函数指针的安全性

`TIME_SOURCE: spin::Once<fn() -> u64>` 中 `fn()` 是 `Copy + Sync` 的，
`Once` 保证只初始化一次，读取 (`get()`) 返回 `Option<&fn>`，无需 `unsafe`。

## 7. 测试策略

### 7.1 测试矩阵（D8 决策：抖动 < 1ms 延后至 QEMU）

| 测试类别 | 测试内容 | 运行环境 | 可测性 |
|---------|---------|---------|--------|
| MajorFrame | add/slot/count 接口 | host (x86_64) | ✅ 完全可测 |
| MajorFrame | 16 slot 上限拒绝 | host | ✅ 完全可测 |
| PartitionId | new/raw/比较 | host | ✅ 完全可测 |
| JitterStats | record/min/max/avg | host | ✅ 完全可测 |
| schedule_add | 运行中拒绝添加 | host | ✅ 完全可测 |
| schedule_run | 空主帧报错 | host | ✅ 完全可测 |
| schedule_run | 未注入定时器报错 | host | ✅ 完全可测 |
| on_tick | slot 推进逻辑 | host（注入 mock 时间源） | ✅ 完全可测 |
| WCET 表 | set/estimate/overrun | host | ✅ 完全可测 |
| 抖动 < 1ms | 实际硬件定时器精度 | aarch64 QEMU | ⏳ 延后（D8） |
| 分区切换 | thread_switch 集成 | aarch64 QEMU | ⏳ 延后 |

### 7.2 host 侧测试示例

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_major_frame_add() {
        let mut frame = MajorFrame::new();
        assert_eq!(frame.count(), 0);
        assert!(frame.add(PartitionSlot::new(PartitionId::new(0), 5)).is_ok());
        assert_eq!(frame.count(), 1);
        assert_eq!(frame.total_duration_ms(), 5);
    }

    #[test]
    fn test_major_frame_full() {
        let mut frame = MajorFrame::new();
        for _ in 0..MAX_SLOTS {
            frame.add(PartitionSlot::new(PartitionId::new(0), 1)).unwrap();
        }
        assert!(frame.add(PartitionSlot::new(PartitionId::new(0), 1))
            .is_err());
    }

    #[test]
    fn test_jitter_stats() {
        let mut js = JitterStats::new();
        js.record(2_000);   // 2us
        js.record(5_000);   // 5us
        js.record(1_000);   // 1us
        assert_eq!(js.samples, 3);
        assert_eq!(js.min_us, 1);
        assert_eq!(js.max_us, 5);
        assert_eq!(js.avg_us, 2);
    }
}
```

### 7.3 覆盖率目标

- 数据结构与状态机：100%
- API 主路径（add/run/stop/on_tick）：≥ 80%
- WCET 表 API：≥ 80%
- 抖动精度验证：延后至 QEMU（D8）

## 8. 设计决策汇总

| # | 决策 | 理由 |
|---|------|------|
| **D1** | 时间源用函数指针注入，不依赖 eneros-time | 保持 sched crate 零外部依赖；host 可注入 mock |
| **D2** | 用 Spinlock 而非 `static mut` | 避免 unsafe 散落；并发安全；v0.16.0 已提供 const fn |
| **D3** | PartitionId 新类型（u32 包装） | 防止与 Tid 混淆；类型安全 |
| **D7** | `schedule_run` 返回 `Result<(), &'static str>` | 区分「未注入」「空主帧」「注册失败」等错误 |
| **D8** | 非瓶颈版本，抖动 < 1ms 验证延后至 QEMU | host 无法验证硬件定时器精度（蓝图 §43.2） |
| **D11** | `switch_partition` 仅记录切换，不执行上下文切换 | 切换由 v0.18.0 thread_switch 完成；职责分离 |

## 9. 未来改进

| 版本 | 改进内容 | 依赖 |
|------|---------|------|
| v0.20.0 | 分区间通信（IPC） | 本版本 + 消息队列 |
| 未来 | 分区模式切换（冷/暖/正常） | 本版本 + 状态机扩展 |
| 未来 | 健康监控表（HM） | 本版本 + 异常回调 |
| 未来 | 动态重配置（运行时修改 MajorFrame） | 本版本 + 安全切换协议 |
| 未来 | 在线 WCET 测量 | 本版本 + 性能计数器 |
| 未来 | 多核分区调度（每核独立 MajorFrame） | 本版本 + v0.16.0 多核 |

## 10. 参考资料

- `蓝图/phase0.md §v0.19.0`—— 本版本蓝图
- `蓝图/Power_Native_Agent_OS_Blueprint.md §4`—— 调度算法
- `蓝图/Power_Native_Agent_OS_Blueprint.md §6.3`—— 性能要求
- `蓝图/Power_Native_Agent_OS_Blueprint.md §43.1`—— no_std 合规
- `docs/smp/multi-core-scheduler-design.md`—— v0.16.0 调度器（前置依赖）
- `docs/smp/thread-abstraction-design.md`—— v0.18.0 线程抽象（前置依赖）
- `docs/smp/arinc653-adaptation.md`—— ARINC 653 适配说明（配套文档）
- `docs/smp/wcet-analysis.md`—— WCET 分析（配套文档）
- ARINC Specification 653P1-3 —— Avionics Application Software Standard Interface

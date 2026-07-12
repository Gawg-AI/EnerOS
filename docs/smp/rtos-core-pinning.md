# EnerOS RTOS 绑核独占设计

> 版本：v0.16.0 | 日期：2026-07-12 | 状态：设计文档
> 蓝图依据：`phase0.md §v0.16.0`（多核调度与绑核）、`Power_Native_Agent_OS_Blueprint.md §1`（混合关键性架构）、§7.1/§7.2（验收标准）、§43.1（no_std 合规）、§43.2（瓶颈版本骨架可用）

## 1. 概述

RTOS 绑核独占是 EnerOS 多核调度器（crate `eneros-sched`）实现**混合关键性架构**
的核心机制，通过 [`CoreReservation`](../../crates/kernel/sched/src/isolation.rs) 把某核（典型为
Core 0）标记为 RTOS 专属，**拒绝**非 RTOS 线程入队，从而保证 RTOS 线程独占该核
的算力，避免被 Agent / AI 工作负载抢占。

本文档对应实现位于 `sched/src/isolation.rs`，与调度器主文档
（`docs/multi-core-scheduler-design.md`）、CPU 亲和性文档
（`docs/cpu-affinity-policy.md`）互为补充。

v0.16.0 RTOS 绑核独占机制的目标与范围：

- **核独占标记**：用 `reserved: [bool; 8]` 标记最多 8 个核的独占状态。
- **入队准入控制**：`can_enqueue(core, is_rtos)` 决定线程能否入队到某核。
- **错误处理**：`SchedError` 枚举统一描述 InvalidCore / CoreReserved /
  NoRunnableTask 三类错误。
- **调度器集成**：`reserve_core` / `release_core` / `enqueue` 三个全局 API
  封装 reservation 操作。

本版本**不**包含的能力（明确标注为「未来扩展」，见 §11）：

- 多 RTOS 分区各自绑核（如 Core 0 给 RTOS-A、Core 1 给 RTOS-B）
- 动态 reserve/release（hotplug 场景）
- RTOS 优先级继承协议
- 真正的 RTOS 线程标志从 TCB 读取（v0.16.0 硬编码 `is_rtos=false`）

crate 顶层属性 `#![cfg_attr(not(test), no_std)]` 遵循蓝图 §43.1；
`isolation.rs` 仅依赖 `core::*`（D2 决策）。

## 2. 混合关键性架构

### 2.1 架构概览

EnerOS 是**混合关键性**（mixed-criticality）操作系统，在同一 SoC 上同时承载：

- **RTOS 域**（高关键性）：实时控制任务（继电保护、PLC 扫描、IEC 61850 GOOSE）。
  要求硬实时响应（< 100μs），不可被 Agent 抢占。
- **Agent 域**（低关键性）：AI 推理、设备协议栈、Solver 求解。
  延迟可弹性（10ms~100ms），可被抢占或迁移。

```
┌─────────────────────────────────────────────────┐
│  SoC（典型 4 核或 8 核 aarch64）                │
│                                                  │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐      │
│  │  Core 0  │  │  Core 1  │  │  Core 2  │ ...  │
│  │ (RTOS)   │  │ (Agent)  │  │ (Agent)  │      │
│  │ reserved │  │  free    │  │  free    │      │
│  └──────────┘  └──────────┘  └──────────┘      │
│       ▲           ▲               ▲             │
│       │           │               │             │
│  RTOS 独占    Agent 线程      Agent 线程         │
│  (Tid 0~99)   (Tid 100~199)  (Tid 200~255)      │
└─────────────────────────────────────────────────┘
```

### 2.2 RTOS 线程独占 Core 0

通过 `reserve_core(&mut sched, 0)` 把 Core 0 标记为 RTOS 专属：

- Core 0 的 `rqs[0].reserved = true`（与 `reservation.reserved[0]` 镜像）。
- 后续 `enqueue(sched, tid, 0)` 中，`can_enqueue(0, is_rtos=false)` 返回 `false`，
  非 RTOS 线程被静默拒绝。
- RTOS 线程的 enqueue 由调用方负责传入 `is_rtos=true`（v0.16.0 简化为硬编码
  `false`，真实 RTOS enqueue 需 v0.18.0 线程抽象补全）。

### 2.3 Agent / AI 线程分布 Core 1+

非 RTOS 线程（Agent / AI / 协议栈）默认可在任何非 reserved 核上运行：

- 调用方 `enqueue(sched, tid, 1)` / `enqueue(sched, tid, 2)` ... 成功入队。
- 试图 `enqueue(sched, tid, 0)`（Core 0 reserved）会被 `can_enqueue` 拒绝，
  RQ[0] 保持空闲给 RTOS。

### 2.4 隔离机制防止非 RTOS 抢占

| 路径 | 防护 |
|------|------|
| Agent enqueue 到 Core 0 | `can_enqueue(0, false) == false` → 静默丢弃 |
| Agent 迁移到 Core 0 | `Balancer.balance` 不检查 reservation（v0.16.0 限制，见 §10） |
| Agent `pick_next` 在 Core 0 | RQ[0] 为空（因 enqueue 被拒），返回 `None` |
| RTOS 线程抢占 | RTOS 线程独占 Core 0，无其它线程可调度到该核 |

**注意**：v0.16.0 的 `Balancer` 不检查 reservation，可能把线程迁移到 reserved 核。
这是已知限制（见 §10 安全性考量），未来版本需补 reservation 检查。

## 3. CoreReservation 数据结构

### 3.1 数据结构

```rust
// sched/src/isolation.rs
pub const MAX_CORES: usize = 8;

#[derive(Debug)]
pub struct CoreReservation {
    /// `reserved[i] == true` means core `i` is RTOS-exclusive.
    pub reserved: [bool; MAX_CORES],
}
```

| 字段 | 类型 | 含义 |
|------|------|------|
| `reserved` | `[bool; 8]` | 8 槽独占标记，`true` 表示该核 RTOS 独占 |

- 容量上限 `MAX_CORES = 8`，与 `Scheduler.MAX_CORES` 一致。
- 固定数组，无堆分配（D2 no_std 合规）。
- `bool` 类型直接索引，比位运算更易读（性能差异可忽略：8 字节 vs 1 字节，cache
  line 内）。

### 3.2 const fn new

```rust
// sched/src/isolation.rs
impl CoreReservation {
    pub const fn new() -> Self {
        Self {
            reserved: [false; MAX_CORES],
        }
    }
}
```

- `const fn` 允许编译期常量初始化（用于 `Scheduler` 的 `const` 上下文）。
- 初始状态：所有 8 个核均为 `false`（未独占）。
- `Default` trait 也实现为 `Self::new()`，支持 `CoreReservation::default()`。

## 4. CoreReservation 操作

### 4.1 reserve

```rust
// sched/src/isolation.rs
pub fn reserve(&mut self, core: u32) -> Result<(), SchedError> {
    if core as usize >= MAX_CORES {
        return Err(SchedError::InvalidCore);
    }
    if self.reserved[core as usize] {
        return Err(SchedError::CoreReserved);
    }
    self.reserved[core as usize] = true;
    Ok(())
}
```

- 标记 `core` 为 RTOS 独占。
- **错误处理**：
  - `core >= 8` → `Err(InvalidCore)`
  - `core` 已 reserved → `Err(CoreReserved)`（幂等性失败，调用方需先 `release`）
- 成功返回 `Ok(())`。

| 入参 | 初始状态 | 结果 |
|------|----------|------|
| `reserve(0)` | 全 false | `Ok(())`，`reserved[0] = true` |
| `reserve(0)` | `reserved[0] = true` | `Err(CoreReserved)` |
| `reserve(8)` | 任意 | `Err(InvalidCore)` |
| `reserve(u32::MAX)` | 任意 | `Err(InvalidCore)` |

### 4.2 release

```rust
// sched/src/isolation.rs
pub fn release(&mut self, core: u32) {
    if (core as usize) < MAX_CORES {
        self.reserved[core as usize] = false;
    }
}
```

- 释放 `core` 的独占标记。
- **越界防御**：`core >= 8` 静默忽略（不报错）。
- **幂等性**：释放未 reserved 的核是无操作（不报错）。

| 入参 | 初始状态 | 结果 |
|------|----------|------|
| `release(0)` | `reserved[0] = true` | `reserved[0] = false` |
| `release(0)` | `reserved[0] = false` | 无操作（仍 false） |
| `release(8)` | 任意 | 静默忽略 |
| `release(u32::MAX)` | 任意 | 静默忽略 |

### 4.3 is_reserved

```rust
// sched/src/isolation.rs
pub fn is_reserved(&self, core: u32) -> bool {
    if (core as usize) < MAX_CORES {
        self.reserved[core as usize]
    } else {
        false
    }
}
```

- 查询 `core` 是否被独占。
- **越界防御**：`core >= 8` 返回 `false`（不报错）。
- 用于调度策略层无锁查询（无需修改 reservation）。

### 4.4 can_enqueue

```rust
// sched/src/isolation.rs
pub fn can_enqueue(&self, core: u32, is_rtos: bool) -> bool {
    if (core as usize) >= MAX_CORES {
        return false;
    }
    if self.is_reserved(core) {
        is_rtos
    } else {
        true
    }
}
```

- 检查线程能否入队到 `core`。
- **核心规则**（见 §5 详解）：
  - 非 reserved 核：任何线程可入队（返回 `true`）。
  - reserved 核：仅 RTOS 线程可入队（`is_rtos=true` 返回 `true`）。
  - reserved 核 + 非 RTOS 线程：拒绝（返回 `false`）。
- **越界防御**：`core >= 8` 返回 `false`。

## 5. can_enqueue 规则详解

### 5.1 决策矩阵

| `core` 状态 | `is_rtos` | `can_enqueue` 返回 |
|-------------|-----------|---------------------|
| 非 reserved（free） | true | `true` |
| 非 reserved（free） | false | `true` |
| reserved（RTOS 独占） | true | `true` |
| reserved（RTOS 独占） | false | `false`（拒绝） |
| 越界（`core >= 8`） | 任意 | `false`（拒绝） |

### 5.2 决策流程图

```
            can_enqueue(core, is_rtos)
                      │
                      ▼
           ┌──────────────────────┐
           │ core >= MAX_CORES?  │
           └──────────────────────┘
              │ Yes          │ No
              ▼              ▼
           false     ┌──────────────────────┐
                     │ is_reserved(core)?   │
                     └──────────────────────┘
                        │ Yes          │ No
                        ▼              ▼
                     return       return true
                     is_rtos      (任何线程都可)
                     (仅 RTOS)
```

### 5.3 设计意图

- **RTOS 独占保证**：reserved 核的算力完全留给 RTOS 线程，Agent 线程无法挤入。
- **RTOS 灵活性**：非 reserved 核也接受 RTOS 线程（RTOS 不局限于 reserved 核，
  仅在 reserved 核上独占）。
- **越界安全**：所有越界返回 `false`（防御性编程，调度路径不 panic）。

### 5.4 v0.16.0 简化

`lib.rs::enqueue` 中 `is_rtos` 硬编码为 `false`：

```rust
// sched/src/lib.rs (D4 simplification)
pub fn enqueue(sched: &mut Scheduler, tid: Tid, core: u32) {
    if !sched.reservation.can_enqueue(core, false) {  // ← 硬编码 false
        return;
    }
    // ...
}
```

- 含义：v0.16.0 所有 `enqueue` 调用都被当作非 RTOS 线程处理。
- **后果**：reserved 核上无法通过 `enqueue` API 加入任何线程（即使 RTOS）。
- **缓解**：RTOS 线程的入队由调用方直接操作 `rqs[core].enqueue(tid)`（绕过
  `lib.rs::enqueue`），或等 v0.18.0 线程抽象补全 `is_rtos` 参数。
- **测试影响**：测试用例 `test_enqueue_rejected_on_reserved_core` 验证此行为
  （非 RTOS 线程被拒绝），`test_rtos_scenario_core0_reserved_agents_on_core1`
  验证 Agent 线程落到非 reserved 核。

## 6. 调度器集成

### 6.1 reserve_core

```rust
// sched/src/lib.rs
pub fn reserve_core(sched: &mut Scheduler, core: u32) -> Result<(), SchedError> {
    sched.reservation.reserve(core)
}
```

- 委托给 `CoreReservation::reserve`。
- 成功返回 `Ok(())`，失败返回 `Err(InvalidCore)` 或 `Err(CoreReserved)`。
- **注意**：v0.16.0 不自动同步 `rqs[core].reserved` 字段（该字段保留但 `enqueue`
  路径不读它，仅 `can_enqueue` 读 `reservation.reserved`）。

### 6.2 release_core

```rust
// sched/src/lib.rs
pub fn release_core(sched: &mut Scheduler, core: u32) {
    sched.reservation.release(core)
}
```

- 委托给 `CoreReservation::release`。
- 无返回值（失败静默忽略，与 `release` 的幂等语义一致）。

### 6.3 enqueue 集成

```rust
// sched/src/lib.rs
pub fn enqueue(sched: &mut Scheduler, tid: Tid, core: u32) {
    // Step 1: reservation 检查（can_enqueue）
    if !sched.reservation.can_enqueue(core, false) {
        return;   // 拒绝：reserved 核 + 非 RTOS 线程
    }
    // Step 2: 越界检查
    if core as usize >= MAX_CORES {
        return;
    }
    // Step 3: 实际入队（加锁）
    let rq = &mut sched.rqs[core as usize];
    rq.lock.lock();
    rq.enqueue(tid);
    rq.lock.unlock();
}
```

- **检查顺序**：reservation → 越界 → 入队。
- **失败策略**：所有失败静默返回（不 panic，符合 D4）。
- **未检查项**（v0.16.0 限制）：
  - 亲和性 `affinity[tid].contains(core)`（由调用方负责，见
    `docs/cpu-affinity-policy.md` §6.3）
  - RTOS 标志（硬编码 false，见 §5.4）
  - RQ 满载（`rq.enqueue` 内部静默丢弃，见调度器主文档 §4.3.1）

## 7. 典型使用场景

### 7.1 场景 1：启动时 reserve_core(0)，RTOS 线程加入 Core 0

```rust
use eneros_sched::*;

let mut sched = sched_init(4);
// 启动时把 Core 0 独占给 RTOS
assert_eq!(reserve_core(&mut sched, 0), Ok(()));
// RTOS 线程入队 Core 0（v0.16.0 需绕过 enqueue API，直接操作 RQ）
// 未来 v0.18.0：enqueue(&mut sched, Tid(1), 0)（is_rtos=true）
// v0.16.0 替代方案：
{
    let rq = &mut sched.rqs[0];
    rq.lock.lock();
    rq.enqueue(Tid(1));
    rq.lock.unlock();
}
// 验证：Core 0 有 1 个 RTOS 线程
assert_eq!(sched.rqs[0].load(), 1);
assert!(sched.reservation.is_reserved(0));
```

### 7.2 场景 2：Agent 线程加入 Core 1+（Core 0 reserved 时被拒绝）

```rust
use eneros_sched::*;

let mut sched = sched_init(4);
reserve_core(&mut sched, 0);
// Agent 线程试图上 Core 0 — 被拒绝
enqueue(&mut sched, Tid(100), 0);
enqueue(&mut sched, Tid(101), 0);
assert_eq!(sched.rqs[0].load(), 0);    // Core 0 仍空
// Agent 线程上 Core 1 — 成功
enqueue(&mut sched, Tid(100), 1);
enqueue(&mut sched, Tid(101), 1);
assert_eq!(sched.rqs[1].load(), 2);     // Core 1 有 2 个 Agent
// 调度：Core 0 留给 RTOS（pick_next 返回 None 因 RQ 空）
assert_eq!(pick_next(&mut sched, 0), None);
assert_eq!(pick_next(&mut sched, 1), Some(Tid(100)));
```

### 7.3 场景 3：运行时释放 Core 0 独占

```rust
use eneros_sched::*;

let mut sched = sched_init(4);
reserve_core(&mut sched, 0);
// 之前 Core 0 拒绝 Agent，现在释放
release_core(&mut sched, 0);
assert!(!sched.reservation.is_reserved(0));
// 现在 Agent 可以上 Core 0
enqueue(&mut sched, Tid(100), 0);
assert_eq!(sched.rqs[0].load(), 1);    // 成功入队
```

**注意**：释放后 Core 0 变成普通核，与 Core 1+ 无差别。若需再次独占，重新
`reserve_core(0)`，但若 Core 0 上已有 Agent 线程，它们不会被自动驱逐
（v0.16.0 限制，未来扩展可加「驱逐」语义）。

### 7.4 场景 4：多核独占（Core 0 + Core 1 给 RTOS）

```rust
use eneros_sched::*;

let mut sched = sched_init(8);
// 双 RTOS 核
assert_eq!(reserve_core(&mut sched, 0), Ok(()));
assert_eq!(reserve_core(&mut sched, 1), Ok(()));
// Agent 线程只能上 Core 2..7
for core in 0..8 {
    enqueue(&mut sched, Tid(100 + core), core);
}
assert_eq!(sched.rqs[0].load(), 0);    // 拒绝
assert_eq!(sched.rqs[1].load(), 0);    // 拒绝
assert_eq!(sched.rqs[2].load(), 1);    // 成功
// ...
```

## 8. SchedError 错误处理

### 8.1 错误枚举

```rust
// sched/src/isolation.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedError {
    InvalidCore,
    CoreReserved,
    NoRunnableTask,
}
```

| 变体 | 含义 | 触发场景 |
|------|------|----------|
| `InvalidCore` | 核索引越界（≥ `MAX_CORES` 或 ≥ `core_count`） | `reserve(core >= 8)` / `pin_to_core(core >= core_count)` |
| `CoreReserved` | 核已被独占 | 重复 `reserve(core)` |
| `NoRunnableTask` | 无可运行线程，或线程 ID 越界 | `set_affinity(tid.0 >= 256)` |

### 8.2 错误传播路径

```
reserve_core(sched, core)
    └─→ CoreReservation::reserve(core)
        ├─→ Err(InvalidCore)     if core >= 8
        ├─→ Err(CoreReserved)    if reserved[core] == true
        └─→ Ok(())               otherwise

set_affinity(sched, tid, cores)
    └─→ Err(NoRunnableTask)      if tid.0 >= 256

pin_to_core(sched, tid, core)
    ├─→ Err(InvalidCore)         if core >= core_count
    └─→ set_affinity(...)        otherwise（可能 Err(NoRunnableTask)）

enqueue(sched, tid, core)
    └─→ 静默丢弃（不返回 Result）
        ├─→ if !can_enqueue(core, false)  // reservation 拒绝
        └─→ if core >= MAX_CORES          // 越界
```

### 8.3 错误处理设计原则

1. **不 panic**：所有错误返回 `Result` 或静默忽略（D4：调度路径不 panic）。
2. **可恢复**：`Err(CoreReserved)` 调用方可先 `release` 再 `reserve`，或选择其它核。
3. **可调试**：`SchedError` 派生 `Debug`，可在日志中打印。
4. **零开销**：`SchedError` 是 `Copy`，无堆分配。

## 9. 与蓝图混合关键性架构关系

### 9.1 蓝图 §1 核心目标

蓝图 §1 描述 EnerOS 的核心目标之一是：

> 在同一 SoC 上同时承载 RTOS 实时控制与 Agent 智能决策，通过核独占保证 RTOS
> 不被 Agent 抢占。

| 蓝图要求 | 实现机制 |
|----------|----------|
| RTOS 独占 Core 0 | `reserve_core(sched, 0)` + `can_enqueue` 拒绝非 RTOS |
| Agent 分布 Core 1+ | `enqueue(sched, tid, 1..)` 自然分布 |
| 隔离机制 | `CoreReservation.reserved` 标记 + `can_enqueue` 准入 |

### 9.2 蓝图 §7.1 验收标准：RTOS 不被 Agent 抢占

蓝图 §7.1 要求：

> RTOS 线程在独占核上运行时，不被 Agent 线程抢占。

| 验收点 | 实现状态 |
|--------|----------|
| Agent 线程无法入队 reserved 核 | ✅ `can_enqueue(core, false) == false` |
| Agent 线程无法通过 `pick_next` 抢占 | ✅ RQ 为空（enqueue 被拒），`pick_next` 返回 `None` |
| Agent 线程无法通过 balance 迁移到 reserved 核 | ⚠️ v0.16.0 balance 不检查 reservation（见 §10） |
| 真实上下文切换不抢占 RTOS | ⏳ v0.18.0 线程抽象（当前无真实切换） |

### 9.3 蓝图 §7.2 验收标准：Agent 分布 Core 1+

蓝图 §7.2 要求：

> Agent 线程应分布在 Core 1+，不集中拥塞 Core 0。

| 验收点 | 实现状态 |
|--------|----------|
| Agent 入队 Core 0 被拒绝 | ✅ `can_enqueue(0, false) == false`（Core 0 reserved 时） |
| Agent 入队 Core 1+ 成功 | ✅ `can_enqueue(1, false) == true` |
| Agent 负载均衡到 Core 1+ | ✅ `Balancer` 在 `0..core_count` 内迁移（不限于 Core 1+） |
| Agent 不被自动驱逐到 Core 0 | ⚠️ balance 可能迁移到 reserved 核（v0.16.0 限制） |

## 10. 安全性考量

### 10.1 reserved 核的 RTOS 线程不可被抢占

- RTOS 线程独占 reserved 核，无其它线程可入队到该核。
- 因此 RTOS 线程在 reserved 核上**永不调度切换**（无 `pick_next` 候选）。
- 真实抢占需 v0.18.0 线程抽象实现上下文切换，当前 `pick_next` 仅返回 `Tid`
  而不切换。

### 10.2 负载均衡不迁移 reserved 核的线程（作为迁移源）

**v0.16.0 已知限制**：`Balancer::balance` 不检查 reservation：

```rust
// sched/src/balance.rs (简化)
pub fn balance(&self, rqs: &mut [PerCoreRq; MAX_CORES], core_count: u32) {
    // 扫描 busiest / idlest — 不跳过 reserved 核
    // 迁移 busiest → idlest — 不检查 idlest 是否 reserved
}
```

**风险**：

1. reserved 核上的 RTOS 线程可能被 balance 当作「busiest」迁出（违反独占语义）。
2. 非 RTOS 线程可能被 balance 迁移到 reserved 核（违反隔离）。

**v0.16.0 缓解**：

- RTOS 线程通常不通过 `enqueue` API 入队（绕过直接操作 RQ），balance 仍可能迁出。
- 测试 `test_balance_load_migrates_thread` 未覆盖 reserved 场景。
- 实际部署时，调用方应在 balance 后检查迁移目标是否 reserved，必要时回滚。

**未来修复**（v0.17.0+）：在 `balance` 中增加 reservation 检查：

```rust
// 未来 balance 改进（未实现）
if self.is_reserved(min_core) && !is_rtos(tid) {
    return;   // 不迁移到 reserved 核
}
```

### 10.3 reserved 核可作为负载均衡的目标（接收迁移线程）

- v0.16.0 当前允许 balance 迁移线程到 reserved 核（见 §10.2 风险）。
- 这是**未预期的行为**：reserved 核应仅接收 RTOS 线程。
- 紧急修复方案：在 `balance` 中跳过 reserved 核作为 `min_core` 候选。

### 10.4 reserved 标志的并发访问

- `reserved: [bool; 8]` 不是原子类型，但 v0.16.0 假设：
  - `reserve_core` / `release_core` 仅在启动阶段（单核运行）调用。
  - `can_enqueue` 在 `enqueue` 路径调用，可能多核并发读 `reserved[i]`。
- **风险**：若运行时多核同时 `reserve` 与 `can_enqueue`，存在数据竞争。
- **缓解**：v0.16.0 假设 reserve/release 是启动时一次性操作（单核），运行时
  只读 `reserved`（多核读 bool 是安全的，但 Rust 内存模型仍需 Sync）。
- **未来修复**：改用 `[AtomicBool; 8]`，与 v0.15.0 `CORE_STATES` 一致。

### 10.5 reservation 与 affinity 的协作

- affinity 控制线程**可运行**的核集合（软限制，由调用方检查）。
- reservation 控制 reserved 核**接受**的线程类型（硬限制，`enqueue` 内置检查）。
- 两者**正交**：affinity 不影响 reservation，反之亦然。
- 调用方应同时遵守：选核时检查 `affinity[tid].contains(core)`，enqueue 时
  `can_enqueue` 自动检查 reservation。

## 11. 未来扩展

### 11.1 多 RTOS 分区各自绑核

- 扩展 `CoreReservation` 为 `CoreReservation { partitions: [u8; 8] }`，每位记录
  该核属于哪个 RTOS 分区（0 = 无分区，1 = RTOS-A，2 = RTOS-B）。
- `can_enqueue(core, partition_id)` 检查线程的 partition 与核的 partition 是否匹配。
- 用于多 RTOS 共存（如继电保护 RTOS + 运动控制 RTOS 各占一核）。

### 11.2 动态 reserve/release（hotplug）

- 支持 `cpu_off`（来自 v0.15.0 PSCI）时自动 `release` 该核的 reservation。
- 支持 `cpu_on` 热添加时动态 `reserve` 新核。
- 需要原子化的 reservation 表（见 §10.4）。
- 与 IPI 配合：reserve/release 时广播 `IpiMsg::Reschedule` 通知所有核刷新本地
  缓存的 reservation 视图。

### 11.3 RTOS 优先级继承

- 当 RTOS 线程因等待 Agent 线程持有的锁而阻塞时，Agent 线程临时继承 RTOS 优先级。
- 防止优先级反转（priority inversion）。
- 需 v0.18.0 线程抽象 + 优先级字段 + 锁的优先级继承协议（如 PI mutex）。

### 11.4 reservation 与 balance 集成

- 在 `Balancer::balance` 中跳过 reserved 核作为 `min_core`（idlest）候选：
  ```rust
  // 未来 balance 修复（未实现）
  if !self.is_reserved(i) || is_rtos_context {
      // 考虑作为 idlest 候选
  }
  ```
- 避免 Agent 线程被迁移到 reserved 核。
- 紧急程度：高（见 §10.2 风险），应在 v0.17.0 修复。

### 11.5 RTOS 线程驱逐

- 当 `release_core` 释放某核独占时，自动驱逐该核上的非 RTOS 线程到其它核。
- 或反向：`reserve_core` 时若核上已有 Agent 线程，先迁移它们到其它核。
- 需 balance 配合：找到目标核 → 迁移 → 再 reserve。

### 11.6 reservation 持久化

- reservation 表在 kexec 重启时持久化到非易失存储，新内核启动时恢复。
- 用于高可用场景：主内核 crash 后备用内核快速恢复 reservation 配置。

### 11.7 与 IPI 配合的核间 reservation 同步

- `reserve_core` / `release_core` 后广播 `IpiMsg::Custom(RESERVE_UPDATE)` 通知
  所有核刷新本地缓存的 reservation 视图。
- 各核的 `PerCoreRq.reserved` 字段同步更新（v0.16.0 已有该字段但未同步）。
- 减少 `can_enqueue` 的跨核缓存争用（每核读本地 `rqs[core].reserved` 而非全局
  `reservation`）。

### 11.8 测试覆盖

`sched/src/isolation.rs` 内 10 个单元测试：

| 测试 | 验证点 |
|------|--------|
| `test_new_has_no_reservations` | `CoreReservation::new()` 全 8 核 `is_reserved == false` |
| `test_reserve_success` | `reserve(0)` 成功，`is_reserved(0) == true` |
| `test_reserve_already_reserved` | 重复 `reserve(2)` 返回 `Err(CoreReserved)` |
| `test_reserve_invalid_core` | `reserve(8)` / `reserve(MAX_CORES)` / `reserve(u32::MAX)` 均返回 `Err(InvalidCore)` |
| `test_release_clears_reservation` | `release(1)` 后 `is_reserved(1) == false`；重复 release 幂等 |
| `test_release_out_of_range_is_noop` | `release(8)` / `release(u32::MAX)` 静默忽略 |
| `test_can_enqueue_free_core_accepts_all` | 非 reserved 核接受 `is_rtos=true/false` 任意 |
| `test_can_enqueue_reserved_rejects_non_rtos` | reserved 核拒绝 `is_rtos=false`，接受 `is_rtos=true` |
| `test_can_enqueue_out_of_range_false` | `can_enqueue(8, _)` / `can_enqueue(u32::MAX, _)` 返回 `false` |
| `test_is_reserved_out_of_range_false` | `is_reserved(8)` / `is_reserved(u32::MAX)` 返回 `false` |

`lib.rs` 内 reservation 相关测试 4 个：

| 测试 | 验证点 |
|------|--------|
| `test_reserve_and_release_core` | `reserve_core(0)` 成功 → `is_reserved(0)` → `release_core(0)` 后 false |
| `test_reserve_core_out_of_range` | `reserve_core(8)` 返回 `Err(InvalidCore)` |
| `test_enqueue_rejected_on_reserved_core` | reserved 核上 enqueue 被拒绝，`load() == 0` |
| `test_rtos_scenario_core0_reserved_agents_on_core1` | 端到端：Core 0 reserved → Agent 落到 Core 1 |

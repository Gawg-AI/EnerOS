# EnerOS 多核调度器设计

> 版本：v0.16.0 | 日期：2026-07-12 | 状态：设计文档
> 蓝图依据：`phase0.md §v0.16.0`（多核调度与绑核）、`Power_Native_Agent_OS_Blueprint.md §4`（调度算法）、§6.3（性能要求）、§43.1（no_std 合规）、§43.2（瓶颈版本骨架可用）

## 1. 概述

EnerOS 多核调度器（crate 名 `eneros-sched`）是 Phase 0 P0-F 的核心交付物，为系统
提供 per-core 运行队列、核亲和性、RTOS 核独占与负载均衡能力。本文档描述调度器的整体
架构与全局 API；对应实现位于 `sched/src/` 目录，按职责拆分为 5 个模块：
`percore.rs` / `affinity.rs` / `isolation.rs` / `balance.rs` / `lib.rs`。

v0.16.0 多核调度器的目标与范围：

- **per-core 运行队列**：每个核拥有独立的固定容量（64）运行队列
  （[`percore::PerCoreRq`](../../crates/kernel/sched/src/percore.rs)），由自定义 TAS Spinlock 保护。
- **核亲和性**：通过 64 位掩码 [`affinity::CoreMask`](../../crates/kernel/sched/src/affinity.rs)
  限制线程可运行的核集合，详见 `docs/cpu-affinity-policy.md`。
- **RTOS 核独占**：通过 [`isolation::CoreReservation`](../../crates/kernel/sched/src/isolation.rs)
  将某核（典型为 Core 0）标记为 RTOS 专属，非 RTOS 线程被拒绝入队，详见
  `docs/rtos-core-pinning.md`。
- **负载均衡**：[`balance::Balancer`](../../crates/kernel/sched/src/balance.rs) 周期性扫描各核 RQ 负载，
  当最大与最小负载差超过阈值时迁移一个线程。
- **全局调度器**：[`Scheduler`](../../crates/kernel/sched/src/lib.rs) 结构体将上述组件整合，
  提供统一的 `enqueue / dequeue / pick_next / balance_load` 接口。

本版本**不**包含的能力（明确标注为「未来扩展」，见 §9）：

- 真正的线程抽象（TCB、上下文切换、内核栈）—— v0.18.0
- IPI 触发的核间重新调度通知 —— 与 `eneros-smp` 集成阶段
- 多核内存一致性（迁移时的 cache 同步） —— v0.17.0
- NUMA 感知的 per-node 运行队列 —— 真机多簇阶段

crate 顶层属性 `#![cfg_attr(not(test), no_std)]` 遵循蓝图 §43.1 全项目 no_std 要求；
`Cargo.toml` 在 `[dependencies]` 下**零依赖**（D2 决策），生产代码仅用 `core::*`。

## 2. 设计决策

### 2.1 D1：自定义 Spinlock 替代 spin::Mutex

| 维度 | `spin::Mutex` 方案 | 自定义 `Spinlock` 方案（采纳） |
|------|--------------------|-------------------------------|
| `const fn new` | `spin::Mutex::new` 非 `const fn`（v0.15.0 版本） | ✅ `const fn` |
| 数组初始化 | 无法 `[PerCoreRq; 8]` const 初始化 | ✅ 可 const 初始化 8 个槽 |
| 外部依赖 | 需引入 `spin` crate | 零依赖（D2） |
| CAS + backoff | 简单 TAS | `compare_exchange_weak` + 双层 backoff |
| Release/Acquire 序 | 由 `spin` 保证 | 显式 `Ordering::Acquire/Release` |

**结论**：在 `sched/src/percore.rs` 中实现自定义 `Spinlock`，`locked: AtomicBool` +
`compare_exchange_weak(false, true, Acquire, Relaxed)` 外层 TAS + 内层
`load(Relaxed) + spin_loop` backoff。`unlock` 用 `store(false, Release)` 发布保护写。

### 2.2 D2：零外部依赖，仅用 core::*

```toml
# sched/Cargo.toml
[dependencies]
# 空 — 零外部依赖
```

- 生产代码仅 `use core::sync::atomic::*` / `use core::hint::spin_loop`。
- **不**依赖 `alloc` / `spin` / `heapless`，运行队列用固定数组 `[Option<Tid>; 64]`。
- 测试代码（`#[cfg(test)]`）可用 `std::sync::Mutex` 串行化，不影响生产二进制。
- 与 v0.15.0 `eneros-smp`（依赖 `spin` + `heapless`）形成对比：sched 进一步去依赖，
  便于将来在更早期启动阶段（heap 未就绪时）链接使用。

### 2.3 D3：不依赖 eneros-smp，core_count 由参数传入

`sched` crate **不** `use eneros_smp`。`core_count` 通过 `sched_init(core_count)`
参数显式传入，由调用方（kernel 启动代码）从 `eneros-smp` 的 `core_count()` 取值后传入。

理由：

1. **依赖方向**：`sched` 是底层调度原语，反向依赖 `smp` 会让依赖图绕回。
2. **可独立测试**：sched 单元测试无需拉起 `eneros-smp` 全局状态。
3. **解耦 IPI**：迁移线程后是否发 `IpiMsg::Reschedule` 通知目标核，由调用方决定
   （`sched` 本身不触发 IPI，见 §9.3）。

### 2.4 D4：瓶颈版本合规（§43.2，骨架可用）

蓝图 §43.2 要求瓶颈版本（★ 标记，v0.16.0 是瓶颈版本）代码必须「骨架可用」：

- ❌ 禁止 `todo!()` / `unimplemented!()` / `unreachable!()` 占位
- ❌ 禁止伪代码签名（trait/struct 必须可编译）
- ✅ 关键算法必须完整实现：
  - CAS + 双层 backoff 的 Spinlock（`percore.rs`）
  - 负载扫描 + 阈值判定 + 单线程迁移（`balance.rs`）
  - reservation 检查 + 拒绝入队（`isolation.rs` + `lib.rs::enqueue`）
  - 位运算掩码操作（`affinity.rs`）

**唯一简化**：`lib.rs::enqueue` 的 `is_rtos` 参数硬编码为 `false`（注释 D4 simplification），
真正的 RTOS 标志需从 TCB 读取，待 v0.18.0 线程抽象接入后补全。此简化不影响骨架可用性：
调度路径（enqueue → pick_next → dequeue）端到端可运行。

## 3. 整体架构

### 3.1 模块组成

```
sched/
├── Cargo.toml          # 零依赖
└── src/
    ├── lib.rs           # Scheduler 结构体 + 全局 API + 测试
    ├── percore.rs       # Spinlock / Tid / PerCoreRq
    ├── affinity.rs      # CoreMask（64 位亲和性掩码）
    ├── isolation.rs      # CoreReservation / SchedError
    └── balance.rs       # Balancer（负载均衡器）
```

### 3.2 Scheduler 结构体

```rust
// sched/src/lib.rs
pub const MAX_CORES: usize = 8;
pub const MAX_THREADS: usize = 256;

#[derive(Debug)]
pub struct Scheduler {
    pub rqs: [PerCoreRq; MAX_CORES],         // 8 个 per-core RQ
    pub core_count: u32,                      // 活跃核数（≤ 8）
    pub reservation: CoreReservation,         // 核独占表
    pub balancer: Balancer,                   // 负载均衡器
    pub affinity: [CoreMask; MAX_THREADS],    // per-thread 亲和性表
}
```

| 字段 | 类型 | 文件 | 作用 |
|------|------|------|------|
| `rqs` | `[PerCoreRq; 8]` | `percore.rs` | 每核一个运行队列，索引 0..core_count 活跃 |
| `core_count` | `u32` | `lib.rs` | 活跃核数，`sched_init` 设置，上限 8 |
| `reservation` | `CoreReservation` | `isolation.rs` | 8 槽独占标记，控制 RTOS 核隔离 |
| `balancer` | `Balancer` | `balance.rs` | 阈值=2、interval=10ms 的负载均衡器 |
| `affinity` | `[CoreMask; 256]` | `affinity.rs` + `lib.rs` | 每线程一个 64 位掩码，索引由 `Tid.0` 决定 |

### 3.3 与 v0.15.0 smp crate 的关系

| 维度 | `eneros-smp`（v0.15.0） | `eneros-sched`（v0.16.0） |
|------|--------------------------|----------------------------|
| 职责 | 多核启动 + IPI 机制 | 调度策略 + 运行队列 + 均衡 |
| 数据 | `CORES` / `CORE_STATES` / `CORE_COUNT` | `Scheduler` 实例（非全局） |
| API 风格 | 全局 static + 函数 | 实例化 `Scheduler` + 函数取引用 |
| 核数来源 | `smp_init(core_count)` 写全局 | `sched_init(core_count)` 传参 |
| 依赖 | `spin` + `heapless` | 零依赖（D2） |
| 调用方向 | smp 先启动 → sched 后初始化 | sched 由 kernel 启动代码调用 |

**集成模式**（kernel 启动代码示意）：

```rust
// 伪代码 — kernel 顶层启动序列
smp_init(4);                              // v0.15.0：写 CORE_COUNT 等
for i in 1..4 {
    wake_secondary(i, secondary_entry);   // v0.15.0：唤醒 secondary
}
let mut sched = sched_init(4);            // v0.16.0：用 smp 的核数初始化调度器
reserve_core(&mut sched, 0);              // Core 0 → RTOS 独占
// ... 之后 secondary_entry 在 wfe 循环中被调度器接管（v0.17.0+）
```

## 4. per-core 运行队列

### 4.1 PerCoreRq 数据结构

```rust
// sched/src/percore.rs
pub const RQ_CAPACITY: usize = 64;

#[derive(Debug)]
pub struct PerCoreRq {
    pub core_id: u32,                          // 所属核 ID
    pub runnable: [Option<Tid>; RQ_CAPACITY],  // 64 槽可运行线程
    pub count: usize,                          // Some 条目数
    pub current: Option<Tid>,                   // 当前核上正在运行的线程
    pub reserved: bool,                         // 该核是否被 RTOS 独占
    pub lock: Spinlock,                         // 保护 runnable/count
}
```

| 字段 | 含义 | 备注 |
|------|------|------|
| `core_id` | 所属核 ID | `sched_init` 中按数组索引填充 0..7 |
| `runnable` | 64 槽 `Option<Tid>` 数组 | `None` = 空槽；`Some(tid)` = 可运行 |
| `count` | `Some` 条目数 | `enqueue` +1 / `dequeue` -1 / `remove` -1 |
| `current` | 当前运行线程 | `pick_next` 取出后写入；idle 时为 `None` |
| `reserved` | 是否 RTOS 独占 | 与 `reservation.reserved[i]` 镜像（便于无锁查询） |
| `lock` | TAS 自旋锁 | 保护 `runnable` / `count` 的并发修改 |

容量 `RQ_CAPACITY = 64` 的设计权衡：

- 足够吸收典型 Agent 工作负载（每核 64 个并发可运行线程）。
- 数组大小固定，避免堆分配，满足 no_std。
- 满载时 `enqueue` 静默丢弃（D4：调度路径不 panic），调用方可通过 `load()` 提前
  检测并触发负载均衡。

### 4.2 Spinlock 实现

```rust
// sched/src/percore.rs
use core::hint::spin_loop;
use core::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug)]
pub struct Spinlock {
    locked: AtomicBool,
}

impl Spinlock {
    pub const fn new() -> Self {
        Self { locked: AtomicBool::new(false) }
    }

    pub fn lock(&self) {
        // 外层：CAS TAS（Acquire 获取，Relaxed 失败）
        while self.locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            // 内层：load + spin_loop backoff，减少 cache line 抢占
            while self.locked.load(Ordering::Relaxed) {
                spin_loop();
            }
        }
    }

    pub fn unlock(&self) {
        // Release 序：发布保护写后再释放锁
        self.locked.store(false, Ordering::Release);
    }
}
```

**双层 backoff 设计**：

| 层 | 操作 | 目的 |
|----|------|------|
| 外层 | `compare_exchange_weak(false, true, Acquire, Relaxed)` | 真正的 TAS，仅一个核成功 |
| 内层 | `load(Relaxed) + spin_loop()` | 等待锁持有者释放，避免持续 CAS 抢 cache line |

`compare_exchange_weak` 选择（vs `compare_exchange`）：

-weak 允许 spurious failure（伪失败），但在 CAS 循环中可接受，性能更优（部分平台
可编译为更轻量的指令序列）。`Acquire` 序保证 lock 后的读写在锁持有期间不被重排到
锁之前；`Release` 序保证 unlock 前的写操作对下一个锁持有者可见。

### 4.3 PerCoreRq 操作

#### 4.3.1 enqueue

```rust
// sched/src/percore.rs
pub fn enqueue(&mut self, tid: Tid) {
    for slot in self.runnable.iter_mut() {
        if slot.is_none() {
            *slot = Some(tid);
            self.count += 1;
            return;
        }
    }
    // 队列满 — 静默丢弃（D4：调度路径不 panic）
}
```

- 找到第一个 `None` 槽位填入 `tid`。
- 满载时静默返回，调用方应提前 `load()` 检测。
- 入队顺序按槽位序，因此 dequeue 是 FIFO。

#### 4.3.2 dequeue

```rust
// sched/src/percore.rs
pub fn dequeue(&mut self) -> Option<Tid> {
    for slot in self.runnable.iter_mut() {
        if let Some(tid) = slot.take() {
            self.count -= 1;
            return Some(tid);
        }
    }
    None
}
```

- 取第一个 `Some` 槽位的 `tid`，置 `None`。
- 与 enqueue 配合形成 FIFO（按槽位序）。
- 空队列返回 `None`。

#### 4.3.3 remove

```rust
// sched/src/percore.rs
pub fn remove(&mut self, tid: Tid) -> bool {
    for slot in self.runnable.iter_mut() {
        if *slot == Some(tid) {
            *slot = None;
            self.count -= 1;
            return true;
        }
    }
    false
}
```

- 线性扫描 `runnable`，匹配 `tid` 后置 `None`。
- 不做数组紧凑（无 shifting），空槽由下次 `enqueue` 复用。
- 用于 `dequeue(sched, tid)` 全核搜索移除。

#### 4.3.4 load

```rust
// sched/src/percore.rs
pub fn load(&self) -> usize {
    self.count
}
```

返回 `count`（`Some` 条目数），是负载均衡器选 busiest/idlest 核的依据。

### 4.4 Tid

```rust
// sched/src/percore.rs
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Tid(pub u32);
```

- `Tid(pub u32)` newtype，`pub` 字段允许直接构造 `Tid(7)`。
- `Copy` 必需：使 `Option<Tid>` 是 `Copy`，进而 `[None; RQ_CAPACITY]` 可 const 初始化。
- `Default` → `Tid(0)`，用于默认占位（实际不调度 `Tid(0)` 之外的零值是调用方责任）。

## 5. 调度器初始化

### 5.1 sched_init 流程

```rust
// sched/src/lib.rs
pub fn sched_init(core_count: u32) -> Scheduler {
    // 1. cap 到 MAX_CORES
    let core_count = if core_count as usize > MAX_CORES {
        MAX_CORES as u32
    } else {
        core_count
    };
    // 2. 手列 8 个 PerCoreRq::new(i)（const fn 数组初始化）
    let mut rqs = [
        PerCoreRq::new(0), PerCoreRq::new(1), PerCoreRq::new(2), PerCoreRq::new(3),
        PerCoreRq::new(4), PerCoreRq::new(5), PerCoreRq::new(6), PerCoreRq::new(7),
    ];
    // 3. 按 core_count 设置 core_id（已在 new 中设过，此处幂等覆盖）
    for (i, rq) in rqs.iter_mut().enumerate().take(core_count as usize) {
        rq.core_id = i as u32;
    }
    // 4. 组装 Scheduler，使用默认 balancer/reservation/affinity
    Scheduler {
        rqs,
        core_count,
        reservation: CoreReservation::new(),         // 全 false
        balancer: Balancer::default(),                // threshold=2, interval_ms=10
        affinity: [CoreMask::default(); MAX_THREADS], // 256 个空掩码
    }
}
```

### 5.2 初始化后的默认状态

| 字段 | 默认值 | 含义 |
|------|--------|------|
| `rqs[i].core_id` | `i`（0..core_count） | 每核 RQ 的归属 |
| `rqs[i].count` | 0 | 空队列 |
| `rqs[i].current` | `None` | 核空闲 |
| `rqs[i].reserved` | `false` | 未独占（与 `reservation` 同步） |
| `core_count` | 入参（capped 8） | 活跃核数 |
| `reservation.reserved[i]` | `false` | 无核独占 |
| `balancer.threshold` | 2 | 迁移阈值 |
| `balancer.interval_ms` | 10 | 期望均衡周期 |
| `affinity[i]` | `CoreMask(0)`（空） | 无亲和性限制 |

### 5.3 默认 balancer / reservation / affinity

- **balancer**：`Balancer::default()` → `threshold=2, interval_ms=10`（蓝图 §4.5 默认）。
  - 含义：当两核负载差 > 2 时迁移一个线程；期望每 10ms 触发一次均衡 pass。
  - `interval_ms` 仅是建议值，`sched` 本身不挂定时器，由调用方周期性调用
    `balance_load`。
- **reservation**：`CoreReservation::new()` → 全 `false`，无核独占。典型调用方在
  `sched_init` 后立即 `reserve_core(&mut sched, 0)` 把 Core 0 标记为 RTOS 独占。
- **affinity**：`[CoreMask::default(); 256]` → 256 个空掩码。
  - 空掩码 = 无限制（线程可在任何核运行，受 reservation 二次过滤）。

## 6. 调度接口

### 6.1 pick_next

```rust
// sched/src/lib.rs
pub fn pick_next(sched: &mut Scheduler, core: u32) -> Option<Tid> {
    if core as usize >= MAX_CORES {
        return None;
    }
    let rq = &mut sched.rqs[core as usize];
    rq.lock.lock();
    let tid = rq.dequeue();
    rq.lock.unlock();
    if let Some(t) = tid {
        rq.current = Some(t);   // 记录当前运行线程
    }
    tid
}
```

- 从 `core` 的 RQ 取出下一个可运行线程。
- 在锁内 `dequeue`，锁外写 `current`（`current` 不被并发修改，无需锁保护）。
- 队列空或 `core >= 8` 返回 `None`。
- 蓝图 §6.3 要求 `pick_next < 1μs`（见 §8 性能考量）。

### 6.2 enqueue

```rust
// sched/src/lib.rs
pub fn enqueue(sched: &mut Scheduler, tid: Tid, core: u32) {
    // D4 简化：is_rtos 硬编码 false（真实实现从 TCB 读 RTOS 标志）
    if !sched.reservation.can_enqueue(core, false) {
        return;   // reserved 核拒绝非 RTOS 线程，静默丢弃
    }
    if core as usize >= MAX_CORES {
        return;   // 越界静默丢弃（防御）
    }
    let rq = &mut sched.rqs[core as usize];
    rq.lock.lock();
    rq.enqueue(tid);
    rq.lock.unlock();
}
```

- **reservation 检查**：`can_enqueue(core, is_rtos=false)` 在 reserved 核上返回 `false`，
  非RTOS 线程被拒绝入队（详见 `docs/rtos-core-pinning.md`）。
- **越界防御**：`core >= 8` 静默丢弃（不 panic，符合 D4）。
- **affinity 检查**：v0.16.0 的 `enqueue` **不**主动检查 `affinity` 表 —— 调用方
  责任。`affinity` 表的写入由 `set_affinity` / `pin_to_core` 完成，调度策略层
  （未来 v0.18.0 thread spawn）应读 `affinity[tid]` 决定 `core` 参数。
- 满载时 `rq.enqueue` 内部静默丢弃（见 §4.3.1）。

### 6.3 dequeue

```rust
// sched/src/lib.rs
pub fn dequeue(sched: &mut Scheduler, tid: Tid) {
    for rq in sched.rqs.iter_mut().take(sched.core_count as usize) {
        rq.lock.lock();
        let removed = rq.remove(tid);
        rq.lock.unlock();
        if removed {
            return;   // 找到并移除，提前退出
        }
    }
    // 未找到 — 无操作
}
```

- 遍历 `0..core_count` 各核 RQ，第一个匹配的 `tid` 被移除。
- 每核单独加锁（不一次性持多锁），减少锁竞争。
- 未找到 `tid` 是无操作（不报错）。

### 6.4 balance_load

```rust
// sched/src/lib.rs
pub fn balance_load(sched: &mut Scheduler) {
    let core_count = sched.core_count;
    sched.balancer.balance(&mut sched.rqs, core_count);
}
```

- 委托给 `Balancer::balance`（见 `sched/src/balance.rs`）。
- 单次 pass 仅迁移最多一个线程（从 busiest 到 idlest）。
- 调用方应周期性调用（典型 10ms 周期，由定时器中断触发）。

#### 6.4.1 balance 算法

```rust
// sched/src/balance.rs
pub fn balance(&self, rqs: &mut [PerCoreRq; MAX_CORES], core_count: u32) {
    if core_count < 2 { return; }   // 单核无均衡意义
    // Step 1: 扫描 busiest / idlest
    let (max_load, min_load, max_core, min_core) = /* scan rqs[0..core_count] */;
    // Step 2: 检查阈值与核区分
    if max_core == min_core { return; }
    let diff = max_load.checked_sub(min_load).unwrap_or(0);
    if diff <= self.threshold { return; }
    // Step 3: 从 busiest 出队一个线程
    let tid = {
        rqs[max_core].lock.lock();
        let t = rqs[max_core].dequeue();
        rqs[max_core].lock.unlock();
        t
    };
    // Step 4: 入队到 idlest
    if let Some(tid) = tid {
        rqs[min_core].lock.lock();
        rqs[min_core].enqueue(tid);
        rqs[min_core].lock.unlock();
    }
    // Step 5: 失败容忍（dequeue 返回 None 时不 panic）
}
```

**关键设计点**：

1. **每 pass 仅迁移一个线程**：避免长临界区，平衡收敛由调用方多次触发完成。
2. **严格大于阈值**：`diff > threshold` 才迁移（`diff <= threshold` 不动）。
   - 例如 threshold=2，diff=2 不迁移；diff=3 迁移。
3. **失败容忍**：源 RQ 在 lock 期间被并发掏空（其它核 pick_next）时 `dequeue` 返回
   `None`，balance pass 提前返回，不 panic（D4）。
4. **不跨 reservation**：v0.16.0 balance 不检查 reservation，迁移目标可能是
   reserved 核（见 `docs/rtos-core-pinning.md` §10 安全性考量）。

## 7. 与蓝图对齐

### 7.1 蓝图 §4.1 数据结构对照

| 蓝图数据结构 | 实现 | 文件 |
|--------------|------|------|
| per-core 运行队列 | `PerCoreRq` | `sched/src/percore.rs` |
| 队列锁 | `Spinlock`（自定义 TAS） | `sched/src/percore.rs` |
| 线程 ID | `Tid(u32)` | `sched/src/percore.rs` |
| 核亲和性掩码 | `CoreMask(u64)` | `sched/src/affinity.rs` |
| 核独占表 | `CoreReservation { reserved: [bool; 8] }` | `sched/src/isolation.rs` |
| 负载均衡器 | `Balancer { threshold, interval_ms }` | `sched/src/balance.rs` |
| 调度器主体 | `Scheduler` | `sched/src/lib.rs` |

### 7.2 蓝图 §4.2 接口对照

| 蓝图接口 | 实现 | 文件 |
|----------|------|------|
| 调度器初始化 | `sched_init(core_count) -> Scheduler` | `sched/src/lib.rs` |
| 加入指定核 RQ | `enqueue(sched, tid, core)` | `sched/src/lib.rs` |
| 从所有 RQ 移除 | `dequeue(sched, tid)` | `sched/src/lib.rs` |
| 取下一个线程 | `pick_next(sched, core) -> Option<Tid>` | `sched/src/lib.rs` |
| 触发负载均衡 | `balance_load(sched)` | `sched/src/lib.rs` |
| 设置亲和性 | `set_affinity(sched, tid, cores)` | `sched/src/lib.rs` |
| 绑定单核 | `pin_to_core(sched, tid, core)` | `sched/src/lib.rs` |
| reserve 核 | `reserve_core(sched, core)` | `sched/src/lib.rs` |
| release 核 | `release_core(sched, core)` | `sched/src/lib.rs` |

### 7.3 蓝图 §4.3 算法流程对照

| 蓝图算法 | 实现要点 |
|----------|----------|
| pick_next 路径 | lock → dequeue → unlock → set current |
| enqueue 路径 | reservation 检查 → 越界检查 → lock → enqueue → unlock |
| balance 路径 | 扫描 busiest/idlest → 阈值判定 → 出队 busiest → 入队 idlest |
| FIFO 调度 | 按 `runnable` 槽位序 enqueue/dequeue |
| 迁移阈值 | `diff > threshold`（严格大于） |

## 8. 性能考量

### 8.1 蓝图 §6.3 要求

| 指标 | 蓝图要求 | 实现路径 |
|------|----------|----------|
| `pick_next` 延迟 | < 1μs | lock + 线性扫描 dequeue + unlock |
| 均衡周期 | 10ms | `balancer.interval_ms = 10`（调用方按此周期调 `balance_load`） |

### 8.2 host 无法测

- host（x86_64）构建下 `Spinlock` 用 `core::sync::atomic` 真实 CAS，但无真核间竞争。
- 真机 `pick_next < 1μs` 的测量需在 QEMU virt 或真机 aarch64 上用 `cntvct_el0`
  计数器测量。
- 留待 v0.16.0+ QEMU 验证阶段补入基准数据。

### 8.3 影响因素

| 因素 | 影响 | 缓解 |
|------|------|------|
| Spinlock 竞争 | 多核同时 enqueue 同一核 RQ 时串行化 | 双层 backoff 减少总线争用 |
| RQ 容量（64） | 满载时 enqueue 丢线程 | 调用方提前 `load()` 检测并触发均衡 |
| 线性扫描 dequeue | O(RQ_CAPACITY) = O(64) | 固定上界，无动态分配 |
| balance 扫描 | O(core_count) ≤ O(8) | 单次 pass 仅迁移 1 个 |
| 迁移后无 IPI | 目标核不知有新线程到达 | v0.17.0+ 集成 `IpiMsg::Reschedule` |

### 8.4 优化空间（未来）

- per-core 多 RQ 优先级分层（高/低优先级分离）
- work-stealing 替代 centralized balancer（每核 idle 时主动偷）
- 多核 cache-aware 放置（迁移时优先选最近 cache 域）

## 9. 未来扩展

### 9.1 v0.17.0：多核内存一致性

- 线程从核 A 迁移到核 B 时，TCB 中缓存的栈/寄存器需 cache 同步。
- 在 balance 迁移路径插入 `dc cvau` / `dsb ish` 等 cache 清理指令。
- 与 MMU 子系统的 ASID 切换配合（线程切换时刷 TLB 或换 ASID）。

### 9.2 v0.18.0：线程抽象

- 引入 `Thread` / `Tcb` 结构体，封装栈指针、上下文、优先级、RTOS 标志。
- `Tid` 作为 TCB 表索引，`affinity[tid.0]` 直接索引亲和性表。
- `enqueue` 的 `is_rtos` 参数从 TCB 读，而非硬编码 `false`。
- 真正的上下文切换（`context_switch` 汇编）取代当前 `pick_next` 仅返回 `Tid`。

### 9.3 IPI 集成

- 调用方在 `balance_load` 后，对迁移的目标核调用
  `ipi_send(target_core, IpiMsg::Reschedule)`（来自 v0.15.0 `eneros-smp`）。
- 目标核 IRQ handler 触发 `pick_next` 切换到迁移来的线程。
- `sched` 本身不触 IPI（D3 解耦），由调用方决定。

### 9.4 NUMA 支持

- 多簇系统（如飞腾 D2000 双簇 8 核）引入 per-node RQ。
- `CoreMask` 扩展为 `(node_id, core_mask)` 二元组。
- balance 优先在 node 内迁移，跨 node 迁移作为最后手段。

### 9.5 优先级与实时调度

- 在 `PerCoreRq` 内引入多优先级队列（如 `runnable: [[Option<Tid>; 64]; 8]` 8 个优先级）。
- `pick_next` 从最高优先级非空队列取。
- RTOS 线程固定最高优先级，与 reservation 配合保证硬实时。

## 10. 全局 API

`sched/src/lib.rs` 通过 `pub use` re-export 以下类型到 crate 根：

| API / 类型 | 作用 | 文件位置 |
|-----------|------|----------|
| `Scheduler` | 全局调度器结构体 | `sched/src/lib.rs` |
| `MAX_CORES` / `MAX_THREADS` | 容量常量（8 / 256） | `sched/src/lib.rs` |
| `sched_init(core_count) -> Scheduler` | 初始化调度器 | `sched/src/lib.rs` |
| `set_affinity(sched, tid, cores) -> Result` | 设置亲和性 | `sched/src/lib.rs` |
| `pin_to_core(sched, tid, core) -> Result` | 绑定单核 | `sched/src/lib.rs` |
| `reserve_core(sched, core) -> Result` | 标记核独占 | `sched/src/lib.rs` |
| `release_core(sched, core)` | 释放核独占 | `sched/src/lib.rs` |
| `enqueue(sched, tid, core)` | 加入指定核 RQ | `sched/src/lib.rs` |
| `dequeue(sched, tid)` | 从所有 RQ 移除线程 | `sched/src/lib.rs` |
| `pick_next(sched, core) -> Option<Tid>` | 取下一个线程 | `sched/src/lib.rs` |
| `balance_load(sched)` | 触发负载均衡 | `sched/src/lib.rs` |
| `Spinlock` | TAS 自旋锁 | `sched/src/percore.rs` |
| `Tid(u32)` | 线程 ID | `sched/src/percore.rs` |
| `PerCoreRq` | per-core 运行队列 | `sched/src/percore.rs` |
| `RQ_CAPACITY` | RQ 容量常量（= 64） | `sched/src/percore.rs` |
| `CoreMask` | 64 位亲和性掩码 | `sched/src/affinity.rs` |
| `Balancer` | 负载均衡器 | `sched/src/balance.rs` |
| `CoreReservation` | 核独占表 | `sched/src/isolation.rs` |
| `SchedError` | 错误枚举 | `sched/src/isolation.rs` |

## 11. 测试覆盖

`sched/src/lib.rs` 内 12 个单元测试，均用 `std::sync::Mutex` 串行化避免共享状态竞争：

| 测试 | 验证点 |
|------|--------|
| `test_sched_init_defaults` | `sched_init(4)` 后 `core_count==4`、8 个 RQ 空、balancer 默认、affinity 全空 |
| `test_sched_init_caps_excess_cores` | `sched_init(99)` 把 `core_count` 截到 `MAX_CORES` (8) |
| `test_reserve_and_release_core` | `reserve_core(0)` 成功 → `is_reserved(0)==true`；`release_core(0)` 后 false |
| `test_reserve_core_out_of_range` | `reserve_core(8)` 返回 `Err(InvalidCore)` |
| `test_enqueue_pick_next_basic` | 入队 2 个 → pick_next 顺序取出 → 第 3 次返回 `None` |
| `test_enqueue_rejected_on_reserved_core` | reserved 核上 enqueue 被静默拒绝，RQ 仍空 |
| `test_enqueue_accepted_on_non_reserved_core` | 非 reserved 核接受 enqueue，`load()==1` |
| `test_set_affinity_valid` | `set_affinity(Tid(5), CoreMask::all(4))` 写入后 `affinity[5]` 正确 |
| `test_set_affinity_out_of_range` | `Tid(256)` 返回 `Err(NoRunnableTask)` |
| `test_pin_to_core_valid` | `pin_to_core(Tid(7), 3)` 后 `affinity[7]` 仅含 bit 3 |
| `test_pin_to_core_invalid_core` | `pin_to_core(Tid(7), 4)` 与 `pin_to_core(Tid(7), 99)` 均返回 `Err(InvalidCore)` |
| `test_dequeue_removes_from_specific_core` | 多核入队后 `dequeue(tid)` 仅移除目标 |
| `test_dequeue_absent_is_noop` | `dequeue(Tid(99))` 在不存在时不报错 |
| `test_balance_load_migrates_thread` | Core 0:4 / Core 1:0 → balance 后 Core 0:3 / Core 1:1 |
| `test_rtos_scenario_core0_reserved_agents_on_core1` | 端到端：RTOS 独占 Core 0，agents 落到 Core 1 |

per-core / affinity / isolation / balance 模块各自另有单元测试，详见各模块文档
（`docs/cpu-affinity-policy.md` §9、`docs/rtos-core-pinning.md` §8）。

## 12. 蓝图符合性

对照 `phase0.md §v0.16.0` 与蓝图 §4：

| 蓝图条目 | 实现状态 |
|----------|----------|
| per-core 运行队列 | ✅ `PerCoreRq` 固定容量 64 槽数组 |
| 队列并发保护 | ✅ 自定义 `Spinlock`（CAS + 双层 backoff） |
| 核亲和性（CoreMask） | ✅ `CoreMask(u64)` + 8 个位运算操作 |
| 核独占（CoreReservation） | ✅ `reserved: [bool; 8]` + `can_enqueue` 规则 |
| 负载均衡器 | ✅ `Balancer` 阈值判定 + 单线程迁移 |
| FIFO 调度 | ✅ `enqueue/dequeue` 按槽位序 |
| `pick_next` 接口 | ✅ `pick_next(sched, core) -> Option<Tid>` |
| `balance_load` 接口 | ✅ 委托 `Balancer::balance` |
| `reserve_core` / `release_core` | ✅ 委托 `CoreReservation` |
| `set_affinity` / `pin_to_core` | ✅ 写 `affinity[tid.0]` |
| no_std 合规（蓝图 §43.1） | ✅ `#![cfg_attr(not(test), no_std)]`，零依赖（D2） |
| 瓶颈版本骨架可用（蓝图 §43.2） | ✅ 关键算法完整实现，无 `todo!()`/`unimplemented!()` |
| `pick_next` < 1μs（蓝图 §6.3） | ⏳ host 无法测，留待 QEMU 阶段（见 §8.2） |
| 均衡周期 10ms（蓝图 §6.3） | ✅ `balancer.interval_ms = 10`（调用方按此周期触发） |
| 真实线程抽象（TCB/上下文切换） | ⏳ v0.18.0 线程抽象版本 |
| IPI 集成（Reschedule） | ⏳ 与 `eneros-smp` 集成阶段（见 §9.3） |
| 多核内存一致性 | ⏳ v0.17.0（见 §9.1） |
| RTOS 标志从 TCB 读 | ⏳ v0.18.0（D4 简化为硬编码 false） |

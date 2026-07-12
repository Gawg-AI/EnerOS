# EnerOS CPU 亲和性策略设计

> 版本：v0.16.0 | 日期：2026-07-12 | 状态：设计文档
> 蓝图依据：`phase0.md §v0.16.0`（多核调度与绑核）、`Power_Native_Agent_OS_Blueprint.md §4.1`（CoreMask 数据结构）、§43.1（no_std 合规）

## 1. 概述

CPU 亲和性（CPU affinity）是 EnerOS 多核调度器（crate `eneros-sched`）的核心机制之一，
用于**限制线程可运行的核集合**。通过 64 位位掩码 [`CoreMask`](../../crates/kernel/sched/src/affinity.rs)
表示线程与核的绑定关系，调度策略层在 spawn/migrate 时根据掩码决定目标核。

本文档对应实现位于 `sched/src/affinity.rs`，与调度器主文档
（`docs/multi-core-scheduler-design.md`）、RTOS 核独占文档
（`docs/rtos-core-pinning.md`）互为补充。

v0.16.0 亲和性机制的目标与范围：

- **64 位核掩码**：用 `u64` 表示最多 64 个核的亲和性（位 i 置 1 = 线程可在核 i 运行）。
- **位运算操作**：`single` / `all` / `contains` / `add` / `remove` / `count` /
  `is_empty` / `intersects` 共 8 个原生操作。
- **per-thread 亲和性表**：`Scheduler.affinity: [CoreMask; 256]`，每线程一个掩码。
- **API**：`set_affinity(tid, cores)` 设置任意掩码；`pin_to_core(tid, core)` 绑定单核。

本版本**不**包含的能力（明确标注为「未来扩展」，见 §10）：

- 动态亲和性调整（运行时根据负载自动收紧/放宽掩码）
- NUMA 感知亲和性（区分 node 内 vs 跨 node 绑定）
- `enqueue` 路径的亲和性自动检查（v0.16.0 由调用方负责选核）

crate 顶层属性 `#![cfg_attr(not(test), no_std)]` 遵循蓝图 §43.1；
`affinity.rs` 仅依赖 `core::*`（D2 决策）。

## 2. CoreMask 设计

### 2.1 数据结构

```rust
// sched/src/affinity.rs
pub const MAX_CORES: u32 = 64;

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct CoreMask(pub u64);
```

| 属性 | 实现 | 含义 |
|------|------|------|
| `pub u64` | newtype 内字段公开 | 允许 `CoreMask(0xFFFF)` 直接构造 |
| `Clone` / `Copy` | 派生 | 64 位值类型，按值传递无开销 |
| `Default` | 派生 → `CoreMask(0)` | 空掩码 = 无限制（见 §6） |
| `Debug` | 派生 | 二进制调试输出 |
| `PartialEq` / `Eq` | 派生 | 掩码相等比较 |

### 2.2 位编码

```
位 63  62  61  ...  3   2   1   0
    ┌───┬───┬───┬───┬───┬───┬───┬───┐
    │   │   │   │...│ ■ │   │ ■ │   │
    └───┴───┴───┴───┴───┴───┴───┴───┘
                    │           │
                    │           └─ bit 1 = 1：可运行于核 1
                    └─ bit 3 = 1：可运行于核 3
```

- 位 `i` 置 1 → 线程可运行于核 `i`。
- 位 `i` 置 0 → 线程不可运行于核 `i`。
- 全 0（`CoreMask(0)` = `CoreMask::default()`）= **无限制**，可运行于任何核
  （这是约定，由调度策略层在选核时检查 `is_empty()` 短路放行，见 §6）。

### 2.3 容量上限

`MAX_CORES = 64`：单个 `u64` 恰好覆盖 64 个核。这与 `Scheduler.MAX_CORES = 8`
（实际活跃核数上限）不冲突 —— CoreMask 是**位空间**上限（64），实际核数受
`Scheduler.core_count`（≤ 8）限制。CoreMask 设计为 64 位是为了：

1. **未来扩展**：飞腾 D2000 等多簇 SoC 可达 64 核，无需重构数据结构。
2. **零开销**：`u64` 在 aarch64 上是原生寄存器宽度，位运算一条指令完成。
3. **对齐**：与 Linux `cpumask_t`（也是 64 位）语义一致，便于移植。

## 3. CoreMask 操作详解

### 3.1 single

```rust
// sched/src/affinity.rs
pub fn single(core: u32) -> Self {
    debug_assert!(core < MAX_CORES, "core index out of range: {core}");
    Self(1u64 << core)
}
```

- 创建一个仅包含 `core` 的单核掩码。
- **debug 断言**：`core >= 64` 在 debug 构建下 panic；release 构建下 `1u64 << core`
  会按 Rust `<<` 语义 wrap（`1 << 64` 在 release 下 UB 但实际表现为 0 或 wrap）。
- 用于 `pin_to_core` 实现。

| 入参 | 输出 |
|------|------|
| `single(0)` | `CoreMask(0b0001)` |
| `single(3)` | `CoreMask(0b1000)` |
| `single(63)` | `CoreMask(1u64 << 63)` |
| `single(64)` | debug panic / release UB（应避免） |

### 3.2 all

```rust
// sched/src/affinity.rs
pub fn all(count: u32) -> Self {
    if count >= MAX_CORES {
        Self(u64::MAX)
    } else {
        Self((1u64 << count) - 1)
    }
}
```

- 创建一个包含核 `0..count` 的全核掩码。
- **count=64 边界处理**：直接返回 `u64::MAX`，避免 `1u64 << 64` 的 UB
  （这是关键防御，见 §7.1）。
- `count=0` 返回 `CoreMask(0)`（空掩码）。

| 入参 | 输出 | 二进制（低 8 位） |
|------|------|-------------------|
| `all(0)` | `CoreMask(0)` | `0b0000_0000` |
| `all(1)` | `CoreMask(1)` | `0b0000_0001` |
| `all(4)` | `CoreMask(0b1111)` | `0b0000_1111` |
| `all(8)` | `CoreMask(0xFF)` | `0b1111_1111` |
| `all(64)` | `CoreMask(u64::MAX)` | 全 1（64 位） |

### 3.3 contains

```rust
// sched/src/affinity.rs
pub fn contains(&self, core: u32) -> bool {
    if core >= MAX_CORES {
        return false;
    }
    (self.0 >> core) & 1 == 1
}
```

- 测试 `core` 是否在掩码中（位 `core` 是否为 1）。
- **越界防御**：`core >= 64` 返回 `false`（不 panic）。
- 用于调度策略层判断「线程能否运行于核 X」。

### 3.4 add / remove

```rust
// sched/src/affinity.rs
pub fn add(&mut self, core: u32) {
    if core < MAX_CORES {
        self.0 |= 1u64 << core;
    }
}

pub fn remove(&mut self, core: u32) {
    if core < MAX_CORES {
        self.0 &= !(1u64 << core);
    }
}
```

- `add(core)`：置位 `core`（加入掩码）。
- `remove(core)`：清位 `core`（从掩码移除）。
- **越界防御**：`core >= 64` 静默忽略（不修改掩码）。
- 用于增量调整亲和性。

### 3.5 count

```rust
// sched/src/affinity.rs
pub fn count(&self) -> u32 {
    self.0.count_ones()
}
```

- 统计掩码中置位的核数。
- 用 `u64::count_ones`（aarch64 `cnt` 指令），单周期完成。
- 用于调度策略层判断「线程可运行核数」。

### 3.6 is_empty

```rust
// sched/src/affinity.rs
pub fn is_empty(&self) -> bool {
    self.0 == 0
}
```

- 是否空掩码（无任何核）。
- 空掩码语义 = **无限制**（见 §6.1），调度策略层据此短路放行。
- 用于 `sched_init` 后验证所有 `affinity[i].is_empty()`。

### 3.7 intersects

```rust
// sched/src/affinity.rs
pub fn intersects(&self, other: CoreMask) -> bool {
    (self.0 & other.0) != 0
}
```

- 测试两个掩码是否有交集（任一位同时为 1）。
- 用于判断两个线程是否可共享某核（例如检测亲和性冲突）。

### 3.8 bits

```rust
// sched/src/affinity.rs
pub fn bits(&self) -> u64 {
    self.0
}
```

- 取原始 `u64`，用于序列化或日志。

## 4. 亲和性 API

亲和性 API 位于 `sched/src/lib.rs`，操作 `Scheduler.affinity` 表。

### 4.1 set_affinity

```rust
// sched/src/lib.rs
pub fn set_affinity(sched: &mut Scheduler, tid: Tid, cores: CoreMask) -> Result<(), SchedError> {
    if tid.0 as usize >= MAX_THREADS {
        return Err(SchedError::NoRunnableTask);
    }
    sched.affinity[tid.0 as usize] = cores;
    Ok(())
}
```

- 设置 `tid` 的亲和性为 `cores`。
- **越界检查**：`tid.0 >= 256` 返回 `Err(NoRunnableTask)`。
- 直接覆盖原掩码（非合并），调用方负责组合（如 `mask.add(3)` 后再 set）。

| 入参 | 结果 |
|------|------|
| `set_affinity(sched, Tid(5), CoreMask::single(3))` | `Ok(())`，`affinity[5] = single(3)` |
| `set_affinity(sched, Tid(5), CoreMask::all(4))` | `Ok(())`，`affinity[5] = all(4)` |
| `set_affinity(sched, Tid(256), ...)` | `Err(NoRunnableTask)` |
| `set_affinity(sched, Tid(u32::MAX), ...)` | `Err(NoRunnableTask)` |

### 4.2 pin_to_core

```rust
// sched/src/lib.rs
pub fn pin_to_core(sched: &mut Scheduler, tid: Tid, core: u32) -> Result<(), SchedError> {
    if core >= sched.core_count {
        return Err(SchedError::InvalidCore);
    }
    set_affinity(sched, tid, CoreMask::single(core))
}
```

- 把 `tid` 绑定到单核 `core`（等价 `set_affinity(tid, CoreMask::single(core))`）。
- **额外的核数检查**：`core >= sched.core_count` 返回 `Err(InvalidCore)`，
  防止绑定到不存在的核。
- 是 `set_affinity` 的语义糖，常用于 RTOS 线程硬绑定。

| 入参 | 结果 |
|------|------|
| `pin_to_core(sched, Tid(7), 3)`（core_count=4） | `Ok(())`，`affinity[7] = single(3)` |
| `pin_to_core(sched, Tid(7), 4)`（core_count=4） | `Err(InvalidCore)` |
| `pin_to_core(sched, Tid(7), 99)` | `Err(InvalidCore)` |

## 5. 亲和性表

### 5.1 数据结构

```rust
// sched/src/lib.rs
pub const MAX_THREADS: usize = 256;

pub struct Scheduler {
    // ...
    pub affinity: [CoreMask; MAX_THREADS],   // 256 个掩码
    // ...
}
```

- 每线程一个 `CoreMask`，索引由 `Tid.0` 决定。
- 固定 256 槽，无动态分配（D2 no_std 合规）。
- 总大小 256 × 8 字节 = 2 KB，编译期常量初始化为全 0。

### 5.2 索引规则

```
Tid(0)   → affinity[0]
Tid(5)   → affinity[5]
Tid(255) → affinity[255]
Tid(256) → Err(NoRunnableTask)（越界）
```

- `Tid.0` 直接作为数组索引，O(1) 访问。
- `Tid(0)` 是合法索引，但调度策略应避免使用 `Tid(0)` 作为真实线程（约定 0 =
  idle/无效，由调用方保证）。

### 5.3 默认值

```rust
// sched/src/lib.rs
affinity: [CoreMask::default(); MAX_THREADS],
```

- `sched_init` 后所有 256 个掩码均为 `CoreMask(0)`（空）。
- 空掩码 = 无限制（见 §6.1），即所有线程默认可在任何核运行。
- 调用方按需通过 `set_affinity` / `pin_to_core` 收紧掩码。

## 6. 亲和性匹配规则

### 6.1 空掩码 = 无限制

```rust
// 调度策略层选核伪代码（v0.16.0 由调用方实现）
fn pick_core_for_thread(sched: &Scheduler, tid: Tid) -> u32 {
    let mask = sched.affinity[tid.0 as usize];
    if mask.is_empty() {
        // 空掩码：任意核都可（仍受 reservation 二次过滤）
        return find_least_loaded_core(sched);   // 0..core_count 中最闲的
    }
    // 非空掩码：在 mask 包含的核中选最闲的
    find_least_loaded_core_in_mask(sched, mask)
}
```

- `CoreMask(0)`（`is_empty() == true`）= **无限制**，可调度到任何活跃核。
- 这与 Linux `cpumask` 语义不同（Linux 空 cpumask = 不可运行），EnerOS 选择
  「空 = 无限制」是为了让 `sched_init` 后所有线程默认全核可运行，简化调用方。

### 6.2 非空掩码 = 仅掩码中的核

- 非空掩码（如 `CoreMask::single(3)`）= 仅可调度到位 3 置 1 的核（核 3）。
- 调度策略层应遍历 `mask.contains(i)` for `i in 0..core_count`，选取可用核。

### 6.3 与 reservation 配合

亲和性检查与 reservation 检查是**两层独立过滤**：

```
线程 T 想入队到核 C
  ↓
1. affinity[T].contains(C)?  ← 亲和性层（若 affinity 空，跳过此检查）
   ↓ 通过
2. reservation.can_enqueue(C, T.is_rtos)?  ← 独占层
   ↓ 通过
3. rqs[C].enqueue(T)  ← 实际入队
```

- **亲和性层**：由调度策略层在选核时检查（v0.16.0 `enqueue` API 不主动检查）。
- **独占层**：`enqueue(sched, tid, core)` 内置检查（见 `docs/rtos-core-pinning.md`）。
- **reserved 核仅允许 RTOS 线程**：即使亲和性允许某线程上核 0，若核 0 被 reserve
  且该线程非 RTOS，仍被拒绝。

典型组合：

| 场景 | affinity[tid] | reservation[0] | 结果 |
|------|---------------|----------------|------|
| Agent 线程默认 | 空（无限制） | false | 可上任何核 |
| Agent 线程默认 | 空（无限制） | true（RTOS 独占） | 不能上核 0，可上核 1+ |
| RTOS 线程绑定核 0 | `single(0)` | true | 仅可上核 0（且是 RTOS） |
| Agent 线程绑定核 1+ | `all(8).remove(0)` | true（核 0 独占） | 仅可上核 1..7 |

## 7. 边界条件处理

### 7.1 count=64 的 UB 防御

```rust
// sched/src/affinity.rs
pub fn all(count: u32) -> Self {
    if count >= MAX_CORES {
        Self(u64::MAX)          // 防御 1u64 << 64 的 UB
    } else {
        Self((1u64 << count) - 1)
    }
}
```

- `1u64 << 64` 在 Rust 中是 **UB**（shift 超出类型位宽）。
- 用 `if count >= MAX_CORES` 显式处理，返回 `u64::MAX`（全 1）。
- 测试 `test_all_mask_boundary_64` 验证此路径。

### 7.2 core≥64 的防御

| 操作 | 行为 | 实现 |
|------|------|------|
| `single(core)` | debug panic / release UB | `debug_assert!`（调用方应避免） |
| `contains(core)` | 返回 `false` | 显式 `if core >= MAX_CORES { return false; }` |
| `add(core)` | 静默忽略 | 显式 `if core < MAX_CORES { ... }` |
| `remove(core)` | 静默忽略 | 显式 `if core < MAX_CORES { ... }` |

设计差异：

- `single` 用 `debug_assert`（构造时调用方应保证合法，否则是 bug）。
- `contains` / `add` / `remove` 用静默忽略（运行时查询/修改，可能来自不可信输入，
  不应 panic）。

### 7.3 tid≥256 的防御

```rust
// sched/src/lib.rs
pub fn set_affinity(sched: &mut Scheduler, tid: Tid, cores: CoreMask) -> Result<(), SchedError> {
    if tid.0 as usize >= MAX_THREADS {
        return Err(SchedError::NoRunnableTask);
    }
    sched.affinity[tid.0 as usize] = cores;
    Ok(())
}
```

- `tid.0 >= 256` 返回 `Err(SchedError::NoRunnableTask)`。
- `pin_to_core` 委托 `set_affinity`，因此同样受保护。
- `NoRunnableTask` 复用为「线程 ID 越界」错误（语义略宽，但符合 D4 简化）。

## 8. 使用示例

### 8.1 绑定线程到 Core 3

```rust
use eneros_sched::{sched_init, pin_to_core, Tid};

let mut sched = sched_init(4);
// 把 Tid(7) 绑定到 Core 3
assert_eq!(pin_to_core(&mut sched, Tid(7), 3), Ok(()));
// 验证：affinity[7] 仅含 bit 3
assert!(sched.affinity[7].contains(3));
assert!(!sched.affinity[7].contains(2));
assert_eq!(sched.affinity[7].count(), 1);
```

### 8.2 设置线程可在 Core 1 和 Core 2 上运行

```rust
use eneros_sched::{sched_init, set_affinity, CoreMask, Tid};

let mut sched = sched_init(4);
// 构造 {Core 1, Core 2} 掩码
let mut mask = CoreMask::default();
mask.add(1);
mask.add(2);
assert_eq!(set_affinity(&mut sched, Tid(5), mask), Ok(()));
// 验证
assert!(sched.affinity[5].contains(1));
assert!(sched.affinity[5].contains(2));
assert!(!sched.affinity[5].contains(0));
assert_eq!(sched.affinity[5].count(), 2);
```

或者用 `all` + `remove`：

```rust
let mut mask = CoreMask::all(4);   // {0,1,2,3}
mask.remove(0);                    // {1,2,3}
mask.remove(3);                    // {1,2}
set_affinity(&mut sched, Tid(5), mask);
```

### 8.3 查询线程亲和性

```rust
use eneros_sched::{sched_init, set_affinity, Tid, CoreMask};

let mut sched = sched_init(8);
set_affinity(&mut sched, Tid(10), CoreMask::all(8));
// 直接读 affinity 表
let mask = sched.affinity[10];
assert!(mask.contains(0));
assert!(mask.contains(7));
assert_eq!(mask.count(), 8);

// 默认（未设置）= 空掩码 = 无限制
let default_mask = sched.affinity[11];
assert!(default_mask.is_empty());
```

### 8.4 交集测试

```rust
use eneros_sched::CoreMask;

let a = CoreMask::all(4);          // {0,1,2,3}
let b = CoreMask::single(2);       // {2}
let c = CoreMask::single(5);       // {5}
assert!(a.intersects(b));          // 共享 bit 2
assert!(!a.intersects(c));         // 不共享
```

### 8.5 与 reservation 配合的完整场景

```rust
use eneros_sched::*;

let mut sched = sched_init(4);
// Core 0 独占给 RTOS
reserve_core(&mut sched, 0);
// Agent 线程绑定到 Core 1+（不能上 Core 0）
let mut mask = CoreMask::all(4);
mask.remove(0);
set_affinity(&mut sched, Tid(100), mask);
// 调度策略层选核（伪代码）
let target = 1;   // 应从 affinity[100] 中选最闲的核
enqueue(&mut sched, Tid(100), target);   // 成功
// 试图上 Core 0 会被 reservation 拒绝（即使 affinity 允许）
enqueue(&mut sched, Tid(100), 0);        // 静默丢弃
assert_eq!(sched.rqs[0].load(), 0);
assert_eq!(sched.rqs[1].load(), 1);
```

## 9. 蓝图对齐

### 9.1 蓝图 §4.1 CoreMask 对照

| 蓝图数据结构 | 实现 | 文件 |
|--------------|------|------|
| 核亲和性掩码 | `CoreMask(pub u64)` | `sched/src/affinity.rs` |
| 64 位容量 | `MAX_CORES = 64` | `sched/src/affinity.rs` |
| 位运算操作 | `single/all/contains/add/remove/count/is_empty/intersects/bits` | `sched/src/affinity.rs` |
| 默认空掩码 | `CoreMask::default() = CoreMask(0)` | `sched/src/affinity.rs` |
| per-thread 亲和性表 | `affinity: [CoreMask; 256]` | `sched/src/lib.rs` |
| 设置亲和性 | `set_affinity(sched, tid, cores)` | `sched/src/lib.rs` |
| 绑定单核 | `pin_to_core(sched, tid, core)` | `sched/src/lib.rs` |

### 9.2 测试覆盖

`sched/src/affinity.rs` 内 8 个单元测试：

| 测试 | 验证点 |
|------|--------|
| `test_single_mask_sets_one_bit` | `single(3)` 仅置 bit 3，`count()==1` |
| `test_all_mask_inclusive_range` | `all(0)` 空、`all(4)` 含 bit 0..3 |
| `test_all_mask_boundary_64` | `all(64)` 返回 `u64::MAX`（无 UB） |
| `test_add_remove` | `add`/`remove` 增删位，`count` 同步变化 |
| `test_add_out_of_range_ignored` | `add(64)` / `add(100)` 静默忽略 |
| `test_contains_out_of_range_false` | `contains(64)` / `contains(u32::MAX)` 返回 false |
| `test_intersects` | 交集测试（含空掩码场景） |
| `test_default_is_empty` | `CoreMask::default()` 是空掩码 |

`lib.rs` 内亲和性相关测试 4 个：

| 测试 | 验证点 |
|------|--------|
| `test_set_affinity_valid` | `set_affinity(Tid(5), all(4))` 写入正确 |
| `test_set_affinity_out_of_range` | `Tid(256)` 返回 `Err(NoRunnableTask)` |
| `test_pin_to_core_valid` | `pin_to_core(Tid(7), 3)` 写入 `single(3)` |
| `test_pin_to_core_invalid_core` | `core >= core_count` 返回 `Err(InvalidCore)` |

## 10. 未来扩展

### 10.1 动态亲和性调整

- 根据运行时负载自动收紧/放宽掩码：当某核持续空闲时，临时允许更多线程上该核。
- 与 `Balancer` 配合：均衡器迁移线程时同步更新 `affinity[tid]`。
- 需引入「软亲和性」（preferred core）vs「硬亲和性」（must run on）区分。

### 10.2 NUMA 感知亲和性

- 多簇系统（如飞腾 D2000 双簇 8 核）扩展为 `(node_id, core_mask)` 二元组：
  ```rust
  pub struct NumaMask {
      pub node: u32,        // 0 或 1（双簇）
      pub cores: CoreMask,  // node 内的核掩码
  }
  ```
- balance 优先在 node 内迁移，跨 node 迁移作为最后手段（延迟更高）。
- `intersects` 扩展为 node+core 双重比较。

### 10.3 enqueue 路径自动亲和性检查

- v0.16.0 的 `enqueue(sched, tid, core)` 不检查 `affinity[tid].contains(core)`，
  由调用方负责选核。
- 未来可在 `enqueue` 内增加：
  ```rust
  let mask = sched.affinity[tid.0 as usize];
  if !mask.is_empty() && !mask.contains(core) {
      return;   // 亲和性不允许
  }
  ```
- 需权衡：增加每次 enqueue 的开销 vs 严格的策略保证。

### 10.4 亲和性继承

- 父线程 spawn 子线程时，子线程继承父线程的 `affinity`。
- 用于「容器化」绑定：一组协作线程共享同一掩码。

### 10.5 亲和性统计

- 在 `Scheduler` 增加 `affinity_violations: u64` 计数器，记录试图违反亲和性的
  enqueue 尝试。
- 用于调试策略层选核逻辑。

### 10.6 与 IPI 配合

- 当 `set_affinity` 把线程从核 A 迁移到核 B 时，调用
  `ipi_send(B, IpiMsg::Reschedule)` 通知核 B 重新调度。
- v0.16.0 `set_affinity` 仅修改掩码，不触发实际迁移（线程仍可能在原核 RQ 中）。

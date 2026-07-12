# 高精度定时器实现

> 版本：v0.12.0
> 适用范围：EnerOS Time 服务 TimerWheel 定时器轮
> 蓝图依据：`蓝图/phase0.md` §v0.12.0
> crate：eneros-time（`time/src/hrtimer.rs`）
> 相关文档：`docs/system-clock-service.md`（定时器 API 入口）、`docs/arm-generic-timer-usage.md`

---

## 1. 概述

EnerOS v0.12.0 实现了一个基于定长数组的高精度定时器轮（TimerWheel），提供一次性与周期定时器能力。定时器轮是上层 `api.rs` 中 `register_timer` / `register_periodic` / `cancel_timer` 的底层支撑，用于调度器时间片、超时回调、心跳等场景。

### 1.1 设计定位

| 维度 | 选择 | 理由 |
|------|------|------|
| 数据结构 | 64 槽定长数组 | Phase 0 简化，无需动态分配，支持 `const fn` 静态初始化 |
| 容量 | 64 个定时器 | 足够覆盖内核态早期需求（调度、看门狗、协议超时） |
| 精度 | 纳秒级（由 `deadline_ns: u64` 决定） | 与 `HalClock` 单调纳秒时钟对齐 |
| 回调类型 | `fn()`（裸函数指针） | 无需堆分配，`Copy` 语义，适合 no_std |
| 并发保护 | 无（由上层 `spin::Mutex` 保护） | 保持轮本身简单，职责分离 |

### 1.2 在 EnerOS 中的位置

```
┌─────────────────────────────────────────────────────┐
│  api.rs — register_timer / register_periodic / ...  │
│  spin::Mutex<WHEEL> 保护并发                          │
└──────────────────┬──────────────────────────────────┘
                   │ WHEEL.lock().add / tick / cancel
┌──────────────────▼──────────────────────────────────┐
│  hrtimer.rs — TimerWheel                             │
│  [Option<HrTimer>; 64]                              │
└──────────────────┬──────────────────────────────────┘
                   │ 定时器中断处理程序调用 tick(now_ns)
┌──────────────────▼──────────────────────────────────┐
│  Arm64Timer 中断（CNTP_IRQ）→ 触发 tick              │
└─────────────────────────────────────────────────────┘
```

---

## 2. 数据结构

### 2.1 TimerId

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimerId(pub u64);
```

- 全局唯一标识符，由 `TimerWheel` 内部的 `next_id` 字段递增分配。
- 从 `1` 开始递增（`0` 保留为"无效"哨兵，虽然代码未显式使用）。
- `u64` 范围足够，实际不会耗尽。
- 单调递增：即使取消某个定时器后再次 `add`，新 id 也大于所有历史 id，避免 ABA 问题。

### 2.2 HrTimer

```rust
#[derive(Clone, Copy)]
pub struct HrTimer {
    pub id: TimerId,
    pub deadline_ns: u64,
    pub callback: fn(),
    pub periodic: bool,
    pub period_ns: u64,
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `id` | `TimerId` | 定时器唯一标识 |
| `deadline_ns` | `u64` | 到期时间（单调时钟纳秒） |
| `callback` | `fn()` | 到期时调用的函数指针 |
| `periodic` | `bool` | `true` 为周期定时器，`false` 为一次性 |
| `period_ns` | `u64` | 周期定时器的重复间隔（一次性定时器为 0） |

> **Copy 派生的意义**：所有字段都是 `Copy` 类型（`TimerId`/`u64`/`fn()`/`bool`），因此 `HrTimer` 可派生 `Clone + Copy`。这使得 `[None; 64]` 数组初始化成为可能——`Option<HrTimer>` 在 `HrTimer: Copy` 时可在 `const` 上下文中用 `[None; 64]` 构造，无需 Rust 1.79+ 的 `const { None }` 块表达式。

### 2.3 TimerWheel

```rust
pub struct TimerWheel {
    pub timers: [Option<HrTimer>; 64],
    pub count: usize,
    pub next_id: u64,
    pub expired_count: u64,
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `timers` | `[Option<HrTimer>; 64]` | 64 个槽位，每槽最多容纳一个定时器 |
| `count` | `usize` | 当前活跃定时器数量 |
| `next_id` | `u64` | 下一个分配的 TimerId 值 |
| `expired_count` | `u64` | 累计已触发定时器次数（可观测性） |

---

## 3. 一次性定时器 vs 周期定时器

| 类型 | `periodic` | `period_ns` | 触发后行为 |
|------|-----------|-------------|-----------|
| 一次性 | `false` | `0` | 触发回调后从轮中移除 |
| 周期 | `true` | `> 0` | 触发回调后重新 arm，下次到期 = `now + period_ns` |

周期定时器的重新 arm 使用 `saturating_add`，避免 `now + period_ns` 溢出：

```rust
timer.deadline_ns = now_ns.saturating_add(timer.period_ns);
```

---

## 4. 生命周期

### 4.1 add —— 添加定时器

```rust
pub fn add(
    &mut self,
    deadline_ns: u64,
    cb: fn(),
    periodic: bool,
    period_ns: u64,
) -> Option<TimerId> {
    if self.count >= 64 {
        return None;
    }
    for slot in self.timers.iter_mut() {
        if slot.is_none() {
            let id = TimerId(self.next_id);
            self.next_id += 1;
            *slot = Some(HrTimer {
                id,
                deadline_ns,
                callback: cb,
                periodic,
                period_ns,
            });
            self.count += 1;
            return Some(id);
        }
    }
    // Unreachable when count < 64: a free slot always exists.
    None
}
```

流程：

1. 若 `count >= 64`，轮已满，返回 `None`。
2. 线性扫描 `timers` 数组，找到第一个空槽（`None`）。
3. 分配新的 `TimerId`（`next_id` 自增），构造 `HrTimer` 存入槽中。
4. `count` 自增，返回 `Some(TimerId)`。

> **复杂度**：最坏 O(64) = O(1)。64 槽线性扫描在嵌入式场景下完全可接受，无需哈希或堆。

### 4.2 tick —— 处理到期定时器

```rust
pub fn tick(&mut self, now_ns: u64) -> u64 {
    let mut next_deadline = u64::MAX;
    for slot in self.timers.iter_mut() {
        if let Some(timer) = slot {
            if now_ns >= timer.deadline_ns {
                // Expired: invoke callback.
                (timer.callback)();
                self.expired_count += 1;
                if timer.periodic {
                    // Periodic timer: re-arm relative to now.
                    timer.deadline_ns = now_ns.saturating_add(timer.period_ns);
                } else {
                    // One-shot timer: remove.
                    *slot = None;
                    self.count -= 1;
                    continue;
                }
            }
            next_deadline = next_deadline.min(timer.deadline_ns);
        }
    }
    next_deadline
}
```

流程：

1. 遍历所有槽位。
2. 若定时器到期（`now_ns >= deadline_ns`）：
   - 调用 `callback`。
   - `expired_count` 自增。
   - 周期定时器：重新 arm（`deadline_ns = now + period_ns`），保留在槽中。
   - 一次性定时器：移除（`*slot = None`），`count` 自减，跳过 `next_deadline` 更新。
3. 未到期或重新 arm 后的定时器，用其 `deadline_ns` 更新 `next_deadline` 的最小值。
4. 返回 `next_deadline`：最近的下一个到期时间；无定时器时返回 `u64::MAX`。

> **返回值的用途**：调用方（通常是定时器中断处理程序）拿到 `next_deadline` 后，可设置硬件定时器截止（`HalClock::set_deadline`），使下一次中断恰好发生在最近到期时刻。

> **多定时器同时到期**：单次 `tick` 调用会处理所有 `now_ns >= deadline_ns` 的定时器，全部回调在同一上下文中顺序执行。

### 4.3 cancel —— 取消定时器

```rust
pub fn cancel(&mut self, id: TimerId) {
    for slot in self.timers.iter_mut() {
        if let Some(t) = slot {
            if t.id == id {
                *slot = None;
                self.count -= 1;
                return;
            }
        }
    }
}
```

- 按 `TimerId` 线性查找并移除。
- 若 id 不存在，静默返回（no-op），不报错。

---

## 5. 辅助方法

### 5.1 next_deadline —— 查询最近到期

```rust
pub fn next_deadline(&self) -> Option<u64> {
    self.timers.iter().flatten().map(|t| t.deadline_ns).min()
}
```

- 不触发回调，仅查询。
- 空轮返回 `None`。

### 5.2 len / is_empty

```rust
pub fn len(&self) -> usize {
    self.count
}

pub fn is_empty(&self) -> bool {
    self.count == 0
}
```

直接返回缓存的 `count` 字段，O(1)。

---

## 6. const fn new 与静态初始化

```rust
impl TimerWheel {
    pub const fn new() -> Self {
        Self {
            timers: [None; 64],
            count: 0,
            next_id: 1,
            expired_count: 0,
        }
    }
}
```

`new()` 是 `const fn`，可在 `static` 上下文中调用。`api.rs` 正是利用这一点初始化全局 `WHEEL`：

```rust
static WHEEL: Mutex<TimerWheel> = Mutex::new(TimerWheel::new());
```

`Mutex::new` 本身也是 `const fn`（`spin` crate 提供），因此整个 `WHEEL` 在编译期完成初始化，无需运行时构造，符合 no_std 内核启动要求。

`Default` trait 也已实现，等价于 `new()`：

```rust
impl Default for TimerWheel {
    fn default() -> Self {
        Self::new()
    }
}
```

---

## 7. 并发安全

### 7.1 TimerWheel 本身非线程安全

`TimerWheel` 的所有方法都接收 `&mut self`，未内置任何同步机制。这是因为：

- 内核定时器轮的典型使用场景是单线程中断上下文或禁中断临界区。
- 在 `tick` 遍历过程中加锁会引入自旋开销，影响中断延迟。

### 7.2 上层 spin::Mutex 保护

`api.rs` 中 `WHEEL` 被 `spin::Mutex<TimerWheel>` 包裹，所有访问通过 `WHEEL.lock()` 串行化：

```rust
// 注册定时器
pub fn register_timer(deadline_ns: u64, cb: fn()) -> Option<TimerId> {
    WHEEL.lock().add(deadline_ns, cb, false, 0)
}

// 取消定时器
pub fn cancel_timer(id: TimerId) {
    WHEEL.lock().cancel(id);
}

// 中断处理中推进
fn timer_irq_handler() {
    let now = get_monotonic_ns();
    let next = WHEEL.lock().tick(now);
    // 设置硬件下次中断...
}
```

这种分层使得 `TimerWheel` 可独立测试（单线程直接调用），生产环境由 `api.rs` 负责同步。

---

## 8. 容量限制

- 定时器轮固定 64 槽，`count >= 64` 时 `add()` 返回 `None`。
- 调用方应处理 `None` 情况（如降级、丢弃低优先级定时器、记录告警）。
- 64 的选择基于 Phase 0 早期需求评估：调度器（1）、看门狗（1）、协议超时（若干）、用户定时器，总计远小于 64。后续版本若容量不足，可升级为动态分配或更大数据结构。

---

## 9. 使用示例

### 9.1 直接使用 TimerWheel（测试或单线程场景）

```rust
use eneros_time::{TimerWheel, TimerId};

static CALLBACK_FIRED: AtomicU32 = AtomicU32::new(0);

fn my_callback() {
    CALLBACK_FIRED.fetch_add(1, Ordering::SeqCst);
}

let mut wheel = TimerWheel::new();

// 注册一次性定时器，1000ns 后触发
let id = wheel.add(1000, my_callback, false, 0).unwrap();
assert_eq!(wheel.len(), 1);

// now < deadline，tick 不触发
let next = wheel.tick(500);
assert_eq!(next, 1000);
assert_eq!(CALLBACK_FIRED.load(Ordering::SeqCst), 0);

// now >= deadline，触发并移除
let next = wheel.tick(1000);
assert_eq!(CALLBACK_FIRED.load(Ordering::SeqCst), 1);
assert_eq!(wheel.len(), 0);
assert_eq!(next, u64::MAX); // 无剩余定时器
```

### 9.2 周期定时器

```rust
let mut wheel = TimerWheel::new();

// 每 500ns 触发一次
let _id = wheel.add(1000, my_callback, true, 500);

// 首次触发
wheel.tick(1000);
assert_eq!(CALLBACK_FIRED.load(Ordering::SeqCst), 1);
assert_eq!(wheel.len(), 1); // 周期定时器保留
assert_eq!(wheel.next_deadline(), Some(1500));

// 第二次触发
wheel.tick(1500);
assert_eq!(CALLBACK_FIRED.load(Ordering::SeqCst), 2);
assert_eq!(wheel.next_deadline(), Some(2000));
```

### 9.3 通过上层 API 使用（生产场景）

```rust
use eneros_time::{register_timer, register_periodic, cancel_timer, timer_expired_count};

// 一次性定时器：500ms 后触发
let id = register_timer(500_000_000, || {
    // 超时处理
});
if let Some(tid) = id {
    // 若需取消
    cancel_timer(tid);
}

// 周期定时器：每 1 秒触发心跳
register_periodic(1_000_000_000, || {
    // heartbeat
});

// 可观测性：查询累计触发次数
let total = timer_expired_count();
```

---

## 10. 设计决策

### 10.1 为什么用 64 槽数组而非最小堆

| 方案 | 优点 | 缺点 | v0.12.0 是否采用 |
|------|------|------|------------------|
| 64 槽定长数组 | `const fn` 静态初始化；无堆分配；`Copy` 语义；实现简单 | 容量固定 64；`add`/`cancel` 最坏 O(64) | 是 |
| 最小堆（BinaryHeap） | `add`/`next_deadline` O(log n)；容量可扩展 | 需 `alloc`（依赖 v0.11.0 用户堆）；不能 `const fn` 静态初始化；`tick` 仍需遍历同时到期的多个定时器 | 否（留给后续版本） |

Phase 0 阶段的核心约束是 no_std + 无堆（v0.11.0 用户堆尚未普遍可用），且内核态定时器数量有限。64 槽数组在满足需求的同时保持最简实现，是恰当的工程权衡。当 v0.11.0 用户堆成熟且定时器数量增长后，可重构为最小堆或层级定时器轮（hierarchical timer wheel）。

### 10.2 为什么用 fn() 而非闭包

`callback: fn()` 是裸函数指针，大小固定（一个 usize），`Copy`，无需堆分配。闭包（`Fn` trait object）需要 `Box<dyn Fn>` 或 `&dyn Fn`，前者依赖堆，后者涉及生命周期管理，均不符合 Phase 0 简化目标。代价是回调无法捕获上下文，调用方需通过全局状态传递参数（嵌入式常见模式）。

### 10.3 为什么 next_id 从 1 开始

`TimerId(0)` 在语义上表示"无效"，虽然代码未显式使用此哨兵，但从 1 开始可避免与"未初始化的 u64 默认值 0"混淆，提升可诊断性。

### 10.4 为什么 tick 返回 u64::MAX 而非 Option

`tick` 返回 `u64`（`u64::MAX` 表示无定时器）而非 `Option<u64>`，便于调用方直接传给 `HalClock::set_deadline`。硬件定时器截止寄存器通常接受一个 u64 值，`u64::MAX` 可表示"无限远，不触发中断"，避免额外的模式匹配。`next_deadline()` 方法则返回 `Option<u64>`，用于纯查询场景。

### 10.5 为什么 expired_count 不区分一次性与周期

`expired_count` 统计所有触发事件，包括周期定时器的重复触发。这是可观测性的正确语义——它反映"定时器轮处理了多少次到期"，而非"多少个独立定时器到期"。监控该值可评估系统中断负载。

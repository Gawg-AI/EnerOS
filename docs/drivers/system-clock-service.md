# 系统时钟服务设计

> 版本：v0.12.0
> 适用范围：EnerOS Time 服务统一时间 API、单调时钟、全局状态管理
> 蓝图依据：`蓝图/phase0.md` §v0.12.0
> crate：eneros-time（`time/src/api.rs`、`time/src/monotonic.rs`）
> 依赖：eneros-hal（`HalClock` trait）、eneros-time/rtc（`Pl031Rtc`）
> 相关文档：`docs/arm-generic-timer-usage.md`（v0.6.0 Arm64Timer）、`docs/rtc-driver-design.md`

---

## 1. 概述

EnerOS v0.12.0 在 HAL 时钟（v0.6.0 Arm64Timer）与 PL031 RTC（v0.12.0）之上构建了统一的时间服务，对外提供单调时钟、墙钟时间、睡眠等待、定时器注册等 API。该服务采用依赖注入模式，由 BSP 在启动阶段将 `HalClock` 实现与 RTC 基地址传入，之后全局可用。

### 1.1 三层架构

时间服务分为三层，自下而上依次为：

```
┌──────────────────────────────────────────────────────┐
│  第三层：Time API（api.rs）                           │
│  get_time / get_monotonic_ns / sleep_until /         │
│  register_timer / rtc_read / ...                     │
└──────────────────┬───────────────────────────────────┘
                   │ 调用
┌──────────────────▼───────────────────────────────────┐
│  第二层：MonotonicClock（monotonic.rs）               │
│  boot_ns 记录启动时刻，now_ns 做 saturating_sub      │
└──────────────────┬───────────────────────────────────┘
                   │ 依赖
┌──────────────────▼───────────────────────────────────┐
│  第一层：HAL HalClock（hal crate，v0.6.0 实现）       │
│  Arm64Timer — 读取 CNTPCT_EL0 物理计数器             │
└──────────────────────────────────────────────────────┘
```

| 层级 | 组件 | 职责 |
|------|------|------|
| 第一层 | `HalClock` trait + `Arm64Timer` | 提供硬件单调纳秒计数（`now_ns()`）、频率查询、定时器截止设置 |
| 第二层 | `MonotonicClock` | 记录启动时刻 `boot_ns`，将 HAL 计数转换为"自启动以来纳秒" |
| 第三层 | Time API（`api.rs`） | 整合单调时钟与 RTC，提供统一时间接口与定时器管理 |

### 1.2 设计目标

- **统一入口**：所有时间相关操作通过 `eneros_time::` 命名空间暴露，隐藏底层细节。
- **依赖注入**：`time_init` 接收 `&'static dyn HalClock`，不与具体 BSP 硬编码耦合。
- **未初始化安全**：`time_init` 调用前所有 API 返回 0 或 epoch，不 panic。
- **no_std 兼容**：使用 `spin::Mutex` 替代 `std::sync::Mutex`，全局状态可静态初始化。

---

## 2. HalClock trait 复用

时间服务不重复实现硬件计数器读取，而是复用 v0.6.0 已实现的 `HalClock` trait。该 trait 定义于 `hal/src/lib.rs`：

```rust
pub trait HalClock {
    /// 返回当前单调时间（纳秒）。
    fn now_ns(&self) -> u64;
    /// 返回时钟频率（Hz）。
    fn frequency_hz(&self) -> u64;
    /// 设置定时器截止时间（纳秒）。
    fn set_deadline(&self, ns: u64) -> Result<(), HalError>;
}
```

ARM64 平台的实现是 `Arm64Timer`（`hal/src/arm64/timer.rs`），通过读取 `CNTPCT_EL0` 物理计数器并按 `CNTFRQ_EL0` 频率换算为纳秒。该实现的细节见 `docs/arm-generic-timer-usage.md`，本文档不重复其内联汇编。

时间服务仅依赖 trait 方法 `now_ns()`，对底层是物理计时器还是虚拟计时器无感知，便于在仿真宿主或其他架构上替换实现。

---

## 3. MonotonicClock 设计

### 3.1 结构体

`MonotonicClock` 仅持有一个启动时刻快照：

```rust
pub struct MonotonicClock {
    boot_ns: u64,
}
```

### 3.2 init —— 记录启动时刻

`init` 在 `time_init` 中调用，捕获当前 HAL 计数器值作为启动基准：

```rust
impl MonotonicClock {
    pub fn init(clock: &dyn HalClock) -> Self {
        Self {
            boot_ns: clock.now_ns(),
        }
    }
}
```

### 3.3 now_ns —— 自启动以来纳秒

`now_ns` 用当前 HAL 计数减去 `boot_ns`，得到自系统启动以来经过的纳秒数。使用 `saturating_sub` 避免下溢：

```rust
pub fn now_ns(&self, clock: &dyn HalClock) -> u64 {
    clock.now_ns().saturating_sub(self.boot_ns)
}
```

> **saturating_sub 的意义**：若底层时钟因非单调原因（如虚拟化偏移调整）返回了低于 `boot_ns` 的值，普通减法会因无符号整数下溢而得到一个巨大的值，破坏时间语义。`saturating_sub` 在此情况下返回 0，保证单调性不出现负值跳变。

### 3.4 单调性保证

`MonotonicClock` 本身不保证底层 `HalClock` 单调，但 ARM Generic Timer 的 `CNTPCT_EL0` 是硬件保证单调递增的，因此实际运行中 `now_ns()` 结果单调非递减。

---

## 4. 依赖注入与全局状态管理

### 4.1 time_init —— 注入入口

`time_init` 是时间服务的唯一初始化入口，采用依赖注入模式接收两个参数：

```rust
pub fn time_init(clock: &'static dyn HalClock, rtc_base: u64) {
    CLOCK.lock().0 = Some(clock);
    *MONO.lock() = Some(MonotonicClock::init(clock));
    *WHEEL.lock() = TimerWheel::new();
    *RTC_BASE.lock() = rtc_base;
    let offset = if rtc_base == 0 {
        0
    } else {
        Pl031Rtc::new(rtc_base).read_secs() * 1_000_000_000
    };
    *RTC_OFFSET_NS.lock() = offset;
}
```

| 参数 | 类型 | 说明 |
|------|------|------|
| `clock` | `&'static dyn HalClock` | HAL 时钟实现，需具有 `'static` 生命周期（通常是 BSP 中的 `static` 单例） |
| `rtc_base` | `u64` | PL031 RTC 的 MMIO 基地址；传 `0` 表示无 RTC，offset 置零 |

初始化步骤：

1. 将 `clock` 存入全局 `CLOCK`。
2. 用 `clock` 初始化 `MonotonicClock`，存入全局 `MONO`。
3. 重置定时器轮 `WHEEL` 为空。
4. 记录 `rtc_base` 到 `RTC_BASE`。
5. 读取 RTC 当前秒数，乘以 `1_000_000_000` 转为纳秒，存入 `RTC_OFFSET_NS` 作为墙钟偏移。若 `rtc_base == 0` 则偏移为零。

### 4.2 全局状态

所有全局状态均用 `spin::Mutex` 保护，支持 no_std 环境下的自旋同步：

```rust
static CLOCK: Mutex<ClockRef> = Mutex::new(ClockRef(None));
static MONO: Mutex<Option<MonotonicClock>> = Mutex::new(None);
static WHEEL: Mutex<TimerWheel> = Mutex::new(TimerWheel::new());
static RTC_OFFSET_NS: Mutex<u64> = Mutex::new(0);
static RTC_BASE: Mutex<u64> = Mutex::new(0);
```

| 静态变量 | 类型 | 初始值 | 用途 |
|----------|------|--------|------|
| `CLOCK` | `Mutex<ClockRef>` | `ClockRef(None)` | 存储 HAL 时钟引用 |
| `MONO` | `Mutex<Option<MonotonicClock>>` | `None` | 单调时钟实例 |
| `WHEEL` | `Mutex<TimerWheel>` | `TimerWheel::new()` | 高精度定时器轮 |
| `RTC_OFFSET_NS` | `Mutex<u64>` | `0` | 墙钟纳秒偏移（RTC 启动时刻） |
| `RTC_BASE` | `Mutex<u64>` | `0` | PL031 MMIO 基地址 |

### 4.3 ClockRef —— dyn HalClock 的 Sync 包装

#### 问题

`dyn HalClock` 默认不实现 `Sync`（trait object 的 `Sync` 仅在 trait 本身要求 `Sync` 时自动获得，而 `HalClock` 未加 `: Sync` 约束），因此无法直接放入 `static Mutex<dyn HalClock>` 或 `Mutex<Option<&'static dyn HalClock>>`。

#### 解决方案

引入包装类型 `ClockRef`，手动实现 `Send + Sync`：

```rust
struct ClockRef(Option<&'static dyn HalClock>);
unsafe impl Send for ClockRef {}
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl Sync for ClockRef {}
```

#### 安全性论证

`unsafe impl Sync` 的正确性依赖以下前提：

1. **真实实现是只读的**：`Arm64Timer` 的 `now_ns()` 仅读取 `CNTPCT_EL0` 系统寄存器，不修改任何内部可变状态。`frequency_hz()` 返回常量。`set_deadline()` 写 `CNTP_CVAL_EL0`，是写硬件寄存器而非共享内存，且该操作本身是原子的单寄存器访问。
2. **硬件单例**：`Arm64Timer` 在系统中只有一个实例，通过 `&'static` 引用共享，不存在所有权竞争。
3. **Mutex 保护访问**：`ClockRef` 外层包裹 `spin::Mutex`，即使存在并发访问也会被锁串行化。

因此，将 `&'static dyn HalClock` 声明为 `Sync` 是 sound 的。`clippy::non_send_fields_in_send_ty` 的告警被显式允许，因为此处正是经过论证的 unsafe 实现。

---

## 5. 时间 API

### 5.1 API 列表

| 函数 | 签名 | 说明 |
|------|------|------|
| `time_init` | `(clock: &'static dyn HalClock, rtc_base: u64)` | 初始化时间服务 |
| `get_monotonic_ns` | `() -> u64` | 返回自启动以来单调纳秒；未初始化返回 0 |
| `get_time` | `() -> TimeStamp` | 返回墙钟时间（`RTC_OFFSET_NS + monotonic_ns`） |
| `sleep_until` | `(deadline_ns: u64)` | 忙等至单调时钟到达 deadline |
| `register_timer` | `(deadline_ns: u64, cb: fn()) -> Option<TimerId>` | 注册一次性定时器 |
| `register_periodic` | `(period_ns: u64, cb: fn()) -> Option<TimerId>` | 注册周期定时器 |
| `cancel_timer` | `(id: TimerId)` | 取消定时器 |
| `rtc_read` | `() -> RtcTime` | 读取 RTC 墙钟时间 |
| `rtc_write` | `(t: RtcTime)` | 校准 RTC |
| `timer_expired_count` | `() -> u64` | 返回已到期定时器总数（可观测性） |

### 5.2 get_monotonic_ns

```rust
pub fn get_monotonic_ns() -> u64 {
    let clock = CLOCK.lock().0;
    let clock = match clock {
        Some(c) => c,
        None => return 0,
    };
    let mono = MONO.lock();
    match &*mono {
        Some(m) => m.now_ns(clock),
        None => 0,
    }
}
```

未初始化时（`CLOCK` 为 `None` 或 `MONO` 为 `None`），返回 0 而非 panic。

### 5.3 get_time

```rust
pub fn get_time() -> TimeStamp {
    TimeStamp(*RTC_OFFSET_NS.lock() + get_monotonic_ns())
}
```

墙钟时间 = RTC 启动时刻偏移 + 自启动以来单调纳秒。这样即使 RTC 是秒级精度，墙钟也能在纳秒级连续递增（由单调时钟驱动）。

### 5.4 sleep_until

```rust
pub fn sleep_until(deadline_ns: u64) {
    while get_monotonic_ns() < deadline_ns {
        #[cfg(target_arch = "aarch64")]
        unsafe {
            core::arch::asm!("wfe");
        }
        #[cfg(not(target_arch = "aarch64"))]
        core::hint::spin_loop();
    }
}
```

- **aarch64**：使用 `wfe`（Wait For Event）指令，使 CPU 进入低功耗等待状态，直到事件触发（如定时器中断）。比纯自旋更节能。
- **非 aarch64**（如 host 单元测试）：使用 `core::hint::spin_loop()` 旋转等待。

`sleep_until` 是忙等（busy-wait），不挂起线程。在 Phase 0 无调度器的阶段，这是唯一可行的等待方式。

### 5.5 定时器注册

```rust
pub fn register_timer(deadline_ns: u64, cb: fn()) -> Option<TimerId> {
    WHEEL.lock().add(deadline_ns, cb, false, 0)
}

pub fn register_periodic(period_ns: u64, cb: fn()) -> Option<TimerId> {
    let deadline = get_monotonic_ns().saturating_add(period_ns);
    WHEEL.lock().add(deadline, cb, true, period_ns)
}

pub fn cancel_timer(id: TimerId) {
    WHEEL.lock().cancel(id);
}
```

- **一次性定时器**：在 `deadline_ns` 时刻触发一次后自动移除。
- **周期定时器**：首次触发时间为 `now + period_ns`，之后每 `period_ns` 重复触发。
- 定时器轮满（64 槽）时 `register_*` 返回 `None`。

### 5.6 RTC 读写

```rust
pub fn rtc_read() -> RtcTime {
    let base = *RTC_BASE.lock();
    if base == 0 {
        secs_to_rtc(0)
    } else {
        Pl031Rtc::new(base).read()
    }
}

pub fn rtc_write(t: RtcTime) {
    let base = *RTC_BASE.lock();
    if base != 0 {
        Pl031Rtc::new(base).write(t);
    }
}
```

未配置 RTC（`rtc_base == 0`）时，`rtc_read` 返回 Unix epoch，`rtc_write` 为空操作。

### 5.7 timer_expired_count

```rust
pub fn timer_expired_count() -> u64 {
    WHEEL.lock().expired_count
}
```

返回自 `time_init` 以来定时器轮累计触发的定时器总数，用于可观测性（监控定时器负载、诊断丢失触发等）。

---

## 6. 未初始化行为

时间服务在 `time_init` 调用前处于"未初始化"状态。此时各 API 行为如下：

| API | 未初始化时返回值 | 是否 panic |
|-----|------------------|-----------|
| `get_monotonic_ns()` | `0` | 否 |
| `get_time()` | `TimeStamp(0)` | 否 |
| `sleep_until(ns)` | 行为正常（`get_monotonic_ns()` 返回 0，若 `ns > 0` 则持续忙等） | 否 |
| `register_timer(...)` | 正常返回 `Some(TimerId)`（WHEEL 已静态初始化） | 否 |
| `rtc_read()` | Unix epoch（1970-01-01 Thursday） | 否 |
| `rtc_write(...)` | 空操作 | 否 |
| `timer_expired_count()` | `0` | 否 |

这一设计确保系统在启动早期（`time_init` 尚未调用时）若意外调用时间 API 不会崩溃，仅返回零值。

---

## 7. 使用示例

### 7.1 完整初始化流程

```rust
use eneros_time::{time_init, get_monotonic_ns, get_time, rtc_read};

// 假设 BSP 提供的 Arm64Timer 单例
static ARM_TIMER: Arm64Timer = Arm64Timer::new();

fn boot_main() {
    // 初始化时间服务：注入 HAL 时钟 + PL031 RTC 基地址
    time_init(&ARM_TIMER, 0x0901_0000);

    // 后续可正常使用所有时间 API
    let mono = get_monotonic_ns();
    let wall = get_time();        // TimeStamp（纳秒）
    let rtc = rtc_read();         // RtcTime
}
```

### 7.2 睡眠与定时器

```rust
use eneros_time::{get_monotonic_ns, sleep_until, register_timer, register_periodic};

fn wait_100ms() {
    let deadline = get_monotonic_ns() + 100_000_000; // 100ms 后
    sleep_until(deadline);
}

fn setup_heartbeat() {
    // 每 1 秒触发一次 heartbeat 回调
    register_periodic(1_000_000_000, || {
        // heartbeat: 打印或更新看门狗
    });
}
```

### 7.3 无 RTC 场景

```rust
// 某些仿真或无 RTC 硬件的平台，rtc_base 传 0
time_init(&ARM_TIMER, 0);

// rtc_read 返回 epoch，get_time 退化为纯单调时钟
let t = get_time(); // TimeStamp(monotonic_ns)，墙钟无绝对意义
```

---

## 8. 设计决策

### 8.1 为什么用依赖注入而非全局 HAL

`time_init` 接收 `&'static dyn HalClock` 而非通过 `hal().clock()` 全局获取，原因：

- **解耦**：time crate 不依赖 `hal` crate 的全局单例机制，可独立测试（注入 mock clock）。
- **显式初始化顺序**：`time_init` 必须在 HAL 初始化之后调用，依赖关系在调用点显式可见。
- **多时钟源支持**：未来若需切换时钟源（如从 Arm64Timer 换为虚拟化时钟），只需改变传入参数。

### 8.2 为什么用 spin::Mutex

| 选择 | 理由 |
|------|------|
| `spin::Mutex` | no_std 兼容，无需操作系统线程支持；自旋锁在禁中断的内核上下文中是安全的 |
| 不用 `std::sync::Mutex` | 违反 no_std 约束（蓝图 §43.1） |
| 不用 `heapless::FnvIndexMap` | 时间状态是全局单例，数组容器不适用 |

### 8.3 为什么墙钟 = RTC_OFFSET_NS + monotonic_ns

直接每次读 RTC 会带来两个问题：RTC 精度仅秒级，且 MMIO 读取开销大。将 RTC 启动时刻缓存为 `RTC_OFFSET_NS`，之后墙钟 = 偏移 + 单调纳秒，既保证纳秒级精度，又避免频繁访问 RTC。代价是 RTC 校准后需重启 `time_init` 才能更新偏移。

### 8.4 为什么 sleep_until 用 wfe 而非纯自旋

`wfe` 使 CPU 进入低功耗事件等待，直到有事件（如定时器中断）唤醒。相比 `spin_loop` 的持续空转，`wfe` 显著降低功耗与总线干扰。在非 aarch64 宿主（单元测试）上退化为 `spin_loop` 保持可移植性。

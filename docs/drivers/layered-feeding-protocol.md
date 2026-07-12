# 分层喂狗协议设计

> 版本：v0.13.0
> 适用范围：EnerOS Watchdog 服务分层喂狗协议与全局 API
> 蓝图依据：`蓝图/phase0.md` §v0.13.0（核心设计 D4 / D7 / D8）
> crate：eneros-watchdog（`watchdog/src/layered.rs` + `watchdog/src/api.rs`）
> 关联文档：`docs/watchdog-design.md`（SP805 硬件驱动）

---

## 1. 概述

### 1.1 设计动机

传统单层看门狗只能检测"整个系统是否卡死"，无法区分是内核、Runtime 还是 Agent 层出了问题。EnerOS v0.13.0 引入**分层喂狗协议**（Layered Feeding Protocol），将系统划分为多个独立的监控层，每层拥有独立的喂狗周期与超时阈值：

- **多层级独立监控**：内核层 / Runtime 层 / Agent 层各自注册，独立喂狗。
- **任一层卡死即触发**：任何一层超时，系统都能感知并采取相应措施。
- **两级超时**：软超时（`period_ms`）仅告警，硬超时（`hard_timeout_ms`）触发硬件复位。

### 1.2 核心目标

| 目标 | 实现 |
|------|------|
| 层级隔离 | 每层独立 `period_ms`，互不干扰 |
| 故障定位 | `LayerTimeout(LayerId)` 返回超时层的真实 ID |
| 硬件兜底 | 超过 `hard_timeout_ms` 调用 `hw.stop()` 触发复位 |
| 可测试性 | 时间参数由外部注入，`layered.rs` 可独立单元测试 |

### 1.3 架构位置

三层结构自上而下：

1. **全局 API 层**（`api.rs`）：`wdt_init` / `wdt_register_layer` / `wdt_feed_layer` / `wdt_check` 等，注入时间戳（`eneros_time::get_monotonic_ns`）。
2. **分层喂狗层**（`layered.rs`）：`Watchdog { hw, layers[8], hard_timeout_ms, next_id }`，提供 `register_layer` / `feed_layer` / `check`。
3. **硬件驱动层**（`wdt.rs`）：SP805 `HwWatchdog`，提供 `init` / `kick` / `stop`。

---

## 2. 核心数据结构

分层喂狗协议定义于 `watchdog/src/layered.rs`，核心数据结构如下。

### 2.1 LayerId

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayerId(pub u32);
```

- **新型别封装**（newtype），避免裸 `u32` 与其他 ID 混淆。
- 实现 `Clone` / `Copy` / `Debug` / `PartialEq` / `Eq`，支持值传递与比较。
- **ID 从 1 开始递增**，0 保留为"无效"哨兵值（见 §4 蓝图 bug 修复说明）。

### 2.2 FeedLayer

```rust
#[derive(Clone, Copy)]
pub struct FeedLayer {
    pub id: LayerId,
    pub name: &'static str,
    pub period_ms: u32,
    pub last_feed_ns: u64,
    pub enabled: bool,
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `id` | `LayerId` | 层的唯一标识，注册时由 `next_id` 分配 |
| `name` | `&'static str` | 层名称（如 `"kernel"` / `"runtime"` / `"agent"`），静态字符串避免 alloc |
| `period_ms` | `u32` | 软超时阈值（毫秒），超过即返回 `LayerTimeout` |
| `last_feed_ns` | `u64` | 上次喂狗时间戳（纳秒），0 表示尚未喂狗 |
| `enabled` | `bool` | 是否参与 `check()` 检测，`false` 时跳过 |

> **注意**：`last_feed_ns` 初始为 0，调用方必须在注册后立即调用 `feed_layer()` 记录首次喂狗时间，否则 `check()` 会因 `elapsed = now - 0` 远大于 `period_ms` 而误报超时。

### 2.3 WatchdogStatus

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum WatchdogStatus {
    AllFed,
    LayerTimeout(LayerId),
    HardReset,
}
```

| 变体 | 含义 | 调用方响应 |
|------|------|-----------|
| `AllFed` | 所有使能层均未超时 | 继续正常运行，`hw.kick()` 已自动执行 |
| `LayerTimeout(LayerId)` | 某层超过 `period_ms` 但未超 `hard_timeout_ms` | 记录日志、告警，不复位 |
| `HardReset` | 某层超过 `hard_timeout_ms` | `hw.stop()` 已执行，硬件复位即将触发 |

### 2.4 Watchdog

```rust
pub struct Watchdog {
    pub hw: HwWatchdog,
    pub layers: [Option<FeedLayer>; 8],
    pub hard_timeout_ms: u32,
    pub next_id: u32,
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `hw` | `HwWatchdog` | 底层硬件驱动实例 |
| `layers` | `[Option<FeedLayer>; 8]` | 8 槽数组式管理，`None` 表示空槽 |
| `hard_timeout_ms` | `u32` | 硬超时阈值，超过即触发硬件复位 |
| `next_id` | `u32` | 下一个待分配的 LayerId，从 1 递增 |

> **8 槽设计**：使用固定大小数组 `[Option<FeedLayer>; 8]` 而非 `Vec`，避免 `alloc` 依赖，符合 no_std 约束。8 个槽位足以覆盖典型三层架构（内核/Runtime/Agent）及未来扩展（如网络层、存储层）。`Watchdog` 另提供 `Default` 实现（`base=0`、`hard_timeout_ms=0`），便于测试。

---

## 3. 两级超时检测机制

两级超时检测是分层喂狗协议的核心设计（蓝图核心设计 D7），由 `check()` 方法实现。

### 3.1 超时分级

| 级别 | 阈值 | 触发动作 | 返回值 |
|------|------|----------|--------|
| 第一级（软超时） | `period_ms` | 无硬件动作，仅记录 | `LayerTimeout(LayerId)` |
| 第二级（硬超时） | `hard_timeout_ms` | `hw.stop()` 触发硬件复位 | `HardReset` |
| 正常 | `elapsed ≤ period_ms` | `hw.kick()` 喂狗 | `AllFed` |

### 3.2 检测逻辑

`check()` 遍历所有使能层，计算每层的 `elapsed_ms`，按优先级返回最严重的状态。核心流程：

1. 将 `now_ns` 转换为毫秒：`now_ms = now_ns / 1_000_000`。
2. 遍历所有 `enabled == true` 的层，跳过禁用层。
3. 对每层计算 `elapsed_ms = now_ms.saturating_sub(last_ms)`。
4. 若 `elapsed_ms > period_ms`：标记软超时，记录第一个超时层 ID。
5. 若 `elapsed_ms > hard_timeout_ms`：标记硬超时，记录最严重层。
6. 返回优先级最高的状态：`HardReset > LayerTimeout > AllFed`。

关键实现细节：

```rust
let elapsed_ms = now_ms.saturating_sub(last_ms) as u32;
```

- `saturating_sub`：防止 `now < last` 时下溢（如时钟回拨），下溢时返回 0。

### 3.3 优先级与短路

检测优先级：**HardReset > LayerTimeout > AllFed**。

- 若任一层触发硬超时（`elapsed > hard_timeout_ms`），立即返回 `HardReset` 并调用 `hw.stop()`。
- 若无硬超时但有软超时，返回第一个软超时层的 `LayerTimeout(id)`。
- 若所有层均正常，调用 `hw.kick()` 喂狗并返回 `AllFed`。

> **注意**：`hw.kick()` 仅在 `AllFed` 分支调用。软超时（`LayerTimeout`）时不喂狗——这是设计意图：某层卡死后停止喂狗，让硬件看门狗计数继续递减，最终触发硬件复位，确保卡死不会无限延续。

### 3.4 禁用层处理

`enabled == false` 的层在 `check()` 中被 `continue` 跳过，不参与超时检测。这允许临时禁用某层（如调试时暂停 Agent 层监控）而不影响其他层。

---

## 4. 蓝图 bug 修复说明

### 4.1 原蓝图问题（D8）

原蓝图设计中，`WatchdogStatus::LayerTimeout(0)` 总是返回 `0`，无法区分是哪个层超时：

```rust
// ❌ 原蓝图（有 bug）
WatchdogStatus::LayerTimeout(0)  // 永远返回 0，无法定位超时层
```

这一问题导致上层无法根据返回值定位故障层，丧失了分层监控的核心价值。

### 4.2 v0.13.0 修复

v0.13.0 修复此 bug，返回第一个超时层的真实 `LayerId`：在 `check()` 中记录第一个超时层的 `layer.id`，最终返回 `WatchdogStatus::LayerTimeout(timeout_layer.unwrap())`，而非固定 0。

### 4.3 修复验证

`test_check_layer_timeout` 测试验证：注册层后喂狗（`t=0`），在 `t=200ms` 检查（`> 100ms` period），断言返回 `LayerTimeout(id)` 其中 `id` 为真实注册 ID，非 0。

---

## 5. API 设计哲学

### 5.1 时间参数注入（D4）

`layered.rs` 的 `feed_layer()` 和 `check()` 方法接受外部时间戳 `now_ns`，而非内部调用全局时间 API：

```rust
pub fn feed_layer(&mut self, id: LayerId, now_ns: u64)
pub fn check(&mut self, now_ns: u64) -> WatchdogStatus
```

### 5.2 设计好处

| 好处 | 说明 |
|------|------|
| **可独立单元测试** | `layered.rs` 不依赖 `eneros_time` crate，测试时直接传入构造的时间戳 |
| **时间源解耦** | `layered.rs` 不关心时间来源（硬件定时器 / 模拟时间 / 测试桩） |
| **无隐藏副作用** | 所有时间相关的行为由调用方显式控制，便于推理与审计 |
| **no_std 友好** | 避免在底层模块引入全局状态依赖 |

### 5.3 api.rs 的时间注入

`api.rs` 层负责调用 `eneros_time::get_monotonic_ns()` 获取真实时间，然后传给 `layered.rs`：

```rust
pub fn wdt_feed_layer(id: LayerId) {
    if !*INITIALIZED.lock() { return; }
    let now_ns = eneros_time::get_monotonic_ns();
    WATCHDOG.lock().feed_layer(id, now_ns);  // 注入时间
}
```

`wdt_check()` 同理：获取 `now_ns` 后调用 `WATCHDOG.lock().check(now_ns)`。

### 5.4 分层职责

| 层 | 职责 | 时间依赖 |
|----|------|----------|
| `wdt.rs` | 硬件 MMIO 操作 | 无 |
| `layered.rs` | 分层状态管理与超时检测 | 无（接受参数） |
| `api.rs` | 全局状态管理与时间注入 | `eneros_time` |

---

## 6. 全局 API

全局 API 定义于 `watchdog/src/api.rs`，提供线程安全的全局看门狗访问入口。

### 6.1 双静态变量

```rust
static WATCHDOG: Mutex<Watchdog> = Mutex::new(Watchdog::new(HwWatchdog::new(0), 0));
static INITIALIZED: Mutex<bool> = Mutex::new(false);
```

| 静态变量 | 类型 | 说明 |
|----------|------|------|
| `WATCHDOG` | `Mutex<Watchdog>` | 全局 `Watchdog` 实例，初始为软件模式（`base=0`） |
| `INITIALIZED` | `Mutex<bool>` | 初始化标志，`false` 时所有操作为 no-op |

> **双静态设计**：`INITIALIZED` 单独存在而非用 `Option<Watchdog>` 替代，是因为 `spin::Mutex<Option<T>>` 的 `const fn` 初始化较复杂，且 `Watchdog::default()` 已提供安全的空状态。

### 6.2 wdt_init

创建 `HwWatchdog` 并初始化硬件，用新 `Watchdog` 替换全局实例（`hard_timeout_ms = timeout_ms`，蓝图 D7 规定），置 `INITIALIZED = true`：

```rust
pub fn wdt_init(timeout_ms: u32, wdt_base: u64) {
    let hw = HwWatchdog::new(wdt_base);
    hw.init(timeout_ms);
    *WATCHDOG.lock() = Watchdog::new(hw, timeout_ms);
    *INITIALIZED.lock() = true;
}
```

### 6.3 未初始化时的行为

`INITIALIZED == false` 时，所有 API 返回安全默认值：

| API | 未初始化行为 |
|-----|-------------|
| `wdt_kick()` | 直接返回，无副作用 |
| `wdt_register_layer(name, period_ms)` | 返回 `None` |
| `wdt_feed_layer(id)` | 直接返回，无副作用 |
| `wdt_check()` | 返回 `WatchdogStatus::AllFed` |
| `wdt_stop()` | 直接返回，无副作用 |
| `wdt_layer_count()` | 返回 `0` |

> **设计意图**：保证系统启动早期（看门狗未初始化）调用 API 不会 panic 或破坏状态，便于在启动流程中安全嵌入看门狗调用。

### 6.4 wdt_stop

停止硬件看门狗（`hw.stop()`），置 `INITIALIZED = false`，后续所有操作降级为 no-op。用途：调试场景下临时禁用看门狗，或 `check()` 触发 `HardReset` 后清理状态。

### 6.5 完整 API 列表

| API | 签名 | 说明 |
|-----|------|------|
| `wdt_init` | `(timeout_ms: u32, wdt_base: u64)` | 初始化全局看门狗 |
| `wdt_kick` | `()` | 直接喂硬件看门狗 |
| `wdt_register_layer` | `(name: &'static str, period_ms: u32) -> Option<LayerId>` | 注册新层 |
| `wdt_feed_layer` | `(id: LayerId)` | 喂指定层（注入时间） |
| `wdt_check` | `() -> WatchdogStatus` | 检查所有层（注入时间） |
| `wdt_stop` | `()` | 停止看门狗并标记未初始化 |
| `wdt_layer_count` | `() -> usize` | 返回已注册层数 |

---

## 7. 分层协议使用模式

### 7.1 典型三层架构

| 层 | period_ms | 说明 |
|----|-----------|------|
| 内核层 | 100 | 调度器每个 tick 喂狗，卡死 100ms 即告警 |
| Runtime 层 | 500 | 任务调度循环喂狗，卡死 500ms 即告警 |
| Agent 层 | 2000 | AI 决策循环喂狗，卡死 2s 即告警 |

### 7.2 使用示例

```rust
use eneros_watchdog::{
    wdt_init, wdt_register_layer, wdt_feed_layer, wdt_check, WatchdogStatus,
};

// 初始化（10s 硬超时，SP805 @ 0x09050000）
wdt_init(10_000, 0x09050000);

// 注册三层
let kernel_id = wdt_register_layer("kernel", 100).unwrap();
let runtime_id = wdt_register_layer("runtime", 500).unwrap();
let agent_id = wdt_register_layer("agent", 2000).unwrap();

// 各层独立喂狗
wdt_feed_layer(kernel_id);
wdt_feed_layer(runtime_id);
wdt_feed_layer(agent_id);

// 调度主循环中检查
match wdt_check() {
    WatchdogStatus::AllFed => {}
    WatchdogStatus::LayerTimeout(id) => log_warn!("layer {} timeout", id.0),
    WatchdogStatus::HardReset => log_error!("hard reset triggered"),
}
```

### 7.3 软件模式使用（QEMU）

`wdt_init(10_000, 0)` 传入 `base=0` 即进入软件模式，所有 MMIO 操作为 no-op，分层逻辑仍完整运行，适用于 QEMU 逻辑验证。

---

## 8. 并发安全

### 8.1 spin::Mutex 保护

全局 `WATCHDOG` 和 `INITIALIZED` 均由 `spin::Mutex` 保护（见 §6.1），适用于 no_std 环境：

- `spin::Mutex` 使用自旋等待，无系统调用依赖，适合内核态与 RTOS 态。
- 临界区极短（寄存器操作 + 数组遍历），自旋开销可忽略。
- 所有 API 内部加锁，对外提供线程安全接口。

### 8.2 锁粒度

每个 API 调用独立加锁，无嵌套锁。以 `wdt_feed_layer` 为例：先 `INITIALIZED.lock()` 检查初始化状态（锁 1），释放后在锁外调用 `get_monotonic_ns()`，再 `WATCHDOG.lock()` 执行操作（锁 2）。

- `INITIALIZED.lock()` 在 `WATCHDOG.lock()` 之前释放，无死锁风险。
- `get_monotonic_ns()` 在锁外调用，避免锁内耗时。

### 8.3 测试序列化

测试代码使用 `std::sync::Mutex` 序列化所有测试，避免并行测试干扰全局状态。每个测试通过 `TEST_LOCK.lock()` 获取 guard，调用 `reset_state()` 重置 `WATCHDOG` 与 `INITIALIZED` 后执行：

```rust
static TEST_LOCK: StdMutex<()> = StdMutex::new(());
fn reset_state() {
    *WATCHDOG.lock() = Watchdog::new(HwWatchdog::new(0), 0);
    *INITIALIZED.lock() = false;
}
```

- `TEST_LOCK` 为 `std::sync::Mutex<()>`，仅用于序列化，不保护数据。
- `reset_state()` 保证测试隔离。

---

## 9. 测试覆盖

`eneros-watchdog` crate 共 22 个单元测试，分布于三个模块。

### 9.1 wdt.rs（7 个测试）

详见 `docs/watchdog-design.md` §7。

### 9.2 layered.rs（10 个测试）

| 测试名 | 说明 |
|--------|------|
| `test_new_watchdog` | 构造 `Watchdog`，验证初始状态（空层、`hard_timeout_ms`、`next_id=1`） |
| `test_register_layer` | 注册单层，验证 ID、name、period、enabled、last_feed_ns、next_id 递增 |
| `test_register_multiple_layers` | 注册多层，验证 ID 递增（1/2/3）与层数统计 |
| `test_register_layer_full` | 注册满 8 槽后第 9 次注册返回 `None` |
| `test_feed_layer` | 喂狗后 `last_feed_ns` 正确更新 |
| `test_check_all_fed` | 未超时返回 `AllFed` |
| `test_check_layer_timeout` | 软超时返回 `LayerTimeout(id)`，验证 D8 bug 修复 |
| `test_check_hard_reset` | 硬超时返回 `HardReset` |
| `test_check_disabled_layer` | 禁用层不参与检测，返回 `AllFed` |
| `test_check_empty_layers` | 无层时 `check()` 返回 `AllFed` |

### 9.3 api.rs（5 个测试）

| 测试名 | 说明 |
|--------|------|
| `test_uninitialized_no_op` | 未初始化时所有 API 返回安全默认值 |
| `test_init_register_feed_check` | 完整流程：init → register → feed → check AllFed |
| `test_layer_count_tracking` | 注册 5 层后 `wdt_layer_count()` 返回 5 |
| `test_wdt_stop_resets_state` | `wdt_stop()` 后状态重置为未初始化 |
| `test_wdt_api_integration` | 集成测试：未初始化 → init → register → feed → stop → re-init 全流程 |

### 9.4 测试代码示例

`test_check_layer_timeout` 验证软超时返回真实 `LayerId`（D8 bug 修复）：

```rust
let id = wd.register_layer("kernel", 100).unwrap();
wd.feed_layer(id, 0);                  // t=0 喂狗
let status = wd.check(200_000_000);     // t=200ms > 100ms period
assert_eq!(status, WatchdogStatus::LayerTimeout(id));  // 返回真实 id
```

`test_check_hard_reset` 验证硬超时触发 `HardReset`：

```rust
wd.feed_layer(id, 0);                    // t=0 喂狗
let status = wd.check(2_000_000_000);    // t=2000ms > 1000ms hard_timeout
assert_eq!(status, WatchdogStatus::HardReset);
```

### 9.5 测试统计

| 模块 | 测试数 | 覆盖范围 |
|------|--------|----------|
| `wdt.rs` | 7 | 硬件驱动构造、使能判断、软件模式 no-op |
| `layered.rs` | 10 | 分层注册、喂狗、两级超时检测、禁用层、空层 |
| `api.rs` | 5 | 全局 API 初始化、注册、喂狗、停止、集成流程 |
| **合计** | **22** | — |

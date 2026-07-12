# SP805 硬件看门狗驱动设计

> 版本：v0.13.0
> 适用范围：EnerOS Watchdog 服务 SP805 硬件看门狗驱动
> 蓝图依据：`蓝图/phase0.md` §v0.13.0（P0-D 第二步）
> crate：eneros-watchdog（`watchdog/src/wdt.rs`）
> 硬件参考：ARM SP805 Dual-Timer Module（ARM DDI 0271）WDT 模式

---

## 1. 概述

SP805 是 ARM PrimeCell 系列的双定时器模块（Dual-Timer Module），其 Timer2 可配置为看门狗模式（Watchdog Mode），提供 32 位递减计数器，计数到 0 时可触发中断或系统复位。EnerOS 在 v0.13.0 引入 SP805 硬件看门狗驱动，作为 P0-D 阶段"第二步"的关键交付物，为系统提供硬件级复位兜底，防止内核/Runtime/Agent 任一层卡死后系统假死。

### 1.1 选型理由

| 原因 | 说明 |
|------|------|
| ARM PrimeCell 标准 | SP805 是 ARM PrimeCell 外设，IP 公开、文档完整，国产 SoC（飞腾/鲲鹏/瑞芯微）广泛集成 |
| 硬件复位兜底 | 计数到 0 可直接触发 SoC 复位信号，不依赖软件干预，最可靠的"最后一道防线" |
| 寄存器锁机制 | 内置 `WDT_LOCK` 锁寄存器，防止误写关键寄存器，安全性高 |
| QEMU 可验证 | QEMU virt 平台可挂载 SP805 设备进行仿真验证（默认未挂载，需手动指定） |

### 1.2 在 EnerOS 中的位置

三层结构自上而下：上层 `api.rs`（`wdt_init` / `wdt_kick` / `wdt_check`）→ `watchdog` crate `HwWatchdog` 驱动（`watchdog/src/wdt.rs`，通过 `HwWatchdog::init` / `kick` / `stop`）→ SP805 WDT 硬件（示例基地址 `0x0905_0000`），通过 MMIO（`read_volatile` / `write_volatile`）访问。

### 1.3 P0-D 第二步定位

v0.13.0 属于 Phase 0 阶段 P0-D（看门狗子系统）的第二步交付：

| 步骤 | 版本 | 交付物 |
|------|------|--------|
| 第一步 | — | 确定看门狗硬件选型与寄存器规范 |
| **第二步** | **v0.13.0** | **SP805 硬件驱动 + 分层喂狗协议 + 全局 API** |
| 后续 | v0.14.0+ | 接入调度主循环、GIC 中断集成、多核喂狗协调 |

### 1.4 关键特性

| 特性 | 说明 |
|------|------|
| 计数器宽度 | 32 位递减计数器 |
| 计时单位 | `WDT_LOAD` 单位为 μs，`load = timeout_ms * 1000` |
| 复位方式 | 计数到 0 触发 SoC 硬件复位（CTRL bit1 = 1） |
| 寄存器锁 | `WDT_LOCK` 写入解锁码后方可操作关键寄存器 |
| 软件模式 | `base == 0` 时所有 MMIO 操作降级为 no-op，用于 QEMU 无 WDT 场景 |

---

## 2. SP805 WDT 硬件规范

### 2.1 寄存器映射

SP805 WDT 寄存器相对于基地址偏移如下。实际平台基地址由设备树或板级配置决定（示例使用 `0x0905_0000`）。

| 偏移 | 名称 | 读写 | 说明 | 驱动使用 |
|------|------|------|------|----------|
| `0x00` | WDT_LOAD | RW | Load Register，写入计数初值（μs） | 是（`init`） |
| `0x04` | WDT_VALUE | RO | Current Value Register，读取当前计数值 | 否（预留） |
| `0x08` | WDT_CTRL | RW | Control Register，控制使能与复位行为 | 是（`init`/`stop`） |
| `0x0C` | WDT_INTCLR | WO | Interrupt Clear Register，写任意值清中断并重装计数 | 是（`init`/`kick`） |
| `0xC00` | WDT_LOCK | WO | Lock Register，控制寄存器访问锁定状态 | 是（`init`/`kick`/`stop`） |

> **说明**：v0.13.0 驱动使用 WDT_LOAD / WDT_CTRL / WDT_INTCLR / WDT_LOCK 四个寄存器完成初始化、喂狗、停止三类操作。WDT_VALUE（读取当前计数值）预留给后续版本（如运行时剩余时间查询）使用，当前未实现。

代码中定义的常量：

```rust
const WDT_LOAD: u64   = 0x00;
const WDT_VALUE: u64  = 0x04;  // reserved, currently unused
const WDT_CTRL: u64   = 0x08;
const WDT_INTCLR: u64 = 0x0c;
const WDT_LOCK: u64   = 0xC00;
```

### 2.2 锁机制

SP805 通过 `WDT_LOCK` 寄存器保护关键寄存器（WDT_LOAD / WDT_CTRL / WDT_INTCLR）免受意外写入。上电后寄存器默认处于锁定状态，必须先写入解锁码才能操作：

| 操作 | 写入值 | 常量 | 说明 |
|------|--------|------|------|
| 解锁 | `0x1ACCE551` | `WDT_UNLOCK` | 解除锁定，允许写关键寄存器 |
| 锁定 | `0x1` | `WDT_LOCK_V` | 恢复锁定，防止误写 |

```rust
const WDT_UNLOCK: u32 = 0x1ACCE551;
const WDT_LOCK_V: u32 = 0x1;
```

> **安全性**：每次操作关键寄存器前后均执行"解锁 → 操作 → 锁定"序列，确保锁定窗口最小化，降低误写风险。

### 2.3 CTRL 寄存器位定义

WDT_CTRL 寄存器控制看门狗的使能与复位行为：

| Bit | 名称 | 说明 |
|-----|------|------|
| bit 0 | `WDCTRL_INTEN` | 看门狗使能（1 = 启动计数） |
| bit 1 | `WDCTRL_RESEN` | 复位使能（1 = 计数到 0 触发 SoC 复位） |

驱动使用的 CTRL 值：

| 值 | 含义 | 使用场景 |
|----|------|----------|
| `0x3` | bit0 + bit1 同时置 1，使能计数 + 使能复位 | `init()` 启动看门狗 |
| `0x0` | 全部清零，停止看门狗 | `stop()` 停止看门狗 |

### 2.4 计时单位转换

`WDT_LOAD` 寄存器的单位为微秒（μs），而驱动 API 接受毫秒（ms）参数。转换公式：

```
load = timeout_ms * 1000
```

例如 10 秒超时（`timeout_ms = 10_000`）：

```rust
let load = timeout_ms * 1000;  // 10_000 * 1000 = 10_000_000 μs = 10s
self.w(WDT_LOAD, load);
```

> **注意**：`timeout_ms * 1000` 在 `timeout_ms` 较大时可能溢出 u32（u32 最大约 4294 秒）。v0.13.0 场景下看门狗超时通常在数秒到数十秒之间，溢出风险可接受；后续版本如需更长超时可升级为 u64 计算。

---

## 3. HwWatchdog 驱动设计

驱动源文件位于 `watchdog/src/wdt.rs`，提供对 SP805 硬件看门狗的最小封装。

### 3.1 结构体定义

`HwWatchdog` 仅持有一个 MMIO 基地址，结构极简：

```rust
pub struct HwWatchdog {
    pub base: u64,
}
```

- `base` 字段为 `pub`，允许上层 `Watchdog` 直接访问 `hw.base` 判断使能状态。
- 不持有任何运行时状态（stateless），所有操作直接映射到硬件寄存器。

### 3.2 new 方法

```rust
pub const fn new(base: u64) -> Self {
    Self { base }
}
```

- 标注为 `const fn`，支持在 `static` 上下文中初始化（如 `api.rs` 的全局 `WATCHDOG` 静态变量）。
- `base == 0` 表示软件模式（见 §4），不对应真实硬件。

### 3.3 init 方法

```rust
pub fn init(&self, timeout_ms: u32)
```

初始化看门狗并启动计数。操作序列：

1. 若 `base == 0`，直接返回（软件模式 no-op）。
2. 解锁寄存器：`WDT_LOCK ← WDT_UNLOCK`
3. 写入超时初值：`WDT_LOAD ← timeout_ms * 1000`
4. 清除中断并重装计数：`WDT_INTCLR ← 1`
5. 使能计数 + 复位：`WDT_CTRL ← 0x3`
6. 锁定寄存器：`WDT_LOCK ← WDT_LOCK_V`

### 3.4 kick 方法

```rust
pub fn kick(&self)
```

喂狗：清除中断并重装计数器，防止超时复位。操作序列：

1. 若 `base == 0`，直接返回（软件模式 no-op）。
2. 解锁寄存器：`WDT_LOCK ← WDT_UNLOCK`
3. 清中断重装：`WDT_INTCLR ← 1`
4. 锁定寄存器：`WDT_LOCK ← WDT_LOCK_V`

> **注意**：`kick()` 不写 `WDT_LOAD`，仅写 `WDT_INTCLR`。SP805 写 WDT_INTCLR 会自动用上次 WDT_LOAD 的值重装计数器，因此喂狗时无需重设超时。

### 3.5 stop 方法

```rust
pub fn stop(&self)
```

停止看门狗：清 CTRL 寄存器，停止计数。操作序列：

1. 若 `base == 0`，直接返回（软件模式 no-op）。
2. 解锁寄存器：`WDT_LOCK ← WDT_UNLOCK`
3. 清 CTRL：`WDT_CTRL ← 0`
4. 锁定寄存器：`WDT_LOCK ← WDT_LOCK_V`

> **用途**：`stop()` 主要用于调试场景（如 `api.rs` 的 `wdt_stop()`）以及 `layered.rs` 检测到 hard timeout 时主动触发复位——停止看门狗后，剩余计数归零将触发 SoC 复位。

### 3.6 is_enabled 方法

```rust
pub fn is_enabled(&self) -> bool {
    self.base != 0
}
```

判断驱动是否绑定到真实硬件。注意此处仅判断基地址是否非零，不读取硬件 CTRL 寄存器——这是一个轻量的软件级判断。

### 3.7 MMIO 操作封装

所有寄存器访问通过私有辅助方法 `w` / `r` 封装，使用 `core::ptr::read_volatile` / `write_volatile` 确保编译器不会优化掉硬件访问。寄存器宽度为 32 位（`*const u32` / `*mut u32`），`#[inline]` 提示编译器内联。`r` 当前预留（标注 `#[allow(dead_code)]`）。`unsafe` 块由调用方（`init`/`kick`/`stop`）承担，调用方需保证 `base` 指向有效设备地址。

---

## 4. 软件模式（base=0）

### 4.1 设计动机

QEMU `virt` 平台默认不挂载 SP805 看门狗设备。为了在 QEMU 上运行并验证分层喂狗协议（`layered.rs`）与全局 API（`api.rs`）的逻辑，驱动支持软件模式：当 `base == 0` 时，所有 MMIO 操作降级为 no-op。

### 4.2 行为规则

| 操作 | `base != 0`（硬件模式） | `base == 0`（软件模式） |
|------|------------------------|------------------------|
| `init(timeout_ms)` | 解锁→写 LOAD→清 INTCLR→写 CTRL=0x3→锁定 | 直接返回，无副作用 |
| `kick()` | 解锁→写 INTCLR=1→锁定 | 直接返回，无副作用 |
| `stop()` | 解锁→写 CTRL=0→锁定 | 直接返回，无副作用 |
| `is_enabled()` | 返回 `true` | 返回 `false` |

### 4.3 软件模式的价值

- **分层喂狗逻辑可验证**：`layered.rs` 的 `check()` 调用 `hw.kick()` / `hw.stop()`，软件模式下这些调用不 panic，分层逻辑（超时检测、状态返回）仍完整运行，可在 QEMU 上进行单元测试与集成测试。
- **无硬件依赖**：CI 环境无需真实硬件或 QEMU WDT 设备即可运行完整测试套件。
- **平滑迁移**：从软件模式切换到硬件模式只需在 `wdt_init()` 时传入真实 `wdt_base`，无需改动上层代码。

### 4.4 软件模式的局限

- 软件模式下 `stop()` 不会真正触发硬件复位，因此 hard timeout 场景只能通过 `WatchdogStatus::HardReset` 返回值在软件层感知，无法验证"硬件复位确实发生"。
- 真实硬件复位行为需在带 SP805 的物理板或 QEMU 手动挂载 WDT 设备时验证。

---

## 5. 安全性与并发

### 5.1 寄存器锁机制

SP805 硬件本身提供 `WDT_LOCK` 寄存器锁，防止意外写入关键寄存器（WDT_LOAD / WDT_CTRL / WDT_INTCLR）。驱动在每次操作前后严格遵循"解锁 → 操作 → 锁定"序列：

```
解锁（WDT_LOCK ← 0x1ACCE551）
  ↓
操作（写 WDT_LOAD / WDT_CTRL / WDT_INTCLR）
  ↓
锁定（WDT_LOCK ← 0x1）
```

锁定窗口最小化，降低以下风险：
- 野指针误写关键寄存器
- DMA 误操作覆盖寄存器
- 调试器误写入

### 5.2 驱动无锁设计

`HwWatchdog` 驱动本身**不持有任何锁**（stateless）：

- 无 `spin::Mutex`、无 `AtomicU32`、无内部可变状态。
- 所有方法接受 `&self`（不可变借用），仅通过 `write_volatile` 副作用操作硬件。
- 并发安全由上层 `Watchdog`（`layered.rs`）的 `spin::Mutex` 保护。

### 5.3 上层并发保护

在 `api.rs` 中，全局 `Watchdog` 实例由 `spin::Mutex<Watchdog>` 保护，所有对 `HwWatchdog` 的访问均经过 mutex 临界区（如 `WATCHDOG.lock().hw.kick()`）。因此 `HwWatchdog` 无需自身加锁，避免双重锁定开销。

### 5.4 MMIO 安全性说明

`w` / `r` 方法为 `unsafe`，调用方（`init` / `kick` / `stop`）需保证：

1. `self.base` 指向有效的 SP805 WDT 设备地址（或为 0 进入软件模式）。
2. 调用上下文已通过上层 mutex 序列化，避免并发 MMIO 访问。
3. 在 QEMU virt 平台上，`base` 由设备树或板级配置保证有效。

---

## 6. 使用示例

### 6.1 直接使用 HwWatchdog

```rust
use eneros_watchdog::HwWatchdog;

// SP805 WDT 基地址（示例）
let wdt = HwWatchdog::new(0x09050000);

// 初始化：10s 超时
wdt.init(10_000);

// 周期性喂狗（必须在 10s 内执行）
loop {
    // ... 业务逻辑 ...
    wdt.kick();
}

// 调试时停止看门狗
wdt.stop();
```

### 6.2 通过全局 API 使用（推荐）

生产环境中通常不直接操作 `HwWatchdog`，而是通过 `api.rs` 提供的统一接口，自动获得 mutex 保护和初始化检查：

```rust
use eneros_watchdog::{wdt_init, wdt_register_layer, wdt_feed_layer, wdt_check, LayerId};

// 初始化全局看门狗（10s 硬超时，硬件基地址 0x09050000）
wdt_init(10_000, 0x09050000);

// 注册分层喂狗
let kernel_id = wdt_register_layer("kernel", 100).unwrap();   // 100ms 软超时
let runtime_id = wdt_register_layer("runtime", 500).unwrap(); // 500ms 软超时

// 周期性喂狗
wdt_feed_layer(kernel_id);
wdt_feed_layer(runtime_id);

// 调度主循环中检查
let status = wdt_check();
// status 为 AllFed / LayerTimeout(id) / HardReset
```

---

## 7. 测试覆盖

`watchdog/src/wdt.rs` 包含 7 个单元测试，覆盖构造、使能判断、软件模式 no-op 三类场景：

| 测试名 | 说明 |
|--------|------|
| `test_new_with_base` | 构造带基地址的实例，验证 `base` 字段正确保存 |
| `test_new_with_zero_base` | 构造 `base=0` 的实例，验证字段为 0 |
| `test_is_enabled_true` | `base != 0` 时 `is_enabled()` 返回 `true` |
| `test_is_enabled_false` | `base == 0` 时 `is_enabled()` 返回 `false` |
| `test_init_zero_base_no_panic` | 软件模式 `init(10_000)` 不 panic |
| `test_kick_zero_base_no_panic` | 软件模式 `kick()` 不 panic |
| `test_stop_zero_base_no_panic` | 软件模式 `stop()` 不 panic |

### 7.1 测试设计要点

- **硬件模式不可测**：真实 MMIO 写操作在无硬件环境下会触发异常，因此硬件模式（`base != 0`）的逻辑通过代码审查保证，不直接测试。
- **软件模式全覆盖**：`base=0` 的所有路径（`init`/`kick`/`stop`）均验证不 panic，确保软件模式安全性。
- **构造与判断分离**：`new` 与 `is_enabled` 单独测试，避免副作用耦合。

### 7.2 整体测试规模

`eneros-watchdog` crate 共 22 个单元测试：

| 模块 | 测试数 | 覆盖范围 |
|------|--------|----------|
| `wdt.rs` | 7 | 硬件驱动构造、使能判断、软件模式 no-op |
| `layered.rs` | 10 | 分层注册、喂狗、两级超时检测、禁用层、空层 |
| `api.rs` | 5 | 全局 API 初始化、注册、喂狗、停止、集成流程 |
| **合计** | **22** | — |

详见 `docs/layered-feeding-protocol.md` §9。

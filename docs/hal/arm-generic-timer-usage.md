# ARM Generic Timer 使用

> 版本：v0.6.0
> 适用范围：EnerOS HAL ARM64 时钟与定时器驱动（ARM Generic Timer）
> 蓝图依据：`蓝图/phase0.md` §v0.6.0、§4.5
> crate：eneros-hal（`hal/src/arm64/timer.rs`）
> 硬件参考：ARM Architecture Reference Manual（ARM DDI 0487）— ARMv8 Generic Timer 章节

---

## 1. 概述

ARMv8 Generic Timer 是 ARM 架构内置的系统计数器与定时器，提供 64 位单调递增计数器与可配置的定时比较中断。EnerOS 在 v0.6.0 使用 Generic Timer 实现 `HalClock` trait，提供单调纳秒时钟与定时器截止（deadline）能力，支撑调度器时间片、超时等待、心跳等时间相关服务。

### 1.1 物理计时器 vs 虚拟计时器

ARMv8 Generic Timer 提供多组计时器，EnerOS 使用物理计时器（Physical Timer）：

| 计时器 | 系统寄存器前缀 | 异常级别 | 用途 | EnerOS 使用 |
|--------|---------------|----------|------|-------------|
| Physical Timer（EL1） | `CNTP_` | EL1/EL2/EL3 | 非安全物理计时器，内核态使用 | ✅ 使用（`CNTP_*`） |
| Virtual Timer | `CNTV_` | EL0/EL1/EL2 | 虚拟计时器，计数器值 = 物理计数器 - 偏移（`CNTVOFF_EL2`），用于虚拟化 | ❌ 不使用 |
| Hypervisor Timer | `CNTHP_` | EL2 | Hypervisor 物理计时器 | ❌ 不使用 |
| Secure Physical Timer | `CNTPS_` | EL3/EL1（若可访问） | 安全物理计时器 | ❌ 不使用 |

> **选择理由**：EnerOS 在 EL1 运行（seL4 微内核），使用物理计时器（`CNTP_*`）直接读取硬件计数器，无虚拟化偏移。虚拟计时器（`CNTV_*`）用于 Guest OS，Hypervisor Timer 用于 EL2。

### 1.2 Generic Timer 架构

```
┌─────────────────────────────────────────┐
│         System Counter（全局）           │
│  64-bit 单调递增计数器 @ CNTFRQ_EL0 Hz  │
│  所有 CPU 核共享同一计数器值             │
└──────────────┬──────────────────────────┘
               │ 广播计数器值
     ┌─────────┼─────────┐
     ▼         ▼         ▼
  ┌─────┐  ┌─────┐  ┌─────┐
  │CPU 0│  │CPU 1│  │CPU n│   per-core 定时器
  │CNTP │  │CNTP │  │CNTP │   每核独立比较器
  │_CTL │  │_CTL │  │_CTL │
  └──┬──┘  └──┬──┘  └──┬──┘
     │        │        │
     ▼        ▼        ▼
   IRQ 30   IRQ 30   IRQ 30  （PPI 14，每核独立）
```

| 特性 | 说明 |
|------|------|
| 计数器类型 | 64-bit 单调递增（永不回退，溢出需 ~584 年 @ 62.5MHz） |
| 频率 | 由 `CNTFRQ_EL0` 寄存器指定（QEMU virt 默认 62.5MHz） |
| 全局共享 | System Counter 是全局的，所有核读到相同的 `CNTPCT_EL0` 值 |
| per-core 定时器 | 每个核有独立的 `CNTP_TVAL_EL0`/`CNTP_CVAL_EL0`/`CNTP_CTL_EL1` 比较器 |
| 中断类型 | PPI 14（IRQ ID 30），per-core 私有，各核独立触发 |

---

## 2. 寄存器参考

### 2.1 寄存器总表

| 系统寄存器 | 编码 | 访问 | 说明 |
|-----------|------|------|------|
| `CNTFRQ_EL0` | `S3_3_C14_C0_0` | RW | 计数器频率（Hz），全局共享；通常由 bootloader（U-Boot/固件）在启动时设置 |
| `CNTPCT_EL0` | `S3_3_C14_C0_1` | RO | 物理计数器当前值（64-bit 单调递增）；读取需配合 `ISB` 保证有序 |
| `CNTP_TVAL_EL0` | `S3_3_C14_C2_0` | RW | 定时器倒计数值（32-bit，相对值）；读返回距触发的剩余 ticks，写设置新的相对截止 |
| `CNTP_CVAL_EL0` | `S3_3_C14_C2_1` | RW | 定时器比较值（64-bit，绝对值）；当 `CNTPCT_EL0 ≥ CNTP_CVAL_EL0` 时触发中断 |
| `CNTP_CTL_EL0` | `S3_3_C14_C2_2` | RW | 定时器控制：使能、中断屏蔽、状态查询 |

> **寄存器命名说明**：`CNTP_CTL_EL0` 中的 `EL0` 后缀表示该寄存器在 EL0 可访问（需 `CNTKCTL_EL1.EL0PTEN=1`），并非仅限 EL0 使用。EL1/EL2/EL3 均可访问。EnerOS 文档中亦称 `CNTP_CTL_EL1`，指同一寄存器。

### 2.2 CNTP_CTL_EL0 位定义

| 位 | 名称 | 值 | 含义 |
|----|------|-----|------|
| bit 0 | ENABLE | `0x1` | 定时器使能：1=使能比较器，0=禁用 |
| bit 1 | IMASK | `0x2` | 中断屏蔽：1=屏蔽定时器中断，0=允许中断 |
| bit 2 | ISTATUS | `0x4` | 中断状态（只读）：1=条件满足且中断已触发，0=未触发 |

**状态机说明**：

| ENABLE | IMASK | ISTATUS | 含义 |
|--------|-------|---------|------|
| 0 | × | 0 | 定时器禁用，不产生中断 |
| 1 | 1 | × | 定时器使能但中断屏蔽，不向 CPU 传递中断信号 |
| 1 | 0 | 0 | 定时器使能，等待条件满足（`CNTPCT ≥ CVAL`） |
| 1 | 0 | 1 | 定时器触发，中断信号已传递到 GIC |

> **IMASK vs ENABLE**：`ENABLE=0` 完全关闭比较器（不消耗功耗）；`IMASK=1` 保持比较器运行但不传递中断（用于临时屏蔽，便于快速恢复）。

### 2.3 TVAL vs CVAL

| 特性 | `CNTP_TVAL_EL0` | `CNTP_CVAL_EL0` |
|------|-----------------|-----------------|
| 位宽 | 32-bit | 64-bit |
| 语义 | 相对值（距当前剩余 ticks） | 绝对值（目标计数器值） |
| 写入效果 | 硬件自动计算 `CVAL = CNTPCT + TVAL` | 直接设置比较目标值 |
| 读取效果 | 返回 `CVAL - CNTPCT`（剩余 ticks） | 返回当前设置的 CVAL |
| 溢出风险 | 32-bit 限制，约 68.7s @ 62.5MHz 会溢出 | 64-bit，约 584 年才会溢出 |
| 适用场景 | 短周期定时（调度时间片） | 长周期或精确绝对截止时间 |

> **EnerOS 策略**：`set_deadline(ns)` 优先使用 `CNTP_CVAL_EL0`（绝对值），避免 32-bit TVAL 溢出。短周期定时也可用 TVAL。

---

## 3. 纳秒转换公式

### 3.1 核心公式

计数器值（ticks）与纳秒（ns）的转换基于 `CNTFRQ_EL0`（频率，Hz）：

```
ns = cntpct × 1_000_000_000 / cntfrq
ticks = ns × cntfrq / 1_000_000_000
```

**推导**：
- `cntfrq` = 每秒 ticks 数（Hz）
- 1 tick = `1 / cntfrq` 秒 = `1_000_000_000 / cntfrq` 纳秒
- `ns = cntpct × (1_000_000_000 / cntfrq)` = `cntpct × 1_000_000_000 / cntfrq`

### 3.2 Rust 实现

```rust
/// 将计数器 ticks 转换为纳秒。
fn ticks_to_ns(ticks: u64, freq: u64) -> u64 {
    // ns = ticks * 1_000_000_000 / freq
    // 注意：ticks * 1_000_000_000 可能溢出 u64（ticks > 1.8e10 时）
    // 使用 saturating_mul 防止溢出
    ticks.saturating_mul(1_000_000_000) / freq
}

/// 将纳秒转换为计数器 ticks。
fn ns_to_ticks(ns: u64, freq: u64) -> u64 {
    // ticks = ns * freq / 1_000_000_000
    // 注意：ns * freq 可能溢出（ns > 2.9e11 @ 62.5MHz 时）
    ns.saturating_mul(freq) / 1_000_000_000
}
```

### 3.3 溢出分析

| 运算 | 最大安全输入（u64） | QEMU virt 62.5MHz 下的含义 | 处理方式 |
|------|-------------------|---------------------------|----------|
| `ticks × 1e9` | ticks ≤ 1.8 × 10^10 | 约 291 年的计数器值 | `saturating_mul` 饱和 |
| `ns × freq` | ns ≤ 2.9 × 10^11 | 约 291 年的纳秒值 | `saturating_mul` 饱和 |

> **安全说明**：使用 `saturating_mul` 而非普通 `*`，当乘积溢出 `u64::MAX` 时饱和到 `u64::MAX` 而非 panic。对于 EnerOS 的运行周期（数年至数十年），不会触及溢出边界，但 `saturating_mul` 是防御性编程的必要措施。

### 3.4 精度说明

| 频率 | 1 tick 精度 | 最小可分辨时间 |
|------|------------|---------------|
| 62.5 MHz（QEMU virt） | 16 ns | 16 ns |
| 24 MHz（部分 ARM 平台） | ~41.67 ns | ~41.67 ns |
| 100 MHz（高端 SoC） | 10 ns | 10 ns |

> **纳秒精度损失**：`ticks_to_ns` 的整数除法会截断小数部分。62.5MHz 下，1 tick = 16ns，因此纳秒值的最低 4 bit 信息丢失（精度为 16ns 步进）。这对调度精度无影响（调度器通常以微秒为单位）。

---

## 4. 定时器中断配置

### 4.1 配置步骤

设置定时器在指定时间后触发的完整流程：

```rust
/// 设置定时器截止时间（纳秒，相对当前时间的偏移）。
fn set_deadline(&self, ns: u64) -> Result<(), HalError> {
    // 步骤1: 读取当前计数器值与频率
    let freq = self.frequency_hz();
    let current = read_cntpct();

    // 步骤2: 计算目标 tick 数（当前值 + 偏移）
    let delta_ticks = ns_to_ticks(ns, freq);
    let target = current.saturating_add(delta_ticks);

    // 步骤3: 写入比较值（CVAL，绝对值）
    write_cntp_cval(target);

    // 步骤4: 使能定时器并取消中断屏蔽
    // CNTP_CTL_EL0: ENABLE=1, IMASK=0
    write_cntp_ctl(0x1);

    Ok(())
}
```

| 步骤 | 寄存器 | 操作 | 说明 |
|------|--------|------|------|
| 1 | `CNTPCT_EL0` / `CNTFRQ_EL0` | 读取 | 获取当前计数器值与频率 |
| 2 | 计算 | `ticks = ns × freq / 1e9` | 将纳秒转换为 tick 偏移量 |
| 3 | `CNTP_CVAL_EL0` | 写入绝对值 | 设置比较目标（`当前值 + 偏移`） |
| 4 | `CNTP_CTL_EL0` | 写入 `0x1` | 使能定时器（ENABLE=1, IMASK=0） |

### 4.2 使用 TVAL 的替代方案

短周期定时可用 `CNTP_TVAL_EL0`（相对值，硬件自动计算）：

```rust
// 设置 1ms 后触发（相对值）
let ticks = ns_to_ticks(1_000_000, freq); // 62500 ticks @ 62.5MHz
write_cntp_tval(ticks);  // 硬件自动设置 CVAL = CNTPCT + TVAL
write_cntp_ctl(0x1);     // 使能
```

> **TVAL 限制**：32-bit，62.5MHz 下最大约 68.7 秒。超过此范围必须用 `CNTP_CVAL_EL0`。

### 4.3 注册 IRQ 30

ARM Generic Timer 物理定时器使用 PPI 14（IRQ ID 30）中断：

```rust
// 注册定时器中断 handler
hal().irq().register(30, IrqTrigger::Level, timer_irq_handler)?;

// 定时器中断 handler
fn timer_irq_handler(irq: u32) -> IrqAction {
    // 处理定时器到期
    // 例如：调度器时间片切换、超时检查等

    // 可选：重新设置下一次定时器截止
    // hal().clock().set_deadline(NEXT_SLICE_NS).ok();

    IrqAction::Handled
}

// 使能 IRQ 30
hal().irq().enable(30);
```

| 中断属性 | 值 | 说明 |
|----------|-----|------|
| IRQ ID | 30 | PPI 14（GIC 中断号 = 16 + 14 = 30） |
| 触发类型 | Level（电平触发） | Generic Timer 中断为电平触发，条件满足时持续有效 |
| 分发方式 | per-core | PPI 是每核私有中断，各核独立触发 |
| GIC 管理 | GICR | PPI(16–31) 由 Redistributor 管理（非 GICD） |

> **中断清除**：Generic Timer 中断是电平触发的，EOI（`ICC_EOIR1_EL1`）不会清除中断源。必须在 handler 中重新设置 `CNTP_CTL_EL0` 或 `CNTP_CVAL_EL0` 来清除中断条件（`ISTATUS` 清零），否则中断会立即重新触发。

### 4.4 关闭定时器

```rust
// 方法1：完全禁用定时器
write_cntp_ctl(0x0);  // ENABLE=0

// 方法2：屏蔽中断但保持比较器运行（快速恢复）
write_cntp_ctl(0x3);  // ENABLE=1, IMASK=1
```

---

## 5. QEMU virt 频率说明

### 5.1 默认频率

QEMU virt 机器的 ARM Generic Timer 默认频率为 **62.5 MHz**（62,500,000 Hz），由 QEMU 固件在启动时写入 `CNTFRQ_EL0`。

| 参数 | 值 | 说明 |
|------|-----|------|
| 频率（`CNTFRQ_EL0`） | 62,500,000 Hz | 62.5 MHz |
| 1 tick | 16 ns | `1 / 62.5e6 × 1e9 = 16` |
| 1 μs | 62.5 ticks | `1e3 ns / 16 ns ≈ 62.5`（实际 62 或 63，因整数截断） |
| 1 ms | 62,500 ticks | `1e6 ns / 16 ns = 62,500` |
| 1 s | 62,500,000 ticks | `1e9 ns / 16 ns = 62,500,000` |
| 32-bit TVAL 最大 | ~68.7 s | `0xFFFFFFFF / 62.5e6 ≈ 68.7` |
| 64-bit CVAL 最大 | ~584 年 | `0xFFFFFFFFFFFFFFFF / 62.5e6 / 86400 / 365 ≈ 584` |

### 5.2 频率验证

在运行时读取频率并验证：

```rust
fn check_frequency() {
    let freq = read_cntfrq();
    assert_eq!(freq, 62_500_000, "Expected 62.5MHz, got {} Hz", freq);
}
```

> **注意**：`CNTFRQ_EL0` 的值由 bootloader 设置。若 bootloader 未正确设置（如裸机 QEMU 直接 `-kernel` 加载），`CNTFRQ_EL0` 可能为 0，导致除零 panic。EnerOS 在 `init()` 中应检查并回退到编译期默认值（`const DEFAULT_FREQ: u64 = 62_500_000;`）。

### 5.3 其他平台频率参考

| 平台 | 频率 | 1 tick |
|------|------|--------|
| QEMU virt | 62.5 MHz | 16 ns |
| 树莓派 4B | 54 MHz | ~18.5 ns |
| 飞腾 D2000 | 100 MHz | 10 ns |
| 鲲鹏 920 | 100 MHz | 10 ns |

> EnerOS 在运行时从 `CNTFRQ_EL0` 读取频率，不硬编码，确保跨平台兼容。

---

## 6. EnerOS 实现说明

### 6.1 Arm64Timer 结构体

```rust
/// ARM Generic Timer 驱动（物理计时器）。
///
/// 实现 `HalClock` trait，提供单调纳秒时钟与定时器截止能力。
/// 基于 ARMv8 `CNTP_*` 系统寄存器，无 MMIO 依赖。
pub struct Arm64Timer {
    // 无字段：频率从 CNTFRQ_EL0 运行时读取，不缓存
}
```

> **设计决策**：`Arm64Timer` 不存储频率字段，每次调用 `frequency_hz()` 时从 `CNTFRQ_EL0` 读取。理由：
> - 频率是全局只读的（bootloader 设置后不变），无需缓存
> - 避免结构体初始化顺序问题（`static` 单例在编译期无法读寄存器）
> - `mrs cntfrq_el0` 是单周期指令，性能开销可忽略

### 6.2 now_ns() 实现

```rust
impl HalClock for Arm64Timer {
    fn now_ns(&self) -> u64 {
        let freq = self.frequency_hz();
        let cntpct = read_cntpct();
        ticks_to_ns(cntpct, freq)
    }
}
```

**辅助函数**：

```rust
/// 读取 CNTPCT_EL0（物理计数器值）。
///
/// 读取前需执行 ISB 确保指令有序（ARM ARM 建议）。
fn read_cntpct() -> u64 {
    let val: u64;
    unsafe {
        asm!(
            "isb",
            "mrs {0}, cntpct_el0",
            out(reg) val,
            options(nostack),
        );
    }
    val
}
```

> **ISB 的作用**：`CNTPCT_EL0` 的读取可能被 CPU 乱序执行到其他指令之前。`ISB`（Instruction Synchronization Barrier）强制后续指令重新取指，确保读取的计数器值严格在 `ISB` 之后的程序点。对于时间测量场景（如性能计数），这是必要的。

### 6.3 frequency_hz() 实现

```rust
impl HalClock for Arm64Timer {
    fn frequency_hz(&self) -> u64 {
        let val: u64;
        unsafe {
            asm!(
                "mrs {0}, cntfrq_el0",
                out(reg) val,
                options(nostack),
            );
        }
        val
    }
}
```

### 6.4 set_deadline() 实现

```rust
impl HalClock for Arm64Timer {
    fn set_deadline(&self, ns: u64) -> Result<(), HalError> {
        // 计算目标 tick 数（绝对值）
        let freq = self.frequency_hz();
        let current = read_cntpct();
        let delta = ns_to_ticks(ns, freq);
        let target = current.saturating_add(delta);

        // 写入 CVAL（绝对比较值）
        write_cntp_cval(target);

        // 使能定时器：ENABLE=1, IMASK=0
        write_cntp_ctl(0x1);

        Ok(())
    }
}
```

**辅助函数**：

```rust
/// 写入 CNTP_CVAL_EL0（64-bit 绝对比较值）。
fn write_cntp_cval(val: u64) {
    unsafe {
        asm!(
            "msr cntp_cval_el0, {0}",
            in(reg) val,
            options(nostack),
        );
    }
}

/// 写入 CNTP_CTL_EL0（控制寄存器）。
fn write_cntp_ctl(val: u64) {
    unsafe {
        asm!(
            "msr cntp_ctl_el0, {0}",
            in(reg) val,
            options(nostack),
        );
    }
}
```

### 6.5 HalClock trait 实现

`Arm64Timer` 实现 v0.5.0 定义的 `HalClock` trait（`hal/src/lib.rs`）：

```rust
pub trait HalClock {
    fn now_ns(&self) -> u64;
    fn frequency_hz(&self) -> u64;
    fn set_deadline(&self, ns: u64) -> Result<(), HalError>;
}
```

| 方法 | 实现方式 | 系统寄存器 | 特权要求 |
|------|----------|-----------|----------|
| `now_ns()` | 读计数器 → 转 ns | `CNTPCT_EL0` + `CNTFRQ_EL0` | 无（EL0 可读，需 `CNTKCTL_EL1.EL0PCTEN=1`） |
| `frequency_hz()` | 读频率 | `CNTFRQ_EL0` | 无 |
| `set_deadline(ns)` | 计算 ticks → 写 CVAL → 使能 | `CNTP_CVAL_EL0` + `CNTP_CTL_EL0` | 需特权（EL1+） |

### 6.6 单例与获取器

```rust
// 全局单例
static ARM64_TIMER: Arm64Timer = Arm64Timer;

/// 获取 ARM64 Timer 的 HAL 引用。
pub fn clock() -> &'static dyn HalClock {
    &ARM64_TIMER
}
```

---

## 7. 使用示例

### 7.1 基本用法

```rust
use eneros_hal::{hal, HalError};

fn timer_example() -> Result<(), HalError> {
    // 读取当前时间（纳秒）
    let ns = hal().clock().now_ns();
    println!("当前时间: {} ns", ns);

    // 读取时钟频率
    let freq = hal().clock().frequency_hz();
    println!("时钟频率: {} Hz", freq);

    // 设置 1ms 后的定时器
    hal().clock().set_deadline(1_000_000)?;

    Ok(())
}
```

### 7.2 定时器中断处理

```rust
use eneros_hal::{hal, IrqAction, IrqTrigger, HalError};

/// 定时器中断号（PPI 14 = IRQ 30）
const TIMER_IRQ: u32 = 30;

/// 定时器中断 handler。
fn timer_irq_handler(_irq: u32) -> IrqAction {
    // 处理定时器到期事件
    // 例如：调度器时间片切换、超时检查、心跳计数等

    // 重新设置下一次定时器截止（周期性定时）
    // 设置 10ms 后再次触发
    hal().clock().set_deadline(10_000_000).ok();

    IrqAction::Handled
}

/// 初始化定时器服务。
fn init_timer_service() -> Result<(), HalError> {
    // 注册定时器中断 handler
    hal().irq().register(TIMER_IRQ, IrqTrigger::Level, timer_irq_handler)?;

    // 使能 IRQ 30
    hal().irq().enable(TIMER_IRQ);

    // 设置首次定时器截止（1ms 后）
    hal().clock().set_deadline(1_000_000)?;

    Ok(())
}
```

### 7.3 时间测量

```rust
use eneros_hal::hal;

fn benchmark_example() {
    let start = hal().clock().now_ns();

    // 执行被测操作
    do_something();

    let end = hal().clock().now_ns();
    let elapsed_ns = end.saturating_sub(start);

    println!("耗时: {} ns ({} us)", elapsed_ns, elapsed_ns / 1000);
}
```

### 7.4 超时等待

```rust
use eneros_hal::hal;

fn wait_with_timeout(timeout_ns: u64) -> bool {
    let deadline_ns = hal().clock().now_ns().saturating_add(timeout_ns);

    loop {
        // 检查条件是否满足
        if condition_met() {
            return true;
        }

        // 检查是否超时
        if hal().clock().now_ns() >= deadline_ns {
            return false;
        }

        // 短暂休眠（WFI 等待中断）
        hal().cpu().wfi();
    }
}
```

---

## 8. 参考

### 8.1 规范文档

| 文档 | 编号 | 说明 |
|------|------|------|
| ARM Architecture Reference Manual (ARMv8) | ARM DDI 0487 | ARMv8 架构手册，含 Generic Timer 完整定义（Chapter D11/D13） |
| ARM Generic Interrupt Controller v3 Architecture Specification | ARM IHI 0069 | GICv3 规范（PPI 14 中断定义） |
| ARMv8-A Architecture Reference Manual — System Timer | — | Generic Timer 设计说明 |

### 8.2 QEMU 参考

| 资源 | 说明 |
|------|------|
| QEMU virt machine 文档 | 默认 Generic Timer 频率 62.5MHz |
| `board/qemu-virt/dts` | EnerOS QEMU virt 设备树（timer 节点定义中断映射） |

### 8.3 设备树中断映射

`board/qemu-virt/dts` 中的 timer 节点定义了 Generic Timer 的四个 PPI 中断：

```
timer {
    compatible = "arm,armv8-timer", "arm,armv7-timer";
    interrupts = <1 13 11>,   // PPI 13 = IRQ 29 (Secure Physical Timer)
                 <1 14 11>,   // PPI 14 = IRQ 30 (Non-secure Physical Timer)
                 <1 11 11>,   // PPI 11 = IRQ 27 (Virtual Timer)
                 <1 10 11>;   // PPI 10 = IRQ 26 (Hypervisor Timer)
    always-on;
};
```

> EnerOS 使用 IRQ 30（PPI 14，Non-secure Physical Timer），对应设备树的 `<1 14 11>` 条目。

### 8.4 EnerOS 内部参考

| 文档 | 说明 |
|------|------|
| `docs/hal-interface-spec.md` | HAL trait 接口规范（v0.5.0，`HalClock` trait 定义） |
| `docs/gicv3-driver-guide.md` | GICv3 驱动说明（IRQ 30 注册与使能） |
| `.trae/specs/develop-v060-hal-arm64-core/spec.md` | v0.6.0 实现规格（Generic Timer 设计要求） |
| `hal/src/arm64/timer.rs` | ARM Generic Timer 驱动源码（`Arm64Timer` 实现） |

---

> 本文档是 EnerOS ARM Generic Timer 驱动的权威参考。寄存器定义或频率变更需同步更新本文档并升级版本号。

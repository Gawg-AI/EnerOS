# HAL 接口规范

> 版本: v0.5.0
> 依据: 蓝图 phase0.md §v0.5.0
> crate: eneros-hal

---

## 1. 概述

`eneros-hal` 是 EnerOS 的硬件抽象层（Hardware Abstraction Layer）契约 crate。它在 v0.5.0 作为纯设计版本交付，定义完整的 HAL trait 接口规范集，不包含任何具体硬件实现。本 crate 的作用是：

- 为硬件与内核之间建立稳定的契约层，隔离架构相关细节
- 为后续 v0.6.0（ARM64 核心实现）与 v0.7.0（ARM64 外设实现）提供统一的实现目标
- 为未来飞腾/鲲鹏/RISC-V 的 BSP（Board Support Package）实现提供可插拔接口

在 EnerOS 架构中，HAL 位于 seL4 微内核之上、runtime/kernel 之下。上层组件通过 `hal()` 获取全局 `HalProvider` 引用，再分别访问 CPU、内存、中断、时钟、串口、GPIO 六个子系统。本 crate 遵循蓝图 §43.1 的 no_std 硬性要求，仅依赖 `core`，不依赖 `alloc` 或 `std`。

本规范覆盖所有 6 个 trait 的每个方法契约，是 HAL 实现者与调用方的权威参考。

---

## 2. 设计目标

| 目标 | 说明 |
|------|------|
| trait 抽象 | 用 Rust trait 描述硬件能力，编译期检查实现完整性 |
| dyn 安全 | 所有 trait 方法满足 trait object 限制，可作 `&'static dyn HalXxx` 使用 |
| no_std 兼容 | 全 crate `#![no_std]`，仅依赖 `core`，不引入 `alloc` |
| BSP 可插拔 | 新硬件只需 impl 6 个 trait + `HalProvider`，无需修改上层代码 |
| 最小必要 | 仅定义蓝图明确要求的方法，避免接口过度设计 |
| 同步语义 | 不使用 `async fn`（蓝图 §8.5 指出 no_std trait async 不稳定） |

---

## 3. 公共类型

本 crate 的公共类型定义在 `hal/src/types.rs`，被所有 HAL trait 共享。

### 3.1 MemFlags

| 字段 | 类型 | 说明 |
|------|------|------|
| `readable` | `bool` | 读访问许可 |
| `writable` | `bool` | 写访问许可 |
| `executable` | `bool` | 执行许可 |
| `device` | `bool` | 设备内存（ARM Device-nGnRE），隐含非缓存 |
| `cacheable` | `bool` | 可缓存（Normal memory, write-back） |

- 定义：`pub struct MemFlags { ... }`
- 派生：`Clone, Copy, Debug`
- 用途：`HalMem::map` 的内存属性参数

便捷构造方法（均为 `const fn`，可在常量上下文使用）：

| 方法 | 组合 | 典型场景 |
|------|------|----------|
| `MemFlags::device()` | R + W, 非缓存, 非执行, device=true | MMIO 寄存器映射 |
| `MemFlags::normal()` | R + W, 缓存, 非执行 | 内核数据段、堆 |
| `MemFlags::code()` | R, 缓存, 执行 | 代码段、跳板页 |

也可直接结构体字面量构造自定义组合，例如只读缓存数据页。

### 3.2 IrqTrigger

| 变体 | 说明 |
|------|------|
| `Edge` | 边沿触发（上升沿/下降沿，由 BSP 决定） |
| `Level` | 电平触发（高电平/低电平有效，由 BSP 决定） |

- 定义：`pub enum IrqTrigger { Edge, Level }`
- 派生：`Clone, Copy, Debug, PartialEq, Eq`
- 用途：`HalIrq::register` 的触发类型参数

### 3.3 HalError

统一错误码枚举，所有返回 `Result` 的 HAL 方法共用。

| 变体 | 含义 | 何时返回 |
|------|------|----------|
| `InvalidParam` | 无效参数 | 参数越界、空切片、非法地址对齐等 |
| `OutOfResource` | 资源耗尽 | IRQ 表已满、页表项耗尽等 |
| `NotSupported` | 不支持 | 当前 BSP 不实现该能力 |
| `HardwareFault` | 硬件故障 | 设备未响应、总线错误等 |
| `PermissionDenied` | 权限不足 | 调用方缺少特权或 capability |

- 定义：`pub enum HalError { ... }`
- 派生：`Debug`（不派生 `PartialEq`，测试用 `matches!()` 匹配变体）
- Display 实现：各变体对应小写字符串（如 `InvalidParam` → `"invalid parameter"`）
- 用途：所有 `Result` 返回型 HAL 方法的错误类型

### 3.4 GpioDir

| 变体 | 说明 |
|------|------|
| `Input` | 输入引脚 |
| `Output` | 输出引脚 |

- 定义：`pub enum GpioDir { Input, Output }`
- 派生：`Clone, Copy, Debug, PartialEq, Eq`
- 用途：`GpioConfig.dir` 字段

### 3.5 PullMode

| 变体 | 说明 |
|------|------|
| `None` | 无上下拉 |
| `Up` | 上拉 |
| `Down` | 下拉 |

- 定义：`pub enum PullMode { None, Up, Down }`
- 派生：`Clone, Copy, Debug, PartialEq, Eq`
- 用途：`GpioConfig.pull` 字段

### 3.6 GpioConfig

| 字段 | 类型 | 说明 |
|------|------|------|
| `pin` | `u32` | 引脚编号 |
| `dir` | `GpioDir` | 方向 |
| `pull` | `PullMode` | 上下拉模式 |

- 定义：`pub struct GpioConfig { ... }`
- 派生：`Clone, Copy`（不派生 `Debug`，保持精简）
- 用途：`HalGpio::set_dir` 的配置参数

### 3.7 IrqAction

中断处理函数的返回值，指示后续动作。

| 变体 | 说明 |
|------|------|
| `Handled` | 中断已处理完毕 |
| `WakeThread` | 唤醒一个等待线程 |
| `Disabled` | 禁用该中断（建议 BSP 调用 `disable(irq)`） |

- 定义：`pub enum IrqAction { Handled, WakeThread, Disabled }`
- 派生：`Debug, PartialEq, Eq`
- 用途：`IrqHandler` 返回值

### 3.8 IrqHandler

```rust
pub type IrqHandler = fn(irq: u32) -> IrqAction;
```

- 类型别名：裸函数指针（非 `Box<dyn Fn>`，无需 alloc）
- 约束：必须为 `fn` 指针，不能是闭包（保证 no_std 无堆依赖）
- 用途：`HalIrq::register` 的处理函数参数

---

## 4. HAL Trait 规范

### 4.1 HalCpu

CPU 核心控制 trait，提供中断屏蔽、核号查询、低功耗管理。

```rust
pub trait HalCpu {
    fn enable_irq(&self);
    fn disable_irq(&self);
    fn current_core(&self) -> u32;
    fn core_count(&self) -> u32;
    fn halt(&self) -> !;
    fn wfi(&self);
}
```

| 方法 | 签名 | 参数 | 返回值 | 错误 | 调用约束 | 线程安全 |
|------|------|------|--------|------|----------|----------|
| `enable_irq` | `fn(&self)` | 无 | `()` | 无 | 需特权；解除 DAIF 屏蔽 | 可在任意上下文调用 |
| `disable_irq` | `fn(&self)` | 无 | `()` | 无 | 需特权；屏蔽 DAIF | 可在任意上下文调用 |
| `current_core` | `fn(&self) -> u32` | 无 | 当前核 ID（0 起索引） | 无 | 无特权要求；只读寄存器 | 只读，线程安全 |
| `core_count` | `fn(&self) -> u32` | 无 | 核总数 | 无 | 无特权要求；启动后恒定 | 只读，线程安全 |
| `halt` | `fn(&self) -> !` | 无 | 永不返回 | 无 | 需特权；用于致命错误兜底 | 调用后核停止 |
| `wfi` | `fn(&self)` | 无 | `()` | 无 | 需特权；进入低功耗等待中断 | 中断到达后唤醒继续 |

说明：
- `enable_irq`/`disable_irq` 操作 ARM64 的 DAIF 异常屏蔽寄存器
- `wfi`（Wait For Interrupt）在收到中断后自动唤醒，不改变中断屏蔽状态
- `halt` 标记为发散函数 `!`，调用方无需处理返回

### 4.2 HalMem

内存管理 trait，提供虚拟内存映射、解映射、地址翻译、保护域设置。

```rust
pub trait HalMem {
    fn map(&self, pa: u64, va: u64, flags: MemFlags) -> Result<(), HalError>;
    fn unmap(&self, va: u64) -> Result<(), HalError>;
    fn translate(&self, va: u64) -> Option<u64>;
    fn set_domain(&self, va: u64, domain: u32) -> Result<(), HalError>;
}
```

| 方法 | 签名 | 参数 | 返回值 | 可能错误 | 调用约束 | 线程安全 |
|------|------|------|--------|----------|----------|----------|
| `map` | `fn(&self, pa, va, flags)` | `pa`: 物理地址；`va`: 虚拟地址；`flags`: 属性 | `Ok(())` 或错误 | `InvalidParam`, `OutOfResource`, `PermissionDenied` | 需特权；修改页表 | 非线程安全，需外部同步 |
| `unmap` | `fn(&self, va)` | `va`: 虚拟地址 | `Ok(())` 或错误 | `InvalidParam`, `PermissionDenied` | 需特权；修改页表 | 非线程安全，需外部同步 |
| `translate` | `fn(&self, va) -> Option<u64>` | `va`: 虚拟地址 | `Some(pa)` 或 `None`（未映射） | 无（返回 Option） | 无特权要求；只读页表 | 只读，线程安全 |
| `set_domain` | `fn(&self, va, domain)` | `va`: 虚拟地址；`domain`: 域编号 | `Ok(())` 或错误 | `InvalidParam`, `NotSupported`, `PermissionDenied` | 需特权；修改页表域字段 | 非线程安全，需外部同步 |

说明：
- `map` 不指定页大小，由 BSP 冺定（通常 4KB 或 2MB）
- `pa`/`va` 应按页对齐，未对齐返回 `InvalidParam`
- `set_domain` 用于 ARMv8 的页表域保护（Domains），部分 BSP 可能返回 `NotSupported`
- 修改页表的方法需调用方在外部加锁（如 `spin::Mutex`）

### 4.3 HalIrq

中断控制器 trait，提供中断注册、注销、使能、禁用、EOI。

```rust
pub trait HalIrq {
    fn register(&self, irq: u32, trigger: IrqTrigger, handler: IrqHandler) -> Result<(), HalError>;
    fn unregister(&self, irq: u32) -> Result<(), HalError>;
    fn enable(&self, irq: u32);
    fn disable(&self, irq: u32);
    fn eoi(&self, irq: u32);
}
```

| 方法 | 签名 | 参数 | 返回值 | 可能错误 | 调用约束 | 线程安全 |
|------|------|------|--------|----------|----------|----------|
| `register` | `fn(&self, irq, trigger, handler)` | `irq`: 中断号；`trigger`: 触发类型；`handler`: 处理函数 | `Ok(())` 或错误 | `InvalidParam`, `OutOfResource`, `PermissionDenied` | 需特权；写入 IRQ 表 | 非线程安全，需外部同步 |
| `unregister` | `fn(&self, irq)` | `irq`: 中断号 | `Ok(())` 或错误 | `InvalidParam`, `PermissionDenied` | 需特权；清空 IRQ 表项 | 非线程安全，需外部同步 |
| `enable` | `fn(&self, irq)` | `irq`: 中断号 | `()` | 无 | 需特权；GIC 使能 | 非线程安全（GIC 寄存器需原子或锁） |
| `disable` | `fn(&self, irq)` | `irq`: 中断号 | `()` | 无 | 需特权；GIC 屏蔽 | 非线程安全（同上） |
| `eoi` | `fn(&self, irq)` | `irq`: 中断号 | `()` | 无 | 需特权；中断上下文内调用 | 仅在中断处理中调用 |

说明：
- `register` 对同一 `irq` 重复注册应返回 `InvalidParam` 或覆盖（BSP 决定，建议返回错误）
- `handler` 是裸函数指针，不可为闭包（避免堆分配）
- `eoi`（End Of Interrupt）必须在中断处理函数返回前调用，否则该中断无法再次触发
- `enable`/`disable` 不返回 `Result`，因 GIC 操作一般不会失败；无效 `irq` 的行为由 BSP 决定（建议 panic 或忽略）

### 4.4 HalClock

时钟与定时器 trait，提供单调纳秒时钟、频率查询、定时截止设置。

```rust
pub trait HalClock {
    fn now_ns(&self) -> u64;
    fn frequency_hz(&self) -> u64;
    fn set_deadline(&self, ns: u64) -> Result<(), HalError>;
}
```

| 方法 | 签名 | 参数 | 返回值 | 可能错误 | 调用约束 | 线程安全 |
|------|------|------|--------|----------|----------|----------|
| `now_ns` | `fn(&self) -> u64` | 无 | 当前单调时间（纳秒） | 无 | 无特权要求；读 ARM64 CNTVCT_EL0 | 只读，线程安全 |
| `frequency_hz` | `fn(&self) -> u64` | 无 | 时钟频率（Hz） | 无 | 无特权要求；读 CNTFRQ_EL0 | 只读，线程安全 |
| `set_deadline` | `fn(&self, ns)` | `ns`: 截止时间（纳秒，绝对值） | `Ok(())` 或错误 | `InvalidParam`, `NotSupported`, `PermissionDenied` | 需特权；写定时器比较寄存器 | 非线程安全，需外部同步 |

说明：
- `now_ns` 返回单调递增值，不回退，基于 ARM64 Generic Timer 的虚拟计数器
- `frequency_hz` 通常为 24MHz 或 62.5MHz（依平台）
- `set_deadline` 的 `ns` 为绝对时间戳，非相对偏移；到期后触发定时器中断
- 同一核只有一个定时器，多次 `set_deadline` 覆盖前一次

### 4.5 HalSerial

串口 trait，提供字节级 I/O（如 UART）。

```rust
pub trait HalSerial {
    fn write(&self, data: &[u8]) -> Result<usize, HalError>;
    fn read(&self, buf: &mut [u8]) -> Result<usize, HalError>;
    fn flush(&self) -> Result<(), HalError>;
}
```

| 方法 | 签名 | 参数 | 返回值 | 可能错误 | 调用约束 | 线程安全 |
|------|------|------|--------|----------|----------|----------|
| `write` | `fn(&self, data)` | `data`: 待写入字节切片 | `Ok(n)` 已写字节数，或错误 | `InvalidParam`, `HardwareFault` | 可在任意上下文（含中断）；阻塞至 FIFO 有空间 | 非线程安全，需外部同步 |
| `read` | `fn(&self, buf)` | `buf`: 接收缓冲区 | `Ok(n)` 已读字节数，或错误 | `InvalidParam`, `HardwareFault` | 可在任意上下文；阻塞至有数据或超时 | 非线程安全，需外部同步 |
| `flush` | `fn(&self)` | 无 | `Ok(())` 或错误 | `HardwareFault` | 可在任意上下文；等待发送 FIFO 排空 | 非线程安全，需外部同步 |

说明：
- `write` 返回实际写入字节数，可能小于 `data.len()`（FIFO 满）
- `read` 返回实际读取字节数，可能小于 `buf.len()`（无更多数据）
- 串口通常无并发保护，多核同时访问需上层加锁
- 早期启动阶段（无堆、无锁）可单核独占使用

### 4.6 HalGpio

GPIO trait，提供引脚方向配置与读/写/翻转。

```rust
pub trait HalGpio {
    fn set_dir(&self, config: GpioConfig) -> Result<(), HalError>;
    fn set(&self, pin: u32, val: bool) -> Result<(), HalError>;
    fn get(&self, pin: u32) -> Result<bool, HalError>;
    fn toggle(&self, pin: u32) -> Result<(), HalError>;
}
```

| 方法 | 签名 | 参数 | 返回值 | 可能错误 | 调用约束 | 线程安全 |
|------|------|------|--------|----------|----------|----------|
| `set_dir` | `fn(&self, config)` | `config`: 引脚配置（pin/dir/pull） | `Ok(())` 或错误 | `InvalidParam`, `PermissionDenied` | 需特权；配置 GPIO 寄存器 | 非线程安全，需外部同步 |
| `set` | `fn(&self, pin, val)` | `pin`: 引脚号；`val`: true=高，false=低 | `Ok(())` 或错误 | `InvalidParam` | 需特权；引脚需先设为 Output | 非线程安全，需外部同步 |
| `get` | `fn(&self, pin) -> Result<bool>` | `pin`: 引脚号 | `Ok(true/false)` 或错误 | `InvalidParam` | 无特权要求；读引脚电平 | 只读，线程安全 |
| `toggle` | `fn(&self, pin)` | `pin`: 引脚号 | `Ok(())` 或错误 | `InvalidParam` | 需特权；原子翻转（若硬件支持） | 非线程安全，需外部同步 |

说明：
- 引脚号 `pin` 由 BSP 定义，通常对应 SoC GPIO 控制器的引脚编号
- `set` 前未调用 `set_dir` 设为 Output 的行为由 BSP 决定（建议返回 `InvalidParam` 或忽略）
- `toggle` 应优先使用硬件原子翻转寄存器（如 PL061 的 GPIOBIT）

---

## 5. HalProvider 注册器模式

### 5.1 设计

`HalProvider` trait 是 BSP 的注入点，聚合全部 6 个子系统：

```rust
pub trait HalProvider {
    fn cpu(&self) -> &'static dyn HalCpu;
    fn mem(&self) -> &'static dyn HalMem;
    fn irq(&self) -> &'static dyn HalIrq;
    fn clock(&self) -> &'static dyn HalClock;
    fn serial(&self) -> &'static dyn HalSerial;
    fn gpio(&self) -> &'static dyn HalGpio;
}
```

全局单例存储于 `static mut HAL: Option<&'static dyn HalProvider>`。

### 5.2 init_hal 契约

```rust
pub fn init_hal(provider: &'static dyn HalProvider);
```

- 调用时机：启动早期，调度器启动前，单线程 boot 上下文
- 调用次数：必须且仅能调用一次（write-once 语义，重复调用覆盖前值但不推荐）
- 安全性：内部使用 `unsafe` 写 `static mut`，契约要求调用方保证单线程
- 后续：调用后 `hal()` 才可安全使用

### 5.3 hal 契约

```rust
pub fn hal() -> &'static dyn HalProvider;
```

- 调用时机：`init_hal` 之后任意时刻
- 返回：全局 HAL provider 的 `&'static` 引用
- Panic 条件：若 `init_hal` 未调用，`HAL` 为 `None`，`expect()` 触发 panic，提示 `"HAL not initialized: call init_hal() during boot first"`
- 安全性：内部 `unsafe` 读 `static mut`，因 write-once 后只读，无数据竞争

### 5.4 典型使用流程

```rust
// BSP 启动早期
static PROVIDER: MyBspProvider = MyBspProvider::new();
init_hal(&PROVIDER);

// 上层调用
hal().cpu().enable_irq();
hal().serial().write(b"boot ok\n")?;
let core = hal().cpu().current_core();
```

---

## 6. 错误码汇总

| 错误码 | 含义 | 何时返回 | 建议处理方式 |
|--------|------|----------|--------------|
| `InvalidParam` | 无效参数 | 参数越界、地址未对齐、空切片、重复注册 | 调用方修正参数后重试 |
| `OutOfResource` | 资源耗尽 | IRQ 表满、页表项耗尽 | 资源回收后重试或返回错误 |
| `NotSupported` | 不支持 | 当前 BSP 不实现该能力 | 降级路径或返回错误 |
| `HardwareFault` | 硬件故障 | 设备未响应、总线错误 | 记录日志、隔离设备 |
| `PermissionDenied` | 权限不足 | 调用方缺少特权或 capability | 拒绝调用，审计记录 |

`HalError` 实现了 `core::fmt::Display`，可被 `unwrap()`/`expect()` 打印，也可用于 `format!`。未派生 `PartialEq`，测试时用 `matches!(err, HalError::NotSupported)` 匹配变体。

---

## 7. 与 v0.6.0/v0.7.0 实现的对接

### 7.1 v0.6.0：ARM64 核心实现

v0.6.0 将实现以下 trait（蓝图 phase0.md §v0.6.0）：

| Trait | 实现依据 | 说明 |
|-------|----------|------|
| `HalCpu` | ARM64 指令（MSR DAIFSet/Clr, MRS MPIDR_EL1） | enable_irq/disable_irq/current_core/core_count/halt/wfi |
| `HalIrq` | GICv3（GICD/GICR/Redistributor） | register/unregister/enable/disable/eoi |
| `HalClock` | ARM64 Generic Timer（CNTVCT_EL0, CNTFRQ_EL0, CNTV_CVAL_EL0） | now_ns/frequency_hz/set_deadline |

v0.6.0 还需实现 `HalProvider` 的真实 BSP，并在启动早期调用 `init_hal()`。

### 7.2 v0.7.0：ARM64 外设实现

v0.7.0 将实现以下 trait（蓝图 phase0.md §v0.7.0）：

| Trait | 实现依据 | 说明 |
|-------|----------|------|
| `HalMem` | ARM64 页表（EL2/EL1 Stage 1/2） | map/unmap/translate/set_domain |
| `HalSerial` | PL011 UART（QEMU virt 默认） | write/read/flush |
| `HalGpio` | SoC GPIO 控制器（依平台） | set_dir/set/get/toggle |

### 7.3 BSP 接入要求

BSP 实现者需：

1. 为目标硬件实现所需的 6 个 trait
2. 实现 `HalProvider`，返回各 trait 的 `&'static dyn` 引用
3. 在启动早期（调度器启动前）调用 `init_hal(&provider)`
4. 确保所有 `&'static` 引用的对象具有 `static` 生命周期（通常用 `static` 变量）
5. 编译期通过 `cargo build -p eneros-hal` 验证 trait 实现完整性

---

## 8. 调用约束与安全说明

### 8.1 特权层级

| 方法类别 | 特权要求 | 说明 |
|----------|----------|------|
| `HalCpu::enable_irq/disable_irq/halt/wfi` | 需特权 | 操作系统级寄存器 |
| `HalMem::map/unmap/set_domain` | 需特权 | 修改页表 |
| `HalMem::translate` | 无特权 | 只读页表 |
| `HalIrq::register/unregister/enable/disable/eoi` | 需特权 | 操作 GIC |
| `HalClock::set_deadline` | 需特权 | 写定时器比较寄存器 |
| `HalClock::now_ns/frequency_hz` | 无特权 | 只读计数器 |
| `HalSerial::write/read/flush` | 无特权 | MMIO（需映射后访问） |
| `HalGpio::set_dir/set/toggle` | 需特权 | 配置 GPIO |
| `HalGpio::get` | 无特权 | 读引脚电平 |

### 8.2 中断上下文可调用性

| 方法 | 中断上下文 | 说明 |
|------|------------|------|
| `HalSerial::write/read/flush` | 可调用 | 早期启动、调试输出常用 |
| `HalClock::now_ns` | 可调用 | 只读，无副作用 |
| `HalIrq::eoi` | 必须在中断上下文调用 | 中断处理返回前 |
| `HalMem::map/unmap` | 不建议 | 可能阻塞、修改全局状态 |
| `HalCpu::wfi` | 不可调用 | 会导致嵌套中断异常 |
| 其他需特权方法 | 视情况 | 避免长时间阻塞 |

### 8.3 并发约束

- 所有写操作（map/unmap/register/enable/disable/set_dir/set/toggle/set_deadline）默认非线程安全，需上层用 `spin::Mutex` 或类似机制同步
- 所有只读操作（current_core/core_count/translate/now_ns/frequency_hz/get）线程安全，可并发调用
- `static mut HAL` 的 `unsafe` 限于 `init_hal`（write-once）与 `hal()`（read-after-init）两处，单线程 boot 上下文写入后只读访问，无数据竞争

### 8.4 no_std 合规

- 全 crate `#![cfg_attr(not(test), no_std)]`：正式构建 no_std，host 测试启用 std（用于 `format!`/`println!`）
- 不依赖 `alloc`：`IrqHandler` 用函数指针而非 `Box<dyn Fn>`
- 不使用 `async fn`：蓝图 §8.5 指出 no_std trait async 不稳定，全部同步签名

---

> 本规范是 HAL 实现者与调用方的权威契约。任何 trait 方法签名变更需同步更新本文档并升级版本号。

# GPIO 使用

> 版本：v0.7.0
> 适用范围：EnerOS HAL ARM64 GPIO 控制器驱动
> 蓝图依据：`蓝图/phase0.md` §v0.7.0、§4.5
> crate：eneros-hal（`hal/arm64/src/gpio.rs`）
> 硬件参考：ARM Generic GPIO Controller、SoC 厂商 datasheet
> 接口规范：`docs/hal-interface-spec.md` §4.6 HalGpio

---

## 1. 概述

GPIO（General Purpose Input/Output）是 SoC 上最基础的数字 I/O 接口，用于连接 LED、按键、继电器、传感器数字信号、外设复位/片选等。EnerOS 在 v0.7.0 实现 `HalGpio` trait 的 ARM64 通用 GPIO 控制器驱动，提供方向配置、上下拉配置、电平读/写/翻转能力，支撑后续版本（v0.13.0 看门狗、v0.18.0 Modbus DE/RE 方向控制等）对引脚级控制的需求。

### 1.1 通用寄存器模型

不同 SoC 厂商的 GPIO 控制器寄存器布局差异较大（PL061、Intel GPIO、Allwinner、飞腾自研等），但都包含三类核心寄存器：

| 功能 | 通用寄存器 | EnerOS 抽象偏移 | 说明 |
|------|-----------|-----------------|------|
| 方向控制 | DIR | `0x04` | 1=Output，0=Input；每 bit 对应一个 pin |
| 数据读写 | DATA | `0x40` | 读返回引脚电平，写设置输出电平；每 bit 对应一个 pin |
| 上下拉配置 | PUD | `0x94` | 每 16 个 pin 一组，每组 2 bit 配置上拉/下拉/无 |

EnerOS v0.7.0 采用"通用偏移"模型，假设目标 SoC 的 GPIO 控制器遵循此布局（QEMU virt 无真实 GPIO，需真机或扩展 QEMU 验证；详见 §8）。真机移植时若偏移不同，只需修改 `gpio.rs` 中的常量即可，上层代码无需改动。

### 1.2 在 EnerOS 中的位置

```
┌─────────────────────────────────────────┐
│   上层：watchdog / modbus / 板级 BSP    │
└──────────────────┬──────────────────────┘
                   │ hal().gpio().set(pin, true)
┌──────────────────▼──────────────────────┐
│   hal crate — HalGpio trait             │
│   docs/hal-interface-spec.md §4.6       │
└──────────────────┬──────────────────────┘
                   │ impl HalGpio
┌──────────────────▼──────────────────────┐
│   hal/arm64/src/gpio.rs                 │
│   Arm64Gpio { base, pin_count }         │
└──────────────────┬──────────────────────┘
                   │ MMIO（read_volatile/write_volatile）
┌──────────────────▼──────────────────────┐
│   GPIO 控制器 @ 0x0902_0000             │
└─────────────────────────────────────────┘
```

### 1.3 关键特性

| 特性 | 说明 |
|------|------|
| 引脚数量 | 构造时指定（默认 32），支持 1–32 pin |
| 方向控制 | 每 pin 独立配置 Output/Input |
| 上下拉 | 每 16 pin 一组，支持 None/Up/Down |
| 数据访问 | 单 pin 读写，bit 掩码操作 |
| 越界保护 | `pin ≥ pin_count` 返回 `HalError::InvalidParam` |
| 原子性 | 单寄存器读改写，非多核原子；多核并发需上层加锁 |

> **v0.7.0 实现策略**：寄存器级访问，无中断支持（GPIO 中断留待 v0.6.0 GICv3 配合实现）。`toggle` 通过 read-modify-write 实现，非硬件原子（部分 SoC 有原子翻转寄存器，可后续优化）。

---

## 2. 寄存器参考

### 2.1 寄存器总表

所有寄存器均为 32-bit，按字访问。偏移量相对于 GPIO 控制器基址（蓝图定义为 `0x0902_0000`，见 §8）。

| 偏移 | 名称 | 访问 | 说明 |
|------|------|------|------|
| `0x00` | — | — | 保留（部分 SoC 用于配置寄存器，EnerOS 不使用） |
| `0x04` | DIR | RW | Direction Register：方向控制，bit n 对应 pin n；1=Output，0=Input |
| `0x08`–`0x3C` | — | — | 保留 |
| `0x40` | DATA | RW | Data Register：读返回引脚电平，写设置输出电平；bit n 对应 pin n |
| `0x44`–`0x90` | — | — | 保留 |
| `0x94` | PUD | RW | Pull-up/down Register：上下拉配置，每 16 pin 一组，每组 2 bit |
| `0x98`+ | — | — | 后续 PUD 组（PUD + (pin/16) × 4），支持更多 pin 时使用 |

> **多 DATA 寄存器说明**：部分 SoC 的 DATA 寄存器按 32 pin 分组（即 pin 0–31 用 `0x40`，pin 32–63 用 `0x44`...）。EnerOS v0.7.0 实现 `set` 时考虑此布局（`GPIO_DATA + (pin/32)*4`），但 `get` 只读 `0x40`（默认 pin_count ≤ 32，单寄存器足够）。

### 2.2 DIR 寄存器位定义

| 位 | 名称 | 值 | 含义 |
|----|------|-----|------|
| bit n | DIRn | `1 << n` | 方向控制：1=Output（输出，可写 DATA 驱动电平），0=Input（输入，可读 DATA 获取电平） |

**复位默认值**：通常为 `0x0`（全部 Input），但 SoC 上电后实际状态可能不确定——蓝图 v0.7.0 §8.5 明确要求"GPIO 上电默认状态不确定，需显式初始化"。

### 2.3 DATA 寄存器位定义

| 位 | 名称 | 值 | 含义 |
|----|------|-----|------|
| bit n | DATAn | `1 << n` | 引脚电平：1=高电平（VCC），0=低电平（GND） |

**读写语义**：
- **读 DATA**：返回所有 pin 的当前电平。无论方向是 Input 还是 Output，都能读到真实电平（Output 模式下读回的是驱动值）。
- **写 DATA**：仅对方向为 Output 的 pin 生效。写 Input pin 通常被硬件忽略（依 SoC 实现）。
- **写策略**：EnerOS v0.7.0 采用"mask + val"策略，先读后改后写（read-modify-write），避免误改其他 pin。

> **注意**：部分 SoC（如 PL061）使用"位掩码地址"模型——写入 `DATA + (mask << 2)` 只修改 mask 指定的 bit。EnerOS v0.7.0 不采用此模型，使用通用 read-modify-write。

### 2.4 PUD 寄存器布局

PUD 寄存器以 16 pin 为一组，每组占 2 bit：

| pin 范围 | 寄存器偏移 | bit 范围 |
|----------|-----------|----------|
| pin 0–15 | `PUD + 0` (= `0x94`) | bit 0–1（pin 0）、bit 2–3（pin 1）、...、bit 30–31（pin 15） |
| pin 16–31 | `PUD + 4` (= `0x98`) | bit 0–1（pin 16）、bit 2–3（pin 17）、...、bit 30–31（pin 31） |
| pin 32–47 | `PUD + 8` (= `0x9C`) | bit 0–1（pin 32）、... |

**每组 2 bit 的值定义**：

| 值 | 模式 | 说明 |
|----|------|------|
| `0b00` | None | 无上下拉，引脚浮空（高阻态） |
| `0b01` | Pull-up | 上拉到 VCC（典型 10kΩ–100kΩ），适用于按键接 GND 场景 |
| `0b10` | Pull-down | 下拉到 GND，适用于按键接 VCC 场景 |
| `0b11` | Reserved | 保留，行为未定义，禁止使用 |

**地址计算公式**：

```
pud_reg_offset = 0x94 + (pin / 16) * 4
pud_bit_shift  = (pin % 16) * 2
pud_value      = (reg >> pud_bit_shift) & 0x3
```

---

## 3. 方向配置

### 3.1 配置规则

| 方向 | DIR bit 操作 | 说明 |
|------|-------------|------|
| Output | `dir \|= 1 << pin`（置位） | 输出模式，可写 DATA 驱动电平 |
| Input | `dir &= !(1 << pin)`（清零） | 输入模式，可读 DATA 获取电平 |

### 3.2 代码示例

```rust
//! hal/arm64/src/gpio.rs — 方向配置

const GPIO_DIR: u64 = 0x04;

impl Arm64Gpio {
    /// 设置单个 pin 的方向（内部辅助函数）
    ///
    /// 调用前应已校验 pin < pin_count
    #[inline]
    unsafe fn set_dir_raw(&self, pin: u32, is_output: bool) {
        let mut dir = self.r32(GPIO_DIR);
        if is_output {
            dir |= 1u32 << pin;
        } else {
            dir &= !(1u32 << pin);
        }
        self.w32(GPIO_DIR, dir);
    }
}
```

### 3.3 应用场景

| 场景 | 方向 | 上下拉 | 说明 |
|------|------|--------|------|
| LED 驱动 | Output | None | 写 1 点亮，写 0 熄灭（或反之，依电路） |
| 按键检测 | Input | Up（按键接 GND） | 默认高电平，按下时拉低 |
| 按键检测 | Input | Down（按键接 VCC） | 默认低电平，按下时拉高 |
| 继电器控制 | Output | None | 写 1 吸合，写 0 释放 |
| 外设复位 | Output | None | 写 0 复位，写 1 释放（或反之） |
| 外设就绪信号 | Input | None/Up | 读电平判断外设状态 |

---

## 4. 上下拉配置

### 4.1 地址计算

PUD 寄存器按 16 pin 分组，地址计算公式：

```
pud_offset = GPIO_PUD + (pin / 16) * 4
bit_shift  = (pin % 16) * 2
```

### 4.2 值映射

| PullMode 枚举 | 数值 | 说明 |
|---------------|------|------|
| `PullMode::None` | 0 | 无上下拉 |
| `PullMode::Up` | 1 | 上拉 |
| `PullMode::Down` | 2 | 下拉 |

> **枚举数值约定**：`hal-interface-spec.md` §3.5 定义 `PullMode { None, Up, Down }`，Rust 默认枚举判别值为 0/1/2，恰好与 PUD 寄存器值映射一致。`gpio.rs` 中可直接 `config.pull as u32` 转换。

### 4.3 配置流程

```
set_pull(pin, mode):
    pud_offset = GPIO_PUD + (pin / 16) * 4
    bit_shift  = (pin % 16) * 2
    mask       = 0x3 << bit_shift
    val        = (mode as u32) << bit_shift

    reg = read(pud_offset)
    reg = (reg & ~mask) | val       // 清旧值，写新值
    write(pud_offset, reg)
```

### 4.4 代码示例

```rust
//! hal/arm64/src/gpio.rs — 上下拉配置

const GPIO_PUD: u64 = 0x94;

use hal_interface::PullMode;

impl Arm64Gpio {
    /// 设置单个 pin 的上下拉模式
    ///
    /// 调用前应已校验 pin < pin_count
    #[inline]
    unsafe fn set_pull_raw(&self, pin: u32, mode: PullMode) {
        let pud_offset = GPIO_PUD + ((pin / 16) as u64) * 4;
        let bit_shift  = (pin % 16) * 2;
        let mask       = 0x3u32 << bit_shift;
        let val        = (mode as u32) << bit_shift;

        let mut reg = self.r32(pud_offset);
        reg = (reg & !mask) | val;
        self.w32(pud_offset, reg);
    }
}
```

### 4.5 注意事项

- **必须先配置方向再配置上下拉**：部分 SoC 在 Input 模式下上下拉才生效，Output 模式下配置可能被忽略或导致短路。
- **避免 Reserved 值**：`0b11` 是保留值，禁止写入。`PullMode` 枚举无第四个变体，编译期保证不会产生 `0b11`。
- **功耗考虑**：未使用的引脚建议配置为 Input + Pull-down，避免浮空导致 CMOS 噪声功耗。

---

## 5. 数据读写

### 5.1 写操作：set(pin, val)

**算法**：

```
set(pin, val):
    mask = 1 << pin
    val  = val ? mask : 0

    // 方式 1：read-modify-write（EnerOS 默认）
    cur = read(GPIO_DATA)
    cur = (cur & ~mask) | val
    write(GPIO_DATA, cur)

    // 方式 2：直接写（依 SoC 实现，部分硬件只写 mask bit）
    // write(GPIO_DATA + (pin/32)*4, val)
```

**EnerOS 实现**（采用方式 2，直接写值）：

```rust
//! hal/arm64/src/gpio.rs — 写数据

const GPIO_DATA: u64 = 0x40;

impl HalGpio for Arm64Gpio {
    fn set(&self, pin: u32, val: bool) -> Result<(), HalError> {
        if pin >= self.pin_count {
            return Err(HalError::InvalidParam);
        }
        unsafe {
            let mask = 1u32 << pin;
            let v = if val { mask } else { 0 };
            // 多 DATA 寄存器布局：pin 0-31 在 0x40，pin 32-63 在 0x44...
            self.w32(GPIO_DATA + ((pin / 32) as u64) * 4, v);
        }
        Ok(())
    }
}
```

> **设计说明**：蓝图 v0.7.0 §4.5 采用"直接写值"策略——写 `val`（mask 或 0）到对应 DATA 寄存器。此策略假设 SoC 的 DATA 寄存器只更新被 `mask` 指定的 bit，其他 bit 保持不变。若目标 SoC 不支持此语义，应改为 read-modify-write。

### 5.2 读操作：get(pin)

**算法**：

```
get(pin):
    data = read(GPIO_DATA)
    return (data & (1 << pin)) != 0
```

**EnerOS 实现**：

```rust
impl HalGpio for Arm64Gpio {
    fn get(&self, pin: u32) -> Result<bool, HalError> {
        if pin >= self.pin_count {
            return Err(HalError::InvalidParam);
        }
        unsafe {
            let data = self.r32(GPIO_DATA);
            Ok(data & (1u32 << pin) != 0)
        }
    }
}
```

> **读语义**：无论 pin 方向是 Input 还是 Output，读 DATA 都返回当前引脚电平。Output 模式下读回的是驱动值（若电路未强制拉反），Input 模式下读回的是外部输入电平。

### 5.3 翻转操作：toggle(pin)

**算法**：

```
toggle(pin):
    cur = get(pin)       // 读当前电平
    set(pin, !cur)       // 写反值
```

**EnerOS 实现**：

```rust
impl HalGpio for Arm64Gpio {
    fn toggle(&self, pin: u32) -> Result<(), HalError> {
        let cur = self.get(pin)?;
        self.set(pin, !cur)
    }
}
```

> **非原子性警告**：`toggle` 是 read-modify-write，非原子操作。多核同时 `toggle` 同一 pin 可能丢失翻转（典型 lost-update）。若需原子翻转：
> - 部分 SoC 有专用翻转寄存器（如 PL061 的 GPIOBIT）
> - 或上层用 `spin::Mutex` 保护
> - v0.7.0 接受此限制，单核场景足够。

### 5.4 操作汇总

| 操作 | 寄存器 | 算法 | 阻塞性 |
|------|--------|------|--------|
| `set_dir` | DIR + PUD | read-modify-write | 非阻塞，立即返回 |
| `set` | DATA | 直接写 mask 值 | 非阻塞，立即返回 |
| `get` | DATA | 读 + bit 提取 | 非阻塞，立即返回 |
| `toggle` | DATA | get + set（两步） | 非阻塞，但非原子 |

---

## 6. 越界保护

### 6.1 检查规则

所有 `HalGpio` 方法在访问寄存器前必须校验 `pin`：

```rust
if pin >= self.pin_count {
    return Err(HalError::InvalidParam);
}
```

### 6.2 pin_count 的指定

`pin_count` 在构造 `Arm64Gpio` 时指定，运行时不可变：

```rust
pub struct Arm64Gpio {
    pub base: u64,
    pub pin_count: u32,
}

impl Arm64Gpio {
    pub const fn new(base: u64, pin_count: u32) -> Self {
        Self { base, pin_count }
    }
}
```

- **默认值**：32（蓝图 v0.7.0 §3 交付物定义）
- **范围**：1–32（单 DATA 寄存器最多 32 bit）；超过 32 需扩展为多 DATA 寄存器布局
- **真机适配**：飞腾 D2000 GPIO 通常 32 pin/组，鲲鹏 920 可能 16 pin/组，构造时按 datasheet 指定

### 6.3 越界场景示例

```rust
let gpio = Arm64Gpio::new(0x0902_0000, 32);

// 合法：pin 0..31
gpio.set(0, true).ok();       // Ok(())
gpio.set(31, true).ok();      // Ok(())

// 越界：pin >= 32
let r = gpio.set(32, true);
assert_eq!(r, Err(HalError::InvalidParam));

let r = gpio.get(100);
assert_eq!(r, Err(HalError::InvalidParam));
```

---

## 7. EnerOS 实现

### 7.1 Arm64Gpio 结构体设计

```rust
//! hal/arm64/src/gpio.rs

#![allow(dead_code)]

use core::ptr::{read_volatile, write_volatile};
use hal_interface::{HalGpio, HalError, GpioConfig, GpioDir, PullMode};

/// GPIO 寄存器偏移
const GPIO_DIR:  u64 = 0x04;   // 方向寄存器
const GPIO_DATA: u64 = 0x40;   // 数据寄存器
const GPIO_PUD:  u64 = 0x94;   // 上下拉寄存器

/// ARM64 通用 GPIO 控制器实例
pub struct Arm64Gpio {
    /// MMIO 基地址（如 0x0902_0000）
    pub base: u64,
    /// 引脚数量（1–32，默认 32）
    pub pin_count: u32,
}

impl Arm64Gpio {
    /// 构造实例
    ///
    /// # 参数
    /// - `base`：GPIO 控制器 MMIO 基地址
    /// - `pin_count`：引脚数量（通常 8/16/32）
    pub const fn new(base: u64, pin_count: u32) -> Self {
        Self { base, pin_count }
    }
}
```

### 7.2 HalGpio trait 实现

```rust
impl HalGpio for Arm64Gpio {
    fn set_dir(&self, config: GpioConfig) -> Result<(), HalError> {
        if config.pin >= self.pin_count {
            return Err(HalError::InvalidParam);
        }
        unsafe {
            // 1. 配置方向
            let mut dir = Self::r(self.base, GPIO_DIR);
            match config.dir {
                GpioDir::Output => dir |= 1u32 << config.pin,
                GpioDir::Input  => dir &= !(1u32 << config.pin),
            }
            Self::w(self.base, GPIO_DIR, dir);

            // 2. 配置上下拉
            let pud_offset = GPIO_PUD + ((config.pin / 16) as u64) * 4;
            let bit_shift  = (config.pin % 16) * 2;
            let mask       = 0x3u32 << bit_shift;
            let val        = (config.pull as u32) << bit_shift;
            let mut pud = Self::r(self.base, pud_offset);
            pud = (pud & !mask) | val;
            Self::w(self.base, pud_offset, pud);
        }
        Ok(())
    }

    fn set(&self, pin: u32, val: bool) -> Result<(), HalError> {
        if pin >= self.pin_count {
            return Err(HalError::InvalidParam);
        }
        unsafe {
            let mask = 1u32 << pin;
            let v = if val { mask } else { 0 };
            Self::w(self.base, GPIO_DATA + ((pin / 32) as u64) * 4, v);
        }
        Ok(())
    }

    fn get(&self, pin: u32) -> Result<bool, HalError> {
        if pin >= self.pin_count {
            return Err(HalError::InvalidParam);
        }
        unsafe {
            Ok(Self::r(self.base, GPIO_DATA) & (1u32 << pin) != 0)
        }
    }

    fn toggle(&self, pin: u32) -> Result<(), HalError> {
        let cur = self.get(pin)?;
        self.set(pin, !cur)
    }
}
```

### 7.3 MMIO 辅助函数

```rust
impl Arm64Gpio {
    /// 写 32-bit 寄存器（关联函数，避免 borrow 复杂度）
    #[inline]
    unsafe fn w(base: u64, off: u64, v: u32) {
        write_volatile((base + off) as *mut u32, v);
    }

    /// 读 32-bit 寄存器
    #[inline]
    unsafe fn r(base: u64, off: u64) -> u32 {
        read_volatile((base + off) as *const u32)
    }
}
```

> **为什么用关联函数而非 `&self` 方法**：蓝图 v0.7.0 §4.5 原始实现采用关联函数 `w`/`r`，传入 `base` 参数。这与 `Pl011Uart` 的 `w32`/`r32`（`&self` 方法）风格略有差异，但功能等价。两种风格都可，关键是 `read_volatile`/`write_volatile` 的使用。
>
> **volatile 必要性**：与 UART 相同，GPIO MMIO 访问有副作用（读 DATA 会采样引脚电平，写 DATA 会驱动引脚），必须用 volatile 防止编译器优化。

### 7.4 单例模式

```rust
//! hal/arm64/src/gpio.rs — 单例

/// 全局 GPIO 实例（蓝图定义基址 0x0902_0000，32 pin）
pub static ARM64_GPIO: Arm64Gpio = Arm64Gpio::new(0x0902_0000, 32);

/// 获取全局 GPIO 引用
pub fn gpio() -> &'static Arm64Gpio {
    &ARM64_GPIO
}
```

在 `HalProvider` 中接入：

```rust
//! hal/arm64/src/lib.rs

impl HalProvider for Arm64Hal {
    // ... 其他 trait
    fn gpio(&self) -> &'static dyn HalGpio {
        &ARM64_GPIO
    }
}
```

> **线程安全说明**：`ARM64_GPIO` 是不可变 `static`，MMIO 访问通过 `&self` 完成。但 GPIO 硬件状态（DIR/DATA/PUD 寄存器）是有状态的，多核并发 `set`/`toggle` 同一 pin 会产生竞争。`HalGpio` 接口规范明确要求"需外部同步"（见 hal-interface-spec.md §4.6）。

---

## 8. QEMU virt 配置

### 8.1 蓝图定义的基址

| 项 | 值 | 来源 |
|----|----|------|
| GPIO 基址 | `0x0902_0000` | 蓝图 `蓝图/phase0.md` §v0.7.0 §4.5 |
| pin_count | 32 | 默认值 |
| 寄存器块大小 | 4 KB（`0x1000`） | 通用 GPIO 控制器惯例 |

### 8.2 QEMU virt 的限制

> **重要警告**：QEMU virt 机器**没有真实的 GPIO 控制器设备**。QEMU virt 的内存映射中 `0x0902_0000` 附近并非 GPIO，访问此地址的行为未定义（可能触发 alignment fault 或返回 0）。

**影响**：
- GPIO 驱动**无法在标准 QEMU virt 上做运行时验证**
- 单元测试只能用 mock 寄存器（内存数组模拟）
- 集成测试需以下任一方案：

| 方案 | 说明 | 难度 |
|------|------|------|
| 真机测试 | 飞腾 D2000 / 鲲鹏 920 开发板，接 LED + 按键 | 推荐，但需硬件 |
| 扩展 QEMU | 自定义 QEMU 设备模型，添加 GPIO 仿真 | 高，需改 QEMU C 源码 |
| mock 测试 | 在 host 上用 `Vec<u32>` 模拟寄存器，验证逻辑正确性 | 低，覆盖逻辑而非硬件 |

### 8.3 mock 测试方案

```rust
//! tests/gpio_mock.rs — 用内存数组模拟 GPIO 寄存器

pub struct MockGpioRegs {
    pub dir:  u32,
    pub data: u32,
    pub pud:  [u32; 2],   // 32 pin 需要 2 组 PUD
}

impl MockGpioRegs {
    pub const fn new() -> Self {
        Self { dir: 0, data: 0, pud: [0; 2] }
    }
}

// 测试用：将 Arm64Gpio 的 base 指向 MockGpioRegs 的地址
// （仅 host 测试环境，no_std 目标不可用）
```

### 8.4 真机配置参考

| 平台 | GPIO 基址 | pin_count | 备注 |
|------|-----------|-----------|------|
| QEMU virt | `0x0902_0000`（蓝图） | 32 | 无真实设备，仅 mock |
| 飞腾 D2000 | 待 datasheet 确认 | 32/组 | 多组 GPIO 控制器 |
| 鲲鹏 920 | 待 datasheet 确认 | 16/组 | 多组 GPIO 控制器 |
| 树莓派 4（参考） | `0xFE200000`（BCM2711） | 58 | 非目标平台，仅参考 |

> **真机移植提示**：移植到新平台时，只需修改 `ARM64_GPIO` 的 `base` 和 `pin_count`，若寄存器偏移不同则修改 `GPIO_DIR`/`GPIO_DATA`/`GPIO_PUD` 常量。上层代码（使用 `hal().gpio()`）无需改动。

---

## 9. 使用示例

### 9.1 LED 控制（输出）

```rust
//! 示例：控制 LED（pin 5，输出模式）

use hal::hal;
use hal_interface::{HalGpio, GpioConfig, GpioDir, PullMode};

fn led_demo() {
    // 配置 pin 5 为输出，无上下拉
    hal().gpio().set_dir(GpioConfig {
        pin: 5,
        dir: GpioDir::Output,
        pull: PullMode::None,
    }).expect("set_dir failed");

    // 点亮 LED
    hal().gpio().set(5, true).ok();

    // 延时 1 秒（用 HalClock）
    // ... 

    // 熄灭 LED
    hal().gpio().set(5, false).ok();

    // 翻转 LED（每次调用切换状态）
    hal().gpio().toggle(5).ok();
    hal().gpio().toggle(5).ok();
}
```

### 9.2 按键检测（输入 + 上拉）

```rust
//! 示例：读取按键（pin 10，输入 + 上拉，按键接 GND）

use hal::hal;
use hal_interface::{HalGpio, GpioConfig, GpioDir, PullMode};

fn button_demo() {
    // 配置 pin 10 为输入，上拉（按键另一端接 GND）
    hal().gpio().set_dir(GpioConfig {
        pin: 10,
        dir: GpioDir::Input,
        pull: PullMode::Up,
    }).expect("set_dir failed");

    // 读取按键状态
    let pressed = hal().gpio().get(10).unwrap();
    if pressed {
        // false = 按下（按键拉低）
        // true  = 未按下（上拉拉高）
    }

    // 轮询检测按键（无中断，需主动轮询）
    loop {
        let level = hal().gpio().get(10).unwrap();
        if !level {
            // 按键按下
            // ... 处理按下事件
            break;
        }
        core::hint::spin_loop();
    }
}
```

> **按键去抖动**：机械按键在按下/释放瞬间会产生抖动（bounce），需软件去抖（延时 10–20ms 后再次读取）或硬件去抖（RC 滤波）。v0.7.0 不提供去抖，由上层应用处理。

### 9.3 继电器控制

```rust
//! 示例：控制继电器（pin 15，输出）

use hal::hal;
use hal_interface::{HalGpio, GpioConfig, GpioDir, PullMode};

fn relay_demo() {
    hal().gpio().set_dir(GpioConfig {
        pin: 15,
        dir: GpioDir::Output,
        pull: PullMode::None,
    }).ok();

    // 吸合继电器
    hal().gpio().set(15, true).ok();
    // ... 控制外设通断
    // 释放继电器
    hal().gpio().set(15, false).ok();
}
```

### 9.4 外设复位

```rust
//! 示例：复位外设（pin 20，输出，低电平复位）

use hal::hal;
use hal_interface::{HalGpio, GpioConfig, GpioDir, PullMode};

fn reset_peripheral() {
    hal().gpio().set_dir(GpioConfig {
        pin: 20,
        dir: GpioDir::Output,
        pull: PullMode::Up,   // 默认高电平（非复位态）
    }).ok();

    // 拉低复位
    hal().gpio().set(20, false).ok();
    // 延时 10ms（复位脉冲宽度）
    // ...

    // 释放复位
    hal().gpio().set(20, true).ok();
    // 延时 100ms（等待外设就绪）
    // ...
}
```

### 9.5 通过单例直接访问（无 HalProvider）

```rust
//! 早期启动阶段直接使用 GPIO

use hal::arm64::gpio::ARM64_GPIO;
use hal_interface::{HalGpio, GpioConfig, GpioDir, PullMode};

fn early_gpio() {
    ARM64_GPIO.set_dir(GpioConfig {
        pin: 5,
        dir: GpioDir::Output,
        pull: PullMode::None,
    }).ok();
    ARM64_GPIO.set(5, true).ok();
}
```

---

## 10. 测试与验证

### 10.1 单元测试（mock 寄存器）

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use hal_interface::{HalGpio, GpioConfig, GpioDir, PullMode, HalError};

    #[test]
    fn test_pin_out_of_range() {
        // 模拟 8 pin GPIO
        let gpio = Arm64Gpio::new(0x0, 8);
        // pin 8 越界
        assert_eq!(gpio.set(8, true), Err(HalError::InvalidParam));
        assert_eq!(gpio.get(8), Err(HalError::InvalidParam));
        // pin 7 合法（在 mock 上行为未定义，但越界检查先返回）
    }

    #[test]
    fn test_pud_offset_calculation() {
        // pin 0 → PUD + 0
        assert_eq!(GPIO_PUD + (0u32 / 16) as u64 * 4, 0x94);
        // pin 15 → PUD + 0
        assert_eq!(GPIO_PUD + (15u32 / 16) as u64 * 4, 0x94);
        // pin 16 → PUD + 4
        assert_eq!(GPIO_PUD + (16u32 / 16) as u64 * 4, 0x98);
        // pin 31 → PUD + 4
        assert_eq!(GPIO_PUD + (31u32 / 16) as u64 * 4, 0x98);
    }
}
```

### 10.2 集成测试（真机）

真机集成测试清单（蓝图 v0.7.0 §6.2）：

| 测试项 | 方法 | 预期 |
|--------|------|------|
| LED 翻转 | 配置 pin 5 输出，循环 set(true)/set(false) | LED 闪烁 |
| 按键读取 | 配置 pin 10 输入+上拉，按按键后 get | 按下时返回 false |
| 方向切换 | 同一 pin 先 Output 后 Input | 行为符合方向 |
| 越界保护 | 调用 set(pin_count, true) | 返回 InvalidParam |
| 上下拉验证 | Input 模式下配置 Up/Down/None，读电平 | Up=true, Down=false, None=不确定 |

### 10.3 验收标准（蓝图 v0.7.0 §7）

- GPIO 可控制 LED（输出有效）
- GPIO 可读取按键（输入有效）
- 越界 pin 返回错误（不 panic）
- 文档齐全（本文档）

---

## 11. 常见问题

### 11.1 set 后引脚电平不变

**原因 1**：方向未配置为 Output。
**解决**：先调用 `set_dir` 设为 `GpioDir::Output`。

**原因 2**：`set` 写入的 mask 不匹配 SoC 的 DATA 寄存器语义。
**解决**：检查 SoC datasheet，确认 DATA 寄存器是"直接写值"还是"位掩码地址"模型。若是后者，需改为 read-modify-write。

**原因 3**：引脚被复用为其他功能（如 UART、SPI）。
**解决**：配置 SoC 的 pin mux 寄存器（不在 GPIO 控制器范围内，需 SoC 专用驱动）。

### 11.2 get 始终返回固定值

**原因 1**：方向为 Output，读回的是驱动值而非外部电平。
**解决**：若需读外部电平，先 `set_dir` 为 Input。

**原因 2**：上下拉未配置，引脚浮空，读值不确定。
**解决**：配置 `PullMode::Up` 或 `PullMode::Down`。

**原因 3**：QEMU virt 无真实 GPIO，读返回 0。
**解决**：用真机或 mock 测试。

### 11.3 toggle 偶尔失效

**原因**：多核并发 toggle，read-modify-write 竞争。
**解决**：上层加 `spin::Mutex`，或使用 SoC 专用原子翻转寄存器。

### 11.4 PUD 配置无效

**原因 1**：SoC 的 PUD 寄存器偏移与 EnerOS 默认（0x94）不同。
**解决**：修改 `GPIO_PUD` 常量为 SoC 实际偏移。

**原因 2**：方向为 Output，部分 SoC 在 Output 模式下忽略 PUD。
**解决**：先设为 Input 再配 PUD，再切回 Output（若需 Output + 上下拉）。

---

## 12. 参考

- ARM Generic GPIO Controller（PL061）Technical Reference Manual — ARM DDI 0190
- ARM Architecture Reference Manual (ARMv8) — ARM DDI 0487
- EnerOS HAL 接口规范 — `docs/hal-interface-spec.md` §4.6 HalGpio、§3.4 GpioDir、§3.5 PullMode、§3.6 GpioConfig
- EnerOS v0.6.0 GICv3 驱动说明 — `docs/gicv3-driver-guide.md`（GPIO 中断挂接参考，后续版本）
- EnerOS v0.7.0 蓝图 — `蓝图/phase0.md` §v0.7.0
- 飞腾 D2000 数据手册（GPIO 章节）
- 鲲鹏 920 数据手册（GPIO 章节）

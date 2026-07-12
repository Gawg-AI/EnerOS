# UART 驱动说明

> 版本：v0.7.0
> 适用范围：EnerOS HAL ARM64 串口驱动（PL011 UART）
> 蓝图依据：`蓝图/phase0.md` §v0.7.0、§4.5
> crate：eneros-hal（`hal/arm64/src/uart_pl011.rs`）
> 硬件参考：ARM PrimeCell UART (PL011) Technical Reference Manual（ARM DDI 0183）
> 接口规范：`docs/hal-interface-spec.md` §4.5 HalSerial

---

## 1. 概述

ARM PrimeCell UART（PL011）是 ARM 提供的通用异步收发器 IP，广泛集成于 ARM SoC 与 QEMU virt 平台。EnerOS 在 v0.7.0 选用 PL011 作为 HAL `HalSerial` trait 的参考实现，承担早期启动日志、调试输出（`serial-debug-manual.md`）以及内核 panic 兜底打印任务。

### 1.1 选型理由

| 原因 | 说明 |
|------|------|
| QEMU virt 默认串口 | `qemu-system-aarch64 -M virt` 内置 PL011 @ `0x0900_0000`，免配置即可使用 |
| 飞腾/鲲鹏兼容 | 国产化目标平台 SoC 大多集成 PL011 兼容 UART，可复用驱动 |
| 寄存器接口成熟 | 文档清晰、Linux/EDK2/U-Boot 均有参考实现，便于交叉验证 |
| 中断 + FIFO 支持 | 32 字节 TX/RX FIFO，支持收发中断与 DMA，可支撑后续 v0.28.0 网络栈日志压力 |

### 1.2 在 EnerOS 中的位置

```
┌─────────────────────────────────────────┐
│   上层：kernel / runtime / panic 例程   │
└──────────────────┬──────────────────────┘
                   │ hal().serial().write(...)
┌──────────────────▼──────────────────────┐
│   hal crate — HalSerial trait           │
│   docs/hal-interface-spec.md §4.5       │
└──────────────────┬──────────────────────┘
                   │ impl HalSerial
┌──────────────────▼──────────────────────┐
│   hal/arm64/src/uart_pl011.rs           │
│   Pl011Uart { base, baud, clock_hz }    │
└──────────────────┬──────────────────────┘
                   │ MMIO（read_volatile/write_volatile）
┌──────────────────▼──────────────────────┐
│   PL011 硬件 @ 0x0900_0000              │
└─────────────────────────────────────────┘
```

### 1.3 关键特性

| 特性 | 说明 |
|------|------|
| 数据宽度 | 5/6/7/8 bit 可配置（EnerOS 默认 8N1） |
| FIFO | 32 字节 TX/RX FIFO，可通过 LCR_H.FEN 使能 |
| 波特率 | 由 IBRD + FBRD 分频产生，公式见 §3 |
| 中断 | RX/TX/超时/错误等多源中断，可独立屏蔽（v0.7.0 默认禁用） |
| DMA | 支持 DMA 联动（DMACR，v0.7.0 不使用） |
| 流控 | 支持 RTS/CTS 硬件流控（v0.7.0 不使用） |

> **v0.7.0 实现策略**：默认使用轮询（polling）模式，不使能中断。中断驱动模式留给后续版本（v0.6.0 GICv3 已就绪，可挂接 IRQ 1）。这样在调度器未启动的早期启动阶段即可直接使用。

---

## 2. PL011 寄存器参考

### 2.1 寄存器总表

所有寄存器均为 32-bit，按字访问（必须用 `read_volatile`/`write_volatile`，见 §6.3）。偏移量相对于 PL011 基址（QEMU virt 上基址为 `0x0900_0000`）。

| 偏移 | 名称 | 访问 | 说明 |
|------|------|------|------|
| `0x00` | DR | RW | Data Register：写时把数据压入 TX FIFO，读时从 RX FIFO 弹出数据；低 8 位为有效数据，高 24 位为接收错误状态（读时） |
| `0x04` | RSR_ECR | RW | Receive Status Register / Error Clear：读返回上次接收的错误标志；写任意值清除错误 |
| `0x08`–`0x14` | — | — | 保留（Reserved） |
| `0x18` | FR | RO | Flag Register：FIFO 与 modem 状态标志（见 §2.2） |
| `0x1C` | — | — | 保留 |
| `0x20` | ILPR | RW | IrDA Low-Power Counter：IrDA 模式低功耗分频（EnerOS 不使用 IrDA） |
| `0x24` | IBRD | RW | Integer Baud Rate Divisor：分频整数部分（16-bit） |
| `0x28` | FBRD | RW | Fractional Baud Rate Divisor：分频小数部分（6-bit） |
| `0x2C` | LCR_H | RW | Line Control：帧格式与 FIFO 使能（数据位/停止位/校验/FIFO） |
| `0x30` | CR | RW | Control Register：UART 使能与 modem 控制信号 |
| `0x34` | IFLS | RW | Interrupt FIFO Level Select：FIFO 中断触发阈值 |
| `0x38` | IMSC | RW | Interrupt Mask Set Clear：中断屏蔽配置（写 1 使能） |
| `0x3C` | RIS | RO | Raw Interrupt Status：原始中断状态（屏蔽前） |
| `0x40` | MIS | RO | Masked Interrupt Status：屏蔽后中断状态（实际向 CPU 传递） |
| `0x44` | ICR | WO | Interrupt Clear：写 1 清除对应中断（写 0 无效） |
| `0x48` | DMACR | RW | DMA Control：DMA 使能与阈值（v0.7.0 不使用，恒为 0） |

> **访问宽度**：所有寄存器按 32-bit 字访问。8-bit/16-bit 访问在 ARM Device-nGnRE 内存上行为未定义，可能触发 alignment fault（v0.8.0 页表启用后会被捕获）。

### 2.2 FR（Flag Register）位定义

| 位 | 名称 | 值 | 含义 |
|----|------|-----|------|
| bit 0 | CTS | `0x01` | Clear To Send：modem CTS 输入电平（0=有效，1=无效） |
| bit 1–2 | — | — | 保留 |
| bit 3 | BUSY | `0x08` | UART Busy：1=正在发送数据（TX FIFO 非空或正在移位），flush 时需等待此位清零 |
| bit 4 | RXFE | `0x10` | Receive FIFO Empty：1=RX FIFO 空，无数据可读 |
| bit 5 | TXFF | `0x20` | Transmit FIFO Full：1=TX FIFO 满，不可再写入 |
| bit 6 | RXFF | `0x40` | Receive FIFO Full：1=RX FIFO 满，应尽快读取防溢出 |
| bit 7 | TXFE | `0x80` | Transmit FIFO Empty：1=TX FIFO 空，可写入大量数据 |
| bit 8 | RI | `0x100` | Ring Indicator：modem RI 输入（v0.7.0 不使用） |
| bit 9 | TXFE (alternate) | `0x200` | 旧名重载，部分手册标为 BUSY 的补充，编程时以 bit 7 为准 |

> **关键位说明**：
> - `TXFF`（bit 5）是 `write` 流程的轮询位，置位时不可写 DR。
> - `RXFE`（bit 4）是 `read` 流程的轮询位，置位时读 DR 返回 0（无意义）。
> - `BUSY`（bit 3）是 `flush` 流程的轮询位，置位表示仍有数据在移位寄存器中。

### 2.3 LCR_H（Line Control）位定义

| 位 | 名称 | 值 | 含义 |
|----|------|-----|------|
| bit 0–1 | WLEN | `0b00`=5, `0b01`=6, `0b10`=7, `0b11`=8 | 数据位宽度（EnerOS 用 `0b11` = 8 位） |
| bit 2 | FEN | `0x4` | FIFO Enable：1=使能 32 字节 FIFO，0=禁用（变成 1 字节 holding） |
| bit 3 | STP2 | `0x8` | 2 Stop Bits：1=2 停止位，0=1 停止位 |
| bit 4–5 | EPS | `0x10`/`0x20` | Even Parity Select：`0b00`=无校验，`0b11`=偶校验，`0b01`=奇校验 |
| bit 6 | PEN | `0x40` | Parity Enable：1=使能校验 |
| bit 7 | BRK | `0x80` | Send Break：1=强制输出连续 0（线路上断开） |

> **EnerOS 默认值 `0x70`**：`WLEN=0b11`（8 位） + `FEN=1`（FIFO 使能） + `PEN=0`（无校验） + `STP2=0`（1 停止位）= 8N1 + FIFO。

### 2.4 CR（Control Register）位定义

| 位 | 名称 | 值 | 含义 |
|----|------|-----|------|
| bit 0 | UARTEN | `0x01` | UART Enable：1=使能 UART，0=禁用（配置寄存器时建议先清零） |
| bit 1 | SIREN | `0x02` | IrDA SIR Enable：1=使能 IrDA 模式（v0.7.0 不使用） |
| bit 2 | SIRLP | `0x04` | IrDA Low-Power Mode（v0.7.0 不使用） |
| bit 3–7 | — | — | 保留 |
| bit 8 | TXE | `0x100` | Transmit Enable：1=使能发送 |
| bit 9 | RXE | `0x200` | Receive Enable：1=使能接收 |
| bit 10 | DTR | `0x400` | Data Transmit Ready（modem 输出，v0.7.0 不使用） |
| bit 11 | RTS | `0x800` | Request To Send（modem 输出，v0.7.0 不使用） |
| bit 12–15 | — | — | 保留 |

> **EnerOS 默认值 `0x301`**：`UARTEN=1` + `TXE=1` + `RXE=1`，使能收发。

---

## 3. 波特率配置

### 3.1 分频公式

PL011 的波特率由整数分频（IBRD）与小数分频（FBRD）共同产生：

```
baud   = uartclk / (16 × (IBRD + FBRD/64))
divisor= uartclk / (16 × baud)
IBRD   = floor(divisor)                         // 16-bit, 0..65535
FBRD   = round((divisor - IBRD) × 64) & 0x3F    // 6-bit, 0..63
```

**变量说明**：
- `uartclk`：PL011 输入时钟频率（QEMU virt 默认 24 MHz，真机由设备树 `clock-frequency` 属性指定）
- `baud`：目标波特率（EnerOS 默认 115200）
- `IBRD`：整数分频（16-bit）
- `FBRD`：小数分频（6-bit，分辨率为 1/64）

### 3.2 计算示例：clock=24 MHz, baud=115200

```
divisor = 24_000_000 / (16 × 115200)
        = 24_000_000 / 1_843_200
        = 13.0208333...

IBRD    = floor(13.0208333) = 13
frac    = 0.0208333 × 64 = 1.3333...
FBRD    = round(1.3333) = 1
```

**验证**：

```
actual_baud = 24_000_000 / (16 × (13 + 1/64))
            = 24_000_000 / (16 × 13.015625)
            = 24_000_000 / 208.25
            = 115_207.69  (误差 ≈ +0.007%, 完全可接受)
```

> **波特率误差容忍度**：UART 异步通信允许 ±2% 误差（每帧 10 bit 累积偏移不超过半位）。上述 0.007% 远在容限内。

### 3.3 常用波特率分频表（uartclk = 24 MHz）

| 波特率 | divisor | IBRD | FBRD | 实际波特率 | 误差 |
|--------|---------|------|------|-----------|------|
| 9600 | 156.25 | 156 | 16 | 9599.49 | -0.005% |
| 19200 | 78.125 | 78 | 8 | 19207.39 | +0.038% |
| 38400 | 39.0625 | 39 | 4 | 38394.56 | -0.014% |
| 57600 | 26.0417 | 26 | 3 | 57692.31 | +0.160% |
| 115200 | 13.0208 | 13 | 1 | 115207.69 | +0.007% |
| 230400 | 6.5104 | 6 | 33 | 230215.83 | +0.007% |
| 460800 | 3.2552 | 3 | 16 | 461538.46 | +0.160% |
| 921600 | 1.6276 | 1 | 40 | 923076.92 | +0.160% |

### 3.4 波特率配置代码（Rust）

```rust
//! hal/arm64/src/uart_pl011.rs — 波特率计算
//! 依赖：core（no_std），无 alloc

/// 根据 uartclk 与目标波特率计算 IBRD/FBRD
///
/// 返回 (ibrd, fbrd)；若 baud 为 0 或超过 uartclk/16，返回 (0, 0) 表示非法。
///
/// 算法（纯整数，无浮点）：
///   divisor  = uartclk / (16 × baud)
///   IBRD     = floor(divisor)
///   FBRD     = round(frac × 64)，其中 frac = divisor - floor(divisor)
/// 等价整数实现：
///   ibrd     = uartclk / (16 × baud)            // 整除即 floor
///   remainder= uartclk - ibrd × 16 × baud       // = uartclk mod (16×baud)
///   fbrd     = (remainder × 4 + baud / 2) / baud // round(remainder × 64 / (16×baud))
pub const fn calc_baud_divisor(uartclk: u32, baud: u32) -> (u32, u32) {
    if baud == 0 || uartclk < 16 * baud {
        return (0, 0);
    }
    let denom = 16u64 * baud as u64;
    let uartclk64 = uartclk as u64;
    let baud64 = baud as u64;

    let ibrd = uartclk64 / denom;
    let remainder = uartclk64 - ibrd * denom;          // uartclk mod (16×baud)
    // fbrd = round(remainder × 64 / denom) = round(remainder × 4 / baud)
    // 加 baud/2 实现四舍五入
    let fbrd = (remainder * 4 + baud64 / 2) / baud64;

    // 若 fbrd 进位到 64，则 ibrd+1，fbrd 归零
    if fbrd >= 64 {
        (ibrd as u32 + 1, 0)
    } else {
        (ibrd as u32, fbrd as u32)
    }
}
```

> **无浮点说明**：PL011 驱动运行在 EL1，ARM64 默认可能未使能 FPEN（`CPACR_EL1.FPEN`）。代码用纯整数运算（`remainder × 4 / baud` 等价于 `frac × 64`），避免依赖浮点单元。
>
> **四舍五入实现**：`(remainder × 4 + baud / 2) / baud` 中的 `+ baud / 2` 实现四舍五入——若小数部分 ≥ 0.5（即 `remainder × 4 mod baud ≥ baud / 2`），则商进 1。
>
> **进位处理**：当 `frac × 64` 四舍五入后等于 64（即 frac ≥ 63.5/64 ≈ 0.992），FBRD 应归零并让 IBRD 进 1。例如 uartclk=24MHz、baud=38400 时，divisor=39.0625，frac=0.0625，frac×64=4，fbrd=4（无需进位）。

---

## 4. 初始化序列

### 4.1 初始化步骤

PL011 初始化遵循 ARM 推荐的"禁用→配置→使能"序列：

| 步骤 | 操作 | 寄存器写入 | 说明 |
|------|------|-----------|------|
| 1 | 禁用 UART | `CR = 0x0` | 关闭 UARTEN/TXE/RXE，确保配置期间不收发 |
| 2 | 清除中断 | `ICR = 0x7FF` | 清除所有 pending 中断（11 个中断源） |
| 3 | 配置波特率 | `IBRD = 13; FBRD = 1` | 见 §3.2，clock=24MHz/baud=115200 |
| 4 | 配置帧格式 | `LCR_H = 0x70` | 8N1 + FIFO 使能（WLEN=0b11, FEN=1） |
| 5 | 禁用中断 | `IMSC = 0x0` | v0.7.0 轮询模式，屏蔽所有中断 |
| 6 | 禁用 DMA | `DMACR = 0x0` | 不使用 DMA |
| 7 | 使能 UART | `CR = 0x301` | UARTEN=1, TXE=1, RXE=1 |

### 4.2 为什么先禁用再配置

ARM PL011 TRM 明确要求：修改 IBRD/FBRD/LCR_H 时必须先清零 CR.UARTEN，否则可能产生毛刺帧（glitch frame），导致接收方误识别起始位。 EnerOS 严格遵守此约束。

### 4.3 初始化代码（Rust）

```rust
//! hal/arm64/src/uart_pl011.rs — 初始化

impl Pl011Uart {
    /// 初始化 PL011 UART
    ///
    /// # 参数
    /// - `baud`：目标波特率（如 115200）
    /// - `clock_hz`：输入时钟频率（QEMU virt 为 24_000_000）
    ///
    /// # 错误
    /// - `HalError::InvalidParam`：baud 为 0 或 clock_hz 不足以产生目标波特率
    pub fn init(&self, baud: u32, clock_hz: u32) -> Result<(), HalError> {
        if baud == 0 || clock_hz < 16 * baud {
            return Err(HalError::InvalidParam);
        }
        let (ibrd, fbrd) = calc_baud_divisor(clock_hz, baud);
        if ibrd == 0 {
            return Err(HalError::InvalidParam);
        }

        unsafe {
            // 步骤 1：禁用 UART
            self.w32(Reg::CR, 0x0);
            // 步骤 2：清除所有中断
            self.w32(Reg::ICR, 0x7FF);
            // 步骤 3：配置波特率
            self.w32(Reg::IBRD, ibrd);
            self.w32(Reg::FBRD, fbrd);
            // 步骤 4：配置帧格式（8N1 + FIFO）
            self.w32(Reg::LCR_H, 0x70);
            // 步骤 5：禁用所有中断（轮询模式）
            self.w32(Reg::IMSC, 0x0);
            // 步骤 6：禁用 DMA
            self.w32(Reg::DMACR, 0x0);
            // 步骤 7：使能 UART（UARTEN | TXE | RXE）
            self.w32(Reg::CR, 0x301);
        }
        Ok(())
    }
}
```

### 4.4 复位后的默认状态

PL011 复位后（RESETn 拉低）的默认值：

| 寄存器 | 复位默认值 | 说明 |
|--------|-----------|------|
| DR | 未知 | 不读取 |
| FR | `0x90` | RXFE=1, TXFE=1（FIFO 空） |
| IBRD | 0 | 必须配置 |
| FBRD | 0 | 必须配置 |
| LCR_H | 0 | 8N1 但 FIFO 禁用 |
| CR | 0 | UART 禁用 |
| IMSC | 0 | 中断全屏蔽 |

> **结论**：复位后必须显式调用 `init()`，不能假设寄存器已就绪。

---

## 5. 收发流程

### 5.1 发送流程（write）

```
write(data):
    sent = 0
    for byte in data:
        // 步骤 1：读 FR 检查 TXFF
        while (FR & TXFF) != 0:
            core::hint::spin_loop()   // FIFO 满，等待
        // 步骤 2：TXFF=0，写 DR
        DR = byte
        sent += 1
    return sent
```

**关键点**：
- `TXFF=1` 表示 TX FIFO 满，此时写 DR 会丢失数据（硬件不会缓冲）
- 使用 `core::hint::spin_loop()` 而非空循环，提示 CPU 进入低功耗（YIELD）
- 没有超时机制——若硬件故障 FR 永远为 TXFF，调用方会死循环；v0.7.0 接受此行为（调试串口信任硬件）

```rust
impl HalSerial for Pl011Uart {
    fn write(&self, data: &[u8]) -> Result<usize, HalError> {
        let mut sent = 0usize;
        for &b in data {
            unsafe {
                // 等待 TX FIFO 有空间
                while self.r32(Reg::FR) & FR_TXFF != 0 {
                    core::hint::spin_loop();
                }
                self.w32(Reg::DR, b as u32);
            }
            sent += 1;
        }
        Ok(sent)
    }
}
```

### 5.2 接收流程（read）

```
read(buf):
    n = 0
    while n < buf.len():
        // 步骤 1：读 FR 检查 RXFE
        if (FR & RXFE) != 0:
            break          // FIFO 空，结束读取
        // 步骤 2：RXFE=0，读 DR
        buf[n] = DR & 0xFF
        n += 1
    return n
```

**关键点**：
- `RXFE=1` 表示 RX FIFO 空，此时读 DR 返回 0（无意义），必须跳过
- 非阻塞：FIFO 空立即返回，调用方若需等待数据需自行轮询或使用中断
- `DR & 0xFF`：DR 高 24 位是错误标志，只取低 8 位数据

```rust
impl HalSerial for Pl011Uart {
    fn read(&self, buf: &mut [u8]) -> Result<usize, HalError> {
        let mut n = 0usize;
        while n < buf.len() {
            unsafe {
                // RX FIFO 空，结束
                if self.r32(Reg::FR) & FR_RXFE != 0 {
                    break;
                }
                buf[n] = (self.r32(Reg::DR) & 0xFF) as u8;
            }
            n += 1;
        }
        Ok(n)
    }
}
```

### 5.3 刷新流程（flush）

```
flush():
    // 步骤 1：读 FR 检查 BUSY
    while (FR & BUSY) != 0:
        spin_loop()        // 仍在发送
    // 步骤 2：BUSY=0，返回
    return Ok(())
```

**关键点**：
- `BUSY=1` 表示 UART 正在发送（TX FIFO 已空但移位寄存器仍在工作）
- `BUSY=0` 才能保证所有字节已物理发送到线路上
- 必须在关机/重启前调用，否则最后几字节会丢失

```rust
impl HalSerial for Pl011Uart {
    fn flush(&self) -> Result<(), HalError> {
        unsafe {
            while self.r32(Reg::FR) & FR_BUSY != 0 {
                core::hint::spin_loop();
            }
        }
        Ok(())
    }
}
```

### 5.4 流程对比

| 操作 | 轮询位 | 等待条件 | 阻塞性 |
|------|--------|----------|--------|
| `write` | FR.TXFF (bit 5) | TXFF=0（FIFO 非满） | 阻塞至 FIFO 有空间 |
| `read` | FR.RXFE (bit 4) | RXFE=0（FIFO 非空） | 非阻塞，FIFO 空立即返回 |
| `flush` | FR.BUSY (bit 3) | BUSY=0（不在发送） | 阻塞至移位完成 |

---

## 6. EnerOS 实现

### 6.1 Pl011Uart 结构体设计

```rust
//! hal/arm64/src/uart_pl011.rs

use core::ptr::{read_volatile, write_volatile};
use hal_interface::{HalSerial, HalError};

/// PL011 寄存器偏移
#[allow(dead_code)]
#[repr(u32)]
enum Reg {
    DR      = 0x00,
    RSR_ECR = 0x04,
    FR      = 0x18,
    ILPR    = 0x20,
    IBRD    = 0x24,
    FBRD    = 0x28,
    LCR_H   = 0x2C,
    CR      = 0x30,
    IFLS    = 0x34,
    IMSC    = 0x38,
    RIS     = 0x3C,
    MIS     = 0x40,
    ICR     = 0x44,
    DMACR   = 0x48,
}

/// FR 位掩码
const FR_CTS:  u32 = 0x01;
const FR_BUSY: u32 = 0x08;
const FR_RXFE: u32 = 0x10;
const FR_TXFF: u32 = 0x20;
const FR_RXFF: u32 = 0x40;
const FR_TXFE: u32 = 0x80;

/// PL011 UART 实例
pub struct Pl011Uart {
    /// MMIO 基地址（如 0x0900_0000）
    pub base: u64,
    /// 当前波特率（init 后更新）
    pub baud: u32,
    /// 输入时钟频率（Hz）
    pub clock_hz: u32,
}

impl Pl011Uart {
    /// 构造实例（不初始化硬件，需调用 init）
    pub const fn new(base: u64) -> Self {
        Self { base, baud: 0, clock_hz: 24_000_000 }
    }
}
```

### 6.2 HalSerial trait 实现

```rust
impl HalSerial for Pl011Uart {
    fn write(&self, data: &[u8]) -> Result<usize, HalError> {
        let mut sent = 0usize;
        for &b in data {
            unsafe {
                while self.r32(Reg::FR) & FR_TXFF != 0 {
                    core::hint::spin_loop();
                }
                self.w32(Reg::DR, b as u32);
            }
            sent += 1;
        }
        Ok(sent)
    }

    fn read(&self, buf: &mut [u8]) -> Result<usize, HalError> {
        let mut n = 0usize;
        while n < buf.len() {
            unsafe {
                if self.r32(Reg::FR) & FR_RXFE != 0 {
                    break;
                }
                buf[n] = (self.r32(Reg::DR) & 0xFF) as u8;
            }
            n += 1;
        }
        Ok(n)
    }

    fn flush(&self) -> Result<(), HalError> {
        unsafe {
            while self.r32(Reg::FR) & FR_BUSY != 0 {
                core::hint::spin_loop();
            }
        }
        Ok(())
    }
}
```

### 6.3 MMIO 辅助函数 w32/r32

```rust
impl Pl011Uart {
    /// 写 32-bit 寄存器
    #[inline]
    unsafe fn w32(&self, reg: Reg, val: u32) {
        write_volatile((self.base + reg as u64) as *mut u32, val);
    }

    /// 读 32-bit 寄存器
    #[inline]
    unsafe fn r32(&self, reg: Reg) -> u32 {
        read_volatile((self.base + reg as u64) as *const u32)
    }
}
```

> **为什么必须用 `read_volatile`/`write_volatile`**：
> - MMIO 寄存器有副作用（读 DR 会弹出 FIFO，写 DR 会压入 FIFO）
> - 普通 `*ptr = val` 可能被编译器优化掉、重排或合并
> - `volatile` 保证每次访问都生成真实的内存访问指令
>
> **为什么不用 `core::ptr::read`/`write`**：Rust 标准库的 `ptr::read`/`write` 不是 volatile，编译器可优化。MMIO 必须用 `read_volatile`/`write_volatile`。
>
> **对齐要求**：所有 PL011 寄存器 32-bit 对齐。`base + offset` 必然 4 字节对齐（offset 均为 4 的倍数）。

### 6.4 单例模式

EnerOS 通过全局单例暴露 PL011 实例，避免每处代码都 new 一个对象：

```rust
//! hal/arm64/src/uart_pl011.rs — 单例

/// 全局 PL011 实例（QEMU virt 基址 0x0900_0000）
pub static ARM64_UART: Pl011Uart = Pl011Uart::new(0x0900_0000);

/// 获取全局 UART 引用
pub fn serial() -> &'static Pl011Uart {
    &ARM64_UART
}
```

在 `HalProvider` 中接入：

```rust
//! hal/arm64/src/lib.rs

pub struct Arm64Hal;

impl HalProvider for Arm64Hal {
    // ... 其他 trait
    fn serial(&self) -> &'static dyn HalSerial {
        &ARM64_UART
    }
}
```

> **线程安全说明**：`ARM64_UART` 是 `static`（不可变），其内部字段 `base/baud/clock_hz` 均为不可变。MMIO 访问通过 `&self`（共享引用）完成，符合 Rust 借用规则。但 PL011 硬件本身是有状态的（FIFO），多核同时 `write` 会导致字节交错——这是 `HalSerial` 接口规范的已知限制（见 hal-interface-spec.md §4.5 说明："串口通常无并发保护，多核同时访问需上层加锁"）。

---

## 7. QEMU virt 配置

### 7.1 默认配置

| 项 | 值 | 来源 |
|----|----|------|
| PL011 基址 | `0x0900_0000` | QEMU virt 内存映射固定值 |
| 寄存器块大小 | 4 KB（`0x1000`） | ARM PrimeCell 标准 |
| 输入时钟 | 24 MHz | QEMU virt 固件设置 |
| 默认波特率 | 115200 | EnerOS `init()` 参数 |
| 中断号 | SPI 1（IRQ 33） | GICv3 SPI 起始为 32，virt UART 为第 1 个 SPI（即 GIC IRQ 33） |
| 兼容字符串 | `arm,pl011`, `arm,primecell` | 设备树 `compatible` 属性 |

> **中断号说明**：ARM GIC 中断号从 32 开始是 SPI。QEMU virt 的 PL011 UART 在设备树中标记为 `interrupts = <0 1 4>`，意为 SPI、IRQ 1（相对 SPI 起始）、电平触发。转换到 GIC 全局中断号为 `32 + 1 = 33`。v0.7.0 轮询模式不使用此中断，但 v0.28.0 网络栈可能挂接。

### 7.2 设备树片段

```
// QEMU virt 设备树（简化）
uart0: serial@9000000 {
    compatible = "arm,pl011", "arm,primecell";
    reg = <0x0 0x09000000 0x0 0x1000>;
    interrupts = <0 1 4>;       // SPI, IRQ 1, Level-sensitive
    clocks = <&clk24mhz>, <&clk24mhz>;
    clock-names = "uartclk", "apb_pclk";
    status = "okay";
};
```

### 7.3 启动 QEMU 命令

```bash
qemu-system-aarch64 \
    -M virt \
    -cpu cortex-a57 \
    -smp 1 \
    -m 256M \
    -nographic \
    -kernel target/aarch64-unknown-none/release/enoros.bin
```

`-nographic` 把 QEMU 的标准输入输出重定向到第一个串口（即 PL011 @ 0x0900_0000），用户在终端即可看到 EnerOS 的串口输出。

---

## 8. 使用示例

### 8.1 完整使用流程

```rust
//! 示例：使用 Pl011Uart 收发数据

use hal::arm64::uart_pl011::{Pl011Uart, ARM64_UART};
use hal_interface::HalSerial;

fn uart_demo() {
    // 方式 1：使用全局单例（推荐）
    let uart = &ARM64_UART;
    uart.init(115200, 24_000_000).expect("UART init failed");

    // 发送数据
    uart.write(b"Hello, EnerOS!\n").ok();

    // 刷新（确保数据物理发送完成）
    uart.flush().ok();

    // 接收数据（非阻塞，最多读 64 字节）
    let mut buf = [0u8; 64];
    let n = uart.read(&mut buf).unwrap();
    if n > 0 {
        uart.write(&buf[..n]).ok();  // 回环
    }
}
```

### 8.2 方式 2：自定义实例

```rust
// 自定义基址（真机 PL011 不在 0x0900_0000 时）
let uart = Pl011Uart::new(0x0900_0000);
uart.init(9600, 24_000_000).ok();
uart.write(b"custom baud\n").ok();
```

### 8.3 通过 HalProvider 访问

```rust
use hal::{hal, HalSerial};

fn log(msg: &[u8]) {
    hal().serial().write(msg).ok();
    hal().serial().flush().ok();
}

// 在 panic handler 中
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    hal().serial().write(b"panic: ").ok();
    // ... 打印 panic 信息
    hal().serial().flush().ok();
    loop { core::hint::spin_loop(); }
}
```

### 8.4 早期启动阶段（无 HAL 初始化）

```rust
//! boot 阶段直接使用 PL011（不经过 HalProvider）
use hal::arm64::uart_pl011::Pl011Uart;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // 此时尚未 init_hal，直接用 PL011
    let uart = Pl011Uart::new(0x0900_0000);
    let _ = uart.init(115200, 24_000_000);
    let _ = uart.write(b"[boot] EnerOS starting...\n");
    // ... 继续 boot 流程
    loop {}
}
```

---

## 9. 测试与验证

### 9.1 单元测试（mock 寄存器）

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_baud_divisor_115200() {
        let (ibrd, fbrd) = calc_baud_divisor(24_000_000, 115200);
        assert_eq!(ibrd, 13);
        assert_eq!(fbrd, 1);
    }

    #[test]
    fn test_baud_divisor_zero_baud() {
        let (ibrd, fbrd) = calc_baud_divisor(24_000_000, 0);
        assert_eq!((ibrd, fbrd), (0, 0));
    }
}
```

### 9.2 集成测试（QEMU 串口回环）

启动 QEMU 后，在 host 终端输入字符，EnerOS 应能 read 到并 write 回显：

```
[host terminal]
$ qemu-system-aarch64 -M virt -nographic -kernel enoros.bin
[boot] EnerOS starting...
Hello, EnerOS!       ← 用户输入
Hello, EnerOS!       ← 回环输出
```

### 9.3 验收标准（蓝图 v0.7.0 §7）

- 串口可收发数据（write/read 均工作）
- 115200 波特率下无字符丢失
- `flush` 后 BUSY 位清零
- QEMU virt 上 `-nographic` 模式可见启动日志

---

## 10. 常见问题

### 10.1 输出乱码

**原因**：波特率配置错误（IBRD/FBRD 与 uartclk 不匹配）。

**排查**：
1. 确认 `clock_hz` 与实际 PL011 输入时钟一致（QEMU virt 为 24 MHz，真机看设备树 `clock-frequency`）
2. 用 §3.3 的分频表核对 IBRD/FBRD
3. QEMU 终端确认波特率为 115200（`-nographic` 默认即 115200）

### 10.2 write 卡死

**原因**：`FR.TXFF` 永远为 1。

**排查**：
1. 确认 `CR.UARTEN=1` 且 `CR.TXE=1`（CR = 0x301）
2. 确认 MMIO 基址正确（`0x0900_0000`）
3. 确认页表已映射 PL011 区域为 Device-nGnRE（v0.8.0 后）
4. QEMU 是否启动（`-nographic` 模式下 host 终端必须保持打开）

### 10.3 read 永远返回 0

**原因**：`FR.RXFE` 永远为 1。

**排查**：
1. 确认 `CR.RXE=1`（CR = 0x301）
2. 确认 host 终端已连接（`-nographic` 模式下需在 QEMU 终端输入）
3. 确认 LCR_H.FEN=1（FIFO 使能），否则每次只能读 1 字节

### 10.4 多核同时 write 导致字节交错

**原因**：`HalSerial` 接口无并发保护（见 hal-interface-spec.md §4.5）。

**解决**：上层加 `spin::Mutex` 或使用 per-core 串口（真机多 UART 场景）。

---

## 11. 参考

- ARM PrimeCell UART (PL011) Technical Reference Manual — ARM DDI 0183
- ARM Architecture Reference Manual (ARMv8) — ARM DDI 0487
- QEMU virt machine documentation — https://www.qemu.org/docs/master/system/arm/virt.html
- EnerOS HAL 接口规范 — `docs/hal-interface-spec.md` §4.5 HalSerial
- EnerOS 串口调试手册 — `docs/serial-debug-manual.md`
- EnerOS v0.6.0 GICv3 驱动说明 — `docs/gicv3-driver-guide.md`（中断挂接参考）
- EnerOS v0.7.0 蓝图 — `蓝图/phase0.md` §v0.7.0

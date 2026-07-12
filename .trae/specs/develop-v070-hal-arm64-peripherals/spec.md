# EnerOS v0.7.0 — HAL ARM64 外设实现 Spec

> **版本**：v0.7.0（Phase 0 / P0-B 终点）
> **类型**：实现版本（补全 HAL 外设层：UART 串口 + GPIO + 网口寄存器级）
> **前序依赖**：v0.6.0（HAL ARM64 核心：CPU/GICv3/Timer）
> **后续版本**：v0.8.0（页表管理与地址空间，实现 HalMem）
> **蓝图依据**：`蓝图/phase0.md` §v0.7.0（第 1279–1440 行）
> **合规性**：蓝图 §43.1（no_std）、§43.2（非瓶颈版本：签名必须可编译）

---

## Why

v0.6.0 实现了 HAL 核心（CPU/中断/时钟），但外设层仍缺失。v0.7.0 补全 HAL 外设：PL011 UART 串口（调试日志通道）、GPIO 控制器（设备控制引脚）、网口寄存器级访问（为后续网络栈铺路）。这是 P0-B 的终点，HAL 三件套（核心+外设+内存）的外设部分完成。

---

## What Changes

- **新增** `hal/src/arm64/uart_pl011.rs`：`Pl011Uart` 实现 `HalSerial`（~180 行）
- **新增** `hal/src/arm64/gpio.rs`：`Arm64Gpio` 实现 `HalGpio`（~150 行）
- **新增** `hal/src/arm64/net_mmio.rs`：网口寄存器级访问（~120 行，无 trait 实现，仅 MMIO 读取）
- **修改** `hal/src/arm64/mod.rs`：添加 `pub mod uart_pl011;` `pub mod gpio;` `pub mod net_mmio;`
- **修改** `hal/src/arm64/provider.rs`：`serial()` 和 `gpio()` 从 panic 改为返回真实单例；`mem()` 仍 panic（推迟到 v0.8.0）
- **修改** workspace 根 `Cargo.toml`：version `0.6.0` → `0.7.0`
- **修改** `.github/workflows/ci.yml`：版本标识 v0.6.0 → v0.7.0
- **修改** `Makefile`：VERSION 0.6.0 → 0.7.0
- **修改** `ci/src/gate.rs`：注释更新
- **新增** 文档：`docs/uart-driver-guide.md`、`docs/gpio-usage-guide.md`

---

## Impact

- **Affected specs**：v0.8.0 将实现 `HalMem`（页表管理）并最终补全 `HalProvider`
- **Affected code**：
  - `hal/src/arm64/` 新增 3 个文件（~450 行代码）
  - `hal/src/arm64/mod.rs` 和 `provider.rs` 修改
  - 工作区 `Cargo.toml`、CI 配置、Makefile
- **不影响**：现有 kernel/runtime/board/sel4-sys/hello crate 的功能行为
- **不影响**：v0.6.0 的 cpu/gicv3/timer 实现（回归兼容）
- **不影响**：v0.5.0 的 mock 实现（回归兼容）

---

## ADDED Requirements

### Requirement: Pl011Uart 实现 HalSerial

系统 SHALL 提供 `Pl011Uart` 结构体，实现 v0.5.0 定义的 `HalSerial` trait，基于 ARM PL011 UART 硬件。

#### Scenario: 串口写入

- **WHEN** 调用 `uart.write(&[0x41, 0x42, 0x43])`
- **THEN** 逐字节读取 FR（Flag Register）检查 TXFF（Transmit FIFO Full）位
- **AND** TXFF 为 0 时写入 DR（Data Register）
- **AND** 所有字节写入完成后返回 `Ok(data.len())`

#### Scenario: 串口读取

- **WHEN** 调用 `uart.read(buf)`
- **THEN** 读取 FR 检查 RXFE（Receive FIFO Empty）位
- **AND** RXFE 为 0 时读取 DR 填入 buf
- **AND** 返回 `Ok(已读字节数)`

#### Scenario: 串口刷新

- **WHEN** 调用 `uart.flush()`
- **THEN** 轮询 FR 的 BUSY 位直到清零
- **AND** 返回 `Ok(())`

#### Scenario: PL011 寄存器常量

- **PL011_DR**（Data Register）= 0x00
- **PL011_FR**（Flag Register）= 0x18
- **PL011_IBRD**（Integer Baud Rate Divisor）= 0x24
- **PL011_FBRD**（Fractional Baud Rate Divisor）= 0x28
- **PL011_LCRH**（Line Control）= 0x2C
- **PL011_CR**（Control Register）= 0x30
- **PL011_IMSC**（Interrupt Mask Set Clear）= 0x38
- **FR_TXFF**（Transmit FIFO Full）= 1 << 5
- **FR_RXFE**（Receive FIFO Empty）= 1 << 4
- **FR_BUSY**（UART Busy）= 1 << 3

### Requirement: Arm64Gpio 实现 HalGpio

系统 SHALL 提供 `Arm64Gpio` 结构体，实现 v0.5.0 定义的 `HalGpio` trait，基于通用 GPIO 控制器寄存器接口。

#### Scenario: GPIO 方向配置

- **WHEN** 调用 `gpio.set_dir(GpioConfig { pin: 5, dir: GpioDir::Output, pull: PullMode::Up })`
- **THEN** 读取 GPIO_DIR 寄存器，置位 bit 5（Output）或清零 bit 5（Input）
- **AND** 配置上下拉寄存器 GPIO_PUD
- **AND** 返回 `Ok(())`

#### Scenario: GPIO 写入

- **WHEN** 调用 `gpio.set(5, true)`
- **THEN** 写入 GPIO_DATA 寄存器对应 bit
- **AND** 返回 `Ok(())`

#### Scenario: GPIO 读取

- **WHEN** 调用 `gpio.get(5)`
- **THEN** 读取 GPIO_DATA 寄存器，返回 bit 5 的状态
- **AND** 返回 `Ok(true/false)`

#### Scenario: GPIO 翻转

- **WHEN** 调用 `gpio.toggle(5)`
- **THEN** 读取当前值，写入相反值
- **AND** 返回 `Ok(())`

#### Scenario: GPIO 越界保护

- **WHEN** 调用 `gpio.set(100, true)` 且 pin_count = 32
- **THEN** 返回 `Err(HalError::InvalidParam)`

#### Scenario: GPIO 寄存器常量

- **GPIO_DIR**（方向寄存器）= 0x04
- **GPIO_DATA**（数据寄存器）= 0x40
- **GPIO_PUD**（上下拉寄存器）= 0x94

### Requirement: 网口寄存器级访问（net_mmio.rs）

系统 SHALL 提供 `NetMmio` 结构体，实现网口 MAC/PHY 寄存器级读取能力（无 trait 实现，仅 MMIO 读取）。

#### Scenario: 读取 PHY ID

- **WHEN** 调用 `net.read_phy_id()`
- **THEN** 通过 MDIO 接口读取 PHY ID 寄存器
- **AND** 返回 `(phy_id_high, phy_id_low)` 元组

#### Scenario: 读取 MAC 地址

- **WHEN** 调用 `net.read_mac_addr()`
- **THEN** 读取 MAC 地址寄存器
- **AND** 返回 `[u8; 6]` 数组

### Requirement: HalProvider 补全 serial/gpio

系统 SHALL 更新 `Arm64HalCoreProvider`，将 `serial()` 和 `gpio()` 从 panic 改为返回真实实现单例。`mem()` 仍 panic（推迟到 v0.8.0 页表管理）。

#### Scenario: serial 获取

- **WHEN** 调用 `provider.serial()`
- **THEN** 返回 `&Pl011Uart` 单例（`&'static dyn HalSerial`）
- **AND** 不再 panic

#### Scenario: gpio 获取

- **WHEN** 调用 `provider.gpio()`
- **THEN** 返回 `&Arm64Gpio` 单例（`&'static dyn HalGpio`）
- **AND** 不再 panic

#### Scenario: mem 仍不可用

- **WHEN** 调用 `provider.mem()`
- **THEN** panic("not implemented: HalMem will be added in v0.8.0")
- **AND** 错误信息指向 v0.8.0

### Requirement: no_std 合规

所有外设实现 MUST 遵循蓝图 §43.1：`#![no_std]`，使用 `core::ptr::read_volatile`/`write_volatile` 进行 MMIO，不使用 `std::*`。

### Requirement: 文档交付

系统 SHALL 交付两份文档：
1. `docs/uart-driver-guide.md`：《UART 驱动说明》——PL011 架构概述、寄存器表、初始化序列、收发流程、波特率配置
2. `docs/gpio-usage-guide.md`：《GPIO 使用》——GPIO 控制器寄存器表、方向配置、上下拉配置、使用示例

---

## MODIFIED Requirements

### Requirement: Workspace 版本

workspace 根 `Cargo.toml` 的 version 从 `0.6.0` 升级到 `0.7.0`。

### Requirement: CI 流水线版本

`.github/workflows/ci.yml` 的版本标识从 v0.6.0 升级到 v0.7.0。

### Requirement: Makefile 版本

`Makefile` 的 VERSION 从 0.6.0 升级到 0.7.0。

### Requirement: ARM64 模块入口

`hal/src/arm64/mod.rs` 新增三个子模块声明：`pub mod uart_pl011;` `pub mod gpio;` `pub mod net_mmio;`

### Requirement: ARM64 HAL Provider

`hal/src/arm64/provider.rs` 的 `serial()` 和 `gpio()` 方法从 panic 改为返回真实单例。`mem()` 的 panic 消息从 "v0.7.0" 改为 "v0.8.0"。

---

## 设计决策（Design Decisions）

### D1: PL011 而非 16550

选择 PL011 UART（ARM PrimeCell）而非 16550 兼容 UART。理由：
- QEMU virt 默认使用 PL011
- v0.3.0 board crate 已有 PL011 经验
- 飞腾/鲲鹏 SoC 普遍集成 PL011 兼容 UART
- 蓝图 §4.5 假设前提明确 PL011

### D2: GPIO 寄存器布局

GPIO 使用蓝图 §4.5 定义的通用寄存器布局（DIR=0x04, DATA=0x40, PUD=0x94），而非特定 SoC 的 GPIO 控制器。理由：
- Phase 0 面向 QEMU virt 验证
- QEMU virt 无真实 GPIO 控制器，寄存器布局为蓝图定义的通用模型
- 后续版本可通过 `hal/arm64/gpio_<soc>.rs` 支持特定 SoC

### D3: 网口仅寄存器级

网口（net_mmio.rs）仅实现寄存器级读取（PHY ID / MAC 地址），不实现完整网络栈。理由：
- 蓝图 §3 明确"基础网口寄存器级访问"
- 完整网络栈属于 v0.28.0（TCP/IP）
- PHY 初始化需 MDIO 时序，复杂度高（蓝图 §5.4 难点）
- v0.7.0 目标是"可读 ID"验证寄存器可达

### D4: HalMem 推迟到 v0.8.0

v0.7.0 不实现 `HalMem`，`provider.mem()` 仍 panic。理由：
- 蓝图 v0.7.0 交付物仅列 HalSerial/HalGpio
- HalMem 的 map/unmap/translate 需要页表支持（v0.8.0 页表管理）
- v0.6.0 spec D4 说"完整 HalProvider 在 v0.7.0 补全"，但实际 HalMem 依赖 v0.8.0 页表
- v0.7.0 补全 serial/gpio，HalMem 推迟到 v0.8.0

### D5: PL011 基址使用 QEMU virt 默认

PL011 UART 基址使用 QEMU virt 默认值 `0x09000000`。理由：
- QEMU virt 平台 PL011 固定映射在 0x09000000
- 与 v0.3.0 board crate 的 PL011 基址一致
- 后续版本可通过配置覆盖

### D6: GPIO 基址使用蓝图定义值

GPIO 基址使用蓝图定义的 `0x09020000`（QEMU virt 无真实 GPIO，此为蓝图约定的模拟基址）。理由：
- QEMU virt 无真实 GPIO 控制器
- 蓝图 §4.5 代码片段使用此布局
- 寄存器级验证以交叉编译通过为主

---

## 非目标（Non-Goals）

- **不实现** `HalMem`（属于 v0.8.0 页表管理）
- **不实现**完整网络栈（属于 v0.28.0 TCP/IP）
- **不做** QEMU 串口回环运行时验证（延后到 kernel 集成）
- **不做** GPIO LED 实物验证（延后到真机部署）
- **不做** PHY 初始化与 MDIO 时序（仅寄存器级读取）
- **不集成**到 kernel/runtime 调用链（属于后续版本）

---

## 风险与缓解

| 风险 | 等级 | 缓解 |
|------|------|------|
| QEMU virt 无真实 GPIO | 中/低 | 使用蓝图定义的通用寄存器模型，交叉编译验证 |
| PL011 寄存器布局差异 | 低/低 | QEMU virt 标准 PL011，与 v0.3.0 board 一致 |
| 网口 PHY 寄存器不可达 | 中/中 | v0.7.0 仅定义结构，运行时验证延后 |
| MMIO 幂等性 | 低/低 | read_volatile/write_volatile 保证 |
| 多板外设差异 | 中/中 | 后续版本通过 SoC 特定文件支持 |

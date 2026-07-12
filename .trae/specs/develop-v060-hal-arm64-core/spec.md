# EnerOS v0.6.0 — HAL ARM64 核心实现 Spec

> **版本**：v0.6.0（Phase 0 / P0-B 第二步）
> **类型**：实现版本（把 v0.5.0 的 trait 规范变成可运行代码）
> **前序依赖**：v0.5.0（HAL 接口规范）
> **后续版本**：v0.7.0（HAL ARM64 外设实现：UART/GPIO/Net）
> **蓝图依据**：`蓝图/phase0.md` §v0.6.0（第 1040–1276 行）
> **合规性**：蓝图 §43.1（no_std）、§43.2（非瓶颈版本：签名必须可编译）

---

## Why

v0.5.0 定义了 HAL trait 接口规范，但只有 mock 实现。v0.6.0 将规范变成可运行代码：实现 ARM64 的 CPU 操作（中断掩码/核间标识/低功耗）、GICv3 中断控制器（注册/使能/EOI）、ARM Generic Timer（单调时钟/定时器中断）。这是调度器、IPC、时钟服务的基础，直接支撑"实时性"出口标准。

---

## What Changes

- **新增** `hal/src/arm64/mod.rs`：ARM64 模块入口（`#[cfg(target_arch = "aarch64")]` 门控）
- **新增** `hal/src/arm64/cpu.rs`：`Arm64Cpu` 实现 `HalCpu`（~150 行）
- **新增** `hal/src/arm64/gicv3.rs`：`Arm64Gic` 实现 `HalIrq`（GICv3，~250 行）
- **新增** `hal/src/arm64/timer.rs`：`Arm64Timer` 实现 `HalClock`（Generic Timer，~120 行）
- **新增** `hal/src/arm64/provider.rs`：`Arm64HalCoreProvider` 部分实现（仅 cpu/irq/clock）
- **修改** `hal/src/lib.rs`：添加 `#[cfg(target_arch = "aarch64")] pub mod arm64;`
- **修改** `hal/Cargo.toml`：无新增依赖（仅 core，使用 inline asm + MMIO）
- **修改** workspace 根 `Cargo.toml`：version `0.5.0` → `0.6.0`
- **修改** `.github/workflows/ci.yml`：版本标识 v0.5.0 → v0.6.0
- **修改** `Makefile`：VERSION 0.5.0 → 0.6.0
- **修改** `ci/src/gate.rs`：注释更新
- **新增** 文档：`docs/gicv3-driver-guide.md`、`docs/arm-generic-timer-usage.md`

---

## Impact

- **Affected specs**：v0.7.0 将实现 `HalMem`/`HalSerial`/`HalGpio` 并补全 `HalProvider`
- **Affected code**：
  - `hal/src/arm64/` 新增 4 个文件（~570 行代码）
  - 工作区 `Cargo.toml`、CI 配置、Makefile
- **不影响**：现有 kernel/runtime/board/sel4-sys/hello crate 的功能行为
- **不影响**：v0.5.0 的 mock 实现（回归兼容）

---

## ADDED Requirements

### Requirement: Arm64Cpu 实现 HalCpu

系统 SHALL 提供 `Arm64Cpu` 结构体，实现 v0.5.0 定义的 `HalCpu` trait，使用 ARM64 系统寄存器与指令。

#### Scenario: 中断掩码

- **WHEN** 调用 `cpu.enable_irq()`
- **THEN** 执行 `msr daifclr, #0xf` 清除 DAIF 中断掩码
- **WHEN** 调用 `cpu.disable_irq()`
- **THEN** 执行 `msr daifset, #0xf` 设置 DAIF 中断掩码

#### Scenario: 核心标识

- **WHEN** 调用 `cpu.current_core()`
- **THEN** 读取 `mpidr_el1` 寄存器，返回 Aff0 字段（低 8 位）
- **WHEN** 调用 `cpu.core_count()`
- **THEN** 返回配置的核心数（从编译期配置或 DTS 读取）

#### Scenario: 低功耗

- **WHEN** 调用 `cpu.wfi()`
- **THEN** 执行 `wfi` 指令进入低功耗等待中断
- **WHEN** 调用 `cpu.halt()`
- **THEN** 循环调用 `wfi()` 永不返回

### Requirement: Arm64Gic 实现 HalIrq（GICv3）

系统 SHALL 提供 `Arm64Gic` 结构体，实现 v0.5.0 定义的 `HalIrq` trait，基于 GICv3 架构。实现 MUST 包含：
1. Distributor (GICD) 初始化（含 ARE 亲和性路由使能）
2. Redistributor (GICR) per-core 唤醒（WAKER 轮询）
3. CPU interface 通过 ICC_*_EL1 系统寄存器（非 GICv2 内存映射模式）

#### Scenario: GICv3 初始化

- **WHEN** 调用 `gic.init()`
- **THEN** GICD_CTLR 使能（ARE_NS=1, EnableGrp1=1）
- **AND** 当前核的 GICR 被唤醒（WAKER.ProcessorSleep 清除，轮询 ChildrenAsleep 清零）
- **AND** CPU interface 通过 ICC_IGRPEN1_EL1 使能

#### Scenario: 中断注册

- **WHEN** 调用 `gic.register(32, IrqTrigger::Edge, handler)`
- **THEN** handler 存入静态 handler 表的 slot 32
- **AND** 返回 `Ok(())`
- **WHEN** 调用 `gic.register(300, ...)`（超过 MAX_IRQ）
- **THEN** 返回 `Err(HalError::InvalidParam)`

#### Scenario: 中断使能/禁用

- **WHEN** 调用 `gic.enable(32)`
- **THEN** GICD_ISENABLER1 的 bit 0 被置位（SPI 32 对应 ISENABLER1）
- **WHEN** 调用 `gic.disable(32)`
- **THEN** GICD_ICENABLER1 的 bit 0 被置位

#### Scenario: 中断结束（EOI）

- **WHEN** 调用 `gic.eoi(32)`
- **THEN** 执行 `msr icc_eoir1_el1, #32`（GICv3 系统寄存器模式）

#### Scenario: 中断分发

- **WHEN** 中断触发并调用 `gic.dispatch_irq()`
- **THEN** 读取 ICC_IAR1_EL1 获取 IRQ ID
- **AND** 查 handler 表调用对应 handler
- **AND** 调用 EOI 结束中断
- **AND** 未知中断号打印告警并 EOI 丢弃

### Requirement: Arm64Timer 实现 HalClock（Generic Timer）

系统 SHALL 提供 `Arm64Timer` 结构体，实现 v0.5.0 定义的 `HalClock` trait，基于 ARMv8 Generic Timer。

#### Scenario: 时钟读取

- **WHEN** 调用 `timer.now_ns()`
- **THEN** 读取 `cntpct_el0`（物理计数器）
- **AND** 乘以 1e9 / frequency 转换为纳秒
- **AND** 返回值单调递增

#### Scenario: 频率获取

- **WHEN** 调用 `timer.frequency_hz()`
- **THEN** 读取 `cntfrq_el0` 返回频率（Hz）

#### Scenario: 定时器设置

- **WHEN** 调用 `timer.set_deadline(1_000_000)`
- **THEN** 读取当前 cntpct_el0，加上 deadline 对应的 tick 数
- **AND** 写入 CNTP_TVAL_EL0 或 CNTP_CVAL_EL0
- **AND** 使能定时器中断（CNTP_CTL_EL1.ENABLE=1）

### Requirement: ARM64 模块 cfg 门控

ARM64 实现 MUST 通过 `#[cfg(target_arch = "aarch64")]` 门控，确保：
1. Host 构建（x86_64）跳过 arm64 模块，eneros-hal 仍可参与 host clippy/test
2. aarch64 交叉编译包含 arm64 模块
3. v0.5.0 的 mock 实现不受影响

#### Scenario: Host 构建不包含 arm64 代码

- **WHEN** 在 x86_64 host 执行 `cargo build -p eneros-hal`
- **THEN** arm64 模块被 cfg 排除，编译成功
- **AND** mock 模块仍可用

#### Scenario: aarch64 交叉编译包含 arm64 代码

- **WHEN** 执行 `cargo build -p eneros-hal --target aarch64-unknown-none`
- **THEN** arm64 模块被编译，inline asm 与 MMIO 代码可用

### Requirement: 静态中断 handler 表

系统 SHALL 使用 `static mut IRQ_HANDLERS: [Option<IrqHandler>; MAX_IRQ]` 存储中断处理函数，因为 `HalIrq::register(&self, ...)` 签名不允许 `&mut self`。

#### Scenario: handler 注册与查找

- **WHEN** 调用 `register(irq, trigger, handler)`
- **THEN** 通过 `unsafe` 写入 `IRQ_HANDLERS[irq] = Some(handler)`
- **WHEN** 中断分发时查找 handler
- **THEN** 通过 `unsafe` 读取 `IRQ_HANDLERS[irq]`

### Requirement: no_std 合规

所有 ARM64 实现 MUST 遵循蓝图 §43.1：`#![no_std]`，使用 `core::arch::asm!` 进行内联汇编，使用 `core::ptr::read_volatile`/`write_volatile` 进行 MMIO。

### Requirement: 文档交付

系统 SHALL 交付两份文档：
1. `docs/gicv3-driver-guide.md`：《GICv3 驱动说明》——GICv3 架构概述、GICD/GICR/ICC 寄存器表、初始化序列、中断分发流程、与 GICv2 差异
2. `docs/arm-generic-timer-usage.md`：《ARM Generic Timer 使用》——CNTFRQ/CNTPCT/CNTP_TVAL/CNTP_CVAL/CNTP_CTL 寄存器说明、纳秒转换公式、定时器中断配置

---

## MODIFIED Requirements

### Requirement: Workspace 版本

workspace 根 `Cargo.toml` 的 version 从 `0.5.0` 升级到 `0.6.0`。

### Requirement: CI 流水线版本

`.github/workflows/ci.yml` 的版本标识从 v0.5.0 升级到 v0.6.0。

### Requirement: Makefile 版本

`Makefile` 的 VERSION 从 0.5.0 升级到 0.6.0。

---

## 设计决策（Design Decisions）

### D1: 模块内嵌（非子 crate）

ARM64 实现作为 `hal/src/arm64/` 模块内嵌在 eneros-hal crate 中，而非独立的 `hal/arm64/` 子 crate。理由：
- 避免 workspace 复杂度
- `#[cfg(target_arch = "aarch64")]` 门控即可隔离 host/target 编译
- 与 eneros-sel4-sys 的 `#[cfg(target_arch = "aarch64")]` 模式一致
- v0.5.0 的 mock 不受影响

### D2: GICv3 系统寄存器模式（非 GICv2 兼容模式）

CPU interface 使用 ICC_*_EL1 系统寄存器（`msr icc_eoir1_el1` 等），而非蓝图 v1.0 的 GICC 内存映射模式。理由：
- 蓝图 §43.2 合规性修复明确要求 GICv3 一致性
- GICv3 使能 ARE 后必须用系统寄存器
- GICv2 兼容模式无法使用 Redistributor 的亲和性路由

### D3: 静态 handler 表（static mut）

中断 handler 存储使用 `static mut IRQ_HANDLERS: [Option<IrqHandler>; 256]`，而非结构体字段。理由：
- `HalIrq::register(&self, ...)` 签名不允许 `&mut self`
- `spin::Mutex` 需引入外部依赖（v0.5.0 hal 无依赖）
- OS 内核场景 `static mut` + unsafe 是标准模式
- handler 表是全局唯一的，不需要 per-instance 存储

### D4: HalProvider 部分实现

v0.6.0 仅实现 `HalCpu`/`HalIrq`/`HalClock`，不实现完整 `HalProvider`。理由：
- `HalProvider` 需要全部 6 个 trait 实现（mem/serial/gpio 属于 v0.7.0）
- 提供 `Arm64HalCoreProvider`（仅 cpu/irq/clock）作为过渡，mem/serial/gpio 方法返回 `NotSupported`
- 完整 `HalProvider` 在 v0.7.0 补全

### D5: QEMU 运行时验证延后

v0.6.0 不做 QEMU 实际中断触发验证。理由：
- seL4 构建集成尚未完成（v0.4.0 D4 决策）
- 中断响应需要完整的异常向量表（属于 kernel crate 范围）
- v0.6.0 验证以交叉编译通过 + 寄存器常量正确性为主
- 运行时验证延后到 kernel 集成版本

### D6: core_count 从编译期配置读取

`Arm64Cpu::core_count()` 从编译期常量读取（默认 4），而非运行时 DTS 解析。理由：
- DTS 解析需要 device tree 库（未引入）
- QEMU virt 默认 4 核
- 后续版本可通过 `hal/arm64/config.rs` 覆盖

---

## 非目标（Non-Goals）

- **不实现** `HalMem`/`HalSerial`/`HalGpio`（属于 v0.7.0）
- **不实现**完整 `HalProvider`（属于 v0.7.0）
- **不做** QEMU 中断触发运行时验证（延后到 kernel 集成）
- **不做** GICv2 兼容层（蓝图 §8.4 提到，但非本版本目标）
- **不做**多核 GICR 遍历（简化为单核，多核 TODO）
- **不集成**到 kernel/runtime 调用链（属于后续版本）

---

## 风险与缓解

| 风险 | 等级 | 缓解 |
|------|------|------|
| GICv3 初始化复杂 | 中/高 | 参考蓝图 §4.5 代码 + ARM GICv3 手册逐寄存器 |
| inline asm 语法 | 低/中 | 使用 `core::arch::asm!` nightly 语法，参考 sel4-sys |
| static mut 安全性 | 低/中 | 限于 register/dispatch 两处，unsafe 块有注释 |
| 多核 GICR 定位 | 中/中 | v0.6.0 简化为单核，多核 TODO |
| 定时器频率未知 | 低/低 | 从 CNTPFRQ_EL0 读取，QEMU virt 默认 62.5MHz |

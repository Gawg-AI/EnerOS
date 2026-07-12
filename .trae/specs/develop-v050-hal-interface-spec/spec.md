# EnerOS v0.5.0 — HAL 接口规范设计 Spec

> **版本**：v0.5.0（Phase 0 / P0-B 起点）
> **类型**：纯设计版本（trait 接口规范 + mock 编译验证，无硬件实现）
> **前序依赖**：v0.4.0（第一个 Rust 用户态组件）
> **后续版本**：v0.6.0（HAL ARM64 核心实现）、v0.7.0（HAL ARM64 外设实现）
> **蓝图依据**：`蓝图/phase0.md` §v0.5.0（第 792–1038 行）
> **合规性**：蓝图 §43.1（no_std）、§43.2（非瓶颈版本：trait/struct 签名必须可编译）

---

## Why

HAL（Hardware Abstraction Layer）是硬件与内核之间的契约层。v0.5.0 先行定义完整的 trait 接口规范集（`HalCpu` / `HalMem` / `HalIrq` / `HalClock` / `HalSerial` / `HalGpio`），让后续 v0.6.0/v0.7.0 的 ARM64 实现以及未来飞腾/鲲鹏/RISC-V 的 BSP 实现有统一的契约可依，避免实现返工。本版本为纯设计版本，交付可编译的 trait 定义 + mock 实现 + 接口规范文档。

---

## What Changes

- **新增** crate `eneros-hal`（no_std 库，无 `panic_handler`，可参与 host 端单元测试）
- **新增** `hal/src/types.rs`：公共类型（`MemFlags` / `IrqTrigger` / `HalError` / `GpioDir` / `GpioConfig` / `PullMode` / `IrqHandler` / `IrqAction`）
- **新增** `hal/src/lib.rs`：6 个核心 HAL trait（`HalCpu` / `HalMem` / `HalIrq` / `HalClock` / `HalSerial` / `HalGpio`）+ `HalProvider` 注册器模式 + `init_hal()` / `hal()` 单例获取
- **新增** `hal/src/mock.rs`：mock 实现（`#[cfg(feature = "mock")]`），用于编译期接口验证
- **新增** `hal/Cargo.toml`：crate 配置，含 `mock` feature
- **修改** workspace 根 `Cargo.toml`：members 增加 `"hal"`，version `0.4.0` → `0.5.0`
- **修改** `ci/src/gate.rs`：clippy/test 排除项无需新增（hal 为库 crate，可参与 host 测试）
- **修改** `.github/workflows/ci.yml`：版本标识 v0.4.0 → v0.5.0，cross-build 新增 `eneros-hal` 构建步骤
- **修改** `Makefile`：VERSION 0.4.0 → 0.5.0，新增 `hal-build` / `hal-test` 目标
- **新增** 文档：`docs/hal-interface-spec.md`（《HAL 接口规范》）、`docs/hal-design-whitepaper.md`（《HAL 设计白皮书》）

---

## Impact

- **Affected specs**：v0.6.0（HAL ARM64 核心实现）将实现 v0.5.0 定义的 `HalCpu`/`HalIrq`/`HalClock`；v0.7.0 将实现 `HalMem`/`HalSerial`/`HalGpio`
- **Affected code**：
  - 新增 `hal/` 目录（~350 行代码 + ~200 行 mock）
  - 工作区 `Cargo.toml`、CI 配置、Makefile、质量门禁
- **Affected docs**：2 份新文档（HAL 接口规范 + 设计白皮书）
- **不影响**：现有 kernel/runtime/board/sel4-sys/hello crate 的功能行为

---

## ADDED Requirements

### Requirement: HAL Trait 接口规范集

系统 SHALL 提供完整的 HAL trait 接口规范集，覆盖 6 类硬件抽象：CPU、内存、中断、时钟、串口、GPIO。所有 trait MUST 满足：
1. `#![no_std]` 兼容（蓝图 §43.1）
2. trait object 安全（`dyn` 兼容，蓝图 §8.4/5.4）
3. 不使用 `async fn`（蓝图 §8.5，no_std 不稳定）
4. 每个方法有文档注释说明契约（蓝图 §9.5）

#### Scenario: 6 个 trait 定义完整且可编译

- **WHEN** 执行 `cargo build -p eneros-hal`
- **THEN** 编译成功，无错误
- **AND** `HalCpu` / `HalMem` / `HalIrq` / `HalClock` / `HalSerial` / `HalGpio` 六个 trait 均已定义

#### Scenario: trait object 安全性

- **WHEN** 将 trait 作为 `&'static dyn HalCpu` 使用
- **THEN** 编译通过（无 `Sized` 约束冲突，无泛型方法，无 `Self` 返回）

### Requirement: HalProvider 注册器模式

系统 SHALL 提供 `HalProvider` trait 作为 BSP 注入点，通过 `init_hal(provider)` 在启动早期注入实现，`hal()` 获取全局 HAL 引用。

#### Scenario: 全局 HAL 初始化与获取

- **WHEN** 调用 `init_hal(&provider)` 注入 BSP 实现
- **AND** 随后调用 `hal().cpu().current_core()`
- **THEN** 返回注入的 BSP 实现的结果

#### Scenario: 未初始化访问

- **WHEN** 在 `init_hal()` 之前调用 `hal()`
- **THEN** panic 并提示 `"HAL not initialized"`

### Requirement: 公共类型定义

系统 SHALL 在 `hal/src/types.rs` 中定义以下公共类型，供所有 HAL trait 共享：

- `MemFlags`：内存映射标志（readable/writable/executable/device/cacheable）
- `IrqTrigger`：中断触发类型（Edge/Level）
- `HalError`：统一错误码（InvalidParam/OutOfResource/NotSupported/HardwareFault/PermissionDenied）
- `GpioDir`：GPIO 方向（Input/Output）
- `GpioConfig`：GPIO 配置（pin/dir/pull）
- `PullMode`：上下拉模式（None/Up/Down）
- `IrqHandler`：中断处理函数签名 `fn(irq: u32) -> IrqAction`
- `IrqAction`：中断处理结果（Handled/WakeThread/Disabled）

#### Scenario: 类型可构造与匹配

- **WHEN** 构造 `MemFlags { readable: true, .. }` 并匹配 `HalError::NotSupported`
- **THEN** 编译通过，类型派生 `Clone`/`Copy`/`Debug`（除 `IrqHandler` 为函数指针类型别名）

### Requirement: Mock 实现编译验证

系统 SHALL 在 `hal/src/mock.rs` 中提供 `#[cfg(feature = "mock")]` 的 mock 实现，覆盖所有 6 个 trait，用于编译期接口验证与单元测试。

#### Scenario: mock feature 启用时编译通过

- **WHEN** 执行 `cargo build -p eneros-hal --features mock`
- **THEN** 编译成功，`MockHal` 实现了全部 6 个 trait

#### Scenario: mock 单元测试通过

- **WHEN** 执行 `cargo test -p eneros-hal --features mock`
- **THEN** 所有测试通过，覆盖类型构造、mock 行为、错误返回

### Requirement: no_std 合规

`eneros-hal` crate MUST 遵循蓝图 §43.1：`#![no_std]`，不使用 `std::*`，仅使用 `core::*`。允许使用 `alloc` 仅在必要时（本版本预计不需要 alloc）。

#### Scenario: 无 std 依赖

- **WHEN** 在 crate 根标记 `#![no_std]`
- **AND** 交叉编译到 `aarch64-unknown-none`
- **THEN** 编译成功

### Requirement: 文档交付

系统 SHALL 交付两份文档：

1. `docs/hal-interface-spec.md`：《HAL 接口规范》——每个 trait 每个方法的契约、参数、返回值、错误码、调用约束
2. `docs/hal-design-whitepaper.md`：《HAL 设计白皮书》——trait 抽象选型理由、HalProvider 单例模式、dyn 安全性分析、与 seL4 libplatsupport/Linux HAL 对比、扩展路径

#### Scenario: 文档完整覆盖所有 trait

- **WHEN** 审阅《HAL 接口规范》
- **THEN** 6 个 trait 的所有方法均有契约说明

### Requirement: 工作区集成

`eneros-hal` MUST 集成到 workspace、CI、Makefile、质量门禁，与现有 crate 保持一致的构建体验。

#### Scenario: workspace 包含 hal

- **WHEN** 执行 `cargo build --workspace`
- **THEN** `eneros-hal` 被编译

#### Scenario: CI 交叉编译 hal

- **WHEN** CI 运行 cross-build 任务
- **THEN** `eneros-hal` 被交叉编译到 `aarch64-unknown-none`

#### Scenario: 质量门禁覆盖 hal

- **WHEN** 执行 `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello`
- **THEN** `eneros-hal` 参与 clippy 检查（作为库 crate，无 panic_handler 冲突）

---

## MODIFIED Requirements

### Requirement: Workspace 版本与成员

workspace 根 `Cargo.toml` 的 version 从 `0.4.0` 升级到 `0.5.0`，members 从 `["kernel", "runtime", "ci", "board", "sel4-sys", "hello"]` 扩展为 `["kernel", "runtime", "ci", "board", "sel4-sys", "hello", "hal"]`。

### Requirement: CI 流水线版本与构建步骤

`.github/workflows/ci.yml` 的版本标识从 v0.4.0 升级到 v0.5.0，cross-build 任务新增 `Build hal crate` 步骤。

### Requirement: Makefile 构建入口

`Makefile` 的 VERSION 从 0.4.0 升级到 0.5.0，新增 `hal-build` 和 `hal-test` 目标，help 文本同步更新。

---

## 设计决策（Design Decisions）

### D1: 新 crate 置于顶层目录

`eneros-hal` 放在 workspace 顶层（非 `hal/` 子目录下再嵌套），遵循 v0.4.0 确立的"crate 平铺"原则（与 board/sel4-sys/runtime 一致），符合工作区规则 §2.3（不超过 3 层嵌套）。

### D2: no_std 库 crate，无 panic_handler

`eneros-hal` 作为库 crate，不定义 `#[panic_handler]` / `#![no_main]`，使其可参与 host 端 clippy/test（与 board/sel4-sys/runtime 一致）。panic_handler 由二进制 crate（kernel/hello）提供。

### D3: HalProvider 单例注入模式

采用蓝图 §4.5 的 `HalProvider` trait + `static mut HAL: Option<&'static dyn HalProvider>` 单例模式。理由：
- 避免每调用方传递 `&dyn HalProvider` 的侵入性
- BSP 在启动早期调用 `init_hal()` 注入，后续 `hal()` 获取
- `unsafe` 块限于 init_hal/hal 两处，契约清晰

### D4: mock 实现通过 feature 门控

mock 实现放在 `hal/src/mock.rs`，用 `#[cfg(feature = "mock")]` 门控。默认构建不含 mock（保持精简），`--features mock` 启用用于测试。这与蓝图 §4.5 代码一致。

### D5: 避免 async fn in traits

蓝图 §8.5 明确指出 `async fn` 在 no_std trait 中不稳定，本版本所有 trait 方法为同步签名。未来若需异步，使用 `embedded-io` 风格的轮询接口或显式状态机。

### D6: trait object 安全性保障

所有 6 个 trait 的方法：
- 不带泛型参数（避免 monomorphization 与 dyn 冲突）
- 不返回 `Self`
- 不带 `where Self: Sized` 约束（除非该方法专为静态分发设计）
确保 `&'static dyn HalXxx` 可用。

### D7: HalError 不实现 PartialEq

`HalError` 仅派生 `Debug`（蓝图 §4.1），不派生 `PartialEq`。理由：硬件实现可能携带额外上下文，强等比较无意义；测试用 `matches!()` 宏匹配变体即可。

---

## 非目标（Non-Goals）

- **不实现**任何具体硬件的 HAL（ARM64/GICv3/PL011 实现属于 v0.6.0/v0.7.0）
- **不实现** `HalProvider` 的真实 BSP（属于 v0.6.0）
- **不集成**到 kernel/runtime 的调用链（属于 v0.6.0+）
- **不做** QEMU 启动验证（纯设计版本，无运行时行为）
- **不做**性能测试（trait 静态分发零开销，无需测量）

---

## 风险与缓解

| 风险 | 等级 | 缓解 |
|------|------|------|
| 接口过度设计 | 中/中 | 保持最小必要，仅定义蓝图明确要求的方法 |
| trait object 限制 | 低/中 | 拆分细粒度 trait，避免泛型方法 |
| `static mut` 不安全 | 低/低 | 限于 init/hal 两处，文档标注契约 |
| 未来 RISC-V 兼容 | 低/中 | trait 不绑定 ARM64 语义，BSP 层处理架构差异 |

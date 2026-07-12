# EnerOS v0.14.0 — Panic 处理框架 Spec

> **蓝图依据**：`蓝图/phase0.md` §v0.14.0（第 2988-3167 行）
> **合规性**：蓝图 §43.1（no_std 全项目硬性要求）、§43.2（非瓶颈版本，签名必须可编译）
> **Phase 0 定位**：P0-D 终点，可靠性闭环，"基础 OS 服务就绪"出口标准的组成

---

## Why

EnerOS 当前（v0.13.0 及之前）的 `#[panic_handler]` 实现过于简陋：
- [kernel/src/lib.rs](file:///e:/eneros/kernel/src/lib.rs#L25-L29) 仅死循环，无任何诊断输出
- [hello/src/main.rs](file:///e:/eneros/hello/src/main.rs#L44-L49) 仅打印固定字符串后死循环，无 panic 上下文

这导致故障无法诊断、无法隔离、无法恢复。v0.14.0 需要建立一个**可诊断、可隔离、可恢复**的 panic 处理框架：分区级 panic 隔离使单分区故障不拖垮全系统；内核 panic 触发硬件复位；panic 日志在复位前同步输出到串口。这是 Phase 0 出口标准"基础 OS 服务就绪"的直接组成（见 phase0.md 第 5190 行、第 5174 行验收清单）。

---

## What Changes

### 新增
- 新建顶层 crate `panic/`（`eneros-panic`），提供 panic 处理框架（PanicContext / PanicStrategy / 分级策略 / 日志器 / 分区隔离）
- 新建文档 `docs/panic-framework-design.md`（Panic 处理框架设计）
- 新建文档 `docs/partition-isolation-recovery.md`（分区隔离恢复策略）

### 修改
- workspace `Cargo.toml`：members 添加 `"panic"`，version 升至 `0.14.0`
- `Makefile`：VERSION 升至 `0.14.0`，新增 `panic-build` / `panic-test` 目标
- `.github/workflows/ci.yml`：版本标识 v0.14.0，新增 panic crate cross-build 步骤
- `ci/src/gate.rs`：注释含 v0.14.0

### 不修改（外科手术原则）
- **不修改** `kernel/src/lib.rs`、`hello/src/main.rs` 中已有的 `#[panic_handler]`。原因见下方"关键设计决策 D2"。这些现有 panic_handler 的迁移到新框架是后续版本的工作，不在 v0.14.0 范围内。

---

## 关键设计决策（Karpathy 原则应用）

### D1：crate 不定义 `#[panic_handler]`
**原因**：Rust 规则——一个 binary 只能有一个 `#[panic_handler]`。现有 [kernel](file:///e:/eneros/kernel/src/lib.rs#L25) 和 [hello](file:///e:/eneros/hello/src/main.rs#L44) 已各自定义。若 `panic` crate 也定义，会导致符号冲突编译失败。同时这也与 [board crate](file:///e:/eneros/board/src/lib.rs#L6) 的既有设计决策一致（"本 crate 不定义 panic_handler 以便在 host 上单元测试"）。

**方案**：`panic` crate 提供 `handle_panic(info: &PanicInfo) -> !` 函数和框架 API。消费者（未来的 kernel/hello 迁移后）在自己的 `#[panic_handler]` 中委托调用 `eneros_panic::handle_panic(info)`。本版本只构建框架，不强制迁移现有 binary。

### D2：日志器使用固定缓冲区，不依赖 alloc
**原因**：蓝图 §5.4 明确指出"panic 时堆可能损坏，日志不能用 alloc"。PanicContext 的字段使用 `&'static str` 而非 `String`；日志格式化用静态栈缓冲区（`heapless::Vec<u8, N>` 或固定字节数组）。

### D3：aarch64 专用代码用 cfg gate
**原因**：core_id 读取（`mpidr_el1`）和硬件复位（`asm!("b 0x0")`）是 aarch64 专属。host 测试时提供 stub。参考 v0.6.0 HAL 的 `#[cfg(target_arch = "aarch64")]` 模式。

### D4：分区状态表用静态数组（非调度器集成）
**原因**：真正的分区调度器在 v0.18.0 才有。v0.14.0 实现一个静态的 `PartitionState` 表（固定 8 槽），支持标记 Dead 状态，但**不**做真正的线程回收/调度器移除——那是 v0.18.0 的工作。这符合 §43.2 非瓶颈版本"签名可编译，逻辑骨架"的要求。

### D5：复位策略可配置
**原因**：蓝图交付物要求"复位策略配置（立即复位/延迟复位）"。通过 `ResetPolicy` 枚举（`Immediate` / `Delayed(u64)` 延迟毫秒数）实现，存于全局静态配置。

### D6：依赖最小化
**原因**：Karpathy 简洁原则。仅依赖 `eneros-time`（获取 timestamp_ns）和 `spin`（Mutex，no_std 同步）。不依赖 `eneros-runtime`（避免循环依赖且 panic 时 runtime 可能不可用），串口输出通过注入 `SerialSink` trait 实现，由消费者提供。

---

## Impact

- **Affected specs**：P0-D（v0.12.0~v0.14.0）终点；为 v0.18.0 调度器（分区 Dead 标记）、v0.22.0 降级（依赖 panic 隔离）奠基
- **Affected code**：
  - 新增：`panic/Cargo.toml`、`panic/src/{lib.rs, isolation.rs, logger.rs}`
  - 修改：`Cargo.toml`（workspace）、`Makefile`、`.github/workflows/ci.yml`、`ci/src/gate.rs`
  - **不修改**：`kernel/src/lib.rs`、`hello/src/main.rs`（现有 panic_handler 保持不变，D1）
- **依赖关系**：
  - 依赖 v0.11.0（用户态堆，已满足）—— 但本框架自身不使用 alloc（D2）
  - 依赖 v0.12.0（`eneros-time`，已满足）—— 用于 timestamp_ns
- **回归风险**：低。新增 crate，不触碰现有 panic_handler，不影响 v0.13.0 看门狗

---

## ADDED Requirements

### Requirement: Panic 处理框架核心
系统 SHALL 提供 no_std panic 处理框架 crate `eneros-panic`，包含 panic 上下文、分级策略、日志器和分区隔离能力，且自身不定义 `#[panic_handler]`（避免与现有 binary 冲突）。

#### Scenario: 构造 PanicContext
- **WHEN** 调用 `PanicContext::new(level, location, message)`
- **THEN** 返回包含 level/location/message/timestamp_ns/core_id 的上下文，timestamp_ns 来自 `eneros_time::get_monotonic_ns()`

#### Scenario: 注册并调用策略
- **WHEN** 通过 `set_strategy()` 注册 `KernelResetStrategy`，然后调用 `handle_panic(info)`
- **THEN** 执行日志输出 → 等待 flush → 硬件复位（aarch64）或死循环（host）

### Requirement: 分级 Panic 策略
系统 SHALL 支持两种 panic 级别：`PanicLevel::Kernel`（触发硬件复位）和 `PanicLevel::Partition(u32)`（隔离该分区）。

#### Scenario: 内核 panic 复位
- **WHEN** panic 级别为 Kernel
- **THEN** 日志输出到串口后，aarch64 执行 `asm!("b 0x0")` 跳转复位向量；host 死循环

#### Scenario: 分区 panic 隔离
- **WHEN** panic 级别为 Partition(id)
- **THEN** 标记该分区状态为 Dead，输出日志，死循环（v0.14.0 不做真正资源回收，留待 v0.18.0）

### Requirement: Panic 日志器（无 alloc）
系统 SHALL 提供固定缓冲区的 panic 日志器，panic 时不依赖堆分配，输出到注入的 `SerialSink`。

#### Scenario: 日志格式化输出
- **WHEN** 调用 `panic_log(ctx)`
- **THEN** 使用静态栈缓冲区格式化 "[PANIC] level=KERNEL/Partition(N) loc=file:line msg=... core=N t=Ns" 并输出到 SerialSink

#### Scenario: 堆损坏时仍可输出
- **WHEN** panic 发生时堆已损坏
- **THEN** 日志器仍能输出（因不使用 alloc，仅用 `core::fmt` + 固定缓冲区）

### Requirement: 分区隔离状态表
系统 SHALL 维护一个静态的分区状态表（固定 8 槽），支持标记分区为 Dead 状态，供未来调度器查询。

#### Scenario: 标记分区 Dead
- **WHEN** 调用 `mark_partition_dead(id)`
- **THEN** 该槽位状态变为 Dead，`partition_state(id)` 返回 `PartitionState::Dead`

#### Scenario: 分区隔离失败升级
- **WHEN** 分区 id 超出表范围（≥8）或状态表损坏
- **THEN** 升级为内核 panic 复位（蓝图 §4.4 错误处理）

### Requirement: 复位策略配置
系统 SHALL 支持可配置的复位策略：`ResetPolicy::Immediate`（立即复位）或 `ResetPolicy::Delayed(u64)`（延迟 N 毫秒后复位）。

#### Scenario: 延迟复位
- **WHEN** 策略为 `Delayed(100)` 且 panic 触发
- **THEN** 日志输出后等待 100ms 再执行硬件复位

---

## MODIFIED Requirements

### Requirement: Workspace 版本基线
workspace `Cargo.toml` 的 version 从 `0.13.0` 升至 `0.14.0`，members 列表新增 `"panic"`。

---

## REMOVED Requirements

无。本版本为纯新增，不删除任何既有能力。

---

## 蓝图符合性核对

| 蓝图条目 | 对应实现 |
|---------|---------|
| §1 核心目标：no_std panic_handler | D1：crate 提供 `handle_panic()` 供消费者 `#[panic_handler]` 委托 |
| §3 交付物：lib.rs(~200行)/isolation.rs(~150行)/logger.rs(~120行) | 三模块对应 |
| §4.1 PanicLevel/PanicContext | 完整实现 |
| §4.2 PanicStrategy trait + KernelResetStrategy + PartitionIsolateStrategy | 完整实现 |
| §4.4 错误处理（日志落盘失败→仅串口；隔离失败→升级内核 panic） | 实现 |
| §5.4 难点：panic 时堆损坏，日志不能用 alloc | D2：固定缓冲区 |
| §6 测试计划（单元≥80%/集成/性能<50ms/回归v0.13.0/故障注入） | checklist 覆盖 |
| §7 验收标准（日志串口/分区隔离/内核复位/文档） | checklist 覆盖 |
| §43.1 no_std | `#![cfg_attr(not(test), no_std)]`（参考 v0.13.0 watchdog 模式） |
| §43.2 非瓶颈版本签名可编译 | 所有 trait/struct 签名完整可编译 |
| §43.3 GPU | N/A（蓝图 §6.6 明确） |

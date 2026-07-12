# EnerOS v0.13.0 硬件看门狗 Spec

## Why

EnerOS 当前（v0.12.0）已有时钟服务，但缺乏系统崩溃的最后一道防线——硬件看门狗。
无看门狗则软件卡死后无法自动复位，无法满足工控可靠性要求。v0.13.0 是 P0-D（时钟看门狗 Panic）
第二步，为 v0.22.0（Agent 崩溃降级）和 v0.58.0（端到端降级）提供可靠性兜底。

## What Changes

- 新建 `watchdog` crate（no_std），实现 SP805 兼容硬件看门狗驱动 + 分层喂狗
- SP805 WDT 驱动：init/kick/stop，通过 MMIO 操作寄存器（WDT_LOAD/WDT_CTRL/WDT_INTCLR/WDT_LOCK）
- 分层喂狗：8 槽 FeedLayer 数组，每层独立注册和喂狗，超时检测（警告 vs 硬复位）
- 全局 API：`wdt_init()` / `wdt_kick()` / `wdt_register_layer()` / `wdt_feed_layer()` / `wdt_check()`
- 复用 v0.12.0 `eneros_time::get_monotonic_ns()` 获取时间戳
- base=0 时为"软件模式"（无 MMIO），用于 QEMU 和测试环境
- 更新 workspace 版本号至 0.13.0
- 新增 2 篇文档：《看门狗设计》、《分层喂狗协议》

## Impact

- **Affected specs**: 无（新功能，不修改现有 spec）
- **Affected code**:
  - 新增 `watchdog/` crate（4 个源文件 + Cargo.toml）
  - 修改 `Cargo.toml`（workspace members + 版本号）
  - 修改 `Makefile`（VERSION + 新目标）
  - 修改 `.github/workflows/ci.yml`（版本标识 + cross-build 步骤）
  - 修改 `ci/src/gate.rs`（注释更新）
- **Affected docs**: 新增 `docs/watchdog-design.md`、`docs/layered-feeding-protocol.md`

## ADDED Requirements

### Requirement: SP805 硬件看门狗驱动

系统 SHALL 提供 SP805 兼容的硬件看门狗驱动，支持初始化、喂狗和停止。

#### Scenario: 初始化看门狗
- **WHEN** 调用 `wdt_init(timeout_ms, wdt_base)`
- **THEN** WDT 加载超时值，使能复位模式，锁定寄存器

#### Scenario: 喂狗
- **WHEN** 调用 `wdt_kick()`
- **THEN** WDT_INTCLR 寄存器被写入，重置倒计时

#### Scenario: 软件模式（base=0）
- **WHEN** wdt_base 为 0
- **THEN** 所有 MMIO 操作为 no-op，分层喂狗逻辑仍正常工作

#### Scenario: 停止看门狗
- **WHEN** 调用 `wdt_stop()`
- **THEN** WDT_CTRL 清零，看门狗禁用（调试用）

### Requirement: 分层喂狗

系统 SHALL 支持分层喂狗，允许不同子系统（内核/Runtime/Agent）各自注册喂狗层。

#### Scenario: 注册喂狗层
- **WHEN** 调用 `wdt_register_layer(name, period_ms)`
- **THEN** 返回 `LayerId`，该层被加入喂狗列表

#### Scenario: 喂特定层
- **WHEN** 调用 `wdt_feed_layer(id)`
- **THEN** 该层的 `last_feed_ns` 更新为当前单调时间

#### Scenario: 层超时（警告）
- **WHEN** 某层超过 `period_ms` 未喂狗，但未超过 `hard_timeout_ms`
- **THEN** `wdt_check()` 返回 `LayerTimeout(id)`，仍执行硬件 kick

#### Scenario: 层超时（硬复位）
- **WHEN** 某层超过 `hard_timeout_ms` 未喂狗
- **THEN** `wdt_check()` 停止硬件 kick，返回 `HardReset`，触发硬件复位

#### Scenario: 所有层正常
- **WHEN** 所有已启用层均在 `period_ms` 内喂狗
- **THEN** `wdt_check()` 执行硬件 kick，返回 `AllFed`

#### Scenario: 层槽位已满
- **WHEN** 8 个槽位全部占用时注册新层
- **THEN** 返回 `None`（注册失败）

### Requirement: 全局 API

系统 SHALL 提供统一的全局看门狗 API 接口。

#### Scenario: 未初始化
- **WHEN** 未调用 `wdt_init()` 就调用 `wdt_kick()` 或 `wdt_check()`
- **THEN** 操作为 no-op，不 panic

### Requirement: no_std 合规

`watchdog` crate SHALL 遵循蓝图 §43.1 no_std 要求，正式构建为 no_std，测试构建链接 std。

### Requirement: 时间戳复用

`watchdog` crate SHALL 复用 v0.12.0 `eneros_time::get_monotonic_ns()` 获取单调时间戳，不重复实现时钟读取。

## MODIFIED Requirements

无。

## REMOVED Requirements

无。

---

## 设计决策

### D1: 新建顶层 `watchdog/` crate

与 `heap/`、`mm/`、`time/` 一致，`watchdog` 作为顶层 workspace 成员。

### D2: SP805 WDT 驱动（兼容 QEMU 和真实硬件）

SP805 是 ARM 标准 WDT IP，飞腾/鲲鹏等国产 SoC 内置 WDT 多兼容此接口。
寄存器：WDT_LOAD(0x00)/WDT_VALUE(0x04)/WDT_CTRL(0x08)/WDT_INTCLR(0x0c)/WDT_LOCK(0xC00)。
QEMU virt 默认无 WDT，base=0 时为"软件模式"——MMIO 操作为 no-op，分层喂狗逻辑仍工作。

### D3: 8 槽数组式分层喂狗

与 TimerWheel 一致的设计风格，8 个 `Option<FeedLayer>` 槽位，足够 Phase 0 使用
（内核/Runtime/Agent/预留 5 层）。`LayerId` 从 1 递增。

### D4: 复用 v0.12.0 时间服务

`watchdog` crate 依赖 `eneros-time`，通过 `eneros_time::get_monotonic_ns()` 获取时间戳。
不重复 `cntpct_el0` 内联汇编。

### D5: `spin::Mutex` 线程安全

全局状态用 `spin::Mutex` 保护，支持中断上下文和线程上下文并发访问。

### D6: `cfg_attr(not(test), no_std)` 模式

正式构建 no_std，测试构建链接 std。与 `heap`、`time` crate 一致。

### D7: 超时检测两级判定

- `period_ms` 超时 → `LayerTimeout` 警告，仍 kick（该层可能暂时繁忙）
- `hard_timeout_ms` 超时 → `HardReset`，停止 kick 触发硬件复位

使用 `saturating_sub` 防止时间差下溢。

### D8: 修复蓝图代码中的 bug

蓝图 `check()` 方法中 `LayerTimeout(0)` 总是返回 0 而非实际层 ID。
实现时修正为返回第一个超时层的 `LayerId`。

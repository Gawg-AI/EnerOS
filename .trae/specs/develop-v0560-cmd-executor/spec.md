# v0.56.0 命令消费与执行 Spec

## Why

v0.22.0 已建立 Control Bus（`command_send`/`command_consume`）与 TTL/约束检查函数，v0.54.0/v0.55.0 已建立控制闭环与采样服务，但 **从 Control Bus 消费命令 → TTL 检查 → 约束包校验 → 协议下发** 的完整执行链路尚未串联。本版本填补该空白，实现 Agent → RTOS 快平面的命令落地环节，是双平面协同的关键。

## What Changes

- **新增 crate** `eneros-rtos-cmd-exec`（位于 `crates/kernel/rtos-cmd-exec/`），归入 kernel 子系统（P1-H RTOS 组件第三层）
- 新增 `CommandExecutor<P, S>` 泛型命令执行器（泛型 `<P: PointAccess, S: DeviceStateProvider>`，不用 `Box<dyn>`）
- 新增 `DeviceStateProvider` trait —— 抽象设备状态来源（生产环境可由 v0.55.0 SamplingService 提供，测试用 MockDeviceStateProvider）
- 新增 `DevicePointMap` —— `controlbus::DeviceId(pub u32)` → `upa_model::PointId(u32)` 映射表（ControlCommand 的 target_device 是 controlbus 命名空间，write_point 需要 upa-model 命名空间的 PointId）
- 新增 `ExecutorStats` —— 成功/失败/过期/拒绝/截断 计数（无 `log_warn!`/`log_error!`，D7）
- 新增 `ExecutorReport` —— 单次 `tick()` 的执行汇总
- 复用 v0.22.0 的 `ttl_check()` / `constraint_check()` / `command_consume()`（D1，不重新实现）
- Emergency 旁路：`ControlAction::Emergency` 立即下发 0.0，跳过 TTL 与约束检查（D8 安全优先）

## Impact

- **Affected specs**: v0.22.0（Control Bus — 复用其 API，不修改）、v0.51.0（PointAccess — 复用 trait，不修改）、v0.54.0（ControlLoopEngine — 可选集成，不修改）、v0.55.0（SamplingService — 可选作为 DeviceStateProvider，不修改）
- **Affected code**: 仅新增 `crates/kernel/rtos-cmd-exec/`，修改 `Cargo.toml`（workspace members + 版本号）、`Makefile`、`ci.yml`、`ci/src/gate.rs`（版本同步）

## ADDED Requirements

### Requirement: CommandExecutor 命令执行器

系统 SHALL 提供 `CommandExecutor<P: PointAccess, S: DeviceStateProvider>` 泛型结构体，通过单步 `tick(now_ns) -> ExecutorReport` 接口从 Control Bus 消费命令、执行 TTL 检查、约束包校验、并通过 PointAccess 下发到设备。

#### Scenario: 正常命令执行
- **WHEN** Agent 通过 `command_send()` 下发一条 `ControlCommand`（action=Charge, setpoint=50.0, ttl_ms=100）
- **AND** 调用 `executor.tick(now_ns)`，其中 `now_ns` 在 TTL 内
- **AND** `DeviceStateProvider` 返回的设备状态满足约束包
- **THEN** `PointAccess::write_point(device_control_point, PointValue::Float(50.0))` 被调用
- **AND** `ExecutorReport.success == 1`，`ExecutorStats.success_count` 递增

#### Scenario: TTL 过期命令被丢弃
- **WHEN** 一条命令的 `elapsed_ms >= ttl_ms`
- **AND** 调用 `executor.tick(now_ns)`
- **THEN** 该命令不被下发到设备
- **AND** `ExecutorReport.expired == 1`，`ExecutorStats.expired_count` 递增

#### Scenario: 约束超限命令被截断
- **WHEN** 命令的 `setpoint` 超出 `constraints.[min_power, max_power]`
- **AND** 设备状态满足 SOC/电压/频率硬限制
- **THEN** `constraint_check` 返回 `Truncated(safe_value)`
- **AND** 截断后的 `safe_value` 被下发到设备
- **AND** `ExecutorReport.truncated == 1`

#### Scenario: 硬限制违反命令被拒绝
- **WHEN** 设备 SOC/电压/频率超出 `constraints` 硬限制
- **THEN** `constraint_check` 返回 `Rejected`
- **AND** 该命令不下发
- **AND** `ExecutorReport.rejected == 1`

#### Scenario: Emergency 紧急停机旁路
- **WHEN** 命令 `action == ControlAction::Emergency`
- **THEN** 跳过 TTL 检查与约束检查
- **AND** 立即向设备控制点下发 `PointValue::Float(0.0)`
- **AND** `ExecutorReport.success == 1`（Emergency 视为成功执行）

#### Scenario: Idle 空闲动作
- **WHEN** 命令 `action == ControlAction::Idle`
- **THEN** 仍经过 TTL + 约束检查
- **AND** 向设备下发 `PointValue::Float(0.0)`（setpoint 被忽略，D9）

#### Scenario: 写入失败统计
- **WHEN** `PointAccess::write_point()` 返回 `Err`
- **THEN** `ExecutorReport.failed == 1`，`ExecutorStats.failure_count` 递增

### Requirement: DeviceStateProvider 设备状态来源

系统 SHALL 提供 `DeviceStateProvider` trait，`CommandExecutor` 通过它获取 `DeviceState`（SOC/电压/频率/当前功率）用于约束检查。这解耦了执行器与状态采集源。

#### Scenario: 生产环境集成
- **WHEN** 生产环境中 v0.55.0 SamplingService 提供设备状态
- **THEN** 用户实现 `DeviceStateProvider` trait 包装 SamplingService
- **AND** `CommandExecutor` 通过 trait 调用获取状态

#### Scenario: 测试环境
- **WHEN** 单元测试中需要模拟设备状态
- **THEN** 使用 `MockDeviceStateProvider` 设置预设状态
- **AND** 可针对不同 DeviceId 返回不同状态

### Requirement: DevicePointMap 设备控制点映射

系统 SHALL 提供 `DevicePointMap`，将 `controlbus::DeviceId(pub u32)` 映射到 `upa_model::PointId(u32)`（该设备的控制设定值点）。这是因为 ControlCommand 的 `target_device` 是 controlbus 命名空间的类型，而 `PointAccess::write_point` 需要 upa-model 命名空间的 `PointId`。

#### Scenario: 已映射设备
- **WHEN** 命令的 `target_device` 在映射表中
- **THEN** 查到对应的 `PointId` 并下发

#### Scenario: 未映射设备
- **WHEN** 命令的 `target_device` 不在映射表中
- **THEN** 该命令跳过执行
- **AND** `ExecutorReport.unmapped == 1`

## MODIFIED Requirements

无（本版本为纯新增，不修改任何已有 crate）。

## REMOVED Requirements

无。

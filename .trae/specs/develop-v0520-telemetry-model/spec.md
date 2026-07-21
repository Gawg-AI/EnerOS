# v0.52.0 四遥标准数据模型 Spec

## Why

v0.51.0 定义了 `PointAccess` 协议抽象 trait，v0.50.0 定义了 UPA `DataPoint` 统一点模型，但电力 SCADA 系统的四遥（遥测/遥信/遥控/遥调）业务语义仍缺失：没有死区过滤（导致无效上报占用通信带宽）、没有变化上报机制、没有品质标志强制上报规则、没有 SBO（Select-Before-Operate）遥控安全语义。本版本在 UPA 模型基础上定义电力行业四遥专用数据模型，实现死区过滤、变化检测、品质传播，为 SOE 事件引擎（v0.53.0）和后续 Agent 业务提供标准数据语义。

## What Changes

### 新增 crate `eneros-telemetry-model`
- **位置**：`crates/protocols/telemetry-model/`（P1-G 四遥与 SOE 数据层，与 upa-model 同级）
- **依赖**：`eneros-upa-model`（path 依赖，复用 `PointId`/`DeviceId`/`PointValue` 类型）
- **零外部依赖**（除 upa-model 外）

### 新增四遥数据模型
- `QualityFlag` 枚举（Good/Invalid/Questionable/Substituted/Blocked/Overflow/Outdated）— IEC 60870-5 兼容
- `DigitalState` 枚举（Off/On/Intermediate/Bad）— 双位置遥信状态
- `SingleCommand` 枚举（Off/On）— 单点遥控
- `DoubleCommand` 枚举（Off/On/Intermediate/Bad）— 双点遥控
- `ControlCommand` 枚举（Single/Double）— 遥控命令统一封装
- `ControlExecState` 枚举（Idle/Selected/Executing/Done/Failed/Timeout）— 遥控执行状态机
- `Telemetry` 结构体 — 遥测（模拟量）：value/unit/quality/deadband/high_limit/low_limit/last_reported + should_report + check_quality
- `Telesignaling` 结构体 — 遥信（状态量）：value/quality/double_point/last_reported + should_report（状态变化立即上报）
- `Telecontrol` 结构体 — 遥控（控制命令）：command/quality/select_before_operate/exec_state
- `Teleadjust` 结构体 — 遥调（设定值）：setpoint/current_value/min_value/max_value/ramp_rate

### 新增 DeadbandFilter 死区过滤器
- `PointDeadband` 内部结构（deadband/last_reported/report_count/skip_count）
- `DeadbandFilter` 结构体（BTreeMap<PointId, PointDeadband>）
- 方法：`new` / `configure` / `should_report` / `force_report` / `get_stats`
- 品质变化时强制上报（`force_report`）

### 新增设计文档
- `docs/protocols/telemetry-model-design.md`

### 通用变更
- **更新** 根 `Cargo.toml`：版本号 0.51.0 → 0.52.0，members 增加 `crates/protocols/telemetry-model`
- **更新** `Makefile` / `ci.yml` / `gate.rs` 版本号同步

## Impact

- **Affected specs**: v0.50.0（upa-model，作为依赖被复用）、v0.51.0（PointAccess，四遥数据可通过 DataPoint 适配 PointAccess）
- **Affected code**:
  - `e:\eneros\Cargo.toml` — workspace 版本号 + members 列表
  - `e:\eneros\crates\protocols\telemetry-model\` — 新 crate
  - `e:\eneros\docs\protocols\telemetry-model-design.md` — 设计文档
  - `e:\eneros\Makefile` / `e:\eneros\.github\workflows\ci.yml` / `e:\eneros\ci\src\gate.rs` — 版本号
- **依赖关系**: v0.52.0 完成后解锁 v0.53.0（SOE 事件顺序记录引擎）、v0.54.0+（基于四遥的告警/控制业务）

## ADDED Requirements

### Requirement: 四遥数据模型（Telemetry/Telesignaling/Telecontrol/Teleadjust）

系统 SHALL 提供电力 SCADA 标准的四遥数据模型，每种模型包含对应的业务语义方法。

#### Scenario: 遥测死区过滤

- **WHEN** 调用 `telemetry.should_report()`
- **AND** 值变化量 ≤ deadband
- **THEN** 返回 `false`（不上报）
- **WHEN** 值变化量 > deadband
- **THEN** 返回 `true` 并更新 last_reported

#### Scenario: 遥测首次上报

- **WHEN** last_reported 为 None
- **THEN** should_report 返回 true（首次必须上报）

#### Scenario: 遥测品质检查

- **WHEN** 调用 `telemetry.check_quality()`
- **AND** 值超过 high_limit 或低于 low_limit
- **THEN** 品质置为 Questionable

#### Scenario: 遥信变化立即上报

- **WHEN** 遥信状态发生变化
- **THEN** should_report 返回 true（无死区）

#### Scenario: 遥控 SBO 语义

- **WHEN** select_before_operate 为 true
- **THEN** exec_state 必须经历 Selected → Executing → Done/Failed 流程

### Requirement: DeadbandFilter 批量死区过滤

系统 SHALL 提供批量死区过滤器，管理多个遥测点的死区配置和上报决策。

#### Scenario: 死区过滤

- **WHEN** 调用 `filter.should_report(point_id, value)`
- **AND** 值变化量 > deadband
- **THEN** 返回 true，report_count 递增

#### Scenario: 品质变化强制上报

- **WHEN** 品质从 Good → Invalid
- **THEN** 调用 `filter.force_report(point_id, value)` 强制上报

#### Scenario: 统计查询

- **WHEN** 调用 `filter.get_stats(point_id)`
- **THEN** 返回 (report_count, skip_count) 元组

### Requirement: QualityFlag 品质标志

系统 SHALL 提供 IEC 60870-5 兼容的品质标志枚举。

#### Scenario: 品质标志语义

- **WHEN** 设备故障或通信中断
- **THEN** 品质置为 Invalid
- **WHEN** 值越限
- **THEN** 品质置为 Questionable
- **WHEN** 人工置数
- **THEN** 品质置为 Substituted

## 偏差声明（D1~D7）

| 偏差 | 说明 |
|------|------|
| **D1** | 时间戳用 `u64` 毫秒参数注入（蓝图使用 `MonotonicTime`/`SystemTime` 类型，no_std 无此类型；与 v0.50.0 D1、v0.51.0 D5 一致） |
| **D2** | crate 放入 `crates/protocols/telemetry-model/`（P1-G 四遥数据层，与 upa-model 同属 protocols/ 子系统） |
| **D3** | 仅依赖 `eneros-upa-model`（复用 PointId/DeviceId/PointValue），不依赖 protocol-abstract（四遥是数据模型，非协议适配器） |
| **D4** | 不实现 `DeviceDriver` trait（数据模型，非设备驱动，与 v0.48.0~v0.51.0 一致） |
| **D5** | 不实现 `PointAccess` trait（四遥是数据定义层；PointAccess 是协议适配器层；四遥数据通过包装为 DataPoint 即可被 PointAccess 访问） |
| **D6** | DeadbandFilter 使用 `BTreeMap`（no_std 无 HashMap，BTreeMap 是 no_std 兼容选择） |
| **D7** | 不要求 `Send + Sync`（no_std 单线程场景，与 v0.51.0 D2 一致） |

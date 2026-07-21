# v0.57.0 降级规则引擎 Spec

## Why

v0.56.0 已建立命令消费与执行链路（TTL + 约束 + 下发），但当 Agent 崩溃或通信中断时，RTOS 快平面需要接管控制权，执行安全降级策略。本版本实现降级规则引擎，定义多种降级模式（Normal/HoldOutput/StopCharge/SafeDefault/EmergencyStop），根据故障上下文按规则优先级自动选择降级模式，并执行对应的下发动作。这是 P1-H RTOS 组件第四层（安全降级层），为 v0.58.0 ★ 看门狗与端到端降级流程奠定基础。

## What Changes

- **新增 crate** `eneros-rtos-degrade`（位于 `crates/kernel/rtos-degrade/`），归入 kernel 子系统（P1-H RTOS 组件第四层）
- 新增 `DegradeMode` 枚举（5 变体：Normal/HoldOutput/StopCharge/SafeDefault/EmergencyStop），派生 `Ord` 以支持严重程度比较
- 新增 `DegradeRule` trait（**不要求 `Send + Sync`**，D6）— `name() / priority() / evaluate(&DegradeContext) -> Option<DegradeMode>`
- 新增 `DegradeContext` 结构体（agent_alive / agent_last_heartbeat_ns / control_bus_active / device_comm_ok / battery_soc / grid_frequency / temperature）— `now_ns: u64` 注入（D5），不使用 `MonotonicTime`
- 新增 `DegradeEngine<P: PointAccess>` 泛型降级引擎（不用 `Box<dyn PointAccess>`，D6）
- 新增 `SafeDefaults` 类型（`BTreeMap<PointId, f64>` 封装）— 安全默认值表
- 新增 `DegradeStats` 统计（mode_switch_count / last_mode / evaluations_count）— 无 `log_warn!`（D7）
- 新增 `DegradeReport` 单次评估汇总
- 新增内置规则集：`AgentDeadRule` / `ControlBusDownRule` / `DeviceCommFailRule` / `LowBatteryRule` / `OverTempRule`
- 复用 v0.51.0 `PointAccess` trait 下发降级动作
- 复用 v0.56.0 `DevicePointMap` 做 DeviceId→PointId 映射

## Impact

- **Affected specs**: v0.51.0（PointAccess — 复用 trait，不修改）、v0.56.0（DevicePointMap + PointAccess — 复用，不修改）
- **Affected code**: 仅新增 `crates/kernel/rtos-degrade/`，修改 `Cargo.toml`（workspace members + 版本号）、`Makefile`、`ci.yml`、`ci/src/gate.rs`（版本同步）

## ADDED Requirements

### Requirement: DegradeMode 降级模式

系统 SHALL 提供 `DegradeMode` 枚举，表示降级严重程度层级，派生 `Ord` 以支持比较。严重程度递增：Normal(0) < HoldOutput(1) < StopCharge(2) < SafeDefault(3) < EmergencyStop(4)。

#### Scenario: 模式比较
- **WHEN** 比较 `DegradeMode::Normal` 与 `DegradeMode::EmergencyStop`
- **THEN** `Normal < EmergencyStop` 为真（严重程度递增）

### Requirement: DegradeRule 降级规则 trait

系统 SHALL 提供 `DegradeRule` trait，**不要求 `Send + Sync`**（D6，no_std 单线程），定义 `name() / priority() / evaluate(&DegradeContext) -> Option<DegradeMode>` 三方法。规则按 `priority()` 降序评估，首个返回 `Some(mode)` 的规则决定降级模式。

#### Scenario: 规则优先级仲裁
- **WHEN** 多个规则同时触发不同模式
- **THEN** 取最高优先级规则返回的模式（priority 值越大优先级越高）
- **AND** 一旦触发则不再评估低优先级规则

#### Scenario: 无规则触发
- **WHEN** 所有规则返回 `None`
- **THEN** 降级模式为 `Normal`

### Requirement: DegradeContext 降级上下文

系统 SHALL 提供 `DegradeContext` 结构体，包含 Agent 存活状态、最后心跳时间戳、控制总线状态、设备通信状态、电池 SOC、电网频率、温度等字段。`now_ns: u64` 作为外部注入参数（D5），不使用 `MonotonicTime`。

#### Scenario: 上下文构造
- **WHEN** 调用方构造 `DegradeContext`
- **THEN** 所有字段可由调用方填充（生产环境由 v0.55.0 SamplingService + v0.22.0 fallback 提供）

### Requirement: DegradeEngine 降级引擎

系统 SHALL 提供 `DegradeEngine<P: PointAccess>` 泛型降级引擎，通过单步 `evaluate(context) -> DegradeReport` 接口按规则优先级评估降级模式，并在模式切换时执行对应的下发动作。

#### Scenario: 模式切换触发动作
- **WHEN** `evaluate()` 返回的新模式与 `current_mode` 不同
- **THEN** 执行 `on_mode_change(from, to)` 下发动作
- **AND** 更新 `previous_mode` 与 `current_mode`
- **AND** `DegradeStats.mode_switch_count` 递增

#### Scenario: HoldOutput 模式
- **WHEN** 切换到 `HoldOutput`
- **THEN** 不执行任何下发动作（保持当前设备设定值不变）

#### Scenario: StopCharge 模式
- **WHEN** 切换到 `StopCharge`
- **THEN** 向 `DevicePointMap` 中映射的功率控制点下发 `PointValue::Float(0.0)`

#### Scenario: SafeDefault 模式
- **WHEN** 切换到 `SafeDefault`
- **THEN** 遍历 `SafeDefaults` 表，向每个点下发预设安全值

#### Scenario: EmergencyStop 模式
- **WHEN** 切换到 `EmergencyStop`
- **THEN** 向 `DevicePointMap` 中映射的紧急停机点下发 `PointValue::Bool(true)`

#### Scenario: 模式不变
- **WHEN** 新模式等于 `current_mode`
- **THEN** 不触发 `on_mode_change`，不执行下发动作
- **AND** `DegradeStats.mode_switch_count` 不递增

#### Scenario: 恢复回切
- **WHEN** 故障恢复后规则返回 `Normal`
- **THEN** 从降级模式切回 `Normal`
- **AND** 不执行下发动作（Normal 模式由 Agent 接管）

### Requirement: 内置规则集

系统 SHALL 提供 5 个内置降级规则，覆盖常见故障场景：

| 规则 | 优先级 | 触发条件 | 返回模式 |
|------|--------|---------|---------|
| `AgentDeadRule` | 100 | `!agent_alive` 或 `now_ns - agent_last_heartbeat_ns > HEARTBEAT_TIMEOUT_NS` | `SafeDefault` |
| `ControlBusDownRule` | 90 | `!control_bus_active` | `HoldOutput` |
| `DeviceCommFailRule` | 80 | `!device_comm_ok` | `SafeDefault` |
| `LowBatteryRule` | 70 | `battery_soc < 10.0` | `StopCharge` |
| `OverTempRule` | 60 | `temperature > 80.0` | `StopCharge` |

#### Scenario: Agent 心跳超时
- **WHEN** `agent_alive == false` 或 `now_ns - agent_last_heartbeat_ns > 5_000_000_000`（5s）
- **THEN** `AgentDeadRule` 返回 `Some(SafeDefault)`
- **AND** 由于优先级最高，最终模式为 `SafeDefault`

#### Scenario: 低电量
- **WHEN** `battery_soc < 10.0`
- **AND** Agent 存活、通信正常
- **THEN** `LowBatteryRule` 返回 `Some(StopCharge)`
- **AND** 最终模式为 `StopCharge`

#### Scenario: 过温
- **WHEN** `temperature > 80.0`
- **AND** Agent 存活、通信正常
- **THEN** `OverTempRule` 返回 `Some(StopCharge)`

#### Scenario: 多规则同时触发
- **WHEN** `!agent_alive` 且 `battery_soc < 10.0`
- **THEN** `AgentDeadRule`（优先级 100）先评估
- **AND** 返回 `SafeDefault`（优先于 `LowBatteryRule` 的 `StopCharge`）

## MODIFIED Requirements

无（本版本为纯新增，不修改任何已有 crate）。

## REMOVED Requirements

无。

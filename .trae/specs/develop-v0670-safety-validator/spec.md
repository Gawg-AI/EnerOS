# v0.67.0 安全校验器 Spec

## Why

v0.66.0 完成了能源调度 LP 模型，但 LP 求解器可能输出理论上最优但实际不安全的解（如功率突变过大、SOC 越限）。v0.67.0 构建安全校验器作为 Solver → Control Bus 之间的安全屏障，执行电气安全校验 + 保护配合校验，校验失败时截断到安全边界而非拒绝。

## What Changes

- **ADDED** 新 crate `eneros-safety-validator`（`crates/ai/safety-validator/`）
- **ADDED** `SafetyRule` trait（校验规则抽象，**无 Send + Sync**，D1）
- **ADDED** `SafetyValidator` 校验器主接口（规则链式执行 + 致命违规终止）
- **ADDED** `ElectricalSafetyRule` 电气安全校验规则（功率/SOC 范围校验 + 截断）
- **ADDED** `ProtectionCoordinationRule` 保护配合校验规则（爬坡率校验 + 截断）
- **ADDED** `ValidationResult` / `Violation` / `Severity` 校验结果类型
- **ADDED** `SystemState` 最小系统状态类型（D2：本地定义，不依赖 HMI crate）
- **MODIFIED** workspace `members` 列表新增 `crates/ai/safety-validator`
- **MODIFIED** workspace 版本号 `0.66.0` → `0.67.0`

## Impact

- Affected specs: v0.66.0 (ScheduleResult 复用)、v0.64.0 (SolveStatus 复用)
- Affected code: 根 `Cargo.toml`、`Makefile`、`.github/workflows/ci.yml`、`ci/src/gate.rs`
- 新增 crate 位置：`crates/ai/safety-validator/`（AI 子系统，项目规则 §2.3.1）

## 偏差声明（D1~D12，Karpathy "Think Before Coding"）

| 偏差 | 蓝图原文 | 本版本处理 | 理由 |
|------|---------|-----------|------|
| **D1** | `pub trait SafetyRule: Send + Sync`（蓝图 line 13987） | **移除 `Send + Sync` bound** | no_std 单线程环境；与 v0.59.0 `LlmEngine` / v0.63.0 `PromptTemplate` / v0.64.0 `Solver` 一致。`Send + Sync` 在单线程无意义，且 `Box<dyn SafetyRule>` 在 no_std 下派生 `Send + Sync` 会引入 `alloc::boxed::Box` 的 trait bound 限制 |
| **D2** | 蓝图使用 `SystemState` 但未定义（蓝图 line 13993 等） | **本地定义最小 `SystemState`**（含 voltage_v / current_a / frequency_hz / soc_pct / timestamp_ms） | HMI crate 的 `SystemState` 是 HMI 显示状态（agent_states/storage_usage/network），与电气安全校验无关。Karpathy "Simplicity First"：定义最小满足校验需求的状态类型，不引入 HMI crate 耦合 |
| **D3** | `rule: self.name().into()`（蓝图 line 14072 等） | 保留 `alloc::string::ToString` 隐式转换（`&str` → `String`） | `extern crate alloc` 后 `.to_string()` 可用，`String::from` 也可用 |
| **D4** | `Vec<Box<dyn SafetyRule>>`（蓝图 line 14201） | 使用 `alloc::vec::Vec` + `alloc::boxed::Box` | no_std 合规：`Vec`/`Box` 在 `extern crate alloc` 后可用 |
| **D5** | `self.rules.sort_by_key(|r| r.priority())`（蓝图 line 14228） | 保留 `Vec::sort_by_key`（`alloc` 原生支持） | no_std 合规：`sort_by_key` 在 `alloc::vec::Vec` 上可用 |
| **D6** | 前置依赖列出 v0.56.0 ConstraintChecker / v0.57.0 DegradeEngine / v0.52.0 四遥数据模型（蓝图 §2） | **不引入这 3 个 crate 依赖** | `SafetyValidator` 仅校验 `ScheduleResult`（v0.66.0）+ 本地 `SystemState`（D2）。与 v0.66.0 D5 一致：解耦，避免未使用依赖。Karpathy "Simplicity First" |
| **D7** | 蓝图未声明 `[features]` | 不声明 `[features]` | 纯 Rust，无 FFI，无 feature gate |
| **D8** | 蓝图 `Severity` 派生 `Debug` + `Clone` + `Copy` + `PartialEq` | 保持一致 | `Severity` 需 `PartialEq` 做 `==` 比较（蓝图 line 14124/14252），`Copy` 因其为简单枚举 |
| **D9** | 蓝图 `ValidationResult`/`Violation` 派生 `Debug` + `Clone` | 保持一致，不额外派生 `PartialEq` | Karpathy "Simplicity First"：当前测试不需要 `PartialEq`，避免过早添加 |
| **D10** | 蓝图 `if entry.soc_pct > 0.95`（蓝图 line 14096）直接比较 | 保留直接比较（f64 有序比较在边界值 0.95/0.05 无精度问题） | 蓝图 §8.3 提到浮点容差，但 SOC 边界 0.95/0.05 是固定阈值，非迭代计算结果，直接比较安全。若未来需要容差，可加 `const EPSILON: f64 = 1e-9` |
| **D11** | 蓝图截断逻辑 `clamped_schedule.schedule[i].discharge_power_kw += diff.max(0.0)`（蓝图 line 14183） | 保留蓝图截断逻辑（精确复制） | 蓝图截断策略是核心业务逻辑，Karpathy "Surgical Changes"：不修改不理解的业务逻辑。截断后重新计算 `net_power_kw` 和 `revenue_yuan` |
| **D12** | 蓝图 `ElectricalSafetyRule` 有 `max_current_a`/`voltage_range`/`freq_range` 字段但 `validate` 未使用 | 保留字段（蓝图已定义），`validate` 仅用 `max_power_kw` 和 SOC 阈值 | Karpathy "Surgical Changes"：蓝图字段为未来扩展预留，不删除；但 `validate` 逻辑严格按蓝图，不扩展 |

## ADDED Requirements

### Requirement: SafetyRule trait

系统 SHALL 提供 `SafetyRule` trait（无 `Send + Sync` bound，D1），包含必需方法 `name() -> &str` / `validate(&self, schedule: &ScheduleResult, state: &SystemState) -> ValidationResult`，默认方法 `priority() -> u32`（默认 100）/ `is_hard() -> bool`（默认 true）。

#### Scenario: 规则优先级
- **WHEN** 多条规则注册到 `SafetyValidator`
- **THEN** 按 `priority()` 升序排序（数值越小优先级越高）

### Requirement: SafetyValidator 校验器主接口

系统 SHALL 提供 `SafetyValidator`，在 `new()` 时注册默认规则（ElectricalSafetyRule + ProtectionCoordinationRule），`validate()` 链式执行所有规则，前序规则截断后继续执行后续规则，致命违规（Fatal）立即终止。

#### Scenario: 链式校验通过
- **WHEN** 调度结果全部规则通过
- **THEN** 返回 `ValidationResult { passed: true, clamped: false, clamped_schedule: None, violations: [] }`

#### Scenario: 截断到安全边界
- **WHEN** 充电功率 > max_power_kw
- **THEN** 截断到 max_power_kw，`clamped: true`，`clamped_schedule: Some(...)`，violation severity = Critical

#### Scenario: 致命违规立即终止
- **WHEN** SOC < 0.05
- **THEN** severity = Fatal，立即终止后续规则，`passed: false`

### Requirement: ElectricalSafetyRule 电气安全校验

系统 SHALL 提供 `ElectricalSafetyRule`，校验充电功率/放电功率 ≤ max_power_kw、SOC ≤ 0.95、SOC ≥ 0.05，超限时截断到安全值。

#### Scenario: 功率超限截断
- **WHEN** charge_power_kw = 120 > max_power_kw = 100
- **THEN** 截断 charge_power_kw = 100，violation field = "charge_power"

#### Scenario: SOC 上限截断
- **WHEN** soc_pct = 0.98 > 0.95
- **THEN** 截断 soc_pct = 0.95，severity = Critical

#### Scenario: SOC 下限致命
- **WHEN** soc_pct = 0.03 < 0.05
- **THEN** 截断 soc_pct = 0.05，severity = Fatal

### Requirement: ProtectionCoordinationRule 保护配合校验

系统 SHALL 提供 `ProtectionCoordinationRule`，校验相邻时段功率变化率 ≤ max_ramp_rate（kW/min），超限时截断功率变化。

#### Scenario: 爬坡率超限截断
- **WHEN** 相邻时段功率变化 > max_ramp_rate
- **THEN** 截断到安全变化率，violation field = "ramp_rate"，severity = Critical

### Requirement: SystemState 最小系统状态

系统 SHALL 提供 `SystemState` 结构体（D2：本地定义），包含 voltage_v / current_a / frequency_hz / soc_pct / timestamp_ms 字段，派生 `Debug` + `Clone` + `Default`。

#### Scenario: 默认系统状态
- **WHEN** 调用 `SystemState::default()`
- **THEN** voltage_v = 380.0, current_a = 0.0, frequency_hz = 50.0, soc_pct = 0.5, timestamp_ms = 0

## MODIFIED Requirements

### Requirement: Workspace members

根 `Cargo.toml` 的 `members` 列表 SHALL 在 `crates/ai/energy-lp-model` 之后添加 `crates/ai/safety-validator`。

### Requirement: Workspace 版本号

根 `Cargo.toml` 的 `[workspace.package].version` SHALL 从 `0.66.0` 更新为 `0.67.0`。

## REMOVED Requirements

### Requirement: v0.56.0/v0.57.0/v0.52.0 crate 依赖
**Reason**: `SafetyValidator` 仅校验 `ScheduleResult` + 本地 `SystemState`，与 ConstraintChecker/DegradeEngine/telemetry-model 解耦（D6）
**Migration**: 调用方负责将实时四遥数据填充到 `SystemState`；v0.56.0 ConstraintChecker 在 RTOS 层做二次校验，与本 crate 形成 double barrier

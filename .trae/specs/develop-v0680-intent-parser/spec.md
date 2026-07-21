# Intent Parser (v0.68.0) Spec

## Why

LLM 输出 JSON 格式的自然语言意图（如"谷时充电、峰时放电"），但 Solver 只能接受 `ScheduleConfig` / `OptProblem` 等结构化参数。需要意图解析器作为"翻译层"，将 LLM 意图转换为 Solver 可执行的调度参数，桥接神经层（LLM 感知）与符号层（Solver 执行），是双脑架构的关键转换环节。

## What Changes

- **新增 crate** `eneros-intent-parser`（`crates/ai/intent-parser/`），实现 `IntentParser` 主接口
- **新增类型** `Intent` / `IntentType` / `TimeRange` / `PowerIntent` / `SocIntent` / `IntentError`
- **新增依赖** `serde`（derive）+ `serde_json`（no_std alloc）+ `eneros-energy-lp-model`（v0.66.0）+ `eneros-safety-validator`（v0.67.0 SystemState）+ `eneros-solver-model`（v0.65.0）+ `eneros-solver-core`（v0.64.0 LpProblem）
- **workspace 同步**：根 `Cargo.toml` 版本 `0.67.0` → `0.68.0`，members 添加 `crates/ai/intent-parser`

## Impact

- Affected specs: v0.66.0（ScheduleConfig/ScheduleEntry 复用） / v0.67.0（SystemState 复用） / v0.65.0（EnergyScheduleModel.compile） / v0.64.0（LpProblem） / v0.63.0（JSON 约束模式参考）
- Affected code: `Cargo.toml` / `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs` / `crates/ai/intent-parser/`（新）
- 下游解锁：v0.69.0（LLM → Solver 意图契约） / v0.71.0（双脑联调）

## ADDED Requirements

### Requirement: Intent 数据结构

系统 SHALL 提供 `Intent` 结构体（JSON 反序列化目标），包含：
- `intent_type: IntentType`（必需，7 种意图类型枚举）
- `time_range: Option<TimeRange>`（可选，时间范围）
- `power: Option<PowerIntent>`（可选，功率指令）
- `soc_target: Option<SocIntent>`（可选，SOC 目标）
- `priority: u8`（默认 3，1-5 优先级）
- `reason: String`（默认空字符串，LLM 决策理由）
- `confidence: f64`（默认 0.0，0.0-1.0 置信度）

所有结构体派生 `Debug + Clone + Serialize + Deserialize`。`IntentType` 额外派生 `PartialEq`。`Option<T>` 字段允许 JSON 缺失时反序列化为 `None`。`priority`/`reason`/`confidence` 使用 `#[serde(default)]` 容错 LLM 省略字段。

#### Scenario: LLM 输出完整意图
- **WHEN** LLM 输出 `{"intent_type":"Charge","time_range":{"start_period":0,"end_period":4},"power":{"power_kw":-50.0},"priority":2,"reason":"谷时充电","confidence":0.9}`
- **THEN** `IntentParser::parse_json` 返回 `Intent`，字段全部填充

#### Scenario: LLM 省略可选字段
- **WHEN** LLM 输出 `{"intent_type":"Hold"}`
- **THEN** `IntentParser::parse_json` 返回 `Intent`，`time_range`/`power`/`soc_target` 为 `None`，`priority=3`，`reason=""`，`confidence=0.0`

### Requirement: IntentType 枚举

系统 SHALL 提供 7 种意图类型枚举：
- `Charge`（充电）
- `Discharge`（放电）
- `Hold`（保持）
- `Stop`（停止）
- `EmergencyStop`（紧急停机）
- `AutonomousSchedule`（自主调度）
- `SetSetpoint`（调整设定值）

派生 `Debug + Clone + PartialEq + Serialize + Deserialize`。

### Requirement: IntentParser 主接口

系统 SHALL 提供 `IntentParser` 结构体，包含：
- `default_config: ScheduleConfig`（默认调度配置，来自 v0.66.0）
- `system_state: SystemState`（系统当前状态，来自 v0.67.0）

#### 方法

- `new(default_config: ScheduleConfig, state: SystemState) -> Self`
- `parse_json(&self, json: &str) -> Result<Intent, IntentError>` — JSON 字符串反序列化为 Intent
- `to_schedule_config(&self, intent: &Intent) -> Result<ScheduleConfig, IntentError>` — Intent → ScheduleConfig
- `to_opt_problem(&self, intent: &Intent) -> Result<(ScheduleConfig, LpProblem), IntentError>` — Intent → (ScheduleConfig, LpProblem)
- `validate_config(&self, config: &ScheduleConfig) -> Result<(), IntentError>` — 校验配置合理性

#### Scenario: Charge 意图转换
- **WHEN** Intent 为 `Charge`，power_kw=-50.0，time_range=[0,4]
- **THEN** `to_schedule_config` 修改 price[0..4] 为负值引导充电，返回 ScheduleConfig

#### Scenario: EmergencyStop 意图转换
- **WHEN** Intent 为 `EmergencyStop`
- **THEN** `to_schedule_config` 设置 pcs_power_kw=0，soc_min=soc_max=当前 SOC（冻结状态）

#### Scenario: AutonomousSchedule 意图转换
- **WHEN** Intent 为 `AutonomousSchedule`，soc_target=Some(0.8)
- **THEN** `to_schedule_config` 设置 soc_final=Some(0.8)，其余使用默认配置

### Requirement: IntentError 错误类型

系统 SHALL 提供 `IntentError` 枚举：
- `ParseError(String)` — JSON 解析失败
- `InvalidConfig(String)` — 配置校验失败
- `ConstraintConflict(String)` — 约束冲突
- `CompileError(String)` — LP 问题编译失败

派生 `Debug`（不派生 Clone/PartialEq — Simplicity First）。

### Requirement: no_std 合规

- `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- 仅使用 `alloc::*` / `core::*` / `serde` / `serde_json`
- 无 `std::*` / `panic!` / `todo!` / `unimplemented!` / `unsafe`
- 可交叉编译到 `aarch64-unknown-none`

## MODIFIED Requirements

### Requirement: Workspace 版本

- 根 `Cargo.toml` `version = "0.67.0"` → `"0.68.0"`
- `members` 添加 `"crates/ai/intent-parser"`（置于 `crates/ai/safety-validator` 之后）
- `Makefile` / `.github/workflows/ci.yml` 版本号同步
- `ci/src/gate.rs` clippy + test 段注释补充 `eneros-intent-parser`

## REMOVED Requirements

### Requirement: v0.26.0 配置管理系统依赖

**Reason**: 蓝图 §2 列出 v0.26.0 config 管理系统为前置依赖，但详细设计代码中未实际使用（仅用 `ScheduleConfig::default()` from v0.66.0）。属于蓝图依赖过度声明。
**Migration**: 不引入 `eneros-config` 依赖，使用 v0.66.0 `ScheduleConfig::default()` 即可。

---

## 偏差声明（D1~D12）

| 偏差 | 蓝图原文 | 本版本处理 | 理由 |
|------|---------|-----------|------|
| **D1** | `#[derive(Serialize, Deserialize)]` + `serde_json::from_str` | 添加 `serde`（derive + alloc）+ `serde_json`（alloc）依赖 | 比手动遍历 `serde_json::Value` 更简洁（Karpathy Simplicity First）；prompt-template v0.63.0 用手动遍历是因为需要先做 Schema 校验，本版本直接反序列化到类型化结构 |
| **D2** | `self.system_state.soc` | `self.system_state.soc_pct` | v0.67.0 `SystemState` 字段名是 `soc_pct`，蓝图简写为 `soc` |
| **D3** | `config.price[t] = -power_kw;` 直接索引 | 使用 `config.price.get_mut(t)` 安全访问 | 蓝图 §8.4 警告时段编号可能从 1 开始；直接索引可能越界 panic（no_std 中 panic = 系统挂死） |
| **D4** | `model.compile()?` 直接用 `?` | `model.compile().map_err(\|e\| IntentError::CompileError(e.to_string()))` | `SolverError` 不实现 `From<SolverError> for IntentError`；需显式映射 |
| **D5** | 前置依赖 v0.26.0 配置管理系统 | **不引入** v0.26.0 依赖 | 蓝图代码未实际使用 config 管理，仅用 `ScheduleConfig::default()`（来自 v0.66.0） |
| **D6** | `system_state: SystemState`（蓝图未明确来源） | 依赖 `eneros-safety-validator` 复用 v0.67.0 `SystemState` | 不重定义类型，避免碎片化；与 v0.67.0 D8 模式一致 |
| **D7** | `IntentError` 派生未指定 | 仅派生 `Debug`（不派生 Clone/PartialEq） | Simplicity First；与 v0.64.0/v0.65.0/v0.66.0 错误类型一致 |
| **D8** | `IntentType` 派生 `PartialEq` | 保持，另加 `Serialize + Deserialize` | `match` 需要枚举变体；`PartialEq` 保留用于测试断言 |
| **D9** | `Intent` 字段 `reason`/`confidence`/`priority` 为必需 | 加 `#[serde(default)]`（reason=""、confidence=0.0、priority=3） | 蓝图 §8.2 警告 LLM 可能省略可选字段；serde 默认值容错 |
| **D10** | 蓝图未声明 no_std | `#![cfg_attr(not(test), no_std)]` + `extern crate alloc` | 项目硬性要求（蓝图 §43.1）；serde + serde_json 均支持 no_std + alloc |
| **D11** | `to_opt_problem` 中 `config.clone()` | 保留 `clone()` | `new(config)` 接收所有权，但需返回 `(config, problem)`，clone 必要 |
| **D12** | `validate_config` 检查 `price.len() != num_periods` | 保持蓝图校验逻辑 | 蓝图 §8.3/§8.4 的坑点已在 D3 处理，validate 逻辑不变 |

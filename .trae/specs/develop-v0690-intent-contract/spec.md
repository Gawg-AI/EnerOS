# EnerOS v0.69.0 LLM → Solver 意图契约 Spec

## Why

v0.68.0 实现了 LLM 输出 JSON → `Intent` → `ScheduleConfig`/`LpProblem` 的单方向解析，但 LLM 与 Solver 之间仍缺乏一个**稳定的、版本化的、可双向通信的契约协议**。双脑架构（LLM 为感知者、Solver 为执行者）要求两侧通过契约解耦：LLM 输出 `IntentContract`，Solver 输出 `FeedbackContract`，由契约校验器保证格式合法，由双向转换器桥接两侧数据模型。本版本补齐这一契约接口层，为 v0.70.0 实时快速路径与 v0.71.0 双脑联调奠基。

## What Changes

- **ADDED** 新增 `eneros-intent-contract` crate（位于 `crates/ai/intent-contract/`）
- **ADDED** `IntentContract` 正向契约（LLM → Solver）：`schema_version` / `request_id` / `timestamp` / `intent` / `context` / `llm_meta`
- **ADDED** `FeedbackContract` 反向契约（Solver → LLM）：`request_id` / `solve_status` / `validation_passed` / `clamp_info` / `executed_schedule` / `actual_revenue` / `solve_ms`
- **ADDED** `SystemContext` 系统快照结构（`current_soc` / `current_power_kw` / `current_price` / `current_period` / `device_status` / `alarms`）
- **ADDED** `LlmMeta` LLM 元信息结构（`model_name` / `inference_ms` / `token_count` / `confidence`）
- **ADDED** `DeviceStatus` 设备状态枚举（蓝图未定义，D7 本地定义最小集合）
- **ADDED** `ContractValidator` 契约校验器：6 项校验规则 + 版本兼容性
- **ADDED** `ContractConverter` 双向转换器：正向 `to_solver_params` / 反向 `to_feedback` / `serialize_feedback`
- **ADDED** `ContractError` 错误类型（4 变体，仅 Debug，D9）
- **MODIFIED** workspace 版本 `0.68.0` → `0.69.0`
- **MODIFIED** 根 `Cargo.toml` members 添加 `crates/ai/intent-contract`
- **MODIFIED** `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs` 版本同步

## Impact

- **Affected specs**：v0.68.0 意图解析器（复用 `Intent`/`IntentType`/`TimeRange`/`PowerIntent`/`SocIntent`，不重定义）
- **Affected code**：
  - 新增：`crates/ai/intent-contract/`（Cargo.toml + src/lib.rs + src/error.rs + src/contract.rs + src/validator.rs + src/converter.rs）
  - 修改：`Cargo.toml`（版本 + members）/ `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs`
  - 复用：`eneros-intent-parser::intent::Intent`、`eneros-energy-lp-model::{ScheduleConfig, EnergyScheduleModel, ScheduleResult, ScheduleEntry}`、`eneros-solver-core::{LpProblem, SolveResult, SolveStatus}`、`eneros-safety-validator::{SystemState, ValidationResult, Violation}`
- **后续解锁**：v0.70.0 实时路径 Solver only（依赖本版本契约结构）；v0.71.0 双脑联调

## ADDED Requirements

### Requirement: IntentContract 正向契约

系统 SHALL 提供 `IntentContract` 结构体作为 LLM → Solver 的稳定通信协议，包含 `schema_version: String` / `request_id: String` / `timestamp: u64` / `intent: Intent`（复用 v0.68.0）/ `context: SystemContext` / `llm_meta: LlmMeta` 五个字段。`IntentContract` SHALL 派生 `Debug + Clone + Serialize + Deserialize`（D8）。

#### Scenario: 完整契约序列化
- **WHEN** 构造一个全字段非空的 `IntentContract`
- **THEN** `serde_json::to_string` 成功，`serde_json::from_str` 可还原

#### Scenario: 反序列化容错
- **WHEN** LLM 省略 `intent.priority` / `intent.reason` / `intent.confidence`
- **THEN** 借助 v0.68.0 `Intent` 的 `#[serde(default)]` 字段，反序列化成功并填充默认值

### Requirement: SystemContext 系统快照

系统 SHALL 提供 `SystemContext` 结构体，字段：`current_soc: f64` / `current_power_kw: f64` / `current_price: f64` / `current_period: usize` / `device_status: DeviceStatus` / `alarms: Vec<String>`。派生 `Debug + Clone + Serialize + Deserialize`（D8）。

### Requirement: LlmMeta LLM 元信息

系统 SHALL 提供 `LlmMeta` 结构体，字段：`model_name: String` / `inference_ms: u64` / `token_count: usize` / `confidence: f64`。派生 `Debug + Clone + Serialize + Deserialize`（D8）。

### Requirement: DeviceStatus 设备状态枚举

系统 SHALL 提供 `DeviceStatus` 枚举（蓝图 §4.1 line 14632 引用但未定义，D7 本地定义最小集合）：`Normal` / `Warning` / `Fault` / `Maintenance` / `Offline`。派生 `Debug + Clone + Serialize + Deserialize`（D8）。

### Requirement: FeedbackContract 反向契约

系统 SHALL 提供 `FeedbackContract` 结构体作为 Solver → LLM 的反馈协议，字段：`request_id: String` / `solve_status: SolveStatus`（复用 v0.64.0）/ `validation_passed: bool` / `clamp_info: Option<Vec<Violation>>`（复用 v0.67.0）/ `executed_schedule: Option<Vec<ScheduleEntry>>`（复用 v0.66.0）/ `actual_revenue: f64` / `solve_ms: u64`。派生 `Debug + Clone + Serialize + Deserialize`（D8）。

### Requirement: ContractError 错误类型

系统 SHALL 提供 `ContractError` 枚举：`UnsupportedVersion(String)` / `MissingField(String)` / `InvalidValue(String, String)` / `SerializationError(String)`。仅派生 `Debug`（D9：Karpathy 简化原则，与 v0.68.0 `IntentError` 一致）。使用 `alloc::string::String`。

### Requirement: ContractValidator 契约校验器

系统 SHALL 提供 `ContractValidator` 结构体，包含 `supported_versions: Vec<String>` 与 `current_version: String`。实现：

- `new()`：默认支持 `["1.0.0", "1.1.0"]`，当前版本 `"1.1.0"`
- `validate(&self, contract: &IntentContract) -> Result<(), ContractError>`：6 项校验
  1. 版本支持检查
  2. `request_id` 非空
  3. `intent.reason` 非空（D12：契约场景比单步 Intent 更严格，合理）
  4. `intent.confidence ∈ [0.0, 1.0]`
  5. `intent.priority ∈ [1, 5]`
  6. `intent.time_range`（如有）`start_period <= end_period`
  7. `intent.soc_target`（如有）`target_soc ∈ [0.0, 1.0]`
- `is_compatible(&self, version: &str) -> bool`

### Requirement: ContractConverter 双向转换器

系统 SHALL 提供 `ContractConverter` 结构体，包含 `default_config: ScheduleConfig`。实现：

- `to_solver_params(&self, contract: &IntentContract, state: &SystemState) -> Result<(ScheduleConfig, LpProblem), ContractError>`
  - 构造 `IntentParser::new(self.default_config.clone(), state.clone())`
  - 调用 `parser.to_schedule_config(&contract.intent)`（D10：`IntentError` 显式 `map_err` 为 `ContractError::SerializationError`）
  - 构造 `EnergyScheduleModel::new(config.clone())` 并 `compile()`（D11：保留蓝图 `SerializationError` 命名，Surgical Changes）
  - 返回 `(config, problem)`
- `to_feedback(&self, request_id: &str, solve_result: &SolveResult, validation: &ValidationResult, schedule: &ScheduleResult, solve_ms: u64) -> FeedbackContract`
- `serialize_feedback(&self, feedback: &FeedbackContract) -> Result<String, ContractError>`：使用 `serde_json::to_string_pretty`（D6：no_std + alloc 支持）

### Requirement: no_std 合规

crate SHALL 遵循 EnerOS §43.1 no_std 规范：
- `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- 子模块不重复 `#![cfg_attr(not(test), no_std)]`
- 禁止 `use std::*` / `panic!` / `todo!` / `unimplemented!`
- `serde` / `serde_json` 使用 `default-features = false, features = ["alloc"]` 配置

## MODIFIED Requirements

### Requirement: Workspace 版本同步

根 `Cargo.toml` 的 `[workspace.package] version` 从 `0.68.0` 更新为 `0.69.0`；`members` 在 `crates/ai/intent-parser` 之后添加 `crates/ai/intent-contract`。`Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs` 同步版本号。

## REMOVED Requirements

（无移除项）

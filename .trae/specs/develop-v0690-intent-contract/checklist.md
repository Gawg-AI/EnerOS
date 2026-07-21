# Checklist

## Workspace 同步
- [x] C1 根 `Cargo.toml` 版本号已更新为 `0.69.0`
- [x] C2 members 列表已添加 `crates/ai/intent-contract`（置于 `crates/ai/intent-parser` 之后）
- [x] C3 `cargo metadata --format-version 1` 成功

## Crate 骨架
- [x] C4 `crates/ai/intent-contract/Cargo.toml` 存在，package name = `eneros-intent-contract`
- [x] C5 dependencies 包含 `eneros-intent-parser` / `eneros-energy-lp-model` / `eneros-solver-core` / `eneros-safety-validator`
- [x] C6 dependencies 包含 `serde = { version = "1.0", default-features = false, features = ["alloc", "derive"] }`
- [x] C7 dependencies 包含 `serde_json = { version = "1.0", default-features = false, features = ["alloc"] }`
- [x] C8 无 `[features]` 段
- [x] C9 `src/lib.rs` 包含 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`（D8）
- [x] C10 `src/lib.rs` 包含 D1~D12 偏差声明表
- [x] C11 模块声明：error / contract / validator / converter

## error.rs — ContractError
- [x] C12 `ContractError` 枚举：`UnsupportedVersion(String)` / `MissingField(String)` / `InvalidValue(String, String)` / `SerializationError(String)`
- [x] C13 派生 `Debug`（D9：不派生 Clone/PartialEq）
- [x] C14 使用 `alloc::string::String`

## contract.rs — 数据结构
- [x] C15 `DeviceStatus` 枚举：Normal / Warning / Fault / Maintenance / Offline（D7：蓝图未定义，本地最小集合）
- [x] C16 `DeviceStatus` 派生 `Debug + Clone + Serialize + Deserialize`（D8）
- [x] C17 `SystemContext` 结构体：current_soc / current_power_kw / current_price / current_period / device_status / alarms: Vec<String>
- [x] C18 `LlmMeta` 结构体：model_name / inference_ms / token_count / confidence
- [x] C19 `IntentContract` 结构体：schema_version / request_id / timestamp / intent（复用 v0.68.0 Intent）/ context / llm_meta
- [x] C20 `FeedbackContract` 结构体：request_id / solve_status（复用 v0.64.0）/ validation_passed / clamp_info: Option<Vec<Violation>>（复用 v0.67.0）/ executed_schedule: Option<Vec<ScheduleEntry>>（复用 v0.66.0）/ actual_revenue / solve_ms
- [x] C21 所有结构体派生 `Debug + Clone + Serialize + Deserialize`（D8）
- [x] C22 编译通过

## validator.rs — ContractValidator
- [x] C23 `ContractValidator` 结构体：supported_versions: Vec<String> / current_version: String
- [x] C24 `new()`：默认支持 `["1.0.0", "1.1.0"]`，当前版本 `"1.1.0"`
- [x] C25 `validate(&self, contract: &IntentContract) -> Result<(), ContractError>`：版本检查
- [x] C26 `validate`：request_id 非空检查
- [x] C27 `validate`：intent.reason 非空检查（D12：契约比 Intent 严格）
- [x] C28 `validate`：intent.confidence ∈ [0.0, 1.0] 检查
- [x] C29 `validate`：intent.priority ∈ [1, 5] 检查
- [x] C30 `validate`：time_range.start_period <= end_period 检查
- [x] C31 `validate`：soc_target.target_soc ∈ [0.0, 1.0] 检查
- [x] C32 `is_compatible(&self, version: &str) -> bool`
- [x] C33 实现 `Default` for `ContractValidator`
- [x] C34 编译通过

## converter.rs — ContractConverter
- [x] C35 `ContractConverter` 结构体：default_config: ScheduleConfig
- [x] C36 `to_solver_params(&self, contract: &IntentContract, state: &SystemState) -> Result<(ScheduleConfig, LpProblem), ContractError>`
- [x] C37 `to_solver_params` 构造 `IntentParser::new(self.default_config.clone(), state.clone())`
- [x] C38 `to_solver_params` 调用 `parser.to_schedule_config(&contract.intent)`（D10：map_err 为 SerializationError）
- [x] C39 `to_solver_params` 构造 `EnergyScheduleModel::new(config.clone())` + `compile()`（D11：保留蓝图 SerializationError 命名）
- [x] C40 `to_feedback(&self, request_id, solve_result, validation, schedule, solve_ms) -> FeedbackContract`
- [x] C41 `to_feedback` 设置 `clamp_info` 为 None 或 Some(violations.clone())
- [x] C42 `to_feedback` 设置 `executed_schedule = Some(schedule.schedule.clone())`
- [x] C43 `serialize_feedback(&self, feedback: &FeedbackContract) -> Result<String, ContractError>`（D6：serde_json::to_string_pretty）
- [x] C44 实现 `Default` for `ContractConverter`
- [x] C45 编译通过

## 集成测试（lib.rs）
- [x] C46 T1 IntentContract 构造 + 序列化
- [x] C47 T2 IntentContract 反序列化（缺可选字段）
- [x] C48 T3 SystemContext 构造
- [x] C49 T4 LlmMeta 构造
- [x] C50 T5 DeviceStatus 枚举变体
- [x] C51 T6 FeedbackContract 构造 + 序列化
- [x] C52 T7 ContractValidator::new 默认版本列表
- [x] C53 T8 validate 合法契约通过
- [x] C54 T9 validate 不支持版本失败
- [x] C55 T10 validate 缺 request_id 失败
- [x] C56 T11 validate 空 reason 失败（D12）
- [x] C57 T12 validate confidence 超界失败
- [x] C58 T13 validate priority 超界失败
- [x] C59 T14 validate time_range 倒置失败
- [x] C60 T15 validate soc_target 超界失败
- [x] C61 T16 is_compatible 正反向
- [x] C62 T17 to_solver_params 正向转换
- [x] C63 T18 to_feedback 反向转换
- [x] C64 T19 serialize_feedback JSON 输出
- [x] C65 T20 端到端：Intent JSON → Contract → Validate → SolverParams
- [x] C66 `cargo test -p eneros-intent-contract` 全部通过

## 设计文档
- [x] C67 `docs/ai/intent-contract-design.md` 存在
- [x] C68 12 章节完整
- [x] C69 2 Mermaid 图（IntentContract 类图 + 双向转换流程图）
- [x] C70 D1~D12 偏差声明表
- [x] C71 文档在 `docs/ai/` 下

## 版本同步
- [x] C72 `Makefile` 版本号 `0.69.0`（header + VERSION 变量 2 处）
- [x] C73 `.github/workflows/ci.yml` 版本号 `0.69.0`
- [x] C74 `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-intent-contract`

## 构建校验（§2.4.2 C6~C11）
- [x] C75 `cargo metadata --format-version 1` 成功
- [x] C76 `cargo test -p eneros-intent-contract` 全部通过
- [x] C77 `cargo build -p eneros-intent-contract --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C78 `cargo fmt -p eneros-intent-contract -- --check` 通过
- [x] C79 `cargo clippy -p eneros-intent-contract --all-targets -- -D warnings` 无 warning
- [x] C80 `cargo deny check licenses bans sources` 通过

## no_std 合规
- [x] C81 无 `use std::*`（仅 `alloc::*` / `core::*` / `serde` / `serde_json`）
- [x] C82 无 `panic!` / `todo!` / `unimplemented!`
- [x] C83 子模块不重复 `#![cfg_attr(not(test), no_std)]`
- [x] C84 无 `unsafe` 块
- [x] C85 serde / serde_json 使用 `no_std + alloc` 配置

## 目录规范
- [x] C86 crate 在 `crates/ai/intent-contract/`
- [x] C87 跨 crate path 引用均为相对路径（`../intent-parser` / `../energy-lp-model` / `../solver-core` / `../safety-validator`）
- [x] C88 文档在 `docs/ai/` 下
- [x] C89 无根目录 crate（除 `ci/`）
- [x] C90 无垃圾文件

## 依赖复用（D1~D6）
- [x] C91 复用 v0.68.0 `Intent` / `IntentType` / `TimeRange` / `PowerIntent` / `SocIntent`（D1：通过 `eneros-intent-parser` 依赖）
- [x] C92 复用 v0.67.0 `SystemState` / `ValidationResult` / `Violation`（D2：通过 `eneros-safety-validator` 依赖）
- [x] C93 复用 v0.66.0 `ScheduleConfig` / `EnergyScheduleModel` / `ScheduleResult` / `ScheduleEntry`（D3：通过 `eneros-energy-lp-model` 依赖）
- [x] C94 复用 v0.64.0 `LpProblem` / `SolveResult` / `SolveStatus`（D4：通过 `eneros-solver-core` 依赖）
- [x] C95 复用 v0.68.0 `IntentParser`（D5：通过 `eneros-intent-parser` 依赖，不重定义）

## 简化设计验证（Karpathy 原则）
- [x] C96 `ContractError` 不派生 Clone/PartialEq（D9：Simplicity First）
- [x] C97 `DeviceStatus` 本地定义（D7：蓝图未定义，最小集合）
- [x] C98 保留蓝图 `SerializationError` 命名（D11：Surgical Changes）
- [x] C99 保留蓝图 `reason` 非空校验（D12：契约比 Intent 严格，合理）
- [x] C100 无 `[features]` 段（纯 Rust）

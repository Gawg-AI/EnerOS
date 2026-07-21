# Tasks

- [x] Task 1: Workspace 同步
  - [x] 根 `Cargo.toml` 版本号 `0.68.0` → `0.69.0`
  - [x] members 添加 `crates/ai/intent-contract`（置于 `crates/ai/intent-parser` 之后）
  - [x] 验证：`cargo metadata --format-version 1` 成功

- [x] Task 2: 创建 `eneros-intent-contract` crate 骨架
  - [x] 新建 `crates/ai/intent-contract/Cargo.toml`，package name = `eneros-intent-contract`
  - [x] dependencies：`eneros-intent-parser` / `eneros-energy-lp-model` / `eneros-solver-core` / `eneros-safety-validator` / `serde`（derive+alloc） / `serde_json`（alloc）
  - [x] 无 `[features]` 段（D9：纯 Rust，无 FFI）
  - [x] no_std 配置：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
  - [x] 新建 `src/lib.rs`，模块声明：error / contract / validator / converter
  - [x] lib.rs 包含 D1~D12 偏差声明表

- [x] Task 3: 实现 `error.rs` — ContractError
  - [x] `ContractError` 枚举：`UnsupportedVersion(String)` / `MissingField(String)` / `InvalidValue(String, String)` / `SerializationError(String)`
  - [x] 派生 `Debug`（D9：不派生 Clone/PartialEq）
  - [x] 使用 `alloc::string::String`

- [x] Task 4: 实现 `contract.rs` — IntentContract / SystemContext / LlmMeta / DeviceStatus / FeedbackContract
  - [x] `DeviceStatus` 枚举：Normal / Warning / Fault / Maintenance / Offline（D7：蓝图未定义，本地最小集合）
  - [x] `SystemContext` 结构体：current_soc / current_power_kw / current_price / current_period / device_status / alarms
  - [x] `LlmMeta` 结构体：model_name / inference_ms / token_count / confidence
  - [x] `IntentContract` 结构体：schema_version / request_id / timestamp / intent（复用 v0.68.0 Intent）/ context / llm_meta
  - [x] `FeedbackContract` 结构体：request_id / solve_status（复用 v0.64.0）/ validation_passed / clamp_info（复用 v0.67.0 Violation）/ executed_schedule（复用 v0.66.0 ScheduleEntry）/ actual_revenue / solve_ms
  - [x] 所有结构体派生 `Debug + Clone + Serialize + Deserialize`（D8）

- [x] Task 5: 实现 `validator.rs` — ContractValidator
  - [x] `ContractValidator` 结构体：supported_versions / current_version
  - [x] `new()`：默认支持 `["1.0.0", "1.1.0"]`，当前版本 `"1.1.0"`
  - [x] `validate(&self, contract: &IntentContract) -> Result<(), ContractError>`：6 项校验（版本 / request_id 非空 / reason 非空 / confidence 范围 / priority 范围 / time_range 顺序 / soc_target 范围）
  - [x] `is_compatible(&self, version: &str) -> bool`
  - [x] 实现 `Default` for `ContractValidator`

- [x] Task 6: 实现 `converter.rs` — ContractConverter
  - [x] `ContractConverter` 结构体：default_config: ScheduleConfig
  - [x] `to_solver_params(&self, contract: &IntentContract, state: &SystemState) -> Result<(ScheduleConfig, LpProblem), ContractError>`
    - 构造 `IntentParser::new(self.default_config.clone(), state.clone())`
    - 调用 `parser.to_schedule_config(&contract.intent)`（D10：map_err 为 SerializationError）
    - `EnergyScheduleModel::new(config.clone())` + `compile()`（D11：保留蓝图 SerializationError 命名）
  - [x] `to_feedback(...)`：5 个参数，构造 FeedbackContract
  - [x] `serialize_feedback(&self, feedback: &FeedbackContract) -> Result<String, ContractError>`（D6：serde_json::to_string_pretty）
  - [x] 实现 `Default` for `ContractConverter`

- [x] Task 7: 集成测试（lib.rs）— 至少 15 个测试
  - [x] T1 IntentContract 构造 + 序列化
  - [x] T2 IntentContract 反序列化（缺可选字段）
  - [x] T3 SystemContext 构造
  - [x] T4 LlmMeta 构造
  - [x] T5 DeviceStatus 枚举变体
  - [x] T6 FeedbackContract 构造 + 序列化
  - [x] T7 ContractValidator::new 默认版本列表
  - [x] T8 ContractValidator::validate 合法契约通过
  - [x] T9 ContractValidator::validate 不支持版本失败
  - [x] T10 ContractValidator::validate 缺 request_id 失败
  - [x] T11 ContractValidator::validate 空 reason 失败（D12）
  - [x] T12 ContractValidator::validate confidence 超界失败
  - [x] T13 ContractValidator::validate priority 超界失败
  - [x] T14 ContractValidator::validate time_range 倒置失败
  - [x] T15 ContractValidator::validate soc_target 超界失败
  - [x] T16 ContractValidator::is_compatible 正反向
  - [x] T17 ContractConverter::to_solver_params 正向转换
  - [x] T18 ContractConverter::to_feedback 反向转换
  - [x] T19 ContractConverter::serialize_feedback JSON 输出
  - [x] T20 端到端：Intent JSON → Contract → Validate → SolverParams

- [x] Task 8: 创建设计文档 `docs/ai/intent-contract-design.md`
  - [x] 12 章节完整（版本目标 / 前置依赖 / 交付物 / 详细设计 / 技术交底 / 测试计划 / 验收标准 / 风险 / 多角度要求 / ADR / 偏差声明 / 参考）
  - [x] 2 Mermaid 图（IntentContract 类图 + 双向转换流程图）
  - [x] D1~D12 偏差声明表
  - [x] 文档位于 `docs/ai/` 下（C12）

- [x] Task 9: 版本同步
  - [x] `Makefile` 版本号 `0.69.0`（header + VERSION 变量 2 处）
  - [x] `.github/workflows/ci.yml` 版本号 `0.69.0`
  - [x] `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-intent-contract`

- [x] Task 10: 6 项构建校验（§2.4.2 C6~C11）
  - [x] `cargo metadata --format-version 1` 成功
  - [x] `cargo test -p eneros-intent-contract` 全部通过
  - [x] `cargo build -p eneros-intent-contract --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
  - [x] `cargo fmt -p eneros-intent-contract -- --check` 通过
  - [x] `cargo clippy -p eneros-intent-contract --all-targets -- -D warnings` 无 warning
  - [x] `cargo deny check licenses bans sources` 通过
  - [x] 更新 tasks.md / checklist.md 全部 [x]

# Task Dependencies
- Task 2 依赖 Task 1
- Task 3~6 依赖 Task 2（并行实现）
- Task 7 依赖 Task 3~6
- Task 8 可与 Task 3~7 并行
- Task 9 依赖 Task 2
- Task 10 依赖 Task 3~9 全部完成

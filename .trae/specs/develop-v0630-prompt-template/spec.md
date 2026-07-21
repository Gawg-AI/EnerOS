# v0.63.0 Prompt 模板系统 + JSON 输出约束 Spec

## Why
双脑架构要求 LLM 输出结构化 JSON 意图（而非自然语言），供 v0.68.0 IntentParser 解析。v0.63.0 是 P1-I LLM 推理层收官版本，构建电力专用 Prompt 模板系统和 JSON 输出约束（Schema 校验 + 重试机制），作为 LLM → Solver 的桥梁。

## What Changes
- 新增 crate `eneros-prompt-template`（路径 `crates/ai/prompt-template/`）
- 新增 `PromptTemplate` trait（无 Send + Sync，与 v0.59.0 `LlmEngine` 一致）
- 新增 `TemplateContext` 结构体（电力场景输入）
- 新增 `SchemaSpec` / `SchemaField` / `SchemaType` 最小 JSON Schema 验证器
- 新增 3 个电力场景模板：`ChargeDischargeTemplate` / `DispatchTemplate` / `AlarmTemplate`
- 新增 `JsonConstraint` 带重试的约束推理器 + `ConstraintStats` 统计
- 新增 `extract_json` 函数（处理 markdown 代码块包裹的 JSON）
- 新增 `TemplateError` 错误枚举（独立于 v0.59.0 `LlmError`）
- 新增设计文档 `docs/ai/prompt-template-design.md`

## Impact
- **Affected specs**: v0.59.0（LlmEngine trait 复用）、v0.68.0（IntentParser 依赖 v0.63.0 Prompt 模板 + JSON 约束）、v0.69.0（意图契约依赖 JSON 格式）
- **Affected code**: 新建 `crates/ai/prompt-template/`；workspace `Cargo.toml` 添加 member；`Makefile`/`ci.yml`/`gate.rs` 版本号同步 0.62.0 → 0.63.0
- **不受影响**: v0.60.0（GGUF 加载）、v0.61.0（模型部署）、v0.62.0（推理调度）— Prompt 模板与模型加载/部署/调度解耦

## ADDED Requirements

### Requirement: PromptTemplate trait
系统 SHALL 提供 `PromptTemplate` trait，包含 `name() -> &'static str` / `build(&self, context: &TemplateContext) -> String` / `output_schema() -> &'static SchemaSpec` 三个必需方法，以及默认实现的 `validate(&self, output: &str) -> Result<Value, TemplateError>` 方法（提取 JSON → 解析 → Schema 校验）。trait 不派生 `Send + Sync`（与 v0.59.0 `LlmEngine` 一致）。

#### Scenario: 模板构建和校验
- **WHEN** 调用 `template.build(&context)` 生成 prompt
- **THEN** 返回包含上下文参数的 prompt 字符串
- **WHEN** 调用 `template.validate(json_output)` 校验 LLM 输出
- **THEN** 返回 `Ok(Value)` 若 JSON 符合 schema；否则返回 `Err(TemplateError)`

### Requirement: TemplateContext 输入上下文
系统 SHALL 提供 `TemplateContext` 结构体，字段包含 `market_price: f64` / `soc: f64` / `power_current: f64` / `temperature: f64` / `time_of_day: String` / `historical_data: Vec<f64>`。提供 `new()` 构造方法和 `default()`（用于测试）。

#### Scenario: 上下文构造
- **WHEN** 调用 `TemplateContext::new(price, soc, power, temp, time, history)`
- **THEN** 返回包含所有字段的 `TemplateContext`

### Requirement: 3 个电力场景模板
系统 SHALL 提供至少 3 个 `PromptTemplate` 实现：
- `ChargeDischargeTemplate` — 充放电策略（action/power_kw/reason/confidence）
- `DispatchTemplate` — 功率调度（target_power/ramp_rate/duration_minutes/reason）
- `AlarmTemplate` — 告警处理（alarm_type/severity/action/target_device）

每个模板的 `output_schema()` 返回对应的 `&'static SchemaSpec`。

#### Scenario: 模板生成和校验
- **WHEN** 调用任一模板的 `build(&context)`
- **THEN** 返回包含电力专用 prompt 文本，要求 LLM 输出 JSON
- **WHEN** 调用 `validate(valid_json)` 传入符合 schema 的 JSON
- **THEN** 返回 `Ok(Value)`
- **WHEN** 调用 `validate(invalid_json)` 传入不符合 schema 的 JSON
- **THEN** 返回 `Err(TemplateError::SchemaValidation(_))`

### Requirement: SchemaSpec 最小 JSON Schema 验证器
系统 SHALL 提供 `SchemaSpec` 结构体（包含 `&'static [SchemaField]`）和 `SchemaField`（字段名/类型/required/enum_values/minimum/maximum），实现 `validate(&self, value: &Value) -> Result<(), TemplateError>` 方法，校验 required 字段存在、字段类型匹配、枚举值合法、数值范围合规。不实现完整 JSON Schema draft 7+ 规范。

#### Scenario: Schema 校验
- **WHEN** 输入 JSON 符合 schema 所有约束
- **THEN** 返回 `Ok(())`
- **WHEN** 缺少 required 字段
- **THEN** 返回 `Err(TemplateError::SchemaValidation(_))` 含字段名
- **WHEN** 字段类型不匹配（如 number 字段传 string）
- **THEN** 返回 `Err(TemplateError::SchemaValidation(_))`
- **WHEN** 枚举值不在 `enum_values` 列表中
- **THEN** 返回 `Err(TemplateError::SchemaValidation(_))`
- **WHEN** 数值超出 `minimum`/`maximum` 范围
- **THEN** 返回 `Err(TemplateError::SchemaValidation(_))`

### Requirement: JsonConstraint 带重试的约束推理
系统 SHALL 提供 `JsonConstraint` 结构体（`max_retries: u8` + `ConstraintStats` 统计），实现 `infer_with_constraint(&mut self, engine: &mut dyn LlmEngine, template: &dyn PromptTemplate, context: &TemplateContext) -> Result<Value, TemplateError>` 方法。调用 `engine.infer()` 执行推理，`template.validate()` 校验输出，失败时重试（最多 `max_retries` 次），重试次数耗尽返回 `Err(TemplateError::MaxRetriesExceeded)`。推理失败（`LlmError`）通过 `From<LlmError> for TemplateError` 转换。

#### Scenario: 约束推理成功
- **WHEN** 调用 `infer_with_constraint(engine, template, &context)` 且首次推理输出有效 JSON
- **THEN** 返回 `Ok(Value)`，`stats.successful += 1`
- **WHEN** 首次推理输出无效 JSON，第二次输出有效 JSON
- **THEN** 返回 `Ok(Value)`，`stats.retries += 1`，`stats.successful += 1`
- **WHEN** 所有 `max_retries + 1` 次尝试均失败
- **THEN** 返回 `Err(TemplateError::MaxRetriesExceeded)`，`stats.failed += 1`

### Requirement: extract_json 函数
系统 SHALL 提供 `extract_json(output: &str) -> Result<String, TemplateError>` 函数，处理三种输出格式：① 纯 JSON（`{...}`）；② markdown 代码块包裹（` ```json ... ``` ` 或 ` ``` ... ``` `）；③ JSON 前后含多余文字。

#### Scenario: JSON 提取
- **WHEN** 输入为纯 JSON `{"action":"charge"}`
- **THEN** 返回 `Ok("{\"action\":\"charge\"}")`
- **WHEN** 输入为 ` ```json\n{"action":"charge"}\n``` `
- **THEN** 返回 `Ok("{\"action\":\"charge\"}")`
- **WHEN** 输入为 `Result: {"action":"charge"} done`
- **THEN** 返回 `Ok("{\"action\":\"charge\"}")`
- **WHEN** 输入不含 JSON
- **THEN** 返回 `Err(TemplateError::NoJson)`

## MODIFIED Requirements

### Requirement: workspace 成员列表
根 `Cargo.toml` 的 `members` 列表新增 `"crates/ai/prompt-template"`，置于 `"crates/ai/infer-scheduler"` 之后。workspace 版本号 `0.62.0` → `0.63.0`。

## 偏差声明（D1~D12，应用 Karpathy "Think Before Coding / Simplicity First / Surgical Changes" 原则）

| ID | 蓝图原文 | 偏差说明 | 理由 |
|----|---------|---------|------|
| D1 | `pub trait PromptTemplate: Send + Sync` | 不派生 `Send + Sync` | 与 v0.59.0 `LlmEngine` trait 保持一致；单线程 no_std 无需 Send/Sync；项目内存约束已记录此规范 |
| D2 | `LlmError::JsonParseFailed`（蓝图伪代码引用） | 新增独立 `TemplateError` 枚举 | v0.59.0 `LlmError` 仅 8 变体（LoadFailed/InferFailed/InvalidPath/InvalidPrompt/Utf8Error/GpuUnavailable/ModelNotLoaded/OutOfMemory），无 `JsonParseFailed`；JSON 解析/Schema 校验失败属于模板层错误，不应污染 LlmError |
| D3 | `infer_with_constraint(...) -> Result<Value, LlmError>` | 返回 `Result<Value, TemplateError>` | 错误类型分离：推理错误（LlmError）通过 `From<LlmError>` 转换为 `TemplateError::Engine(_)`；Schema 校验错误属于模板层 |
| D4 | `lazy_static! { static ref CHARGE_DISCHARGE_SCHEMA: JsonSchema = json!({...}); }` | 改用 `SchemaSpec` 结构体 + `const` 静态字段 | `lazy_static!` 是 std-only；no_std 下用 `&'static SchemaSpec` + `&'static [SchemaField]` 编译期常量，运行时零分配 |
| D5 | 完整 JSON Schema draft 7+ 校验 | 实现最小验证器：required / type / enum / minimum / maximum | 电力场景仅需字段存在性、类型、枚举值、数值范围；完整 JSON Schema 是过度工程（违反 Simplicity First） |
| D6 | `log_warn!("JSON parse attempt {} failed: {:?}", attempt, e)` | 静默重试，失败计入 `ConstraintStats` 统计 | `log_warn!` 在 no_std 不可用；统计计数器比日志更适合 no_std 场景 |
| D7 | 蓝图 Python 测试代码（`test_prompt_template_gpu`） | 实现等价 Rust 测试（`MockEngine` + 固定 JSON 输出） | 项目规则：v0.63.0 是 Rust no_std；GPU 优先测试通过 `ComputeDevice` 控制（v0.59.0 已实现），无需 Python；MockEngine 用于单元测试，LlamaCppEngine（feature-gated in v0.59.0）用于集成测试 |
| D8 | crate 位置未明确 | 路径 `crates/ai/prompt-template/` | 遵循 §2.3.1 crate 分组规则（AI 子系统）；与 v0.59.0/v0.60.0/v0.61.0/v0.62.0 同级 |
| D9 | `JsonSchema = serde_json::Value`（蓝图类型混用） | 分离 `SchemaSpec`（验证规范）和 `serde_json::Value`（解析输出） | 蓝图将"JSON Schema 验证规范"与"JSON 解析结果"混为 `serde_json::Value`；分离后职责清晰 |
| D10 | 无 `unsafe` 块声明 | 显式声明无 `unsafe`（纯 safe Rust） | Prompt 模板不涉及 FFI/内存操作；与 v0.62.0 一致 |
| D11 | 依赖未明确 | 仅依赖 `eneros-llm-engine`（LlmEngine/InferParams/LlmError）+ `serde_json`（alloc feature） | 不依赖 v0.60.0（GgufLoader）/ v0.61.0（model-deploy）/ v0.62.0（infer-scheduler）— Prompt 模板与模型加载/部署/调度解耦；`serde_json` 使用 `{ version = "1.0", default-features = false, features = ["alloc"] }` no_std 配置（参考 eneros-config v0.26.0 模式） |
| D12 | 无 feature 门控声明 | 无 `[features]` 段（纯 Rust，无 FFI） | Prompt 模板不直接调用 llama.cpp；`infer_with_constraint` 通过 `&mut dyn LlmEngine` trait 对象间接调用，由调用方决定是否启用 `llama-cpp` feature（v0.59.0） |

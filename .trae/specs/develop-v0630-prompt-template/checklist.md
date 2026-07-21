# Checklist

## Workspace 同步
- [x] C1 根 `Cargo.toml` 版本号已更新为 `0.63.0`
- [x] C2 members 列表已添加 `crates/ai/prompt-template`（置于 `crates/ai/infer-scheduler` 之后）
- [x] C3 `cargo metadata --format-version 1` 成功

## Crate 骨架
- [x] C4 `crates/ai/prompt-template/Cargo.toml` 存在，package name = `eneros-prompt-template`
- [x] C5 dependencies 包含 `eneros-llm-engine = { path = "../llm-engine" }` + `serde_json = { version = "1.0", default-features = false, features = ["alloc"] }`（D11，参考 eneros-config v0.26.0 模式）
- [x] C6 **不声明** `[features]`（D12：纯 Rust，无 FFI）
- [x] C7 `src/lib.rs` 包含 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- [x] C8 `src/lib.rs` 包含 D1~D12 偏差声明表
- [x] C9 模块声明：error / context / schema / template / templates / extract / constraint

## error.rs — TemplateError
- [x] C10 `TemplateError` 枚举包含 5 变体（NoJson / ParseError / SchemaValidation(String) / MaxRetriesExceeded / Engine(LlmError)）
- [x] C11 派生 `Debug` + `Clone`
- [x] C12 实现 `core::fmt::Display`
- [x] C13 实现 `From<LlmError> for TemplateError`（D2/D3）
- [x] C14 手动实现 `PartialEq`（LlmError 未派生 PartialEq，使用 `core::mem::discriminant` 比较，参考 v0.62.0 模式）

## context.rs — TemplateContext
- [x] C15 `TemplateContext` 结构体：market_price / soc / power_current / temperature / time_of_day / historical_data（D4）
- [x] C16 派生 `Debug` + `Clone`
- [x] C17 `TemplateContext::new(market_price, soc, power_current, temperature, time_of_day, historical_data) -> Self`
- [x] C18 `TemplateContext::default() -> Self`（price=0.5, soc=50.0, power=0.0, temp=25.0, time="谷时", history=vec![]）

## schema.rs — SchemaSpec 验证器
- [x] C19 `SchemaType` 枚举：String / Number / Boolean / Object / Array（派生 Debug/Clone/Copy/PartialEq/Eq）
- [x] C20 `SchemaField` 结构体：name / field_type / required / enum_values / minimum / maximum
- [x] C21 `SchemaSpec` 结构体：fields: &'static [SchemaField]
- [x] C22 `SchemaSpec::new(fields) -> Self`（const fn）
- [x] C23 `SchemaSpec::validate(&self, value: &Value) -> Result<(), TemplateError>` — 校验：Object / required / type / enum / minimum / maximum（D5）
- [x] C24 单元测试：有效 JSON / 缺字段 / 类型错误 / 枚举错误 / 范围错误

## extract.rs — extract_json
- [x] C25 `extract_json(output: &str) -> Result<String, TemplateError>` 函数
- [x] C26 处理三种格式：纯 JSON / markdown 代码块（```json 或 ```）/ 含多余文字
- [x] C27 无 JSON 返回 `Err(TemplateError::NoJson)`
- [x] C28 单元测试：纯 JSON / markdown 包裹 / 含多余文字 / 无 JSON / 空

## template.rs — PromptTemplate trait
- [x] C29 `PromptTemplate` trait 定义（无 Send + Sync，D1）
- [x] C30 必需方法：`name() -> &'static str` / `build(&self, context: &TemplateContext) -> String` / `output_schema() -> &'static SchemaSpec`
- [x] C31 默认方法 `validate(&self, output: &str) -> Result<Value, TemplateError>`：extract_json → serde_json::from_str → output_schema().validate
- [x] C32 trait 编译通过

## templates.rs — 3 电力场景模板
- [x] C33 `ChargeDischargeTemplate` 结构体 + 实现 PromptTemplate
- [x] C34 schema：action(string, enum=[charge,discharge,standby]) / power_kw(number) / reason(string) / confidence(number, min=0.0, max=1.0)
- [x] C35 `DispatchTemplate` 结构体 + 实现 PromptTemplate
- [x] C36 schema：target_power(number) / ramp_rate(number) / duration_minutes(number) / reason(string)
- [x] C37 `AlarmTemplate` 结构体 + 实现 PromptTemplate
- [x] C38 schema：alarm_type(string, enum=[overvoltage,undervoltage,overcurrent,overtemperature,fault]) / severity(string, enum=[info,warning,critical]) / action(string) / target_device(string)
- [x] C39 每个模板的 `build` 返回含 ctx 参数的中文 prompt
- [x] C40 每个模板的 `output_schema` 返回对应静态 `&'static SchemaSpec` 常量
- [x] C41 单元测试：build 输出非空 / schema 校验有效 JSON / schema 校验无效 JSON

## constraint.rs — JsonConstraint + ConstraintStats
- [x] C42 `ConstraintStats` 结构体：total_attempts / successful / failed / retries（全 u64，派生 Debug/Clone/Default）
- [x] C43 `JsonConstraint` 结构体：max_retries: u8 / stats: ConstraintStats
- [x] C44 `JsonConstraint::new(max_retries: u8) -> Self`
- [x] C45 `infer_with_constraint(&mut self, engine: &mut dyn LlmEngine, template: &dyn PromptTemplate, context: &TemplateContext) -> Result<Value, TemplateError>`（D3）
- [x] C46 推理流程：stats.total_attempts += 1 → 循环 0..=max_retries → engine.infer → template.validate → 成功返回 / 失败重试 → 全部失败返回 MaxRetriesExceeded
- [x] C47 engine.infer 失败通过 `From<LlmError>` 转换为 `TemplateError::Engine(_)` 并立即返回（不重试）
- [x] C48 首次成功不增 retries，重试成功增 retries
- [x] C49 `stats(&self) -> &ConstraintStats` / `max_retries(&self) -> u8` 查询
- [x] C50 单元测试：首次成功 / 重试成功 / 全部失败 / 引擎错误

## 集成测试（lib.rs）
- [x] C51 T1 TemplateContext::new 构造 + 字段访问
- [x] C52 T2 TemplateContext::default 默认值
- [x] C53 T3 SchemaSpec::validate 有效 JSON 通过
- [x] C54 T4 SchemaSpec::validate 缺 required 字段失败
- [x] C55 T5 SchemaSpec::validate 类型不匹配失败
- [x] C56 T6 SchemaSpec::validate 枚举值不合法失败
- [x] C57 T7 extract_json 纯 JSON 提取
- [x] C58 T8 extract_json markdown 代码块提取
- [x] C59 T9 extract_json 含多余文字提取
- [x] C60 T10 extract_json 无 JSON 返回 NoJson
- [x] C61 T11 ChargeDischargeTemplate build 输出含 ctx 参数
- [x] C62 T12 DispatchTemplate + AlarmTemplate build 输出
- [x] C63 T13 JsonConstraint 首次推理成功（MockEngine + 固定 JSON）
- [x] C64 T14 JsonConstraint 重试成功（MockEngine 首次无效，第二次有效）
- [x] C65 T15 JsonConstraint 重试耗尽返回 MaxRetriesExceeded + ConstraintStats 累加
- [x] C66 `cargo test -p eneros-prompt-template` 15/15 通过

## 设计文档
- [x] C67 `docs/ai/prompt-template-design.md` 存在
- [x] C68 12 章节完整
- [x] C69 2 Mermaid 图（PromptTemplate trait 类图 + infer_with_constraint 时序图）
- [x] C70 D1~D12 偏差声明表
- [x] C71 文档在 `docs/ai/` 下（符合目录规范）

## 版本同步
- [x] C72 `Makefile` 版本号 `0.63.0`
- [x] C73 `.github/workflows/ci.yml` 版本号 `0.63.0`
- [x] C74 `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-prompt-template`

## 构建校验（§2.4.2 C6~C11）
- [x] C75 `cargo metadata --format-version 1` 成功
- [x] C76 `cargo test -p eneros-prompt-template` 全部通过（15 tests）
- [x] C77 `cargo build -p eneros-prompt-template --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C78 `cargo fmt -p eneros-prompt-template -- --check` 通过
- [x] C79 `cargo clippy -p eneros-prompt-template --all-targets -- -D warnings` 无 warning
- [x] C80 `cargo deny check licenses bans sources` 通过

## no_std 合规
- [x] C81 无 `use std::*`（仅 `alloc::*` / `core::*` + serde_json 的 alloc feature）
- [x] C82 无 `panic!` / `todo!` / `unimplemented!`
- [x] C83 子模块不重复 `#![cfg_attr(not(test), no_std)]`
- [x] C84 无 `unsafe` 块（D10：纯 safe Rust）
- [x] C85 无 `lazy_static!` / `once_cell`（D4：使用 `&'static` 静态常量）

## 目录规范
- [x] C86 crate 在 `crates/ai/prompt-template/`（D8）
- [x] C87 跨 crate path 引用 `../llm-engine`（相对路径）
- [x] C88 文档在 `docs/ai/` 下
- [x] C89 无根目录 crate（除 `ci/`）
- [x] C90 无垃圾文件（`target/` / `*.elf` / `*.bin` 被忽略）

## 依赖复用（D11）
- [x] C91 复用 v0.59.0 `LlmEngine` trait（不重定义）
- [x] C92 复用 v0.59.0 `InferParams` / `LlmError`（不重定义）
- [x] C93 `From<LlmError> for TemplateError` 转换实现
- [x] C94 **不依赖** v0.60.0（GgufLoader）/ v0.61.0（model-deploy）/ v0.62.0（infer-scheduler）— Prompt 模板与模型加载/部署/调度解耦
- [x] C95 `serde_json` 使用 `{ default-features = false, features = ["alloc"] }` no_std 配置（参考 eneros-config v0.26.0）

## 简化设计验证（Karpathy 原则）
- [x] C96 无 `Send + Sync` bounds（D1：与 v0.59.0 LlmEngine 一致）
- [x] C97 无完整 JSON Schema draft 7+ 实现（D5：最小验证器，仅 required/type/enum/minimum/maximum）
- [x] C98 无 `lazy_static!` / `once_cell`（D4：使用 `&'static` 静态常量）
- [x] C99 无 `log_warn!` 宏（D6：静默重试 + ConstraintStats 统计）
- [x] C100 无 Python 测试代码（D7：Rust MockEngine + 固定 JSON 输出）

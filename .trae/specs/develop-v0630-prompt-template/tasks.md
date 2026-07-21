# Tasks

- [x] Task 1: Workspace 同步
  - [x] 根 `Cargo.toml` 版本号 `0.62.0` → `0.63.0`
  - [x] members 添加 `crates/ai/prompt-template`（置于 `crates/ai/infer-scheduler` 之后）
  - [x] 验证：`cargo metadata --format-version 1` 成功（待 crate 骨架创建后）

- [x] Task 2: 创建 `eneros-prompt-template` crate 骨架
  - [x] 新建 `crates/ai/prompt-template/Cargo.toml`，package name = `eneros-prompt-template`
  - [x] dependencies 添加 `eneros-llm-engine = { path = "../llm-engine" }`（D11）+ `serde_json = { version = "1.0", default-features = false, features = ["alloc"] }`（D11，参考 eneros-config v0.26.0 模式）
  - [x] **不声明** `[features]`（D12：纯 Rust，无 FFI）
  - [x] no_std 配置：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
  - [x] 新建 `src/lib.rs`，模块声明：error / context / schema / template / templates / extract / constraint
  - [x] lib.rs 包含 D1~D12 偏差声明表
  - [x] 验证：`cargo metadata --format-version 1` 成功

- [x] Task 3: 实现 `error.rs` — TemplateError 错误类型
  - [x] `TemplateError` 枚举：NoJson / ParseError / SchemaValidation(String) / MaxRetriesExceeded / Engine(LlmError)
  - [x] 派生 `Debug` + `Clone`，实现 `core::fmt::Display`
  - [x] 实现 `From<LlmError> for TemplateError`（D2/D3：推理错误转换）
  - [x] 实现手动 `PartialEq`（LlmError 未派生 PartialEq，使用 `core::mem::discriminant` 比较，参考 v0.62.0 模式）
  - [x] 验证：`cargo build -p eneros-prompt-template` 通过

- [x] Task 4: 实现 `context.rs` — TemplateContext 输入上下文
  - [x] `TemplateContext` 结构体：market_price: f64 / soc: f64 / power_current: f64 / temperature: f64 / time_of_day: String / historical_data: Vec<f64>
  - [x] 派生 `Debug` + `Clone`
  - [x] `TemplateContext::new(market_price, soc, power_current, temperature, time_of_day, historical_data) -> Self`
  - [x] `TemplateContext::default() -> Self`（用于测试：price=0.5, soc=50.0, power=0.0, temp=25.0, time="谷时", history=vec![]）
  - [x] 验证：编译通过

- [x] Task 5: 实现 `schema.rs` — SchemaSpec 最小 JSON Schema 验证器
  - [x] `SchemaType` 枚举：String / Number / Boolean / Object / Array（派生 Debug/Clone/Copy/PartialEq/Eq）
  - [x] `SchemaField` 结构体：name: &'static str / field_type: SchemaType / required: bool / enum_values: &'static [&'static str] / minimum: Option<f64> / maximum: Option<f64>
  - [x] `SchemaSpec` 结构体：fields: &'static [SchemaField]
  - [x] `SchemaSpec::new(fields: &'static [SchemaField]) -> Self`（const fn）
  - [x] `SchemaSpec::validate(&self, value: &serde_json::Value) -> Result<(), TemplateError>` — 校验：① value 为 Object ② 遍历 fields，required 字段必须存在 ③ 字段类型匹配 ④ enum_values 非空时校验枚举 ⑤ minimum/maximum 校验数值范围
  - [x] 验证：单元测试 — 有效 JSON / 缺字段 / 类型错误 / 枚举错误 / 范围错误

- [x] Task 6: 实现 `extract.rs` — extract_json 函数
  - [x] `extract_json(output: &str) -> Result<String, TemplateError>` — 处理三种格式：① 纯 JSON（`{...}`）② markdown 代码块（` ```json ... ``` ` 或 ` ``` ... ``` `）③ JSON 前后含多余文字
  - [x] 算法：trim → 若 `starts_with("```")` 找首行换行后内容到 `rfind("```")`；否则找首个 `{` 到末个 `}`
  - [x] 无 JSON 时返回 `Err(TemplateError::NoJson)`
  - [x] 验证：单元测试 — 纯 JSON / markdown 包裹 / 含多余文字 / 无 JSON / 空

- [x] Task 7: 实现 `template.rs` — PromptTemplate trait
  - [x] `PromptTemplate` trait（无 Send + Sync，D1）：`name() -> &'static str` / `build(&self, context: &TemplateContext) -> String` / `output_schema() -> &'static SchemaSpec`
  - [x] 默认方法 `validate(&self, output: &str) -> Result<serde_json::Value, TemplateError>`：调用 `extract_json` → `serde_json::from_str` → `output_schema().validate`
  - [x] 验证：编译通过

- [x] Task 8: 实现 `templates.rs` — 3 个电力场景模板
  - [x] `ChargeDischargeTemplate` — 字段：action(string, enum=[charge,discharge,standby]) / power_kw(number) / reason(string) / confidence(number, min=0.0, max=1.0)
  - [x] `DispatchTemplate` — 字段：target_power(number) / ramp_rate(number) / duration_minutes(number) / reason(string)
  - [x] `AlarmTemplate` — 字段：alarm_type(string, enum=[overvoltage,undervoltage,overcurrent,overtemperature,fault]) / severity(string, enum=[info,warning,critical]) / action(string) / target_device(string)
  - [x] 每个模板的 `build(&self, ctx: &TemplateContext) -> String` 返回包含 ctx 参数的中文 prompt，要求 LLM 输出 JSON
  - [x] 每个模板的 `output_schema() -> &'static SchemaSpec` 返回对应静态常量
  - [x] 验证：单元测试 — 模板 build 输出非空 / schema 校验有效 JSON / schema 校验无效 JSON

- [x] Task 9: 实现 `constraint.rs` — JsonConstraint + ConstraintStats
  - [x] `ConstraintStats` 结构体：total_attempts: u64 / successful: u64 / failed: u64 / retries: u64（派生 Debug/Clone/Default）
  - [x] `JsonConstraint` 结构体：max_retries: u8 / stats: ConstraintStats
  - [x] `JsonConstraint::new(max_retries: u8) -> Self`
  - [x] `infer_with_constraint(&mut self, engine: &mut dyn LlmEngine, template: &dyn PromptTemplate, context: &TemplateContext) -> Result<serde_json::Value, TemplateError>`（D3：返回 TemplateError）
    - 流程：`stats.total_attempts += 1` → 循环 `0..=max_retries`：`engine.infer(prompt, params)` → `template.validate(output)` → 成功返回（首次成功不增 retries，重试成功增 retries）→ 全部失败返回 `Err(MaxRetriesExceeded)` + `stats.failed += 1`
    - `engine.infer` 失败通过 `From<LlmError> for TemplateError` 转换为 `TemplateError::Engine(_)` 并立即返回（不重试）
  - [x] `stats(&self) -> &ConstraintStats`
  - [x] `max_retries(&self) -> u8`
  - [x] 验证：单元测试 — 首次成功 / 重试成功 / 全部失败 / 引擎错误

- [x] Task 10: 集成测试模块（lib.rs `#[cfg(test)] mod tests`）
  - [x] T1 TemplateContext::new 构造 + 字段访问
  - [x] T2 TemplateContext::default 默认值
  - [x] T3 SchemaSpec::validate 有效 JSON 通过
  - [x] T4 SchemaSpec::validate 缺 required 字段失败
  - [x] T5 SchemaSpec::validate 类型不匹配失败
  - [x] T6 SchemaSpec::validate 枚举值不合法失败
  - [x] T7 extract_json 纯 JSON 提取
  - [x] T8 extract_json markdown 代码块提取
  - [x] T9 extract_json 含多余文字提取
  - [x] T10 extract_json 无 JSON 返回 NoJson
  - [x] T11 ChargeDischargeTemplate build 输出含 ctx 参数
  - [x] T12 DispatchTemplate + AlarmTemplate build 输出
  - [x] T13 JsonConstraint 首次推理成功（MockEngine + 固定 JSON）
  - [x] T14 JsonConstraint 重试成功（MockEngine 首次返回无效，第二次有效）
  - [x] T15 JsonConstraint 重试耗尽返回 MaxRetriesExceeded + ConstraintStats 累加
  - [x] 验证：`cargo test -p eneros-prompt-template` 全部通过

- [x] Task 11: 设计文档 `docs/ai/prompt-template-design.md`
  - [x] 12 章节：版本目标 / 架构定位 / PromptTemplate trait / TemplateContext / SchemaSpec 验证器 / 3 电力场景模板 / extract_json / JsonConstraint 重试机制 / 错误处理 / GPU 策略 / 内存预算 / 偏差声明
  - [x] 2 Mermaid 图：PromptTemplate trait 类图 + infer_with_constraint 时序图
  - [x] D1~D12 偏差声明表
  - [x] 文档位置在 `docs/ai/` 下（复用 v0.59.0/v0.60.0/v0.61.0/v0.62.0 创建的目录）

- [x] Task 12: 版本号同步 + gate.rs 注释更新
  - [x] `Makefile` 版本号 `0.62.0` → `0.63.0`
  - [x] `.github/workflows/ci.yml` 版本号 `0.62.0` → `0.63.0`
  - [x] `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-prompt-template` 说明
  - [x] 验证：`cargo build -p eneros-prompt-template` 通过

- [x] Task 13: 构建校验（§2.4.2 C6~C11）
  - [x] `cargo metadata --format-version 1` 成功
  - [x] `cargo test -p eneros-prompt-template` 全部通过（15 tests）
  - [x] `cargo build -p eneros-prompt-template --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 交叉编译通过
  - [x] `cargo fmt -p eneros-prompt-template -- --check` 格式通过
  - [x] `cargo clippy -p eneros-prompt-template --all-targets -- -D warnings` lint 通过
  - [x] `cargo deny check licenses bans sources` 安全扫描通过

- [x] Task 14: 更新 tasks.md + checklist.md 所有项 → [x]
  - [x] tasks.md 14 任务全部 [x]
  - [x] checklist.md 所有检查点全部 [x]

# Task Dependencies

- Task 2 → Task 1（crate 骨架需先于 metadata 验证）
- Task 3（error）→ Task 4~9（各模块使用 TemplateError）
- Task 4（context）→ Task 7（template 使用 TemplateContext）
- Task 5（schema）→ Task 7（template 使用 SchemaSpec）
- Task 6（extract）→ Task 7（template 默认 validate 使用 extract_json）
- Task 7（template trait）→ Task 8（templates 实现 trait）
- Task 8（templates）→ Task 9（constraint 使用 &dyn PromptTemplate）
- Task 9（constraint）→ Task 10（集成测试依赖 constraint）
- Task 10 → Task 3~9（集成测试依赖所有模块）
- Task 11（设计文档）可与 Task 9~10 并行（独立工作）
- Task 12 → Task 11（版本同步在功能完成后）
- Task 13 → Task 12（构建校验在版本同步后）
- Task 14 → Task 13（更新文档在全部校验通过后）

# Parallelizable Work

- Task 3（error）+ Task 4（context）+ Task 6（extract）可并行（无相互依赖）
- Task 5（schema）依赖 Task 3（error）
- Task 7（template trait）依赖 Task 4 + Task 5 + Task 6
- Task 8（templates）依赖 Task 7
- Task 9（constraint）依赖 Task 8
- Task 10 → Task 9
- Task 11（设计文档）可与 Task 9~10 并行（独立工作）

# Checklist

## Workspace 同步
- [x] C1 根 `Cargo.toml` 版本号已更新为 `0.71.0`
- [x] C2 members 列表已添加 `crates/ai/dual-brain`（置于 `crates/ai/fast-path` 之后）
- [x] C3 `cargo metadata --format-version 1` 成功

## Crate 骨架
- [x] C4 `crates/ai/dual-brain/Cargo.toml` 存在，package name = `eneros-dual-brain`
- [x] C5 dependencies 包含 8 个 eneros crate + serde + serde_json
- [x] C6 无 `[features]` 段（纯 Rust，无 FFI）
- [x] C7 `src/lib.rs` 包含 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- [x] C8 `src/lib.rs` 包含 D1~D12 偏差声明表
- [x] C9 模块声明：error / latency / sink / coordinator

## error.rs — DualBrainError
- [x] C10 `DualBrainError` 枚举：`LlmError(String)` / `ParseError(String)` / `ContractError(String)` / `SolveError(String)` / `DispatchError(String)`
- [x] C11 派生 `Debug`（D12：不派生 Clone/PartialEq）
- [x] C12 使用 `alloc::string::String`

## latency.rs — LatencyBreakdown
- [x] C13 `LatencyBreakdown` 结构体：7 环节 + total_ms（u64）
- [x] C14 派生 `Debug + Clone + Default`
- [x] C15 `calculate_total(&mut self)` 累加 7 环节
- [x] C16 `is_within_target(&self) -> bool` — `total_ms < 2000`
- [x] C17 `bottleneck(&self) -> &'static str` 返回耗时最长环节名
- [x] C18 `to_table(&self) -> String` Markdown 表格（D1：`alloc::format!`）

## sink.rs — DispatchCommand + CommandSink + MockCommandSink
- [x] C19 `DispatchCommand` 结构体：`target_device: String` / `power_kw: f64` / `ttl_ms: u32` / `timestamp: u64`
- [x] C20 `DispatchCommand` 派生 `Debug + Clone`
- [x] C21 `CommandSink` trait：`fn write(&mut self, cmd: DispatchCommand) -> Result<(), DualBrainError>`
- [x] C22 `MockCommandSink` 结构体：`commands: Vec<DispatchCommand>`
- [x] C23 `MockCommandSink::new()` / `commands()` / `write()` 实现

## coordinator.rs — DualBrainCoordinator + DualBrainResult
- [x] C24 `DualBrainResult` 结构体：`path_type` / `schedule` / `latency` / `feedback: Option<FeedbackContract>`，派生 `Debug`（D12）
- [x] C25 `DualBrainCoordinator<S: Solver>` 结构体（D4：泛型 Solver）
- [x] C26 字段：path_selector / fast_path: RealtimePathEngine<S> / llm_engine: Box<dyn LlmEngine> / prompt_template / intent_parser / converter / validator / contract_validator / sink: Box<dyn CommandSink> / request_counter
- [x] C27 `new(config, llm_engine, solver, sink) -> Self`
- [x] C28 `execute(&mut self, state: &RealtimeState, now_ms: u64) -> Result<DualBrainResult, DualBrainError>`
- [x] C29 execute：路径选择（FastPath 早返回）
- [x] C30 execute：感知层构建 SystemContext（D5：从 RealtimeState）
- [x] C31 execute：LLM 推理（D9：build + infer+params）
- [x] C32 execute：意图解析（D9：parse_json + IntentContract + validate + to_solver_params）
- [x] C33 execute：LP 求解（D8：set_param + solve+now_ms）
- [x] C34 execute：安全校验（parse_result + validate+state.system）
- [x] C35 execute：命令下发（D6：构建 DispatchCommand + sink.write）
- [x] C36 execute：延迟分解 calculate_total
- [x] C37 execute：反馈契约 to_feedback
- [x] C38 execute：request_id 格式 `req-{now_ms}-{counter}`（D2）
- [x] C39 `default_with_mock()` 实现（MockSolver + MockEngine + MockCommandSink）

## 集成测试（lib.rs）
- [x] C40 T1 LatencyBreakdown::default 全 0
- [x] C41 T2 LatencyBreakdown::calculate_total 累加正确
- [x] C42 T3 LatencyBreakdown::is_within_target 达标
- [x] C43 T4 LatencyBreakdown::is_within_target 超标
- [x] C44 T5 LatencyBreakdown::bottleneck 返回最长环节
- [x] C45 T6 LatencyBreakdown::to_table 包含环节名
- [x] C46 T7 DispatchCommand 构造
- [x] C47 T8 MockCommandSink::new 空
- [x] C48 T9 MockCommandSink::write 收集命令
- [x] C49 T10 DualBrainError 变体构造
- [x] C50 T11 DualBrainCoordinator::new 构造
- [x] C51 T12 DualBrainCoordinator::default_with_mock 构造
- [x] C52 T13 DualBrainCoordinator::execute 快路径
- [x] C53 T14 DualBrainCoordinator::execute 慢路径端到端
- [x] C54 T15 DualBrainCoordinator::execute 慢路径返回 SlowPath
- [x] C55 T16 DualBrainCoordinator::execute 慢路径 latency 各字段
- [x] C56 T17 DualBrainCoordinator::execute 慢路径 feedback 为 Some
- [x] C57 T18 DualBrainCoordinator::execute 慢路径 schedule 非空
- [x] C58 T19 DualBrainCoordinator::execute 命令下发到 sink
- [x] C59 T20 DualBrainCoordinator::execute request_id 格式
- [x] C60 T21 端到端：RealtimeState → execute → path_type
- [x] C61 T22 LatencyBreakdown::bottleneck 全 0 时返回 "none"
- [x] C62 `cargo test -p eneros-dual-brain` 全部通过

## 设计文档
- [x] C63 `docs/ai/dual-brain-design.md` 存在
- [x] C64 12 章节完整
- [x] C65 2 Mermaid 图（双脑协同端到端流程图 + 延迟分解时序图）
- [x] C66 D1~D12 偏差声明表
- [x] C67 文档在 `docs/ai/` 下

## 版本同步
- [x] C68 `Makefile` 版本号 `0.71.0`（header + VERSION 变量 2 处）
- [x] C69 `.github/workflows/ci.yml` 版本号 `0.71.0`
- [x] C70 `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-dual-brain`

## 构建校验（§2.4.2 C6~C11）
- [x] C71 `cargo metadata --format-version 1` 成功
- [x] C72 `cargo test -p eneros-dual-brain` 全部通过
- [x] C73 `cargo build -p eneros-dual-brain --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C74 `cargo fmt -p eneros-dual-brain -- --check` 通过
- [x] C75 `cargo clippy -p eneros-dual-brain --all-targets -- -D warnings` 无 warning
- [x] C76 `cargo deny check licenses bans sources` 通过

## no_std 合规
- [x] C77 无 `use std::*`（仅 `alloc::*` / `core::*`）
- [x] C78 无 `panic!` / `todo!` / `unimplemented!`
- [x] C79 子模块不重复 `#![cfg_attr(not(test), no_std)]`
- [x] C80 无 `unsafe` 块
- [x] C81 无 `Instant::now()` / `SystemTime::now()` / `uuid::Uuid::new_v4()`（D1/D2）
- [x] C82 无 `log::warn!` / `log::info!`（D10）

## 目录规范
- [x] C83 crate 在 `crates/ai/dual-brain/`
- [x] C84 跨 crate path 引用均为相对路径
- [x] C85 文档在 `docs/ai/` 下
- [x] C86 无根目录 crate（除 `ci/`）
- [x] C87 无垃圾文件

## 依赖复用
- [x] C88 复用 v0.70.0 `PathSelector` / `RealtimePathEngine` / `PathType` / `RealtimeState`
- [x] C89 复用 v0.69.0 `IntentContract` / `FeedbackContract` / `ContractValidator` / `ContractConverter`
- [x] C90 复用 v0.68.0 `IntentParser` / `Intent`
- [x] C91 复用 v0.67.0 `SafetyValidator` / `SystemState` / `ValidationResult`
- [x] C92 复用 v0.66.0 `ScheduleConfig` / `EnergyScheduleModel` / `ScheduleResult`
- [x] C93 复用 v0.64.0 `Solver` trait / `MockSolver`
- [x] C94 复用 v0.63.0 `PromptTemplate` / `ChargeDischargeTemplate` / `TemplateContext`
- [x] C95 复用 v0.59.0 `LlmEngine` / `MockEngine` / `InferParams`

## 简化设计验证（Karpathy 原则）
- [x] C96 `DualBrainError` 不派生 Clone/PartialEq（D12）
- [x] C97 `DualBrainResult` 不派生 Clone（D12）
- [x] C98 无 `[features]` 段（纯 Rust）
- [x] C99 `DispatchCommand` 本地定义（D6：不依赖 eneros-controlbus）
- [x] C100 `request_id` 用计数器而非 uuid（D2）

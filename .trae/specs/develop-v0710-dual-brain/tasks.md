# Tasks

- [x] Task 1: Workspace 同步
  - [x] 根 `Cargo.toml` 版本号 `0.70.0` → `0.71.0`
  - [x] members 添加 `crates/ai/dual-brain`（置于 `crates/ai/fast-path` 之后）
  - [x] 验证：`cargo metadata --format-version 1` 成功

- [x] Task 2: 创建 `eneros-dual-brain` crate 骨架
  - [x] 新建 `crates/ai/dual-brain/Cargo.toml`，package name = `eneros-dual-brain`
  - [x] dependencies：`eneros-solver-core` / `eneros-energy-lp-model` / `eneros-safety-validator` / `eneros-intent-parser` / `eneros-intent-contract` / `eneros-fast-path` / `eneros-llm-engine` / `eneros-prompt-template` + `serde` / `serde_json`
  - [x] 无 `[features]` 段（纯 Rust，无 FFI）
  - [x] no_std 配置：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
  - [x] 新建 `src/lib.rs`，模块声明：error / latency / sink / coordinator
  - [x] lib.rs 包含 D1~D12 偏差声明表

- [x] Task 3: 实现 `error.rs` — DualBrainError
  - [x] `DualBrainError` 枚举：`LlmError(String)` / `ParseError(String)` / `ContractError(String)` / `SolveError(String)` / `DispatchError(String)`
  - [x] 派生 `Debug`（D12：不派生 Clone/PartialEq）
  - [x] 使用 `alloc::string::String`

- [x] Task 4: 实现 `latency.rs` — LatencyBreakdown
  - [x] `LatencyBreakdown` 结构体：7 环节 + total_ms（u64）
  - [x] 派生 `Debug + Clone + Default`
  - [x] `calculate_total(&mut self)` — 累加 7 环节
  - [x] `is_within_target(&self) -> bool` — `total_ms < 2000`
  - [x] `bottleneck(&self) -> &'static str` — 返回耗时最长环节名（数组 + max_by_key）
  - [x] `to_table(&self) -> String` — Markdown 表格格式化（D1：`alloc::format!`）

- [x] Task 5: 实现 `sink.rs` — DispatchCommand + CommandSink + MockCommandSink
  - [x] `DispatchCommand` 结构体：`target_device: String` / `power_kw: f64` / `ttl_ms: u32` / `timestamp: u64`
  - [x] 派生 `Debug + Clone`
  - [x] `CommandSink` trait：`fn write(&mut self, cmd: DispatchCommand) -> Result<(), DualBrainError>`
  - [x] `MockCommandSink` 结构体：`commands: Vec<DispatchCommand>`
  - [x] `MockCommandSink::new()` / `commands()` / `write()` 实现

- [x] Task 6: 实现 `coordinator.rs` — DualBrainCoordinator + DualBrainResult
  - [x] `DualBrainResult` 结构体：`path_type` / `schedule` / `latency` / `feedback: Option<FeedbackContract>`，派生 `Debug`（D12）
  - [x] `DualBrainCoordinator<S: Solver>` 结构体（D4：泛型 Solver）：path_selector / fast_path / llm_engine / prompt_template / intent_parser / converter / validator / contract_validator / sink / request_counter
  - [x] `new(config, llm_engine, solver, sink) -> Self`
  - [x] `execute(&mut self, state: &RealtimeState, now_ms: u64) -> Result<DualBrainResult, DualBrainError>`：7 步执行
    - 路径选择 → FastPath 早返回 / SlowPath 完整链路
    - 感知层（D5：RealtimeState → SystemContext）
    - LLM 推理（D9：build + infer+params）
    - 意图解析（D9：parse_json + IntentContract + validate + to_solver_params）
    - LP 求解（D8：set_param + solve+now_ms）
    - 安全校验（parse_result + validate+state.system）
    - 命令下发（D6：构建 DispatchCommand + sink.write）
    - 延迟分解（每步记录 ms，calculate_total）
    - 反馈契约（to_feedback）
  - [x] 实现 `default_with_mock()`（使用 MockSolver + MockEngine + MockCommandSink）

- [x] Task 7: 集成测试（lib.rs）— 至少 20 个测试
  - [x] T1 LatencyBreakdown::default 全 0
  - [x] T2 LatencyBreakdown::calculate_total 累加正确
  - [x] T3 LatencyBreakdown::is_within_target 达标
  - [x] T4 LatencyBreakdown::is_within_target 超标
  - [x] T5 LatencyBreakdown::bottleneck 返回最长环节
  - [x] T6 LatencyBreakdown::to_table 包含环节名
  - [x] T7 DispatchCommand 构造
  - [x] T8 MockCommandSink::new 空
  - [x] T9 MockCommandSink::write 收集命令
  - [x] T10 DualBrainError 变体构造
  - [x] T11 DualBrainCoordinator::new 构造
  - [x] T12 DualBrainCoordinator::default_with_mock 构造
  - [x] T13 DualBrainCoordinator::execute 快路径（首次走慢路径不适用，用非首次状态走快路径）
  - [x] T14 DualBrainCoordinator::execute 慢路径端到端（MockEngine 返回 JSON intent）
  - [x] T15 DualBrainCoordinator::execute 慢路径返回 SlowPath
  - [x] T16 DualBrainCoordinator::execute 慢路径 latency 各字段 > 0
  - [x] T17 DualBrainCoordinator::execute 慢路径 feedback 为 Some
  - [x] T18 DualBrainCoordinator::execute 慢路径 schedule 非空
  - [x] T19 DualBrainCoordinator::execute 命令下发到 sink
  - [x] T20 DualBrainCoordinator::execute request_id 格式 req-{ms}-{counter}
  - [x] T21 端到端：RealtimeState → execute → DualBrainResult.path_type
  - [x] T22 LatencyBreakdown::bottleneck 全 0 时返回 "none"

- [x] Task 8: 创建设计文档 `docs/ai/dual-brain-design.md`
  - [x] 12 章节完整（版本目标 / 前置依赖 / 交付物 / 详细设计 / 技术交底 / 测试计划 / 验收标准 / 风险 / 多角度要求 / ADR / 偏差声明 / 参考）
  - [x] 2 Mermaid 图（双脑协同端到端流程图 + 延迟分解时序图）
  - [x] D1~D12 偏差声明表
  - [x] 文档位于 `docs/ai/` 下（C12）

- [x] Task 9: 版本同步
  - [x] `Makefile` 版本号 `0.71.0`（header + VERSION 变量 2 处）
  - [x] `.github/workflows/ci.yml` 版本号 `0.71.0`
  - [x] `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-dual-brain`

- [x] Task 10: 6 项构建校验（§2.4.2 C6~C11）
  - [x] `cargo metadata --format-version 1` 成功
  - [x] `cargo test -p eneros-dual-brain` 全部通过
  - [x] `cargo build -p eneros-dual-brain --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
  - [x] `cargo fmt -p eneros-dual-brain -- --check` 通过
  - [x] `cargo clippy -p eneros-dual-brain --all-targets -- -D warnings` 无 warning
  - [x] `cargo deny check licenses bans sources` 通过
  - [x] 更新 tasks.md / checklist.md 全部 [x]

# Task Dependencies
- Task 2 依赖 Task 1
- Task 3~6 依赖 Task 2（并行实现）
- Task 7 依赖 Task 3~6
- Task 8 可与 Task 3~7 并行
- Task 9 依赖 Task 2
- Task 10 依赖 Task 3~9 全部完成

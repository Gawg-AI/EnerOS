# v0.71.0 双脑协同联调 Spec

## Why

Phase 1 核心里程碑：双脑架构首次端到端跑通。打通"感知→LLM 推理→意图解析→LP 求解→安全校验→命令下发"完整链路，实现 `DualBrainCoordinator` 统一编排与 `LatencyBreakdown` 延迟分解测量，目标端到端 < 2s。

## What Changes

- 新建 crate `eneros-dual-brain`（`crates/ai/dual-brain/`），实现双脑协同编排层
- 新增 `DualBrainCoordinator<S: Solver>` — 端到端协调器（泛型 Solver，默认 MockSolver）
- 新增 `LatencyBreakdown` — 7 环节延迟分解测量（perception/llm_inference/intent_parse/lp_build/lp_solve/safety_validate/command_dispatch）
- 新增 `DualBrainResult` — 双脑结果（路径类型 + 调度 + 延迟 + 反馈契约）
- 新增 `DualBrainError` — 错误枚举（LlmError/ParseError/ContractError/SolveError/DispatchError）
- 新增 `DispatchCommand` + `CommandSink` trait + `MockCommandSink` — 命令下发抽象（D6：本地定义，避免跨子系统内核依赖）
- 快/慢路径切换：复用 v0.70.0 `PathSelector` + `RealtimePathEngine`
- 契约闭环：复用 v0.69.0 `IntentContract` / `FeedbackContract` / `ContractValidator` / `ContractConverter`
- 根 `Cargo.toml` 版本号 `0.70.0` → `0.71.0`，members 添加 `crates/ai/dual-brain`

## Impact

- Affected specs: P1-K 双脑协同第三层（v0.69.0~v0.71.0 收官）
- Affected code:
  - 新建 `crates/ai/dual-brain/`（6 源文件 + Cargo.toml）
  - 根 `Cargo.toml`（版本 + members）
  - `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs`（版本同步）
  - 新建设计文档 `docs/ai/dual-brain-design.md`

## ADDED Requirements

### Requirement: LatencyBreakdown 延迟分解测量

系统 SHALL 提供 `LatencyBreakdown` 结构体，记录双脑链路 7 个环节的耗时（ms）：

```rust
pub struct LatencyBreakdown {
    pub perception_ms: u64,
    pub llm_inference_ms: u64,
    pub intent_parse_ms: u64,
    pub lp_build_ms: u64,
    pub lp_solve_ms: u64,
    pub safety_validate_ms: u64,
    pub command_dispatch_ms: u64,
    pub total_ms: u64,
}
```

- `calculate_total(&mut self)` — 累加 7 环节为 `total_ms`
- `is_within_target(&self) -> bool` — `total_ms < 2000`
- `bottleneck(&self) -> &str` — 返回耗时最长的环节名
- `to_table(&self) -> String` — 格式化为 Markdown 表格（D1：用 `alloc::format!`）

派生 `Debug + Clone + Default`。

#### Scenario: 延迟达标
- **WHEN** 各环节耗时总和 < 2000ms
- **THEN** `is_within_target()` 返回 `true`

#### Scenario: 瓶颈识别
- **WHEN** LLM 推理耗时最长（1200ms）
- **THEN** `bottleneck()` 返回 `"llm_inference"`

### Requirement: DualBrainCoordinator 双脑协调器

系统 SHALL 提供 `DualBrainCoordinator<S: Solver>` 结构体，端到端编排双脑链路：

```rust
pub struct DualBrainCoordinator<S: Solver> {
    path_selector: PathSelector,
    fast_path: RealtimePathEngine<S>,
    llm_engine: Box<dyn LlmEngine>,
    prompt_template: ChargeDischargeTemplate,
    intent_parser: IntentParser,
    converter: ContractConverter,
    validator: SafetyValidator,
    contract_validator: ContractValidator,
    sink: Box<dyn CommandSink>,
    request_counter: u64,
}
```

- `new(config, llm_engine, solver, sink) -> Self` — 构造协调器
- `execute(&mut self, state: &RealtimeState, now_ms: u64) -> Result<DualBrainResult, DualBrainError>` — 端到端执行

执行流程（7 步）：
1. **路径选择** — `path_selector.select(state, now_ms)`；若 `FastPath`，调用 `fast_path.execute()` 并返回（跳过 LLM）
2. **感知层** — 从 `RealtimeState` 构建 `SystemContext`（D5）
3. **LLM 推理** — `prompt_template.build(&TemplateContext)` → `llm_engine.infer(&prompt, &InferParams)`（D10）
4. **意图解析** — `intent_parser.parse_json(&llm_output)` → 构建 `IntentContract` → `contract_validator.validate(&contract)` → `converter.to_solver_params(&contract, &state.system)`（D9）
5. **LP 求解** — `fast_path.solver.set_param("time_limit", "0.5")` → `fast_path.solver.solve(&problem, now_ms)`（D8）
6. **安全校验** — `EnergyScheduleModel::parse_result()` → `validator.validate(&schedule, &state.system)`
7. **命令下发** — 查找 `current_period` 的 `ScheduleEntry` → 构建 `DispatchCommand` → `sink.write(cmd)`（D6）

#### Scenario: 快路径执行
- **WHEN** `PathSelector::select()` 返回 `FastPath`
- **THEN** 调用 `RealtimePathEngine::execute()`，不调用 LLM，`DualBrainResult.path_type == FastPath`

#### Scenario: 慢路径执行
- **WHEN** `PathSelector::select()` 返回 `SlowPath`
- **THEN** 执行完整 7 步链路，`DualBrainResult.path_type == SlowPath`，`feedback` 为 `Some`

### Requirement: DualBrainResult 双脑结果

```rust
pub struct DualBrainResult {
    pub path_type: PathType,
    pub schedule: ScheduleResult,
    pub latency: LatencyBreakdown,
    pub feedback: Option<FeedbackContract>,
}
```

派生 `Debug`（D12：不派生 Clone，Karpathy 简化原则）。

### Requirement: DualBrainError 错误枚举

```rust
pub enum DualBrainError {
    LlmError(String),
    ParseError(String),
    ContractError(String),
    SolveError(String),
    DispatchError(String),
}
```

派生 `Debug`（D12：不派生 Clone/PartialEq）。使用 `alloc::string::String`。

### Requirement: DispatchCommand + CommandSink 命令下发抽象

```rust
pub struct DispatchCommand {
    pub target_device: String,
    pub power_kw: f64,
    pub ttl_ms: u32,
    pub timestamp: u64,
}

pub trait CommandSink {
    fn write(&mut self, cmd: DispatchCommand) -> Result<(), DualBrainError>;
}

pub struct MockCommandSink { /* commands: Vec<DispatchCommand> */ }
```

- `MockCommandSink::new()` — 创建空 sink
- `MockCommandSink::commands()` — 返回已收集的命令引用
- `MockCommandSink::write()` — 将命令存入 Vec，返回 `Ok(())`

### Requirement: no_std 合规

- `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- 仅使用 `alloc::*` / `core::*`
- 无 `Instant::now()` / `SystemTime::now()` / `uuid::Uuid::new_v4()`（D1/D2）
- 无 `log::warn!` / `log::info!`（D10）
- 子模块不重复 `#![cfg_attr(not(test), no_std)]`

## MODIFIED Requirements

### Requirement: Workspace 版本同步

- 根 `Cargo.toml` 版本号 `0.70.0` → `0.71.0`
- members 列表添加 `"crates/ai/dual-brain"`（置于 `"crates/ai/fast-path"` 之后）
- `Makefile` 版本号 `0.71.0`（header + VERSION 变量）
- `.github/workflows/ci.yml` 版本号 `0.71.0`
- `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-dual-brain`

## 偏差声明（D1~D12，Karpathy "Think Before Coding"）

| 偏差 | 蓝图原文 | 本版本处理 | 理由 |
|------|---------|-----------|------|
| **D1** | `Instant::now()` / `SystemTime::now()` | `now_ms: u64` 参数 | no_std 合规：`Instant`/`SystemTime` 不可用；与 v0.57/v0.64/v0.70 一致 |
| **D2** | `uuid::Uuid::new_v4().to_string()` | `format!("req-{}-{}", now_ms, counter)` | no_std 无 uuid crate；计数器确定性可测试 |
| **D3** | `LlamaCppEngine::new("/models/qwen2.5-7b-q4_k_m.gguf")` | `Box<dyn LlmEngine>` + 默认 `MockEngine` | v0.59.0 `LlamaCppEngine` feature-gated；蓝图字段类型已是 `Box<dyn LlmEngine>` |
| **D4** | `solver: HighsSolver` | `DualBrainCoordinator<S: Solver>` 泛型 | v0.64.0 `HighsSolver` feature-gated；与 v0.70.0 一致 |
| **D5** | 蓝图 `SystemState` 含 `soc`/`current_power`/`current_price`/`current_period`/`device_status`/`alarms` | 输入用 `RealtimeState`（v0.70.0），内部构建 `SystemContext`（v0.69.0） | v0.67.0 `SystemState` 仅含电气字段；v0.70.0 `RealtimeState` 已包装电价/负荷 |
| **D6** | `ControlBusHandle` + `self.control_bus.write(command)` | 本地定义 `DispatchCommand` + `CommandSink` trait + `MockCommandSink` | `ControlBusHandle` 不存在；v0.22.0 `command_send` 是全局函数需 ring 初始化；本地抽象保持 crate 自包含可测试 |
| **D7** | 蓝图 `ControlCommand` 字段（`command_id: String` / `target_device: String` / `power_kw: f64`） | `DispatchCommand` 字段（`target_device: String` / `power_kw: f64` / `ttl_ms: u32` / `timestamp: u64`） | v0.22.0 `ControlCommand` 字段差异大（`cmd_id: [u8;16]` / `DeviceId` / `setpoint: f32`）；本地类型匹配蓝图语义 |
| **D8** | `solver.set_time_limit(0.5)` + `solver.solve(&problem)` | `solver.set_param("time_limit", "0.5")` + `solver.solve(&problem, now_ms)` | v0.64.0 `Solver` trait API：`set_param` 非 `set_time_limit`；`solve` 需 `now_ms` 参数 |
| **D9** | `prompt_template.render(&context)` / `llm_engine.infer(&prompt)` / `validator.validate(&contract)` | `prompt_template.build(&TemplateContext)` / `llm_engine.infer(&prompt, &InferParams)` / `contract_validator.validate(&contract)` 返回 `Result<(), ContractError>` | v0.63.0/v0.59.0/v0.69.0 实际 API 签名 |
| **D10** | `log::warn!("双脑链路延迟超标...")` / `log::info!("瓶颈环节...")` | 移除日志；`DualBrainResult.latency` 携带延迟数据 | no_std 无 `log` crate；caller 自行检查 `latency.is_within_target()` |
| **D11** | crate 位置未明确 | `crates/ai/dual-brain/` | 项目规则 §2.3.1：AI 子系统 |
| **D12** | `DualBrainError` 派生 `Debug + Clone` | 仅 `Debug` | Karpathy 简化原则，与 v0.68/v0.69/v0.70 一致 |

## 依赖复用清单

| 复用版本 | 复用类型 | 用途 |
|---------|---------|------|
| v0.70.0 | `PathSelector` / `RealtimePathEngine<S>` / `PathType` / `RealtimeState` / `FastPathResult` | 快/慢路径选择与快速路径执行 |
| v0.69.0 | `IntentContract` / `FeedbackContract` / `ContractValidator` / `ContractConverter` / `SystemContext` / `LlmMeta` / `DeviceStatus` | 意图契约与转换 |
| v0.68.0 | `IntentParser` / `Intent` | JSON → Intent 解析 |
| v0.67.0 | `SafetyValidator` / `SystemState` / `ValidationResult` | 安全校验 |
| v0.66.0 | `ScheduleConfig` / `EnergyScheduleModel` / `ScheduleResult` | LP 模型构建与结果解析 |
| v0.64.0 | `Solver` trait / `MockSolver` / `SolveResult` | LP 求解 |
| v0.63.0 | `PromptTemplate` / `ChargeDischargeTemplate` / `TemplateContext` | Prompt 模板 |
| v0.59.0 | `LlmEngine` / `MockEngine` / `InferParams` | LLM 推理引擎 |

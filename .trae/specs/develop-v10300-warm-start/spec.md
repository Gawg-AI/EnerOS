# v0.103.0 Solver 神经部分热启动 Spec

> 蓝图：`e:\eneros\蓝图\phase2.md` §v0.103.0（P2-F 第 2 版，9 节齐全）。新建 crate `crates/ai/solver-warm/`（eneros-solver-warm）+ solver-core 热启动注入增量（feature-gated）。蓝图检索确认无 v0.103.x 刚性子版本（Phase 2 刚性子版本仅 v0.98.1）。

## Why

日前 MILP 冷启动求解耗时分钟级。蓝图要求用神经网络启发式生成 MILP 初始候选解注入 HiGHS 热启动，加速 ≥ 30%。v0.102.0 已落地 UC MILP 基座（`UnitCommitment`/`DayAheadScheduler`），本版补齐热启动链路：ONNX 推理（feature-gated）+ 特征编解码 + 候选解投影 + 注入 seam + 冷启动降级，为 v0.104.0 Pareto 提供加速底座。

## What Changes

- **新建** `crates/ai/solver-warm/`（`eneros-solver-warm`，no_std + alloc，零第三方依赖）：
  - `src/candidate.rs`：`CandidateSolution` + 可行性投影（连续 clamp 到界 / 整数 0.5 阈值二值化）+ 按 `LpProblem.var_types` 合并为完整解向量
  - `src/heuristic_net.rs`：`InferEngine` trait（推理 seam）+ `MockEngine`（默认，预设输出）+ `OnnxEngine`（`onnx-ffi` feature，NonNull+Drop RAII）+ `HeuristicNet<E>`（encode_input / decode_output 纯 Rust 逻辑）
  - `src/warm_start.rs`：`WarmStartProvider` trait + `SolveContext` + `WarmStarter` 编排（生成 → 置信度阈值判定 → 投影 → 合并 → 注入）+ 可观测计数器
  - `src/ffi.rs`：`#[cfg(feature = "onnx-ffi")]` ONNX Runtime C API 声明（`ort_create_session`/`ort_run_session`/`ort_free_session`，纯新增模块）
  - `src/lib.rs`：模块声明 + 重导出 + crate 文档（含 D1~D12 偏差表）
- **修改** `crates/ai/solver-core/src/solver.rs`：`Solver` trait 追加**默认方法** `set_warm_start(&mut self, _solution: &[f64]) -> Result<(), SolverError> { Ok(()) }`（默认 no-op，非 BREAKING）
- **修改** `crates/ai/solver-core/src/mock.rs`：`MockSolver` 覆写 `set_warm_start` 记录注入向量（`pub warm_start: Option<Vec<f64>>` 可断言）
- **修改** `crates/ai/solver-core/src/ffi.rs`：feature-gated 追加 `Highs_setSolution` extern 声明（纯追加，既有声明零改动）
- **修改** `crates/ai/solver-core/src/highs.rs`：`HighsSolver` 覆写 `set_warm_start` 调 `Highs_setSolution`（SAFETY 注释同既有风格）
- **修改** `crates/ai/solver-core/src/lib.rs`：crate 文档追加 v0.103.0 一句说明（既有偏差表不动）；`Cargo.toml` description 追加 v0.103.0
- **新增** `configs/warm-start.toml`：`[warm_start]` model_path / confidence_threshold / input_dim / output_dim + 中文注释 ≥6 点
- **新增** `docs/ai/warm-start-design.md`：12 章节 + ≥2 Mermaid + D1~D12 偏差表
- **新增 30 个单元测试**（solver-warm src 内嵌 `#[cfg(test)]`）+ solver-core 2 个（T19/T20）
- 根 `Cargo.toml`：members 追加 `"crates/ai/solver-warm"` + version 0.102.0 → 0.103.0；`Makefile` / `ci.yml` / `gate.rs` 注释串尾 2 处同步
- **无 BREAKING**：`Solver::set_warm_start` 为默认方法，既有全部实现（MockSolver/HighsSolver/各 crate stub）零改动可编译

## Impact

- Affected specs：develop-v10300-warm-start（新建）；develop-v0640-solver-core（MODIFIED，默认方法 + feature-gated 增量）
- Affected code：`crates/ai/solver-warm/`（新建）、`crates/ai/solver-core/src/{solver,mock,ffi,highs,lib}.rs` + `Cargo.toml`（增量）、`configs/`、`docs/ai/`、根 4 文件版本号
- 上游：v0.102.0 solver-milp（LpProblem/UC 模型）、v0.64.0 solver-core（Solver trait/MockSolver/HiGHS FFI）、v0.59.0 LLM 引擎（推理框架复用先例）
- 下游：v0.104.0 Pareto（热启动加速底座）；v0.116.0 模型签名校验（蓝图 §7.3 预留集成点）

## ADDED Requirements

### Requirement: 候选解与可行性投影（candidate.rs）

The system SHALL provide `CandidateSolution { continuous, integer, confidence }` 与投影/合并能力：连续变量 clamp 到 `[lower, upper]`，整数变量 > 0.5 → 1 否则 0，并按 `LpProblem.var_types` 逐列合并为完整热启动解向量（连续列取 continuous 序、整数列取 integer 序）。

#### Scenario: 投影到可行域（蓝图 §4.4）
- **WHEN** 连续值越界（如 12.0，上界 10.0）或整数值 0.7 / 0.2
- **THEN** 投影后连续 == 10.0（clamp）；整数 == 1 / 0（0.5 阈值）；confidence 不变（投影不修改信度）

#### Scenario: 合并为完整解向量
- **WHEN** `LpProblem.var_types = [Continuous, Binary, Continuous, Integer]`，continuous=[1.0, 2.0]，integer=[1, 0]
- **THEN** `to_solution(&problem)` 返回 `[1.0, 1.0, 2.0, 0.0]`；`solution.len() == problem.variables.len()`

### Requirement: 神经网络编解码（heuristic_net.rs）

The system SHALL provide `HeuristicNet<E: InferEngine>`：`encode_input(ctx)` 按 负荷预测 + 电价 + 历史计划（末条 schedule 逐机组 generation 顺序）拼接，零填充/截断至 `input_dim`；`infer` 委托 `InferEngine` seam；`decode_output(output, problem)` 逐列判定 `var_types`：连续 clamp 到列界、整数二值化且按蓝图公式累积置信度 `confidence *= 1.0 - |v−0.5|·2`，最终 `confidence /= num_vars`。

#### Scenario: 特征编码维度
- **WHEN** load_forecast 24 + price_signal 24 + 历史 2 机组×24，input_dim=72
- **THEN** 编码后 `input.len() == 72`；超出截断、不足零填充；空 history 时仅 负荷+电价+零填充

#### Scenario: 解码二值化与置信度
- **WHEN** 输出列对应 Binary 变量，值 0.9 / 0.5
- **THEN** 0.9 → 1（因子 0.2 累积）；0.5 → 0（因子 0.0 → 整体 confidence 归零）

#### Scenario: 推理失败（蓝图 §4.4）
- **WHEN** `InferEngine::infer` 返回 Err
- **THEN** `generate` 返回 `Err(WarmError::InferenceFailed(_))`，由上层回退冷启动（不静默吞没）

### Requirement: 热启动编排与注入（warm_start.rs）

The system SHALL provide `WarmStartProvider` trait（`generate(&self, problem, ctx) -> Result<CandidateSolution, WarmError>`）与 `WarmStarter { confidence_threshold, warm_used_count, warm_rejected_count, cold_fallback_count }`：`plan_warm` 流程 = provider.generate → Err → cold_fallback_count+=1 返回 None；confidence < threshold → warm_rejected_count+=1 返回 None；否则投影合并 → `solver.set_warm_start(&solution)` → warm_used_count+=1 返回 Some(solution)。

#### Scenario: 置信度过低忽略热启动（蓝图 §4.4）
- **WHEN** candidate.confidence = 0.3 < threshold 0.5
- **THEN** 返回 None，不调用 set_warm_start；warm_rejected_count == 1，warm_used_count == 0

#### Scenario: 推理失败回退冷启动
- **WHEN** provider.generate 返回 Err
- **THEN** 返回 None，cold_fallback_count == 1；求解器状态不受污染（后续 solve 即冷启动）

#### Scenario: 成功注入
- **WHEN** confidence 0.9 ≥ threshold，MockSolver 注入
- **THEN** 返回 Some(solution)；`mock.warm_start == Some(solution)`；warm_used_count == 1

### Requirement: solver-core 热启动注入增量

The system SHALL 在 `Solver` trait 追加默认方法 `set_warm_start(&mut self, _solution: &[f64]) -> Result<(), SolverError> { Ok(()) }`；`MockSolver` 覆写记录到 `pub warm_start: Option<Vec<f64>>`；`HighsSolver`（`highs-ffi`）覆写调 `Highs_setSolution`；`ffi.rs` 追加对应 extern 声明（纯追加）。

#### Scenario: 既有实现零回归
- **WHEN** 既有 stub（各 crate `impl Solver`）未覆写 set_warm_start
- **THEN** 默认 no-op 编译通过；solver-core 18 测试 + 全 workspace 回归全绿

#### Scenario: Mock 记录可断言
- **WHEN** `mock.set_warm_start(&[1.0, 0.0, 2.0])`
- **THEN** `mock.warm_start == Some(vec![1.0, 0.0, 2.0])`；重复调用覆盖（保留末次）

## MODIFIED Requirements

### Requirement: solver-core crate 文档与描述

crate 文档与 `Cargo.toml` description 追加 v0.103.0 热启动注入说明（既有 D1~D12 偏差表与模块结构零改动）。

## REMOVED Requirements

无。

## 偏差声明（D1~D12，相对蓝图 §3/§4/§5）

| 编号 | 偏差 | 理由 |
|------|------|------|
| **D1** | 蓝图 `crates/solver_warm/` → `crates/ai/solver-warm/` | 记忆 §2.3.1 强制：crate 归 `crates/<subsystem>/`；与 solver-core/solver-milp 同 AI 子系统 |
| **D2** | 蓝图 `docs/phase2/warm_start.md` → `docs/ai/warm-start-design.md` | 记忆 §2.3.3 强制：文档按方向分类 |
| **D3** | 蓝图 `tests/warm_start_bench.rs` → src 内嵌 `#[cfg(test)]` | v0.87.0~v0.102.0 项目惯例，不新增 tests/ 文件 |
| **D4** | 不重定义 `MilpModel`；蓝图 `model.integrality/col_lower/col_upper/num_vars` → 复用 v0.64.0 `LpProblem.var_types/lower_bounds/upper_bounds/variables.len()`（v0.102.0 D4 复用先例） | 避免平行类型体系（Karpathy Simplicity First） |
| **D5** | 蓝图 `WarmStartProvider: Send + Sync` → 去除 bound | 与 v0.64.0 `Solver`/v0.59.0 `LlmEngine` 惯例一致；ONNX session 原始指针本非 Send/Sync，bound 与 FFI 设计自相矛盾 |
| **D6** | ONNX FFI 独立 feature `onnx-ffi`（默认关闭）；`InferEngine` seam + `MockEngine` 默认可测 | 真实 ONNX C 库编译超出单元测试范围（v0.64.0 D2/D10、v0.102.0 D5 先例）；默认构建零 unsafe 零 C 依赖 |
| **D7** | 蓝图 `HeuristicNet::load(path, device)` 返回具体 struct → `HeuristicNet<E: InferEngine>` 泛型 + `OnnxEngine::load`（feature-gated）/ `MockEngine::new` | 推理后端可注入（记录型 stub 验证编码路径）；蓝图 72/96 维度硬编码改为构造参数 `input_dim/output_dim` |
| **D8** | 蓝图"置信度过低 → 忽略"未量化 → `confidence_threshold` 构造注入（默认 0.5，配置化） | 判定阈值显式化（D10 参数配置化惯例） |
| **D9** | 可行性投影落地为 连续 clamp + 整数 0.5 二值化（即蓝图 decode_output 语义），不做约束级 LP 投影 | 约束级投影需求解 LP，过度工程化；界内投影已满足 HiGHS setSolution 要求 |
| **D10** | 加速 metric 落地为 `warm_used_count/warm_rejected_count/cold_fallback_count` 计数器 | no_std 无 log crate，metric 字段化（v0.102.0 D9 先例） |
| **D11** | 加速 ≥30% 为硬件集成验证项（真实 ONNX 模型 + HiGHS）；本版测试注入路径正确性（Mock 记录断言），不对加速比实测断言 | v0.102.0 D11 性能口径先例；设计文档声明口径 |
| **D12** | `ComputeDevice` 仅保留 `Cpu` 变体（蓝图 `Gpu(String)` 删除）；边缘推理 CPU-only（蓝图 §6.6 GPU 规则不适用 Solver） | 蓝图自相矛盾（§4.1 定义 Gpu 变体 vs §6.6 声明不适用 GPU）；避免死代码 |

## 接口契约

```rust
// candidate.rs
pub struct CandidateSolution {
    pub continuous: Vec<f64>, pub integer: Vec<i32>, pub confidence: f64,
}  // Debug/Clone
impl CandidateSolution {
    pub fn project(&mut self, problem: &LpProblem);          // 连续 clamp 列界（D9）
    pub fn to_solution(&self, problem: &LpProblem) -> Vec<f64>; // 按 var_types 合并
}

// heuristic_net.rs
pub trait InferEngine {
    fn infer(&self, input: &[f32]) -> Result<Vec<f32>, WarmError>;
    fn input_dim(&self) -> usize;
    fn output_dim(&self) -> usize;
}
pub struct MockEngine { /* 预设输出 */ }                      // 默认可用
pub struct OnnxEngine { /* NonNull session + Drop */ }       // feature = "onnx-ffi"
pub struct HeuristicNet<E: InferEngine> { pub engine: E }
impl<E: InferEngine> HeuristicNet<E> {
    pub fn new(engine: E) -> Self;
    pub fn encode_input(&self, ctx: &SolveContext) -> Vec<f64>;   // 拼接+零填充/截断
    pub fn decode_output(&self, output: &[f64], problem: &LpProblem) -> CandidateSolution; // D9 投影+蓝图置信度公式
}

// warm_start.rs
pub struct SolveContext {
    pub load_forecast: Vec<f64>, pub price_signal: Vec<f64>,
    pub history: Vec<DayAheadPlan>,   // 复用 v0.102.0 eneros-solver-milp
}
pub trait WarmStartProvider {         // 无 Send+Sync（D5）
    fn generate(&self, problem: &LpProblem, ctx: &SolveContext) -> Result<CandidateSolution, WarmError>;
}
impl<E: InferEngine> WarmStartProvider for HeuristicNet<E> { /* encode → infer → decode */ }
pub struct WarmStarter {
    pub confidence_threshold: f64,
    pub warm_used_count: u64, pub warm_rejected_count: u64, pub cold_fallback_count: u64,
}
impl WarmStarter {
    pub fn new(confidence_threshold: f64) -> Self;
    pub fn plan_warm(
        &mut self, provider: &dyn WarmStartProvider, problem: &LpProblem,
        ctx: &SolveContext, solver: &mut dyn Solver,
    ) -> Option<Vec<f64>>;   // Some=已注入；None=回退冷启动（D10 计数器可观测）
}
pub enum WarmError { ModelLoadFailed, InferenceFailed(i32), InvalidDim }  // Debug/Clone/PartialEq

// solver-core solver.rs 增量（默认方法，非 BREAKING）
pub trait Solver {
    /* 既有方法零改动 */
    fn set_warm_start(&mut self, _solution: &[f64]) -> Result<(), SolverError> { Ok(()) }
}
// solver-core ffi.rs 增量（#[cfg(feature = "highs-ffi")])
extern "C" { pub fn Highs_setSolution(highs: HighsPtr, col_value: *const f64) -> c_int; }
```

## 测试规划（solver-warm 30 个 + solver-core 2 个）

| 文件 | 编号 | 数量 | 覆盖 |
|------|------|------|------|
| candidate.rs | TC1~TC8 | 8 | 构造字段 / 投影 clamp 上下界 / 投影不改整数不改信度 / to_solution 合并顺序（4 列混排）/ 长度==num_vars / 全连续问题 / 全整数问题 / 空候选 |
| heuristic_net.rs | TH9~TH20 | 12 | encode 维度==input_dim / 拼接顺序（负荷→电价→历史 generation）/ 零填充 / 截断 / 空 history / decode 连续 clamp / decode 二值化 0.9→1 / 0.5→0 信度归零 / 蓝图置信度公式累积 / MockEngine 驱动 generate e2e / infer Err 透传 / 维度 mismatch InvalidDim |
| warm_start.rs | TW21~TW30 | 10 | 成功注入 Some+Mock 记录 / 低置信 None+rejected 计数 / 推理失败 None+fallback 计数 / 计数器真值（三分支各 1）/ threshold 边界 ==判定通过 / 解向量内容==合并投影结果 / 空 ctx 可用 / WarmStarter::new 计数器清零 / WarmError 变体 / provider dyn seam |
| solver-core | T19/T20 | 2 | MockSolver 记录注入（含覆盖末次）/ 默认方法 no-op（自定义 stub 不覆写） |

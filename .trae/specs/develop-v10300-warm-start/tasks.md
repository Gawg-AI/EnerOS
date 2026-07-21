# Tasks — v0.103.0 Solver 神经部分热启动

> Spec：`spec.md`（develop-v10300-warm-start）。T1 与 T2 无依赖可并行；T3 依赖 T2；T4 依赖 T1+T3；T5/T6 顺序收尾。

- [x] **T1：solver-core 热启动注入增量（默认方法 + feature-gated FFI）**
  - [x] 1.1 `solver.rs`：`Solver` trait 追加默认方法 `set_warm_start(&mut self, _solution: &[f64]) -> Result<(), SolverError> { Ok(()) }`（既有方法签名零改动，注释注明 v0.103.0）
  - [x] 1.2 `mock.rs`：`MockSolver` 追加 `pub warm_start: Option<Vec<f64>>`（`new`/`with_result` 初始 None）+ 覆写 `set_warm_start` 记录末次注入
  - [x] 1.3 `ffi.rs` 追加 `Highs_setSolution(highs, col_value: *const f64) -> c_int` extern 声明（纯追加，注释注明 v0.103.0）；`highs.rs` 覆写 `set_warm_start` 调 FFI（SAFETY 注释同既有风格）
  - [x] 1.4 `lib.rs` crate 文档追加 v0.103.0 一句说明（既有偏差表不动）；`Cargo.toml` description 追加 v0.103.0
  - [x] 1.5 测试 T19/T20：Mock 记录注入（含重复调用覆盖末次）/ 自定义 stub 不覆写走默认 no-op
  - 验证：`cargo test -p eneros-solver-core` 20/20（18 旧 + 2 新）；`cargo build -p eneros-solver-core --features highs-ffi` 编译通过；clippy 0 warning

- [x] **T2：新建 crate 骨架 + candidate.rs — 候选解与投影**
  - [x] 2.1 `crates/ai/solver-warm/Cargo.toml`：`eneros-solver-warm`，workspace 继承，依赖 `eneros-solver-core`/`eneros-solver-milp`（path 同级相对引用）；`[features] onnx-ffi = []`（默认关）
  - [x] 2.2 `src/lib.rs`：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc` + 模块声明 + 重导出 + crate 文档（版本定位 + 核心类型 + D1~D12 偏差表 + no_std 合规声明，风格对齐 solver-milp）
  - [x] 2.3 `src/candidate.rs`：`CandidateSolution`（3 字段 pub，Debug/Clone）+ `project(&mut self, problem)`（连续列 clamp 到 `[lower_bounds[i], upper_bounds[i]]`；整数列/信度不动，D9）+ `to_solution(&self, problem) -> Vec<f64>`（按 `var_types` 逐列取 continuous/integer 序合并，len == num_vars）
  - [x] 2.4 测试 TC1~TC8（8 个，见 spec 测试规划表）
  - 验证：`cargo test -p eneros-solver-warm candidate` 全过 ✅

- [x] **T3：heuristic_net.rs — 推理 seam 与编解码**
  - [x] 3.1 `InferEngine` trait（infer/input_dim/output_dim）+ `MockEngine`（预设输出 + 可注入 Err）+ `WarmError`（3 变体，Debug/Clone/PartialEq）
  - [x] 3.2 `src/ffi.rs`：`#[cfg(feature = "onnx-ffi")]` extern 声明 `ort_create_session`/`ort_run_session`/`ort_free_session`（蓝图 §4.5 签名）；`OnnxEngine`（NonNull session + Drop 调 `ort_free_session`，SAFETY 注释）
  - [x] 3.3 `HeuristicNet<E: InferEngine>`：`new(engine)` + `encode_input(ctx)`（负荷 → 电价 → 末条历史 schedule 逐机组 generation 顺序拼接，零填充/截断至 input_dim）+ `decode_output(output, problem)`（连续 clamp 列界；Binary/Integer >0.5→1；蓝图置信度公式 `c *= 1.0−|v−0.5|·2` 累积后 `/= num_vars`）
  - [x] 3.4 `impl WarmStartProvider for HeuristicNet<E>`：encode → f32 转换 infer → f64 回转 decode；infer 返回维度 ≠ output_dim → `Err(InvalidDim)`
  - [x] 3.5 测试 TH9~TH20（12 个，见 spec 测试规划表）
  - 验证：`cargo test -p eneros-solver-warm heuristic_net` 全过 ✅；`cargo build -p eneros-solver-warm --features onnx-ffi` 编译通过 ✅

- [x] **T4：warm_start.rs — 编排器与降级链**
  - [x] 4.1 `SolveContext`（3 字段 pub，history 复用 `eneros_solver_milp::DayAheadPlan`，Debug/Clone/Default）
  - [x] 4.2 `WarmStartProvider` trait（无 Send+Sync，D5）
  - [x] 4.3 `WarmStarter { confidence_threshold, warm_used_count, warm_rejected_count, cold_fallback_count }` + `new(threshold)` 计数器清零
  - [x] 4.4 `plan_warm(provider, problem, ctx, solver)`：generate Err → cold_fallback_count+=1 返回 None；confidence < threshold → warm_rejected_count+=1 返回 None；否则 project + to_solution → `solver.set_warm_start(&sol)` → warm_used_count+=1 返回 Some(sol)（set_warm_start Err 视同 fallback：cold_fallback_count+=1 返回 None）
  - [x] 4.5 测试 TW21~TW30（10 个，见 spec 测试规划表）
  - 验证：`cargo test -p eneros-solver-warm` 30/30 全过 ✅

- [x] **T5：workspace 接线 + 配置 + 设计文档**
  - [x] 5.1 根 `Cargo.toml` members 追加 `"crates/ai/solver-warm"`（字母序插入 ai 段，solver-milp 之后）
  - [x] 5.2 `configs/warm-start.toml`：`[warm_start]` model_path / confidence_threshold = 0.5 / input_dim = 72 / output_dim = 96 + 中文注释 ≥6 点（NN 选型 §5.1 / 加速 ≥30% §7.2 / 冷启动回退 §4.4 / 投影 D9 / ONNX CPU 非 GPU §6.6 / 内存预算 ≤128MB §5.6 / 模型签名 v0.116.0 预留）
  - [x] 5.3 `docs/ai/warm-start-design.md`：12 章节 + ≥2 Mermaid（蓝图 §4.3 热启动流程图重绘 + plan_warm 判定/降级时序图）+ D1~D12 偏差表 + 性能口径声明（加速比硬件验证项 D11）
  - 验证：`cargo test -p eneros-solver-warm` 30 全过；`cargo metadata` 解析成功

- [x] **T6：版本同步 0.103.0 + 全量构建验证**
  - [x] 6.1 根 `Cargo.toml` version = "0.103.0"；`Makefile` VERSION；`ci.yml` 注释；`gate.rs` 注释串尾追加 v0.103.0 类型清单（2 处：CandidateSolution/InferEngine/MockEngine/OnnxEngine/HeuristicNet/SolveContext/WarmStartProvider/WarmStarter/WarmError）
  - [x] 6.2 §2.4.2 构建校验：C6 metadata / C7 solver-warm 30 + solver-core 20 零回归 + 全 workspace 回归 / C8 aarch64 交叉编译（solver-warm + solver-core）/ C9 fmt / C10 clippy -D warnings / C11 cargo deny
  - 验证：C6~C11 全绿

- [x] **T7：checklist 逐项核验收工**
  - [x] 7.1 `checklist.md` 逐项核验勾选 + 验收记录
  - 验证：checklist 全勾，收工

# Task Dependencies

- T1、T2 独立（可并行：T2 仅用 LpProblem，不触 solver.rs/mock.rs）
- T3 depends on T2（HeuristicNet 消费 CandidateSolution；WarmStartProvider 引用 SolveContext 由 T4 定义 → 实际 T3.4 依赖 T4.1，实施时 T3 内先内联最小 SolveContext 引用或按 T2→T4 定义前置再 T3.4；为简化：T3 与 T4 顺序执行，T3.4 与 T4 同批）
- T4 depends on T1 + T3（plan_warm 消费 set_warm_start + provider）
- T5 depends on T4
- T6 depends on T5
- T7 depends on T6

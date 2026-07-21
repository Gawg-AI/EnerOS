# Tasks — v0.104.0 多目标 Pareto 优化

> Spec：`spec.md`（develop-v10400-pareto）。T1→T2→T3 顺序（T2/T3 均消费 T1 类型；T2 与 T3 可并行）；T4 依赖 T2+T3；T5/T6 顺序收尾。

- [x] **T1：新建 crate 骨架 + pareto_front.rs — 数据结构与核心算法**
  - [x] 1.1 `crates/ai/solver-pareto/Cargo.toml`：`eneros-solver-pareto`，workspace 继承，依赖 `eneros-solver-core`（path 同级相对引用）；零第三方依赖
  - [x] 1.2 `src/lib.rs`：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc` + 模块声明 + 重导出 + crate 文档（版本定位 + 核心类型 + D1~D12 偏差表 + no_std 合规声明，风格对齐 solver-warm）
  - [x] 1.3 `src/pareto_front.rs`：`MultiObjectiveProblem`/`Objective`/`OptDirection`/`VariableSpec`/`ParetoSolution`/`ParetoFront`（全字段 pub，派生按接口契约）+ `ParetoSolver` trait（无 Send+Sync，D5）+ `dominates`（统一最小化口径，D7）+ `non_dominated` + `select_by_weight`（权重归一化：负值 clamp 0、全零→均匀；空 front 返回 None；`f64::total_cmp` 取最小，D8）+ `is_empty`/`len`
  - [x] 1.4 测试 PF1~PF10（10 个，见 spec 测试规划表）
  - 验证：`cargo test -p eneros-solver-pareto pareto_front` 全过

- [x] **T2：nsga2.rs — PRNG 与 NSGA-II 求解器**
  - [x] 2.1 内置确定性 xorshift64* PRNG（私有 struct，~20 行，`next_u64` + `gen_range_f64(lower, upper)`；零 unsafe，D4）
  - [x] 2.2 `Nsga2Solver { pub crossover_rate, pub mutation_rate, pub seed }` + `new()`（0.9/0.1/固定默认 seed）+ `with_seed(seed)`
  - [x] 2.3 `init_population`（界内均匀随机）+ `evaluate`（蓝图口径 cost=Σv / carbon=Σ0.5v / lifespan 等 Maximize 取负归一，D7）+ `non_dominated_sort` + `crowding_distance`（≤2 边界 MAX；total_cmp 排序，D8）
  - [x] 2.4 `impl ParetoSolver`：`solve` = init → evaluate → gen × {非支配排序 + 拥挤度 + 锦标赛选择（rank 优先平手比 crowding）+ 均匀交叉 + 均匀变异补满 pop_size，D9} → 末次排序输出 rank==0 前沿；空 variables/objectives/pop_size==0 → `Err(SolverError::InvalidProblem(_))`
  - [x] 2.5 测试 NS11~NS22（12 个，见 spec 测试规划表；性能项 50×100 < 10s 用 `std::time::Instant`，仅 cfg(test)，D12）
  - 验证：`cargo test -p eneros-solver-pareto nsga2` 全过

- [x] **T3：decision.rs — 决策者选择**
  - [x] 3.1 `DecisionMaker { pub preferences: Vec<f64> }` + `new(preferences)` + `choose(&front) -> Option<&ParetoSolution>`（归一化后委托 `select_by_weight`；偏好长度 < 目标数缺省补 0）
  - [x] 3.2 测试 DM23~DM30（8 个，见 spec 测试规划表）
  - 验证：`cargo test -p eneros-solver-pareto` 30/30 全过

- [x] **T4：workspace 接线 + 配置 + 设计文档**
  - [x] 4.1 根 `Cargo.toml` members 追加 `"crates/ai/solver-pareto"`（字母序插入 ai 段 solver-milp 与 solver-warm 之间）
  - [x] 4.2 `configs/solver-pareto.toml`：`[pareto]` pop_size = 100 / gen = 50 / crossover_rate = 0.9 / mutation_rate = 0.1 / seed / 三目标权重 + 中文注释 ≥6 点（NSGA-II 选型 §5.1 / 性能 <10s §6.3 / 确定性 seed D4 / 方向归一 D7 / LP 兜底编排层 D10 / 内存预算 ≤128MB §5.6 / GPU 不适用 §6.6）
  - [x] 4.3 `docs/ai/pareto-design.md`：12 章节 + ≥2 Mermaid（蓝图 §4.3 NSGA-II 流程图重绘 + solve 一代进化时序图）+ D1~D12 偏差表 + 性能口径声明（D12）
  - 验证：`cargo metadata` 解析成功；`cargo test -p eneros-solver-pareto` 30 全过

- [x] **T5：版本同步 0.104.0 + 全量构建验证**
  - [x] 5.1 根 `Cargo.toml` version = "0.104.0"；`Makefile` VERSION；`ci.yml` 注释；`gate.rs` 注释串尾追加 v0.104.0 类型清单（2 处：MultiObjectiveProblem/Objective/OptDirection/VariableSpec/ParetoSolution/ParetoFront/ParetoSolver/Nsga2Solver/DecisionMaker）
  - [x] 5.2 §2.4.2 构建校验：C6 metadata / C7 solver-pareto 30 零回归 + 全 workspace 回归 / C8 aarch64 交叉编译（solver-pareto）/ C9 fmt / C10 clippy -D warnings / C11 cargo deny
  - 验证：C6~C11 全绿

- [x] **T6：checklist 逐项核验收工**
  - [x] 6.1 `checklist.md` 逐项核验勾选 + 验收记录
  - 验证：checklist 全勾，收工

# Task Dependencies

- T1 先行（T2/T3 均消费 pareto_front 类型）
- T2、T3 互相独立（可并行）
- T4 depends on T2 + T3
- T5 depends on T4
- T6 depends on T5

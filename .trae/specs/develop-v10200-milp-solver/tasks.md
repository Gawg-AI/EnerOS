# Tasks — v0.102.0 MILP 求解器集成

> Spec：`spec.md`（develop-v10200-milp-solver）。T1 与 T2 无依赖可并行；T3 依赖 T2；T4 依赖 T1+T3；T5/T6 顺序收尾。

- [x] **T1：solver-core MILP FFI 增量（feature-gated）**
  - [ ] 1.1 `ffi.rs` 追加 `Highs_passMip` extern 声明：签名同 `Highs_passLp` + 尾部 `integrality: *const c_int`（既有声明零改动，注释注明 v0.102.0）
  - [ ] 1.2 `highs.rs` `solve()`：提取 `problem.var_types` 是否含非 Continuous；全 Continuous → 原 `Highs_passLp` 路径不变；否则构建 `Vec<c_int>` integrality（Continuous→0 / Integer→1 / Binary→1）改调 `Highs_passMip`（SAFETY 注释同既有风格）
  - [ ] 1.3 `lib.rs` crate 文档追加 v0.102.0 一句说明（既有偏差表不动）；`Cargo.toml` description 追加 v0.102.0
  - 验证：`cargo test -p eneros-solver-core` 18/18 零回归；`cargo build -p eneros-solver-core --features highs-ffi` 编译通过；`cargo clippy -p eneros-solver-core --all-targets -- -D warnings` 通过

- [x] **T2：新建 crate 骨架 + uc_model.rs — UC MILP 模型构建**
  - [ ] 2.1 `crates/ai/solver-milp/Cargo.toml`：`eneros-solver-milp`，workspace 继承，依赖 `eneros-solver-core`/`eneros-solver-model`（path 同级相对引用），无 `[features]`
  - [ ] 2.2 `src/lib.rs`：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc` + 模块声明 + 重导出 + crate 文档（版本定位 + 核心类型 + D1~D12 偏差表 + no_std 合规声明，风格对齐 energy-lp-model）
  - [ ] 2.3 `src/uc_model.rs`：`UcUnit`（9 字段，蓝图 §4.1）/ `UnitCommitment` + `new`/`num_vars`/`var_index`
  - [ ] 2.4 `build_model(load, price) -> Result<LpProblem, SolverError>`：长度校验（D8）→ DSL 建变量（P continuous [0,p_max]，U/V/W `.binary()`）→ 目标（P→price[t]，V→start_cost，D6 修正）→ 6 类约束（平衡 Eq / pmax Le / pmin Ge / 爬坡 ±ramp·interval_min / 启停逻辑含 t=0 init_status 常数项 / 最小启停窗口，min_up/min_down=0 按 1）→ `compile()` Minimize
  - [ ] 2.5 `build_model_relaxed`：同一构建路径，跳过最小启停 2nt 行（内部布尔参数复用，不复制代码）
  - [x] 2.6 测试 TU1~TU15（15 个，见 spec 测试规划表；5×24 规模断言行数 t+5nt+2n(t−1)=854、relaxed=614）
  - 验证：`cargo test -p eneros-solver-milp uc_model` 全过

- [x] **T3：day_ahead.rs — 日前计划与三级降级链**
  - [ ] 3.1 `UnitSchedule` / `DayAheadPlan`（蓝图 §4.1 字段，Debug/Clone）
  - [ ] 3.2 `DayAheadScheduler { time_limit_s, mip_rel_gap, relax_count, lp_fallback_count }` + `new(time_limit_s, mip_rel_gap)`
  - [ ] 3.3 `relax_lp(model) -> LpProblem`：var_types 全转 Continuous；原 Binary 上界保持 1.0（其余字段 Clone 透传）
  - [ ] 3.4 `plan()`：set_param 注入 time_limit/mip_rel_gap（D10）→ build_model → solve → 状态分派：Optimal/Suboptimal/Timeout 接受；Infeasible/Unbounded/Error → relaxed 重建重解（relax_count+=1）→ 仍失败 → relax_lp 重解（lp_fallback_count+=1）→ 三级全失败返回空 schedule + 末级状态（D9）
  - [ ] 3.5 结果解析：`var_index` 定位 P/U；U>0.5→commitments；P→generation；total_cost=objective_value；unit_id 按 units 顺序
  - [ ] 3.6 测试 TD16~TD31（16 个，见 spec 测试规划表；含 5×24 e2e、2×3 手工解映射、三级降级分支、记录型 Solver stub 验证参数注入、10×24 构建性能 <1s）
  - 验证：`cargo test -p eneros-solver-milp day_ahead` 全过

- [x] **T4：workspace 接线 + 配置 + 设计文档**
  - [ ] 4.1 根 `Cargo.toml` members 追加 `"crates/ai/solver-milp"`（字母序插入 ai 段）
  - [ ] 4.2 `configs/milp-solver.toml`：`[milp]` time_limit_s = 30.0 / mip_rel_gap = 0.01 + 中文注释 ≥6 点（HiGHS 选型 §5.1 / 超时返回当前最优 §4.4 / 松弛链 D9 / 内存预算 ≤128MB §5.6 / 性能 10 机组 <5s §7.2 / 整数规模 n·t·4 §5.4 难点）
  - [ ] 4.3 `docs/ai/milp-solver-design.md`：12 章节（版本目标/前置依赖/交付物清单/详细设计/技术交底/测试计划/验收标准/风险/多角度要求/接口契约/偏差声明/附录）+ ≥2 Mermaid（蓝图 §4.3 UC 建模流程重绘 + 三级降级链时序图）+ D1~D12 偏差表 + 与 v0.66.0 energy-lp-model 的 LP/MILP 层次区分声明
  - 验证：`cargo test -p eneros-solver-milp` 31 全过；`cargo metadata` 解析成功

- [x] **T5：版本同步 0.102.0 + 全量构建验证**
  - [ ] 5.1 根 `Cargo.toml` version = "0.102.0"；`Makefile` VERSION；`ci.yml` 注释；`gate.rs` 注释串尾追加 v0.102.0 类型清单（2 处：UcUnit/UnitCommitment/UnitSchedule/DayAheadPlan/DayAheadScheduler）
  - [ ] 5.2 §2.4.2 构建校验：C6 metadata / C7 solver-milp 31 + solver-core 18 零回归 + 全 workspace 回归 / C8 aarch64 交叉编译（solver-milp + solver-core）/ C9 fmt / C10 clippy -D warnings / C11 cargo deny
  - 验证：C6~C11 全绿

- [x] **T6：checklist 逐项核验收工**
  - [x] 6.1 `checklist.md` 逐项核验勾选 + 验收记录（105/105 全勾，未通过项无）
  - 验证：checklist 全勾，收工

# Task Dependencies

- T1、T2 独立（可并行：T2 仅用 DSL + LpProblem，不触 FFI）
- T3 depends on T2（plan 消费 build_model/relax_lp 模型）
- T4 depends on T1 + T3（文档偏差表需双方落地）
- T5 depends on T4
- T6 depends on T5

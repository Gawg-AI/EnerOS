# Checklist — v0.102.0 MILP 求解器集成

> Spec：`spec.md`（develop-v10200-milp-solver）。逐项核验，未通过禁止收工。

## A. 目录结构校验（§2.4.1，C1~C5）

- [x] C1: 新 crate 位于 `crates/ai/solver-milp/`，未直接放根目录（D1）
- [x] C2: 根 `Cargo.toml` `members` 已追加 `"crates/ai/solver-milp"`，workspace 可解析
- [x] C3: solver-milp `Cargo.toml` path 引用为 `../solver-core` / `../solver-model` 相对路径；依赖仅这两个，零新增第三方依赖
- [x] C4: 新文档 `milp-solver-design.md` 位于 `docs/ai/`，未平面化放 `docs/` 根（D2）
- [x] C5: 仓库根目录无除 `ci/` 外的新 crate 文件夹

## B. 构建校验（§2.4.2，C6~C11）

- [x] C6: `cargo metadata --format-version 1` 成功
- [x] C7: `cargo test -p eneros-solver-milp` 31/31 通过；`cargo test -p eneros-solver-core` 18/18 零回归；全 workspace 回归全绿（含 energy-lp-model 22 / solver-model / dual-brain / fast-path 等依赖方）
- [x] C8: `cargo build -p eneros-solver-milp --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过；solver-core 同命令通过
- [x] C9: `cargo fmt --all -- --check` 通过
- [x] C10: `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` 0 warning
- [x] C11: `cargo deny check advisories licenses bans sources` 通过（零新增第三方依赖，SBOM 不变）

## C. 文档与规范校验（§2.4.3，C12~C15）

- [x] C12: 新文档在 `docs/ai/` 下，不在 `docs/` 根
- [x] C13: `git status` 无 `target/`、`*.elf`、`*.bin`、`*.dtb`、IDE 缓存被追踪
- [x] C14: 无新文件类型需 `.gitignore` 覆盖
- [x] C15: 新代码无 `use std::*` / `panic!` / `todo!` / `unimplemented!` / `async`；solver-milp 零 `unsafe`（FFI 增量在 solver-core feature-gated 路径，默认构建不编译）；测试模块内 std（Instant 性能测量）位于 `#[cfg(test)]` 下允许

## D. solver-core FFI 增量（C16~C24，feature-gated，D5）

- [x] C16: `ffi.rs` 追加 `Highs_passMip` 声明，签名 = `Highs_passLp` 参数 + 尾部 `integrality: *const c_int`，返回 `c_int`
- [x] C17: 既有 `ffi.rs` 全部声明（Highs_create/destroy/passLp/run/getModelStatus/getObjectiveValue/getSolution/setStringOptionValue/setDoubleOptionValue）零改动
- [x] C18: `highs.rs` `solve()`：全 Continuous var_types → 仍走 `Highs_passLp`（LP 路径零变化）
- [x] C19: `highs.rs` `solve()`：任一非 Continuous → 构建 integrality 数组（Continuous→0 / Integer→1 / Binary→1）调用 `Highs_passMip`
- [x] C20: integrality 数组长度 == num_col，生命周期覆盖 FFI 调用（SAFETY 注释）
- [x] C21: FFI 增量全部位于 `#[cfg(feature = "highs-ffi")]` 门控内，默认构建（Mock）不编译
- [x] C22: `cargo build -p eneros-solver-core --features highs-ffi` 编译通过
- [x] C23: solver-core `lib.rs` 文档追加 v0.102.0 说明且既有 D1~D12 偏差表零改动
- [x] C24: solver-core 18 测试零回归（MockSolver 路径不受 FFI 增量影响）

## E. uc_model.rs UC MILP 建模（C25~C45）

- [x] C25: `UcUnit` 9 字段全 pub（id/p_min/p_max/ramp_up/ramp_down/start_cost/min_up/min_down/init_status），derive Debug/Clone
- [x] C26: `UnitCommitment` 3 字段全 pub（units/periods/interval_min），derive Debug/Clone
- [x] C27: `num_vars()` == units.len() · periods · 4
- [x] C28: `var_index(i, t, k)` == (i · periods + t) · 4 + k
- [x] C29: build_model 变量数 == n·t·4（5×24 → 480）
- [x] C30: var_types：k=0 P 为 Continuous；k=1,2,3 U/V/W 为 Binary
- [x] C31: 变量边界：P ∈ [0, p_max]；U/V/W ∈ [0, 1]
- [x] C32: 目标系数：P[i,t] == price[t]（蓝图语义）
- [x] C33: 目标系数：V[i,t] == start_cost_i（**D6 蓝图 Bug 修正**，挂 V=base+2 非 U=base+1）；U/W 系数 == 0
- [x] C34: sense == Minimize
- [x] C35: 功率平衡 t 行 Eq：Σ_i P[i,t] == load[t]
- [x] C36: 出力联动 2nt 行：P[i,t] − p_max_i·U[i,t] ≤ 0 且 P[i,t] − p_min_i·U[i,t] ≥ 0
- [x] C37: 爬坡 2n(t−1) 行：P[i,t] − P[i,t−1] ≤ ramp_up_i·interval_min 且反向 ≤ ramp_down_i·interval_min（D12 单位换算）
- [x] C38: 启停逻辑 nt 行：t=0 为 V−W−U == −init_status（常数项进 RHS）；t≥1 为 V−W−U+U_prev == 0
- [x] C39: 最小运行 nt 行：窗口 τ ∈ [max(0, t+1−min_up), t]，ΣV[i,τ] − U[i,t] ≤ 0
- [x] C40: 最小停机 nt 行：窗口 τ ∈ [max(0, t+1−min_down), t]，ΣW[i,τ] + U[i,t] ≤ 1
- [x] C41: 总行数 == t + 5nt + 2n(t−1)（5×24 → 854）；CSR row_start.len() == 行数 + 1（855）；col_index/values 等长
- [x] C42: load/price 长度 ≠ periods → `Err(SolverError::InvalidProblem)`（D8，无 panic）
- [x] C43: min_up/min_down == 0 按 1 处理（窗口=当期），无空表达式行
- [x] C44: build_model_relaxed 跳过最小启停 2nt 行，行数 == t + 3nt + 2n(t−1)（5×24 → 614），其余约束一致
- [x] C45: build_model 与 build_model_relaxed 复用同一构建路径（布尔开关），无代码复制

## F. day_ahead.rs 日前计划与降级链（C46~C65）

- [x] C46: `UnitSchedule`（unit_id/commitments/generation）/ `DayAheadPlan`（schedule/total_cost/solve_status）字段全 pub，derive Debug/Clone
- [x] C47: `DayAheadScheduler` 字段全 pub：time_limit_s / mip_rel_gap / relax_count / lp_fallback_count；`new(time_limit_s, mip_rel_gap)` 计数器清零
- [x] C48: `relax_lp`：全部 var_types → Continuous；原 Binary 变量上界保持 1.0；其余字段 Clone 透传
- [x] C49: `plan()` 求解前经 `Solver::set_param` 注入 `"time_limit"` 与 `"mip_rel_gap"`（D10）
- [x] C50: Optimal/Suboptimal/Timeout → 直接解析接受，计数器保持 0（D9 超时返回当前最优可行解）
- [x] C51: Infeasible/Unbounded/Error → build_model_relaxed 重建重解，relax_count += 1
- [x] C52: relaxed 仍失败 → relax_lp 重解，lp_fallback_count += 1
- [x] C53: 三级全失败 → 返回 Ok（空 schedule + total_cost 0.0 + 末级 solve_status），非 panic 非静默
- [x] C54: 解析：U[i,t] > 0.5 → commitments true；P[i,t] → generation
- [x] C55: total_cost == objective_value；solve_status 透传末级求解状态
- [x] C56: schedule 顺序与 uc.units 一致，unit_id 取自 UcUnit.id
- [x] C57: 解向量经 `var_index` 定位，无越界 panic（安全访问）
- [x] C58: e2e 5 机组 × 24 周期（蓝图 §6.2）：MockSolver 480 维最优解 → schedule 5 × 24
- [x] C59: 2×3 手工解映射：U=1.0/P=50.0 → commitments[0]=true、generation[0]=50.0
- [x] C60: 计数器语义：仅真实触发降级才递增（Optimal 路径双 0；relax 路径 relax=1/lp=0；全链各 1）
- [x] C61: 注入 `&mut dyn Solver` seam（MockSolver 与记录型 stub 均可驱动）
- [x] C62: now_ms 透传 solver.solve（D1 时钟注入惯例）
- [x] C63: 10 机组 × 24 周期模型构建耗时 < 1s（Instant，cfg(test) 内 std 允许；真实求解 <5s 留待硬件，D11）
- [x] C64: 记录型 stub 验证 set_param 以 "time_limit"/"mip_rel_gap" 键与配置值调用
- [x] C65: crate 无 `unsafe`、无新增第三方依赖、无 `[features]`

## G. 配置文件（C66~C71）

- [x] C66: `configs/milp-solver.toml` 存在，`[milp]` 段含 time_limit_s / mip_rel_gap
- [x] C67: 中文注释 ≥6 点（HiGHS 选型 §5.1 / 超时返回当前最优 §4.4 / 松弛链 D9 / 内存预算 ≤128MB §5.6 / 性能 10 机组 <5s §7.2 / 整数规模 n·t·4 §5.4）
- [x] C68: time_limit_s 默认 30.0 与蓝图 §4.5 一致
- [x] C69: 配置键名与设计文档接口契约一致
- [x] C70: 代码无硬编码默认参数（time_limit_s/mip_rel_gap 由 new 注入，生产配置化）
- [x] C71: 内存预算声明：Solver 分区 ≤128MB（蓝图 §5.6）在注释体现

## H. 设计文档（C72~C79）

- [x] C72: `docs/ai/milp-solver-design.md` 存在且 12 章节齐全（版本目标/前置依赖/交付物清单/详细设计/技术交底/测试计划/验收标准/风险/多角度要求/接口契约/偏差声明/附录）
- [x] C73: Mermaid ≥2（蓝图 §4.3 UC 建模流程重绘 + 三级降级链时序图）
- [x] C74: D1~D12 偏差表与 spec.md 一致（含 D6 蓝图 Bug 修正、D7 完整约束集、D9 状态驱动降级链）
- [x] C75: 接口契约章节与实现签名一致（build_model 返回 Result / relax_lp / plan 参数 / var_index 布局）
- [x] C76: 技术交底含选型对比表（HiGHS/CBC/Gurobi/LP 松弛，蓝图 §5.1）
- [x] C77: 与 v0.66.0 energy-lp-model 层次区分声明（储能连续 LP 调度 vs 机组启停 MILP 日前计划）
- [x] C78: 性能章节声明口径：构建 <1s 本版实测；10 机组求解 <5s 为 HiGHS 硬件集成验证项（D11）
- [x] C79: 安全章节覆盖 FFI 内存安全（NonNull+Drop 复用 solver-core）与降级链可观测（计数器）

## I. 版本同步（C80~C85）

- [x] C80: 根 `Cargo.toml` `[workspace.package] version = "0.102.0"` 且 members 含 solver-milp
- [x] C81: `Makefile` 版本注释 + VERSION 变量同步 0.102.0
- [x] C82: `.github/workflows/ci.yml` 版本注释同步 0.102.0
- [x] C83: `ci/src/gate.rs` 注释串尾追加 v0.102.0 类型清单（UcUnit/UnitCommitment/UnitSchedule/DayAheadPlan/DayAheadScheduler），2 处
- [x] C84: eneros-solver-milp `Cargo.toml` description 含 v0.102.0
- [x] C85: eneros-solver-core `Cargo.toml` description 追加 v0.102.0

## J. 测试覆盖（C86~C95）

- [x] C86: uc_model.rs 内嵌 15 测试（TU1~TU15）通过
- [x] C87: day_ahead.rs 内嵌 16 测试（TD16~TD31）通过
- [x] C88: 新增测试总计 31 个，`cargo test -p eneros-solver-milp` 31/31 全过
- [x] C89: 约束计数断言覆盖 6 类约束（TU7~TU12）且与 C41 公式一致
- [x] C90: 降级链三分支覆盖（TD21 relax / TD22 LP / TD31 全失败）+ 计数器真值（TD25/TD28）
- [x] C91: 蓝图 Bug 修正断言覆盖（TU5：V 系数 == start_cost，U 系数 == 0）
- [x] C92: 参数注入覆盖（TD30 记录型 stub）
- [x] C93: 性能测试存在且通过（TD29 构建 <1s）
- [x] C94: solver-core 18 测试零回归；energy-lp-model 22 零回归；solver-model 零回归
- [x] C95: 全 workspace 回归全绿（agents/protocols/drivers/kernel/hal/runtime/security 各 crate test result: ok）

## K. 蓝图对齐与验收（C96~C105）

- [x] C96: v0.102.0 交付物全覆盖：solver-milp crate（uc_model/day_ahead）/ UnitCommitment / DayAheadPlan / HiGHS MILP FFI（蓝图 §3，D1/D5 偏差落地）
- [x] C97: 日前调度计划生成功能可用（蓝图 §7.1：e2e 5×24 plan 输出）
- [x] C98: UC 建模完整（蓝图 §9 功能：启停 V/U/W + 最小时间 + 爬坡 + 平衡，D7）
- [x] C99: 超时降级保证（蓝图 §9 可靠：Timeout/Suboptimal 接受 + 三级降级链 + 计数器可观测）
- [x] C100: 参数配置化（蓝图 §9 可维护：time_limit_s/mip_rel_gap toml 配置 + new 注入）
- [x] C101: FFI 内存安全（蓝图 §7.3：复用 solver-core NonNull+Drop RAII，增量路径 SAFETY 注释）
- [x] C102: 性能口径声明（蓝图 §7.2：构建 <1s 实测；10 机组 <5s 硬件验证项，D11）
- [x] C103: 上游 v0.64.0/v0.65.0/v0.66.0 复用关系在文档声明（§5.5 交互）
- [x] C104: 下游 v0.103.0 热启动 / v0.104.0 Pareto 解锁声明（§5.5 交互）
- [x] C105: HiGHS 开源无出口限制国产化声明（蓝图 §5.6）在文档体现

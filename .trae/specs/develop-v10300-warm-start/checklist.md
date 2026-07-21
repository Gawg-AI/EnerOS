# Checklist — v0.103.0 Solver 神经部分热启动

> Spec：`spec.md`（develop-v10300-warm-start）。逐项核验，未通过禁止收工。

## A. 目录结构校验（§2.4.1，C1~C5）

- [x] C1: 新 crate 位于 `crates/ai/solver-warm/`，未直接放根目录（D1）
- [x] C2: 根 `Cargo.toml` `members` 已追加 `"crates/ai/solver-warm"`，workspace 可解析
- [x] C3: solver-warm `Cargo.toml` path 引用为 `../solver-core` / `../solver-milp` 相对路径；依赖仅这两个，零新增第三方依赖
- [x] C4: 新文档 `warm-start-design.md` 位于 `docs/ai/`，未平面化放 `docs/` 根（D2）
- [x] C5: 仓库根目录无除 `ci/` 外的新 crate 文件夹

## B. 构建校验（§2.4.2，C6~C11）

- [x] C6: `cargo metadata --format-version 1` 成功
- [x] C7: `cargo test -p eneros-solver-warm` 30/30 通过；`cargo test -p eneros-solver-core` 20/20（18 旧 + 2 新）；全 workspace 回归全绿（含 solver-milp 31 / energy-lp-model / solver-model 等依赖方）
- [x] C8: `cargo build -p eneros-solver-warm --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过；solver-core 同命令通过
- [x] C9: `cargo fmt --all -- --check` 通过
- [x] C10: `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` 0 warning
- [x] C11: `cargo deny check advisories licenses bans sources` 通过（零新增第三方依赖，SBOM 不变）

## C. 文档与规范校验（§2.4.3，C12~C15）

- [x] C12: 新文档在 `docs/ai/` 下，不在 `docs/` 根
- [x] C13: `git status` 无 `target/`、`*.elf`、`*.bin`、`*.dtb`、IDE 缓存被追踪
- [x] C14: 无新文件类型需 `.gitignore` 覆盖
- [x] C15: 新代码无 `use std::*` / `panic!` / `todo!` / `unimplemented!` / `async`；solver-warm 默认构建零 `unsafe`（unsafe 仅在 `onnx-ffi` feature-gated 路径）

## D. solver-core 注入增量（C16~C24，非 BREAKING）

- [x] C16: `solver.rs` `Solver` trait 追加 `set_warm_start` 为**默认方法**（`{ Ok(()) }`），既有 5 个方法签名零改动
- [x] C17: `mock.rs` `MockSolver` 追加 `pub warm_start: Option<Vec<f64>>`；`new`/`with_result` 初始 None；覆写记录末次注入
- [x] C18: `ffi.rs` 追加 `Highs_setSolution(highs, col_value: *const f64) -> c_int`，既有全部声明（含 v0.102.0 `Highs_passMip`）零改动
- [x] C19: `highs.rs` 覆写 `set_warm_start` 调 `Highs_setSolution`，SAFETY 注释（指针有效、长度 == num_col、生命周期覆盖调用）
- [x] C20: FFI 增量全部位于 `#[cfg(feature = "highs-ffi")]` 门控内，默认构建（Mock）不编译
- [x] C21: `cargo build -p eneros-solver-core --features highs-ffi` 编译通过
- [x] C22: solver-core `lib.rs` 文档追加 v0.103.0 说明且既有 D1~D12 偏差表零改动
- [x] C23: solver-core 18 旧测试零回归（默认方法对既有行为无影响）
- [x] C24: solver-core `Cargo.toml` description 追加 v0.103.0

## E. candidate.rs 候选解与投影（C25~C33）

- [x] C25: `CandidateSolution` 3 字段全 pub（continuous/integer/confidence），derive Debug/Clone
- [x] C26: `project`：连续列 clamp 到 `[lower_bounds[i], upper_bounds[i]]`（按 problem 列序对齐 continuous 序）
- [x] C27: `project` 不修改 integer 与 confidence
- [x] C28: `to_solution` 按 `var_types` 逐列合并：Continuous 取 continuous 序、Binary/Integer 取 integer 序（i32→f64）
- [x] C29: `to_solution` 返回长度 == `problem.variables.len()`
- [x] C30: 全连续问题：to_solution == continuous 克隆
- [x] C31: 全整数问题：to_solution == integer 转 f64
- [x] C32: 混排 4 列 [C,B,C,I] 合并顺序正确（[1.0, 1.0, 2.0, 0.0] 用例）
- [x] C33: 空候选（continuous/integer 空）不 panic，产出全 0.0 或按界 clamp

## F. heuristic_net.rs 编解码（C34~C47）

- [x] C34: `InferEngine` trait 三方法（infer/input_dim/output_dim）；`WarmError` 3 变体（ModelLoadFailed/InferenceFailed(i32)/InvalidDim）derive Debug/Clone/PartialEq
- [x] C35: `MockEngine` 默认可用：预设输出 + 可注入 Err；零 unsafe
- [x] C36: `OnnxEngine` 位于 `#[cfg(feature = "onnx-ffi")]`；NonNull session + Drop 调 `ort_free_session`；`ort_*` extern 声明与蓝图 §4.5 签名一致
- [x] C37: `cargo build -p eneros-solver-warm --features onnx-ffi` 编译通过
- [x] C38: `encode_input` 拼接顺序：load_forecast → price_signal → 末条 history 逐机组 generation（机组序 × 周期序）
- [x] C39: `encode_input` 输出长度 == `engine.input_dim()`（不足零填充、超出截断）
- [x] C40: 空 history：仅 负荷+电价+零填充，不 panic
- [x] C41: `decode_output` 连续列 clamp 到列界（D9）
- [x] C42: `decode_output` Binary/Integer 列：>0.5 → 1，否则 0
- [x] C43: 蓝图置信度公式：整数列 `confidence *= 1.0 − |v−0.5|·2` 累积；最终 `/= num_vars`；0.5 值 → 整体归零
- [x] C44: `impl WarmStartProvider for HeuristicNet<E>`：encode → f32 转换 → infer → f64 回转 → decode
- [x] C45: infer 输出维度 ≠ `output_dim` → `Err(WarmError::InvalidDim)`
- [x] C46: infer Err 透传为 `Err(WarmError::InferenceFailed(_))`（generate 不吞没）
- [x] C47: 蓝图 72/96 维度硬编码消除：`input_dim/output_dim` 来自 engine（D7）

## G. warm_start.rs 编排与降级（C48~C60）

- [x] C48: `SolveContext` 3 字段全 pub（load_forecast/price_signal/history: Vec<DayAheadPlan>），derive Debug/Clone/Default
- [x] C49: `WarmStartProvider` trait 无 Send + Sync bound（D5）
- [x] C50: `WarmStarter` 4 字段全 pub（confidence_threshold/warm_used_count/warm_rejected_count/cold_fallback_count）；`new` 计数器清零
- [x] C51: generate Err → cold_fallback_count+=1，返回 None，不调 set_warm_start
- [x] C52: confidence < threshold → warm_rejected_count+=1，返回 None，不调 set_warm_start
- [x] C53: confidence == threshold → 视为通过（>= 判定），正常注入
- [x] C54: 通过路径：project → to_solution → `solver.set_warm_start(&sol)` → warm_used_count+=1 → 返回 Some(sol)
- [x] C55: set_warm_start 返回 Err → 视同冷启动回退：cold_fallback_count+=1，返回 None
- [x] C56: 返回 Some(sol) 时 sol == 投影合并后的完整解向量（与 MockSolver.warm_start 记录一致）
- [x] C57: 计数器真值：成功路径 used=1/rejected=0/fallback=0；低置信 rejected=1；Err fallback=1
- [x] C58: `plan_warm` 注入 `&dyn WarmStartProvider` + `&mut dyn Solver` 双 seam（MockEngine/MockSolver 可驱动）
- [x] C59: 空 SolveContext（Default）可用，不 panic
- [x] C60: crate 无 `unsafe`（默认构建）、无新增第三方依赖、`onnx-ffi` 默认关闭

## H. 配置文件（C61~C66）

- [x] C61: `configs/warm-start.toml` 存在，`[warm_start]` 段含 model_path / confidence_threshold / input_dim / output_dim
- [x] C62: 中文注释 ≥6 点（NN 选型 §5.1 / 加速 ≥30% §7.2 / 冷启动回退 §4.4 / 投影 D9 / ONNX CPU 非 GPU §6.6 / 内存预算 ≤128MB §5.6 / 模型签名 v0.116.0 预留）
- [x] C63: confidence_threshold 默认 0.5 与 spec D8 一致
- [x] C64: 配置键名与设计文档接口契约一致
- [x] C65: 代码无硬编码默认阈值（threshold 由 WarmStarter::new 注入）
- [x] C66: 内存预算声明：Solver 分区 ≤128MB（蓝图 §5.6）在注释体现

## I. 设计文档（C67~C74）

- [x] C67: `docs/ai/warm-start-design.md` 存在且 12 章节齐全（版本目标/前置依赖/交付物清单/详细设计/技术交底/测试计划/验收标准/风险/多角度要求/接口契约/偏差声明/附录）
- [x] C68: Mermaid ≥2（蓝图 §4.3 热启动流程图重绘 + plan_warm 判定/降级时序图）
- [x] C69: D1~D12 偏差表与 spec.md 一致（含 D5 去 Send+Sync、D6 onnx-ffi 门控、D11 加速比口径、D12 删 Gpu 变体）
- [x] C70: 接口契约章节与实现签名一致（project/to_solution/encode/decode/plan_warm/set_warm_start 默认方法）
- [x] C71: 技术交底含选型对比表（神经网络/贪心启发式/历史平均/无热启动，蓝图 §5.1）
- [x] C72: 性能章节声明口径：加速 ≥30% 为硬件集成验证项（真实 ONNX + HiGHS），本版断言注入路径正确性（D11）
- [x] C73: 安全章节覆盖 ONNX FFI 内存安全（NonNull+Drop）与模型签名 v0.116.0 预留（蓝图 §7.3）
- [x] C74: GPU 规则声明：Solver 不适用 GPU 规则，边缘推理 ONNX Runtime CPU，无 PyTorch/CUDA（蓝图 §6.6）

## J. 版本同步（C75~C80）

- [x] C75: 根 `Cargo.toml` `[workspace.package] version = "0.103.0"` 且 members 含 solver-warm
- [x] C76: `Makefile` 版本注释 + VERSION 变量同步 0.103.0
- [x] C77: `.github/workflows/ci.yml` 版本注释同步 0.103.0
- [x] C78: `ci/src/gate.rs` 注释串尾追加 v0.103.0 类型清单（CandidateSolution/InferEngine/MockEngine/OnnxEngine/HeuristicNet/SolveContext/WarmStartProvider/WarmStarter/WarmError），2 处
- [x] C79: eneros-solver-warm `Cargo.toml` description 含 v0.103.0
- [x] C80: eneros-solver-core `Cargo.toml` description 追加 v0.103.0

## K. 测试覆盖（C81~C90）

- [x] C81: candidate.rs 内嵌 8 测试（TC1~TC8）通过
- [x] C82: heuristic_net.rs 内嵌 12 测试（TH9~TH20）通过
- [x] C83: warm_start.rs 内嵌 10 测试（TW21~TW30）通过
- [x] C84: solver-warm 新增测试总计 30 个，`cargo test -p eneros-solver-warm` 30/30 全过
- [x] C85: 投影断言覆盖 clamp 上下界 + 整数不动 + 信度不动（TC2/TC3）
- [x] C86: 合并顺序断言覆盖混排 4 列（TC5）+ 全连续/全整数边界（TC6/TC7）
- [x] C87: 编码断言覆盖拼接顺序/零填充/截断/空 history（TH9~TH13）
- [x] C88: 蓝图置信度公式断言覆盖（TH16/TH17：0.5 归零 + 累积衰减）
- [x] C89: 降级链三分支覆盖（TW22 低置信 / TW23 推理失败 / TW24 set_warm_start Err）+ 计数器真值（TW25）
- [x] C90: solver-core T19/T20 通过（Mock 记录 + 默认 no-op）；全 workspace 回归全绿

## L. 蓝图对齐与验收（C91~C100）

- [x] C91: v0.103.0 交付物全覆盖：solver-warm crate（candidate/heuristic_net/warm_start）/ WarmStartProvider / CandidateSolution / ONNX FFI（feature-gated）/ 配置模型路径（蓝图 §3，D1/D6 偏差落地）
- [x] C92: 热启动候选解生成功能可用（蓝图 §7.1：generate → 投影 → 注入 e2e）
- [x] C93: 编码完整（蓝图 §5.2：负荷 + 电价 + 历史调度三特征源）
- [x] C94: 错误处理三路径（蓝图 §4.4：推理失败回退 / 不可行投影 / 低置信忽略）
- [x] C95: 加速口径声明（蓝图 §7.2：≥30% 硬件验证项；本版注入路径正确性，D11）
- [x] C96: 冷启动一致性（蓝图 §6.4：回退路径 solver 状态无污染，后续 solve 即冷启动）
- [x] C97: 上游 v0.64.0/v0.102.0/v0.59.0 复用关系在文档声明（§5.5 交互）
- [x] C98: 下游 v0.104.0 Pareto / v0.116.0 模型签名解锁声明（§5.5 交互 + §7.3）
- [x] C99: ONNX Runtime 开源支持国产 NPU 声明（蓝图 §5.6）在文档体现
- [x] C100: 无 BREAKING 声明验证：全 workspace 既有 crate 零改动编译通过（默认方法向后兼容）

---

## 验收记录（T7 逐项核验）

- **核验日期**：2026-07-19
- **核验人**：AI Agent（Trae IDE）
- **通过项数**：100 / 100（C1~C100 全勾，失败 0 项）

### 关键验证命令结果摘要

| 类别 | 命令 | 结果 |
|------|------|------|
| C6 | `cargo metadata --format-version 1` | exit 0，workspace 73 members 全部解析 |
| C7/C84/C90 | `cargo test -p eneros-solver-warm` | **30/30 通过**（TC1~TC8 + TH9~TH20 + TW21~TW30） |
| C7/C23/C90 | `cargo test -p eneros-solver-core` | **20/20 通过**（T1~T18 旧测试零回归 + T19/T20 新增） |
| C7/C100 | `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` | exit 0 全绿（含 solver-milp **31/31**、energy-lp-model、solver-model 等依赖方） |
| C8 | `cargo build -p eneros-solver-warm --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem`（solver-core 同命令） | 双双 Finished，交叉编译通过 |
| C9 | `cargo fmt --all -- --check` | exit 0 |
| C10 | `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` | exit 0，0 warning |
| C11 | `cargo deny check advisories licenses bans sources` | **advisories ok / bans ok / licenses ok / sources ok**（在线模式因网络无法拉取 advisory-db，以 `cargo deny --offline` 用本地缓存库通过；零新增第三方依赖，SBOM 不变） |
| C21 | `cargo build -p eneros-solver-core --features highs-ffi` | 编译通过 |
| C37 | `cargo build -p eneros-solver-warm --features onnx-ffi` | 编译通过 |
| C13 | `git status --short` | 无 `target/`、`*.elf`、`*.bin`、`.dtb`（编译产物）、IDE 缓存被追踪（`configs/qemu-virt.dts` 为设备树源文件，属应入仓类型） |
| C15 | grep `use std\|panic!\|todo!\|unimplemented!\|async\|unsafe`（solver-warm/src） | 无违规；`unsafe` 仅存在于 `ffi.rs`（`onnx-ffi` feature-gated），默认构建零 unsafe |

### 源码核验要点

- **C16~C24**：`solver.rs` 默认方法 `{ Ok(()) }` 在位、既有 5 方法零改动；`MockSolver.warm_start` pub 字段 + 覆写记录；`ffi.rs` `Highs_setSolution` 纯追加、`Highs_passMip` 等既有声明零改动；`highs.rs` 覆写带 SAFETY 注释（指针有效/长度==num_col/生命周期覆盖调用）；FFI 全部 `#[cfg(feature = "highs-ffi")]` 门控；lib.rs 文档追加 v0.103.0 且 D1~D12 偏差表零改动。
- **C25~C33**：`CandidateSolution` 3 字段全 pub（Debug/Clone）；project clamp 连续列不动整数/信度；to_solution 按 var_types 合并 len==num_vars；混排 [C,B,C,I]→[1.0,1.0,2.0,0.0]、全连续/全整数/空候选用例全部断言（代码内编号 TC4 为混排 4 列用例，checklist C86 括注 TC5 为笔误，实质断言覆盖无缺口）。
- **C34~C47**：`InferEngine` 3 方法、`WarmError` 3 变体（Debug/Clone/PartialEq）；`OnnxEngine` NonNull+Drop RAII、`ort_*` 签名与蓝图 §4.5 一致；编码顺序 负荷→电价→末条历史逐机组 generation、零填充/截断/空 history；decode clamp/二值化/蓝图置信度公式（累积后 `/= num_vars`）；InvalidDim/Err 透传；维度取自 engine 消除 72/96 硬编码。
- **C48~C60**：`SolveContext`/`WarmStarter` 字段全 pub；`WarmStartProvider` 无 Send+Sync；降级链三分支 + `==`阈值通过 + 计数器真值 + 双 seam 全部测试锚定。
- **C61~C66**：`configs/warm-start.toml` 4 键齐全 + 中文注释 7 点；阈值默认 0.5（D8），代码无硬编码。
- **C67~C74**：设计文档 12 章节 + 2 Mermaid；D1~D12 偏差表与 spec.md 逐字一致（grep 逐行比对 D1~D12 全 12 行）；接口契约与实现签名一致；选型对比表/性能口径 D11/ONNX FFI 内存安全 + v0.116.0 预留/GPU 规则声明齐全。
- **C75~C80**：根 `Cargo.toml` version="0.103.0" + members 含 solver-warm；`Makefile` Version 注释 + `VERSION := 0.103.0`；`ci.yml` 版本注释 v0.103.0；`gate.rs` 注释串尾 v0.103.0 九类型清单位于 run_clippy/run_tests **2 处**；两 crate description 均含 v0.103.0。
- **C91~C100**：蓝图 §3 交付物全覆盖、generate→投影→注入 e2e（TW21）、三特征源编码（TH10）、错误三路径（TW22/TW23/TW24）、加速 ≥30% 口径声明（文档 §12.1，D11）、冷启动一致性（文档 §6.2）、上游 v0.64.0/v0.102.0/v0.59.0 与下游 v0.104.0/v0.116.0 声明（§1.3/§1.4/§2.1/§12.4）、ONNX Runtime 开源支持国产 NPU（§2.2/§12.4）、无 BREAKING（全 workspace 回归 exit 0）。

### 备注

- tasks.md 中 T1 复选框遗留未勾，但 T1 全部子项交付物已经本核验确认在位（20/20 测试、highs-ffi 编译、clippy 0 warning），属记录遗漏而非实现缺失，本核验不改动代码，仅如实记录。


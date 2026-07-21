# Tasks — v0.93.0 Edge Coordinator 域级优化

> Spec：`spec.md`（develop-v0930-domain-optimizer）。所有改动 Surgical：既有 arbiter.rs / bid.rs / conflict.rs 零改动，仅新增 1 源文件 + 追加 lib.rs / Cargo.toml。

- [x] Task 1: 创建 `crates/agents/coordinator/src/domain_optimizer.rs`（核心实现，约 700 行含测试）
  - [x] SubTask 1.1: 模块文档头（D1~D12 偏差表，与 spec 一致）+ `use`（alloc::boxed/collections/vec、eneros_solver_core::{problem/result/solver}、eneros_energy_market_agent::{DevicePool/DeviceCapability/DeviceAssignment/DispatchPlan/equal_split/MarketData}）
  - [x] SubTask 1.2: `EdgeBoxState { box_id: u64, devices: DevicePool, socs: BTreeMap<u64, f32>, capacity_mw: f32, online: bool }`（Debug/Clone）
  - [x] SubTask 1.3: `DomainPlan { box_plans: BTreeMap<u64, DispatchPlan>, total_revenue: f32, timestamp: u64 }`（Debug/Clone/PartialEq/Default）
  - [x] SubTask 1.4: `OptError { EmptyDomain, InvalidTarget }`（Debug/Clone/Copy/PartialEq/Eq）
  - [x] SubTask 1.5: NaN 防御 sanitize 辅助（D12）：`sanitize_soc`（NaN→耗尽跳过语义，返回 Option）、`sanitize_capacity`（非有限或≤0 → None 排除 box）、`sanitize_efficiency`（非有限→0.5，越界 clamp [0,1]）、`sanitize_price`（非有限→0.0）
  - [x] SubTask 1.6: `DomainOptimizer { edge_boxes: BTreeMap<u64, EdgeBoxState>, solver: Box<dyn Solver>, optimize_count, fallback_count, empty_count }`（字段全 pub，D9）+ `new(solver)`（计数器全零）+ `add_box` / `remove_box` / `set_online`
  - [x] SubTask 1.7: 私有 `build_domain_lp(boxes: &BTreeMap<u64, EdgeBoxState>, target_mw: f32) -> Option<(LpProblem, Vec<(u64, u64, DeviceCapability)>)>`：合格 box（online && sanitize_capacity>0）内合格设备（有 soc 记录时 sanitize_soc 为 None→跳过）每设备 1 变量 `p_{box}_{dev}`，bounds [p_min,p_max]，Continuous，目标系数 `1−sanitize_efficiency`；行 0 域平衡 `Σp = target`（rhs 上下界相等）；其后每合格 box 1 行容量约束 `Σ_{i∈box} p_i ≤ capacity`（rhs_lower=`-f64::INFINITY`）；列序 (box_id, dev_id) 升序；无合格设备 → None
  - [x] SubTask 1.8: `optimize(&mut self, market: &MarketData, target_mw: f32, now_ms: u64) -> Result<DomainPlan, OptError>`：`optimize_count+=1` → target 非有限 `Err(InvalidTarget)` → `build_domain_lp` None → `empty_count+=1` + `Err(EmptyDomain)` → target clamp 到 Σ在线 capacity（D11）→ 重建 LP（clamp 后）→ solve：Optimal 且解长度匹配 → 按 box 聚合 `DispatchPlan`（逐设备 clamp [p_min,p_max]，box 内 objective_value=Σ(1−eff)·p_i），否则 → `fallback_count+=1` + 容量比例兜底（活跃 box 间按 capacity 比例分摊 target，box 内 `equal_split`，objective_value=0.0，D10）→ `total_revenue = sanitize_price(market.current_price as f32) × (total_power − total_loss)`（D12）→ `timestamp = now_ms`
  - [x] SubTask 1.9: 内嵌单元测试 40 个（T1~T40）：数据结构默认值/派生（T1~T8）、sanitize 四函数（T9~T16）、盒管理 add/remove/set_online（T17~T20）、build_domain_lp 结构与确定性（T21~T26）、optimize 校验路径（T27~T29）、优化路径聚合与收益（T30~T32）、离线排除重优化（T33~T34）、容量比例兜底（T35~T36）、域容量 clamp（T37）、NaN 风暴（T38~T39）、5-Box 集成 + 收益优于单机（T40，蓝图 §7.2）

- [x] Task 2: 更新 `crates/agents/coordinator/src/lib.rs`
  - [x] SubTask 2.1: crate 文档升级为 v0.92.0 + v0.93.0 双版本说明（域内仲裁 + 域级优化；核心类型清单表追加 domain_optimizer 行；D1~D12 简表保留 v0.92.0 并追加 v0.93.0 条目）
  - [x] SubTask 2.2: 追加 `pub mod domain_optimizer;` 与 `pub use domain_optimizer::{DomainOptimizer, EdgeBoxState, DomainPlan, OptError};`（既有 pub 项零改动）

- [x] Task 3: 更新 `crates/agents/coordinator/Cargo.toml`
  - [x] SubTask 3.1: description 更新为 "EnerOS v0.92.0+v0.93.0 Edge Coordinator (域内仲裁 + 域级优化, no_std)"
  - [x] SubTask 3.2: dependencies 追加 `eneros-solver-core = { path = "../../ai/solver-core" }` 与 `eneros-energy-market-agent = { path = "../energy-market-agent" }`（workspace 内既有 crate，SBOM 无新第三方依赖）

- [x] Task 4: 创建 `configs/domain_optimizer.toml`
  - [x] SubTask 4.1: `[domain_optimizer]` 段：`max_boxes = 32`（内存上限）、`fallback = "capacity_proportional"`
  - [x] SubTask 4.2: 中文注释覆盖：求解 <2s（蓝图 §6.3，集成阶段验收）/ 域容量安全（§7.3，D11）/ 离线排除（§4.4，D8）/ 状态一致性坑点（§8.5）/ 动态 EdgeBox 增删（§9）/ NaN 防御（D12）

- [x] Task 5: 创建 `docs/agents/domain-optimizer-design.md`
  - [x] SubTask 5.1: 12 章节（版本目标/前置依赖/交付物清单/详细设计/技术交底/测试计划/验收标准/风险/多角度要求/接口契约/验收清单引用/附录）
  - [x] SubTask 5.2: 2 个 Mermaid 图（域级优化数据流图、optimize 决策流程图含兜底分支）
  - [x] SubTask 5.3: D1~D12 偏差表（与 spec 偏差声明一致）

- [x] Task 6: 根目录 4 文件版本同步 0.92.0 → 0.93.0
  - [x] SubTask 6.1: 根 `Cargo.toml` version = "0.93.0"
  - [x] SubTask 6.2: `Makefile` 版本注释同步
  - [x] SubTask 6.3: `.github/workflows/ci.yml` 版本注释同步
  - [x] SubTask 6.4: `ci/src/gate.rs` 版本注释同步

- [x] Task 7: 构建验证（§2.4.2 全项）
  - [x] SubTask 7.1: `cargo metadata --format-version 1` 成功
  - [x] SubTask 7.2: `cargo test -p eneros-coordinator` 80 通过（40 既有 + 40 新增）
  - [x] SubTask 7.3: 交叉编译 `cargo build -p eneros-coordinator --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
  - [x] SubTask 7.4: `cargo fmt --all -- --check` 通过
  - [x] SubTask 7.5: `cargo clippy -p eneros-coordinator --all-targets -- -D warnings` 0 warning
  - [x] SubTask 7.6: `cargo deny check advisories licenses bans sources` 通过
  - [x] SubTask 7.7: 回归测试零破坏：eneros-energy-market-agent（185）/ eneros-twin-agent（120）/ eneros-agent-bus-dds（63）/ eneros-agent 全通过

- [x] Task 8: checklist.md 全项核验（C1~C80）
  - [x] SubTask 8.1: 逐项核验 checklist.md 并勾选
  - [x] SubTask 8.2: 失败项新建修复任务并重验

# Task Dependencies

- Task 1 → Task 2（lib.rs 依赖模块存在）
- Task 1 → Task 3（依赖追加后方可编译 domain_optimizer.rs 的 use）
- Task 2/3 → Task 7（构建验证依赖源码与 manifest 就绪）
- Task 4 / Task 5 / Task 6 相互独立，与 Task 1 可并行；Task 7 依赖全部代码任务完成
- Task 8 → Task 7（核验依赖构建验证通过）

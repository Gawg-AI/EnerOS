# Checklist — v0.93.0 Edge Coordinator 域级优化

> 核验依据：`spec.md`（develop-v0930-domain-optimizer）+ 蓝图 phase2 v0.93.0 + 项目 §2.4 校验清单。逐项核验后勾选。

## A. 数据结构与 API（spec Requirement: 域状态数据结构 / DomainOptimizer）

- [x] C1 `EdgeBoxState` 含 5 字段：`box_id: u64, devices: DevicePool, socs: BTreeMap<u64, f32>, capacity_mw: f32, online: bool`
- [x] C2 `EdgeBoxState` 派生 Debug/Clone
- [x] C3 `DomainPlan` 含 3 字段：`box_plans: BTreeMap<u64, DispatchPlan>, total_revenue: f32, timestamp: u64`
- [x] C4 `DomainPlan` 派生 Debug/Clone/PartialEq/Default
- [x] C5 `OptError` 2 变体 `EmptyDomain / InvalidTarget`，派生 Debug/Clone/Copy/PartialEq/Eq
- [x] C6 `DomainOptimizer` 5 字段全 pub：`edge_boxes: BTreeMap<u64, EdgeBoxState>, solver: Box<dyn Solver>, optimize_count, fallback_count, empty_count`（D9）
- [x] C7 `DomainOptimizer::new(solver)` 计数器全零
- [x] C8 `add_box` 同 id 覆盖；`remove_box` 存在 true / 不存在 false
- [x] C9 `set_online` 存在 id 设置 online 返回 true；不存在返回 false；状态不删除（D8）
- [x] C10 `optimize(&mut self, market: &MarketData, target_mw: f32, now_ms: u64) -> Result<DomainPlan, OptError>` 签名与 spec 一致（sync，D5/D6）
- [x] C11 复用 `eneros-energy-market-agent` 的 `DevicePool/DeviceCapability/DeviceAssignment/DispatchPlan/equal_split/MarketData`，未重复定义

## B. NaN 防御 sanitize（spec D12）

- [x] C12 `sanitize_soc`：soc NaN 或 ≤0 → None（设备按耗尽跳过）；有效值 → Some
- [x] C13 `sanitize_capacity`：capacity 非有限或 ≤0 → None（box 排除）；有效正值 → Some
- [x] C14 `sanitize_efficiency`：NaN → 0.5（中性）；越出 [0,1] → clamp；正常值原样
- [x] C15 `sanitize_price`：非有限 → 0.0；正常值原样

## C. build_domain_lp（spec Requirement: 域级 LP 构建）

- [x] C16 合格 box 判定：`online == true` 且 `sanitize_capacity(capacity_mw)` 为 Some
- [x] C17 合格设备判定：有 soc 记录时 `sanitize_soc` 为 Some；无 soc 记录视为合格（同 v0.87.0 dispatch 惯例）
- [x] C18 每合格设备 1 变量 `p_{box_id}_{dev_id}`，bounds `[p_min, p_max]`，VarType::Continuous
- [x] C19 目标系数 `1 − sanitize_efficiency(eff)`，ObjectiveSense::Minimize（损耗最小，v0.87.0 D14 一致）
- [x] C20 行 0 域平衡 `Σp = target`（rhs_lower == rhs_upper == clamped target）
- [x] C21 其后每合格 box 1 行容量约束 `Σ_{i∈box} p_i ≤ capacity`，rhs_lower = `-f64::INFINITY`，rhs_upper = capacity
- [x] C22 列序按 (box_id, dev_id) 升序（BTreeMap 确定性，D2）
- [x] C23 约束矩阵 CSR（row_start/col_index/values）结构正确：行数 = 1 + 合格 box 数；非零数 = 变量数（平衡行）+ Σ 各 box 设备数（容量行）
- [x] C24 无合格设备 → 返回 None
- [x] C25 同一输入两次 build → LpProblem 逐字段一致（确定性可重放）

## D. optimize 主流程（spec Requirement: DomainOptimizer 域级优化）

- [x] C26 每次调用 `optimize_count += 1`（含错误路径）
- [x] C27 target 非有限（NaN/±Inf）→ `Err(OptError::InvalidTarget)`，不产生 plan，不增 empty/fallback
- [x] C28 无 box / 全离线 / 全设备 SOC 耗尽 → `empty_count += 1` + `Err(OptError::EmptyDomain)`
- [x] C29 `target_mw > Σ在线 capacity` → clamp 到总在线容量后建 LP（不报错，D11）
- [x] C30 Solver 返回 Optimal 且解长度匹配 → 按 box 聚合为 `DispatchPlan`
- [x] C31 优化路径逐设备 setpoint clamp 到 `[p_min, p_max]`
- [x] C32 优化路径各 box `DispatchPlan.objective_value = Σ(1−eff_i)·p_i`（box 内）
- [x] C33 各 box `DispatchPlan.timestamp == now_ms`；`DomainPlan.timestamp == now_ms`（D6）
- [x] C34 Solver Err / 非 Optimal / 解长度不符 → `fallback_count += 1` + D10 容量比例兜底（非错误返回）
- [x] C35 兜底：活跃 box 间按 `capacity_mw` 比例分摊 target（box 分配 = target × cap_box / Σcap）
- [x] C36 兜底：box 内复用 v0.87.0 `equal_split`（clamp [p_min, p_max]），`objective_value == 0.0`
- [x] C37 `total_revenue = sanitize_price(market.current_price) × (total_power − total_loss)`，`total_loss = Σ(1−eff_i)·p_i`（D12 净收益语义）
- [x] C38 兜底路径 revenue 仍按实际分配计算（不归零）
- [x] C39 离线 box 不出现在 `box_plans`（LP 与兜底路径均排除，D8）
- [x] C40 `set_online(false)` 后 optimize target 全部分摊给在线 box；`set_online(true)` 恢复纳入

## E. 单元测试（40 个，T1~T40）

- [x] C41 T1~T8 数据结构默认值与派生（EdgeBoxState Clone / DomainPlan Default/PartialEq / OptError Copy/Eq 等）全部通过
- [x] C42 T9~T16 sanitize 四函数边界（NaN/±Inf/0/负数/越界/正常值）全部通过
- [x] C43 T17~T20 盒管理（add 覆盖 / remove 真假 / set_online 真假 / 离线不删除状态）全部通过
- [x] C44 T21~T26 build_domain_lp（变量数/列序/行数/平衡行 rhs/容量行 rhs/两次构建一致）全部通过
- [x] C45 T27~T29 optimize 校验（InvalidTarget×3 / EmptyDomain×3 / 计数器）全部通过
- [x] C46 T30~T32 优化路径（多 box 协同分配 / clamp / timestamp / revenue 公式）全部通过
- [x] C47 T33~T34 离线排除重优化（排除后重分摊 / 恢复纳入）全部通过
- [x] C48 T35~T36 容量比例兜底（Err 与 Infeasible 与解长度不符 / 比例分配 / objective 0.0 / revenue 仍计算）全部通过
- [x] C49 T37 域容量 clamp（target=100 总 cap=10 → 总出力 ≤10，不报错）通过
- [x] C50 T38~T39 NaN 风暴（soc NaN 跳过 / capacity NaN 排除 box / eff NaN→0.5 / price NaN→revenue 0）全部通过
- [x] C51 T40 5-Box 集成 + 收益优于单机判定（优化路径 revenue 严格大于同输入兜底路径，蓝图 §7.2）通过
- [x] C52 `cargo test -p eneros-coordinator` 80 通过（40 既有 v0.92.0 + 40 新增），0 失败

## F. crate 集成（spec MODIFIED Requirement）

- [x] C53 `lib.rs` crate 文档升级为 v0.92.0 + v0.93.0 双版本说明
- [x] C54 `lib.rs` 追加 `pub mod domain_optimizer;`
- [x] C55 `lib.rs` 追加 4 项重导出：`DomainOptimizer, EdgeBoxState, DomainPlan, OptError`
- [x] C56 `lib.rs` 核心类型清单表追加 domain_optimizer 行 + v0.93.0 D1~D12 偏差简表
- [x] C57 既有 pub 项与 arbiter.rs / bid.rs / conflict.rs **零改动**（git diff 验证）
- [x] C58 coordinator `Cargo.toml` dependencies 追加 `eneros-solver-core = { path = "../../ai/solver-core" }` 与 `eneros-energy-market-agent = { path = "../energy-market-agent" }`
- [x] C59 coordinator `Cargo.toml` description 更新含 v0.93.0
- [x] C60 无新第三方依赖（SBOM 不变，`cargo tree -p eneros-coordinator` 仅新增 2 个 workspace 内 crate）

## G. no_std 合规（§4.3 硬规则）

- [x] C61 `lib.rs` 保持 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`（子模块不重复加，继承惯例）
- [x] C62 domain_optimizer.rs 无 `use std::*` / 无 `panic!` / `todo!` / `unimplemented!` / 无 `unsafe` / 无 `async`
- [x] C63 仅用 `alloc::*` / `core::*`；无 `log` / `uuid` / `serde` 等新依赖

## H. 配置与文档

- [x] C64 `configs/domain_optimizer.toml` 存在且含 `[domain_optimizer]` 段（`max_boxes` / `fallback = "capacity_proportional"`）
- [x] C65 配置中文注释覆盖 6 点：求解 <2s（§6.3）/ 域容量安全（§7.3）/ 离线排除（§4.4）/ 状态一致性（§8.5）/ 动态增删（§9）/ NaN 防御（D12）
- [x] C66 `docs/agents/domain-optimizer-design.md` 存在且为 12 章节结构
- [x] C67 设计文档含 2 个 Mermaid 图（数据流图 / optimize 决策流程图）
- [x] C68 设计文档含 D1~D12 偏差表且与 spec 一致
- [x] C69 文档放入 `docs/agents/`（§2.3.3 分类合规，非 docs/ 根）

## I. 版本同步（根 4 文件）

- [x] C70 根 `Cargo.toml` workspace version = "0.93.0"
- [x] C71 `Makefile` 版本注释 0.93.0
- [x] C72 `.github/workflows/ci.yml` 版本注释 0.93.0
- [x] C73 `ci/src/gate.rs` 版本注释 0.93.0

## J. 构建验证（§2.4.2 全项）

- [x] C74 `cargo metadata --format-version 1` 成功（workspace 成员路径全部正确）
- [x] C75 交叉编译 `cargo build -p eneros-coordinator --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C76 `cargo fmt --all -- --check` 通过
- [x] C77 `cargo clippy -p eneros-coordinator --all-targets -- -D warnings` 0 warning
- [x] C78 `cargo deny check advisories licenses bans sources` 通过
- [x] C79 回归测试零破坏：eneros-energy-market-agent（185）/ eneros-twin-agent（120）/ eneros-agent-bus-dds（63）/ eneros-agent 全部通过
- [x] C80 `git status` 无 `target/`、`*.elf`、`*.bin`、IDE 缓存被追踪；无垃圾文件

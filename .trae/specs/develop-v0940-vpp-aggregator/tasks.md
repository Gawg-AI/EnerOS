# Tasks — v0.94.0 Edge Coordinator VPP 聚合

> Spec：`spec.md`（develop-v0940-vpp-aggregator）。所有改动 Surgical：既有 bid.rs / arbiter.rs / conflict.rs 零改动，domain_optimizer.rs 仅 sanitize 可见性放宽，仅新增 1 源文件 + 追加 lib.rs / Cargo.toml description。

- [x] Task 1: 创建 `crates/agents/coordinator/src/vpp_aggregator.rs`（核心实现，约 700 行含测试）
  - [x] SubTask 1.1: 模块文档头（D1~D12 偏差表，与 spec 一致）+ `use`（alloc::collections/vec、eneros_energy_market_agent::{Bid/BidSide/BidStrategy/MarketData/MarketType/Period/DeviceCapability/DevicePool}、eneros_solver_core::solver::Solver、crate::domain_optimizer::{DomainOptimizer + 3 个 sanitize 复用}）
  - [x] SubTask 1.2: `ResourceType { Battery, Pv, Load, Charger }`（Debug/Clone/Copy/PartialEq/Eq/Default，默认 Battery）
  - [x] SubTask 1.3: `VppResource { resource_id: u64, capacity_mw: f32, available_mw: f32, ramp_rate: f32, efficiency: f32, type_: ResourceType, online: bool }`（Debug/Clone/Copy/PartialEq）
  - [x] SubTask 1.4: `VppProfile { total_capacity_mw, available_mw, ramp_up_mw_per_min, ramp_down_mw_per_min }`（Debug/Clone/Copy/PartialEq/Default）
  - [x] SubTask 1.5: `Allocation { resource_id: u64, setpoint_mw: f32 }`（Debug/Clone/Copy/PartialEq）+ `AggregatedDispatch { target_mw, allocations: Vec<Allocation>, timestamp: u64 }`（Debug/Clone/PartialEq/Default）
  - [x] SubTask 1.6: `VppError { InsufficientCapacity, InvalidTarget, NoResource }`（Debug/Clone/Copy/PartialEq/Eq）
  - [x] SubTask 1.7: sanitize 辅助：`sanitize_available(avail, cap) -> f32`（非有限→0，clamp [0, cap]）；`sanitize_ramp(r) -> f32`（非有限/负→0）；capacity/efficiency/price 复用 domain_optimizer（SubTask 配合 Task 2 可见性放宽）
  - [x] SubTask 1.8: `VppAggregator { resources: BTreeMap<u64, VppResource>, optimizer: DomainOptimizer, aggregate_count, dispatch_count, reject_count }`（字段全 pub，D9）+ `new(solver)` + `add_resource` / `remove_resource` / `set_online` / `set_available`
  - [x] SubTask 1.9: 私有 `compute_profile(&self) -> VppProfile`（免计数内部版）+ pub `aggregate(&mut self)`（aggregate_count+=1 后调 compute_profile）
  - [x] SubTask 1.10: 私有 `sync_boxes(&mut self)`（D8）：在线资源 → EdgeBoxState（box_id=device_id=resource_id，单设备 DeviceCapability { p_min: 0, p_max: sanitize_available, ramp_rate: sanitize_ramp, efficiency: sanitize_efficiency }，socs={id:1.0}，capacity_mw=sanitize_available，online=true）写入 self.optimizer.edge_boxes（先清后填，离线/无效 capacity 不写入）
  - [x] SubTask 1.11: `dispatch(&mut self, market, target_mw, now_ms)`：dispatch_count+=1 → 非有限 reject+InvalidTarget → profile |target|>available reject+InsufficientCapacity → sync_boxes → optimizer.optimize：Ok→allocations（resource_id 升序 flat_map），Err(EmptyDomain)→reject+NoResource，Err(InvalidTarget)→reject+InvalidTarget（防御不可达）；timestamp=now_ms
  - [x] SubTask 1.12: `market_bid(&self, market, strategy, now_ms) -> Vec<Bid>`（resource_id 升序，跳过 available≤0，quantity=min(avail, max_quantity>0?sanitize:avail)，price=sanitize_price(current)+margin，bid_id 从 1 递增，Spot/Sell/Flat，timestamp=now_ms）
  - [x] SubTask 1.13: 内嵌单元测试 40 个（T1~T40）：数据结构 T1~T6 / 资源管理 T7~T10 / 聚合 T11~T16 / dispatch 校验 T17~T20 / dispatch 分配 T21~T28 / 离线动态 T29~T32 / market_bid T33~T38 / 5 资源集成+NaN 风暴 T39~T40

- [x] Task 2: 更新 `crates/agents/coordinator/src/domain_optimizer.rs`（仅可见性）
  - [x] SubTask 2.1: `sanitize_capacity` / `sanitize_efficiency` / `sanitize_price` 三个函数 `fn` → `pub(crate) fn`（零逻辑改动；sanitize_soc 不需要导出）
  - [x] SubTask 2.2: 确认 v0.93.0 全部 80 测试仍通过

- [x] Task 3: 更新 `crates/agents/coordinator/src/lib.rs`
  - [x] SubTask 3.1: crate 文档升级为 v0.92.0 + v0.93.0 + v0.94.0 三版本说明（核心类型清单表追加 vpp_aggregator 行；追加 v0.94.0 D1~D12 简表）
  - [x] SubTask 3.2: 追加 `pub mod vpp_aggregator;` 与 `pub use vpp_aggregator::{AggregatedDispatch, Allocation, ResourceType, VppAggregator, VppError, VppProfile, VppResource};`（既有 pub 项零改动）

- [x] Task 4: 更新 `crates/agents/coordinator/Cargo.toml`
  - [x] SubTask 4.1: description 更新为 "EnerOS v0.92.0+v0.93.0+v0.94.0 Edge Coordinator (域内仲裁 + 域级优化 + VPP 聚合, no_std)"（无新依赖）

- [x] Task 5: 创建 `configs/vpp_aggregator.toml`
  - [x] SubTask 5.1: `[vpp_aggregator]` 段：`max_resources = 64`、`default_margin = 5.0`
  - [x] SubTask 5.2: `[[vpp_resource]]` ≥3 个资源清单示例（Battery/Pv/Charger，含 capacity/available/ramp/efficiency/online）
  - [x] SubTask 5.3: 中文注释覆盖 6 点：响应 <30s（§6.3/§7.2 集成验收）/ 不超聚合容量（§7.3，D10）/ 离线重算（§6.5，D6）/ 申报执行偏差（§8.5）/ 清单配置化（§9 可维护）/ NaN 防御（D12）

- [x] Task 6: 创建 `docs/agents/vpp-aggregation-design.md`
  - [x] SubTask 6.1: 12 章节（版本目标/前置依赖/交付物清单/详细设计/技术交底/测试计划/验收标准/风险/多角度要求/接口契约/偏差声明/附录）
  - [x] SubTask 6.2: 2 个 Mermaid 图（VPP 聚合数据流图、dispatch 决策流程图含拒绝/兜底分支）
  - [x] SubTask 6.3: D1~D12 偏差表（与 spec 偏差声明一致）

- [x] Task 7: 根目录 4 文件版本同步 0.93.0 → 0.94.0
  - [x] SubTask 7.1: 根 `Cargo.toml` version = "0.94.0"
  - [x] SubTask 7.2: `Makefile` 版本注释同步
  - [x] SubTask 7.3: `.github/workflows/ci.yml` 版本注释同步
  - [x] SubTask 7.4: `ci/src/gate.rs` 注释同步（追加 v0.94.0 类型清单：VppAggregator / VppResource / VppProfile / AggregatedDispatch / Allocation / VppError / ResourceType）

- [x] Task 8: 构建验证（§2.4.2 全项）
  - [x] SubTask 8.1: `cargo metadata --format-version 1` 成功
  - [x] SubTask 8.2: `cargo test -p eneros-coordinator` 120 通过（80 既有 + 40 新增）
  - [x] SubTask 8.3: 交叉编译 `cargo build -p eneros-coordinator --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
  - [x] SubTask 8.4: `cargo fmt --all -- --check` 通过
  - [x] SubTask 8.5: `cargo clippy -p eneros-coordinator --all-targets -- -D warnings` 0 warning（workspace clippy 同步验证）
  - [x] SubTask 8.6: `cargo deny check advisories licenses bans sources` 通过（licenses/bans/sources ok；advisories 因 github.com 网络不可达跳过 DB 更新，零新增依赖，供应链面同 v0.93.0）
  - [x] SubTask 8.7: 回归测试零破坏：eneros-energy-market-agent（185）/ eneros-twin-agent（120）/ eneros-agent-bus-dds（63）/ eneros-agent（270+79 集成）全通过

- [x] Task 9: checklist.md 全项核验（C1~C80）
  - [x] SubTask 9.1: 逐项核验 checklist.md 并勾选
  - [x] SubTask 9.2: 失败项新建修复任务并重验（无失败项，80/80 一次通过）

# Task Dependencies

- Task 1 ↔ Task 2（vpp_aggregator 复用 sanitize 需 Task 2 可见性放宽；同一子代理一并完成）
- Task 1/2 → Task 3（lib.rs 依赖模块存在）
- Task 1~4 → Task 8（构建验证依赖源码与 manifest 就绪）
- Task 5 / Task 6 / Task 7 相互独立，与 Task 1 可并行；Task 8 依赖全部代码任务完成
- Task 9 → Task 8（核验依赖构建验证通过）

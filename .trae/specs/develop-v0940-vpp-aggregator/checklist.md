# Checklist — v0.94.0 Edge Coordinator VPP 聚合

> Spec：`spec.md`（develop-v0940-vpp-aggregator）。逐项核验，未通过禁止收工。

## A. 目录结构校验（§2.4.1，C1~C5）

- [x] C1: 新源文件 `vpp_aggregator.rs` 位于既有 `crates/agents/coordinator/src/` 下，未新建根目录 crate
- [x] C2: 根 `Cargo.toml` workspace members 无需新增（复用既有 coordinator 成员），workspace 仍可解析
- [x] C3: 无新增跨 crate path 引用；coordinator `Cargo.toml` 无新依赖
- [x] C4: 新文档 `vpp-aggregation-design.md` 位于 `docs/agents/`，未平面化放 `docs/` 根
- [x] C5: 仓库根目录无除 `ci/` 外的新 crate 文件夹

## B. 构建校验（§2.4.2，C6~C11）

- [x] C6: `cargo metadata --format-version 1` 成功
- [x] C7: `cargo test -p eneros-coordinator` 120 通过（80 既有 + 40 新增）
- [x] C8: `cargo build -p eneros-coordinator --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C9: `cargo fmt --all -- --check` 通过
- [x] C10: `cargo clippy -p eneros-coordinator --all-targets -- -D warnings` 0 warning
- [x] C11: `cargo deny check advisories licenses bans sources` 通过（licenses/bans/sources ok；advisories 因 github.com 网络不可达跳过 DB 更新，零新增依赖，供应链面同 v0.93.0）

## C. 文档与规范校验（§2.4.3，C12~C15）

- [x] C12: 新文档在 `docs/agents/` 下，不在 `docs/` 根
- [x] C13: `git status` 无 `target/`、`*.elf`、`*.bin`、`*.dtb`、IDE 缓存被追踪
- [x] C14: 无新文件类型需 `.gitignore` 覆盖
- [x] C15: 新代码无 `use std::*` / `panic!` / `todo!` / `unimplemented!` / `unsafe` / `async`（no_std 合规）

## D. 数据结构（C16~C21）

- [x] C16: `ResourceType { Battery, Pv, Load, Charger }` 派生 Debug/Clone/Copy/PartialEq/Eq/Default，默认 Battery
- [x] C17: `VppResource` 8 字段（`resource_id: u64` / `capacity_mw` / `available_mw` / `ramp_rate` / `efficiency` / `type_` / `online`）派生 Debug/Clone/Copy/PartialEq
- [x] C18: `VppProfile` 4 字段（`total_capacity_mw` / `available_mw` / `ramp_up_mw_per_min` / `ramp_down_mw_per_min`）派生 Debug/Clone/Copy/PartialEq/Default
- [x] C19: `Allocation { resource_id: u64, setpoint_mw: f32 }` 派生 Debug/Clone/Copy/PartialEq
- [x] C20: `AggregatedDispatch { target_mw, allocations: Vec<Allocation>, timestamp: u64 }` 派生 Debug/Clone/PartialEq/Default
- [x] C21: `VppError { InsufficientCapacity, InvalidTarget, NoResource }` 派生 Debug/Clone/Copy/PartialEq/Eq

## E. 资源管理（C22~C27）

- [x] C22: `VppAggregator` 字段全 pub：`resources: BTreeMap<u64, VppResource>` / `optimizer: DomainOptimizer` / `aggregate_count` / `dispatch_count` / `reject_count`
- [x] C23: `new(solver)` 计数器全零、空资源表
- [x] C24: `add_resource` 插入；同 id 再次 add 覆盖
- [x] C25: `remove_resource` 存在 → true，再删 → false
- [x] C26: `set_online` 更新返回 true；不存在 id → false；离线资源状态保留不删除（D6）
- [x] C27: `set_available` 更新返回 true；不存在 id → false

## F. 容量聚合（C28~C35）

- [x] C28: `aggregate(&mut self)` 使 `aggregate_count += 1`；内部 `compute_profile(&self)` 免计数
- [x] C29: 仅统计 `online && sanitize_capacity(capacity_mw)` 有效（有限且 >0）的资源
- [x] C30: `total_capacity_mw = Σ capacity`
- [x] C31: `available_mw = Σ sanitize_available`（非有限 → 0，clamp [0, capacity]，D12）
- [x] C32: `ramp_up = ramp_down = Σ sanitize_ramp`（非有限/负 → 0，对称 D11）
- [x] C33: 空聚合器 → 全零 profile
- [x] C34: 全离线 → 全零 profile
- [x] C35: 离线重算（蓝图 §6.5）：`set_online(false)` 后再 aggregate，离线资源即时排除

## G. dispatch（C36~C46）

- [x] C36: `dispatch_count` 每次调用 += 1
- [x] C37: target 非有限 → `reject_count += 1` + `Err(InvalidTarget)`
- [x] C38: `|target| > available` → `reject_count += 1` + `Err(InsufficientCapacity)`（含负 target 充电 abs 判定，D10）
- [x] C39: `sync_boxes` 将每在线资源映射为单设备 box（`box_id = device_id = resource_id`，`p_min = 0`，`p_max = capacity_mw = sanitize_available`，`soc = 1.0` 恒合格，D8）
- [x] C40: `sync_boxes` 先清后填：离线/无效 capacity 资源不写入 `optimizer.edge_boxes`
- [x] C41: `optimizer.optimize` `Ok(plan)` → allocations 按 resource_id 升序 flat_map（device_id 即 resource_id）
- [x] C42: `Err(EmptyDomain)` → `reject_count += 1` + `Err(NoResource)`
- [x] C43: `Err(InvalidTarget)` 防御分支 → `reject_count += 1` + `Err(InvalidTarget)`
- [x] C44: `AggregatedDispatch.timestamp == now_ms`；`target_mw` 回显
- [x] C45: solver Err / Infeasible / 解长度不符 → DomainOptimizer 内建容量比例兜底生效（v0.93.0 D10），dispatch 仍返回 Ok
- [x] C46: 离线资源排除分配；`set_online(true)` 恢复后重新纳入

## H. market_bid（C47~C52）

- [x] C47: 纯查询无计数器更新；按 resource_id 升序遍历在线资源
- [x] C48: 跳过 `sanitize(available) ≤ 0` 的资源
- [x] C49: `quantity = min(available, max_quantity)`；`max_quantity` 非有限或 ≤0 → available 全额
- [x] C50: `price = sanitize_price(market.current_price as f32) + strategy.margin`
- [x] C51: `Bid` 字段：`bid_id` 从 1 顺序递增（resource_id 升序）、`MarketType::Spot`、`BidSide::Sell`、`Period::Flat`、`timestamp = now_ms`
- [x] C52: 空聚合器/全离线 → 空 Vec

## I. sanitize 复用（C53~C55）

- [x] C53: `domain_optimizer.rs` 的 `sanitize_capacity` / `sanitize_efficiency` / `sanitize_price` 改为 `pub(crate)`（零逻辑改动）
- [x] C54: `sanitize_soc` 保持私有不导出
- [x] C55: v0.93.0 全部 80 测试仍通过（可见性调整无回归）

## J. crate 集成（C56~C60）

- [x] C56: `lib.rs` 追加 `pub mod vpp_aggregator;`
- [x] C57: `lib.rs` 追加 7 项重导出（`VppAggregator` / `VppResource` / `VppProfile` / `AggregatedDispatch` / `Allocation` / `VppError` / `ResourceType`）
- [x] C58: `lib.rs` crate 文档升级为 v0.92.0 + v0.93.0 + v0.94.0 三版本说明（核心类型清单表追加 vpp_aggregator 行 + v0.94.0 D1~D12 简表）
- [x] C59: 既有 pub 项与 4 个既有模块零改动（`bid.rs` / `arbiter.rs` / `conflict.rs` 完全未动）
- [x] C60: coordinator `Cargo.toml` description 追加 v0.94.0，无新依赖

## K. 配置文件（C61~C64）

- [x] C61: `configs/vpp_aggregator.toml` 存在，`[vpp_aggregator]` 段含 `max_resources = 64`、`default_margin = 5.0`
- [x] C62: `[[vpp_resource]]` ≥ 3 个资源清单示例（覆盖 Battery / Pv / Charger）
- [x] C63: 资源字段含 capacity / available / ramp / efficiency / online
- [x] C64: 中文注释覆盖 6 点：响应 <30s（§6.3/§7.2 集成验收）/ 不超聚合容量（§7.3，D10）/ 离线重算（§6.5，D6）/ 申报执行偏差（§8.5）/ 清单配置化（§9 可维护）/ NaN 防御（D12）

## L. 设计文档（C65~C68）

- [x] C65: `docs/agents/vpp-aggregation-design.md` 存在，12 章节齐全（版本目标/前置依赖/交付物清单/详细设计/技术交底/测试计划/验收标准/风险/多角度要求/接口契约/偏差声明/附录）
- [x] C66: 含 2 个 Mermaid 图（VPP 聚合数据流图 + dispatch 决策流程图含拒绝/兜底分支）
- [x] C67: 含 D1~D12 偏差表，与 spec 偏差声明一致
- [x] C68: 接口契约与实现一致（函数签名、字段、错误变体、计数器语义）

## M. 版本同步（C69~C72）

- [x] C69: 根 `Cargo.toml` version = "0.94.0"
- [x] C70: `Makefile` 版本注释同步 0.94.0
- [x] C71: `.github/workflows/ci.yml` 版本注释同步 0.94.0
- [x] C72: `ci/src/gate.rs` 注释同步（追加 v0.94.0 类型清单：VppAggregator / VppResource / VppProfile / AggregatedDispatch / Allocation / VppError / ResourceType）

## N. 测试覆盖（C73~C78）

- [x] C73: 内嵌 40 个单元测试（T1~T40）全部实现并通过
- [x] C74: 测试分布：数据结构 T1~T6 / 资源管理 T7~T10 / 聚合 T11~T16 / dispatch 校验 T17~T20 / dispatch 分配 T21~T28 / 离线动态 T29~T32 / market_bid T33~T38 / 集成+NaN T39~T40
- [x] C75: 含 5 资源集成测试（聚合 + dispatch + market_bid 全链路）
- [x] C76: 含资源离线重算故障注入测试
- [x] C77: 含 NaN 风暴防御测试（capacity / available / ramp / efficiency / price 非有限输入）
- [x] C78: 回归零破坏：eneros-energy-market-agent（185）/ eneros-twin-agent（120）/ eneros-agent-bus-dds（63）/ eneros-agent（33）全通过

## O. 蓝图达成（C79~C80）

- [x] C79: v0.94.0 交付物全覆盖：VPP 容量聚合（VppProfile）/ 聚合出力控制（dispatch→AggregatedDispatch）/ 市场申报（market_bid→Vec\<Bid\>）
- [x] C80: 无 BREAKING：既有公共 API 全保留，下游 v0.95.0 云端策略下发 / v0.96.0 Cloud Coordinator 解锁

# ADR-0041：energy-market-agent re-export 收缩 + LP 构建合并

> 状态：Accepted（2026-08-09）
> 决策者：架构深化实施（蓝图 23 W4.1）
> 前置：蓝图 23 §1 锁定 1（interface 诚实）；§1 锁定 3（不变量单一持有）。
> 审计证据（2026-08-09 实施时 grep 复核）：
> - `BidGenerator`/`MultiObjectiveOptimizer`/`MultiDeviceDispatcher`/`AgentRuntimeAdapter`/
>   `RuleBidIntentSource`/`SolverBidOptimizer`/`MarketChannelBidPublisher` 全仓零非测试调用方。
> - `parser`/`subscriber`/`MarketChannel`/`MarketFeed`/`MarketError`/`MarketDataSource`/
>   `MarketSignal`/`ParetoFront`/`ParetoSolution`/`WeightedSum`/`filter_dominated` 等零非测试调用方
>   （外部命中均为同名不同物，如 spot-market 的 `MarketError`、solver-pareto 的 `ParetoFront`，
>   或测试代码）。
> - `build_lp_problem`（multi_dispatch.rs:174）与 `build_weighted_lp`（multi_objective.rs:423）
>   逐字重复（仅 objective 系数来源不同）。

## 1. 背景

蓝图 23 §0.2 结论 1：energy-market-agent re-export 40+ 符号，但大量符号零非测试调用方，
制造"看起来在用"的假象。§0.3 结论 1：`build_lp_problem` 与 `build_weighted_lp` 是同一
deep module 的两种入参，逐字重复。

## 2. 决策

1. **re-export 收缩**（§1 锁定 1：暴露即真实在用或明确标注）：
   - **保留**（有非测试消费者）：`BidStrategy`/`ResourceBid`（coordinator vpp_aggregator）、
     `DeviceCapability`/`DeviceMode`/`DevicePool`（coordinator）、`EnergyAgent`/`MarketAgent`/
     `AgentRuntime`（mvp-scenario）、`AgentRuntimeError`/`HeartbeatStatus`（device-agent/
     grid_agent）、`MarketData`（coordinator）、`YuanPerKwh`/`DrSignal`/`MarketType`/`Period`/
     `PricePoint`（coordinator/cloud-coordinator）、`equal_split`/`DeviceAssignment`/
     `DispatchError`/`DispatchPlan`（coordinator）、`Objective`（cloud-coordinator）。
   - **降级 `#[cfg(any(test, feature = "mock"))]`**（零生产消费者，算法有扎实测试保留价值）：
     `BidGenerator` 族（含 `BidIntent`/`BidOptimizer`/`BidPublisher`/`BidSide`/`BidError`/
     `conservative_optimize`/`rule_intent`）、`MultiDeviceDispatcher`、`MultiObjectiveOptimizer`
     族（含 `ParetoFront`/`ParetoSolution`/`WeightedSum`/`filter_dominated`/
     `eval_plan_objectives`/`generate_weight_sample`/`normalize_costs`/`objective_costs`）、
     `parser` 三函数、`subscriber` 四类型、`MarketChannel`/`MarketFeed`/`MarketError`/
     `MarketDataSource`/`MarketSignal`。
   - **整模块降级**：`real_adapters`（RuleBidIntentSource/SolverBidOptimizer/
     MarketChannelBidPublisher）、`sdk_bridge`（AgentRuntimeAdapter）——零生产消费者，
     mock feature 下测试保留，待真机报价胶水需求时接线。
2. **LP 构建合并**（§1 锁定 3）：`build_weighted_lp` 成为约束骨架唯一持有者
   （`pub(crate)`），`build_lp_problem` 收敛为薄包装（objective 系数 = `1 - efficiency`
   在此生成后调用）。删除 multi_dispatch.rs 中 66 行逐字重复。
3. **测试不减少**：所有降级符号的模块测试在 `cfg(any(test, feature="mock"))` 下完整保留
   （207 测试全过）。coordinator 测试经 `[dev-dependencies] features=["mock"]` 保持可编译
   （mvp-scenario 已有同款模式）。

## 3. 备选方案与拒绝理由

| 备选 | 拒绝理由 |
|---|---|
| 删除零消费者的算法模块（BidGenerator/MultiObjectiveOptimizer 等） | 算法逻辑有扎实测试（bid_generator 720 行、multi_dispatch 615 行、multi_objective Pareto 前沿 t154~t161），v1.0.0 后多设备调度/多目标优化是真实演进方向。红线 6：有测试的可能未来有用的 module 降级而非删除 |
| 给零消费者符号接线（补真实调用方） | v1.0.0 的 mvp-scenario 只用 EnergyAgent/MarketAgent；无真实需求时不制造假胶水（ADT 纪律"先有真实需求再加"） |
| 保留全部 re-export 仅加 doc 标注 | 锁定 1 要求明确降级或接线，标注不改变"暴露但无人用"的事实；cfg 降级让编译器成为守门人 |

## 4. 影响面

- `crates/agents/energy-market-agent/src/lib.rs`：re-export 收缩 + 模块降级。
- `crates/agents/energy-market-agent/src/multi_objective.rs`：`build_weighted_lp` 改 `pub(crate)`。
- `crates/agents/energy-market-agent/src/multi_dispatch.rs`：`build_lp_problem` 收敛为薄包装；
  LP 结构 imports 移至测试。
- `crates/agents/coordinator/Cargo.toml`：dev-dependencies 启用 energy-market-agent mock feature。

## 5. 验收

- 零生产消费者符号从默认构建 public API 撤出（`cargo build` 默认无 mock 通过）。
- `build_lp_problem`/`build_weighted_lp` 重复消除（grep 复核单一 `ConstraintMatrix::new` 构建点）。
- `cargo test -p eneros-energy-market-agent`（默认 + mock）→ 207 passed; 0 failed。
- `cargo test -p eneros-coordinator` → 120 passed; 0 failed。
- `cargo test -p eneros-mvp-scenario` → 24 passed; 0 failed。
- `cargo test -p eneros-cloud-coordinator` → 85 passed; 0 failed。

## 6. 追踪

- 蓝图：23 §5 W4.1；§1 锁定 1/3。
- 纪律：锁定 1（interface 诚实）、锁定 3（不变量单一持有）。
- 遗留：`Period` 收敛到 market-types 由 W5.1 处理（本 ADR 不涉及）。

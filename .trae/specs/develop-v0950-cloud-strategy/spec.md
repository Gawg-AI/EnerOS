# v0.95.0 Cloud Coordinator 基础 — 策略下发 Spec

## Why

v0.94.0 完成 VPP 聚合（边缘侧容量可市场化），蓝图 phase2 v0.95.0（P2-D 第 4 版，云边协同起点）要求实现 Cloud Coordinator **策略下发**能力：云端生成全局策略（优化权重/电价预测/DR 响应/模型更新）→ 下发到边缘 → 边缘安全校验，**策略非强制、边缘主权保留**（违反安全约束可拒绝），为 v0.96.0 数据汇聚与 v0.112.0 云端孪生主节点提供云边协同基础。

## What Changes

- **新建 crate `eneros-cloud-coordinator`**（`crates/agents/cloud-coordinator/`，D1），3 个源文件（蓝图结构）：
  - `src/strategy.rs` — `Strategy` / `StrategyContent`（4 变体）/ `ModelRef` / `EdgeAck` / `RejectReason` / `LocalState` + `validate_strategy` 边缘安全校验（蓝图 §4.5 关键代码落地）+ 常量 `SAFETY_WEIGHT_MIN` / `DEFAULT_ACK_TIMEOUT_MS` / `DEFAULT_MAX_RETRIES`
  - `src/channel.rs` — `CloudChannel` trait（sync，D3/D8）+ `CloudError` + `MockCloudChannel`（可故障注入：broadcast 前 N 次失败、预置 acks）
  - `src/publisher.rs` — `StrategyPublisher`（`Box<dyn CloudChannel>` + 超时重试 + pending 断网补发队列 + 4 个 pub 可观测计数器，D9）
  - `src/lib.rs` — no_std crate 文档（D1~D12 偏差表）+ 重导出
- `Cargo.toml`（新 crate）：依赖仅 2 个既有 path crate（`eneros-energy-market-agent` 提供 Objective/PricePoint/DrSignal；`eneros-coordinator` 提供 Priority，D5）
- 根 `Cargo.toml` members 追加 `"crates/agents/cloud-coordinator"`（既有成员零改动）
- 新增 `configs/cloud_coordinator.toml`（`[cloud_coordinator]` 连接/超时/重试/安全门限 + 中文注释 6 点）
- 新增 `docs/agents/cloud-strategy-design.md`（12 章节 + 2 Mermaid + D1~D12 偏差表）
- 根目录 4 文件版本同步 0.94.0 → 0.95.0（Cargo.toml / Makefile / ci.yml / gate.rs 注释）
- 内嵌单元测试 40 个（T1~T40），含 publish+ack 全链路集成、断网补发故障注入、NaN 风暴防御
- **无 BREAKING**：既有全部 crate 零改动

## Impact

- Affected specs：无既有 spec 受影响（全新 crate）；关联 develop-v0940-vpp-aggregator（前序 VPP 聚合）
- Affected code：新增 `crates/agents/cloud-coordinator/`、`configs/`、`docs/agents/`、根 4 文件
- 依赖：无新第三方依赖（仅 2 个既有 workspace path 依赖）
- 下游解锁：v0.96.0 数据汇聚（P2-D 收尾）、v0.112.0 云端孪生主节点

## 偏差声明（D1~D12，Karpathy Think Before Coding：显式取舍）

| 偏差 | 蓝图原文 | 本版本处理 |
|------|---------|-----------|
| **D1** | crate 路径 `crates/cloud_coordinator/`；文档 `docs/phase2/cloud_strategy.md` | `crates/agents/cloud-coordinator/` + `docs/agents/cloud-strategy-design.md`（项目 §2.3.1/§2.3.3 硬规则；Agent 实现归 agents 子系统） |
| **D2** | `strategy_id: String` / `targets: Vec<String>` / `edge_id: String` | 全部 `u64` / `Vec<u64>`（无堆字符串 + 确定性，v0.87.0 D3 / v0.94.0 D2 惯例） |
| **D3** | `pub async fn publish/collect_acks`；`Duration::from_secs(10)` | sync 方法（no_std 无 async runtime，v0.93.0 D5 惯例）；`timeout_ms: u64` 参数注入（默认常量 10_000，语义等价 10s） |
| **D4** | `OptimizationWeights(HashMap<Objective, f32>)` | `BTreeMap<Objective, f32>`（no_std alloc 无 HashMap；Objective 已 derive Ord（v0.88.0 核实）；BTreeMap 确定性迭代可重放） |
| **D5** | 蓝图 `priority: Priority` 未指明来源 | 复用 `eneros-coordinator::Priority`（v0.92.0，派生 Ord 序即优先级序；§5.5 防重复造轮子，不重新定义） |
| **D6** | 蓝图 `ModelRef` / `LocalState` / `CloudError` 未定义 | MVP 最小定义：`ModelRef { model_id: u64, version: u32 }`、`LocalState { edge_id: u64, max_capacity_mw: f32 }`、`CloudError { BroadcastFailed }`（单变体，重试耗尽即广播失败） |
| **D7** | `EdgeAck.reason: Option<String>` | `Option<RejectReason>`（结构化无堆字符串，机读审计；RejectReason 2 变体 `SafetyWeightTooLow / ExceedsCapacity`，与蓝图关键代码一致） |
| **D8** | `CloudChannel` 为蓝图隐式 async 通道 | 本地 sync trait `CloudChannel { broadcast, collect_acks }` + `MockCloudChannel`（v0.86.0 D11 BidPublisher 模式；Socket v0.29.0 / DDS 适配器后续注入，不在本版本） |
| **D9** | §9 可观测要求"Ack/拒绝 metric"；§6.5"网络断开 → 重连补发" | `StrategyPublisher` 4 个 pub 计数器 `published_count` / `ack_count` / `reject_count` / `retry_count` + `pending: Vec<Strategy>` 待补发队列与 `republish_pending() -> u32`（补发成功数） |
| **D10** | 蓝图硬编码 `0.5` 安全门限 | 命名常量 `SAFETY_WEIGHT_MIN: f32 = 0.5`；safety weight **缺失或 NaN 按 0.0 → 拒绝**（安全侧默认拒绝，宁拒勿放） |
| **D11** | 测试 `tests/strategy_push.rs` | crate 内嵌 `#[cfg(test)]` 40 测试（v0.87.0~v0.94.0 项目惯例，不新增 tests/ 文件；集成场景以 Mock 故障注入覆盖） |
| **D12** | 蓝图未覆盖 NaN | NaN 防御（v0.88.0 C140 / v0.94.0 D12 教训）：weight 非有限 → 0.0（触发安全拒绝）；DR `target_mw` 非有限 → `ExceedsCapacity`；`max_capacity_mw` 非有限或 ≤0 → 一切 DR 策略拒绝（安全侧） |

## ADDED Requirements

### Requirement: 策略数据结构与边缘安全校验

系统 SHALL 提供（全部 no_std + alloc 兼容）：`Strategy { strategy_id: u64, version: u32, targets: Vec<u64>, content: StrategyContent, deadline: u64, priority: Priority }`（Debug/Clone/PartialEq）、`StrategyContent { OptimizationWeights(BTreeMap<Objective, f32>), PriceForecast(Vec<PricePoint>), DrResponse(DrSignal), ModelUpdate(ModelRef) }`（Debug/Clone/PartialEq）、`ModelRef { model_id: u64, version: u32 }`（Debug/Clone/Copy/PartialEq/Eq/Default）、`EdgeAck { strategy_id: u64, edge_id: u64, accepted: bool, reason: Option<RejectReason> }`（Debug/Clone/Copy/PartialEq）、`RejectReason { SafetyWeightTooLow, ExceedsCapacity }`（Debug/Clone/Copy/PartialEq/Eq）、`LocalState { edge_id: u64, max_capacity_mw: f32 }`（Debug/Clone/Copy/PartialEq/Default）；`Objective`/`PricePoint`/`DrSignal` 复用 `eneros-energy-market-agent`，`Priority` 复用 `eneros-coordinator`（不重复定义）。

系统 SHALL 提供 `validate_strategy(strategy: &Strategy, local_state: &LocalState) -> Result<(), RejectReason>`（蓝图 §4.5 落地）：`OptimizationWeights` → safety weight（缺失/非有限按 0.0，D10/D12）< `SAFETY_WEIGHT_MIN`(0.5) → `Err(SafetyWeightTooLow)`；`DrResponse` → `target_mw` 非有限或 `abs() > max_capacity_mw` → `Err(ExceedsCapacity)`（`max_capacity_mw` 非有限或 ≤0 → 一切 DR 拒绝，D12）；`PriceForecast`/`ModelUpdate` → `Ok(())`。

#### Scenario: 安全校验通过与拒绝（蓝图 §4.5）

- **WHEN** weights 含 `Safety: 0.6`，local_state 任意
- **THEN** `Ok(())`
- **WHEN** weights 含 `Safety: 0.4`，或 Safety 缺失，或 Safety 为 NaN
- **THEN** `Err(SafetyWeightTooLow)`（缺失/NaN 按 0.0，安全侧默认拒绝）
- **WHEN** DR `target_mw = 15.0`，`max_capacity_mw = 10.0`（或 target NaN，或 capacity ≤0）
- **THEN** `Err(ExceedsCapacity)`

### Requirement: 云边通道抽象

系统 SHALL 提供 `CloudError { BroadcastFailed }`（Debug/Clone/Copy/PartialEq/Eq）与 sync trait `CloudChannel`：`fn broadcast(&mut self, strategy: &Strategy) -> Result<(), CloudError>`、`fn collect_acks(&mut self, strategy_id: u64, timeout_ms: u64) -> Vec<EdgeAck>`（不要求 Send + Sync，no_std 单线程惯例）；`MockCloudChannel`：broadcast 记录已发策略、可配置前 N 次失败后成功（重试测试）；collect_acks 从预置 acks 过滤 `strategy_id` 返回。

#### Scenario: Mock 通道故障注入

- **WHEN** Mock 配置 `fail_times = 2`，连续 3 次 broadcast 同一策略
- **THEN** 前 2 次 `Err(BroadcastFailed)`，第 3 次 `Ok` 且策略入已发记录

### Requirement: 策略发布器（重试 + 断网补发 + 可观测）

系统 SHALL 提供 `StrategyPublisher { channel: Box<dyn CloudChannel>, max_retries: u32, published_count, retry_count, ack_count, reject_count, pending: Vec<Strategy> }`（字段全 pub，D9）：
- `new(channel)`（max_retries = `DEFAULT_MAX_RETRIES`(3)，计数器全零，pending 空）
- `publish(&mut self, strategy) -> Result<(), CloudError>`：至多 `max_retries` 次尝试（每次失败 `retry_count += 1`）；成功 → `published_count += 1` + `Ok`；耗尽 → 策略克隆入 `pending`（断网补发，§6.5）+ `Err(BroadcastFailed)`
- `republish_pending(&mut self) -> u32`：逐条重发 pending（每条仍限 max_retries），成功补发数作为返回值，成功者从 pending 移除，失败者保留
- `collect_acks(&mut self, strategy_id, timeout_ms) -> Vec<EdgeAck>`：委托 channel；`ack_count += accepted 数`，`reject_count += rejected 数`

#### Scenario: 下发重试与断网补发（蓝图 §4.4/§6.5）

- **WHEN** channel 前 1 次 broadcast 失败，`publish` 一策略
- **THEN** 第 2 次成功：`Ok`、`published_count == 1`、`retry_count == 1`、pending 空
- **WHEN** channel 恒失败，`publish` 一策略（max_retries=3）
- **THEN** `Err(BroadcastFailed)`、`retry_count == 3`、策略入 pending（len=1）；channel 恢复后 `republish_pending()` → 返回 1、pending 清空、`published_count == 1`

#### Scenario: Ack 收集与拒绝可观测（蓝图 §9）

- **WHEN** channel 预置 3 条 ack（2 accepted / 1 rejected+reason），`collect_acks(id, 10_000)`
- **THEN** 返回 3 条；`ack_count == 2`、`reject_count == 1`

### Requirement: 云端连接配置

系统 SHALL 提供 `configs/cloud_coordinator.toml`：`[cloud_coordinator]` 段（`ack_timeout_ms = 10000` / `max_retries = 3` / `safety_weight_min = 0.5` / 云端 endpoint 占位），中文注释含：下发延迟 <1s（§7.2，集成阶段验收）/ 策略非强制边缘可拒绝（§5.2，边缘主权 §9）/ 断网重连补发（§6.5/§9 可靠，D9 pending）/ 策略版本化（§5.2/§8.4 多版本兼容）/ NaN 防御（D10/D12）/ 新策略类型可扩展（§9 可扩展，StrategyContent 加变体）。

## MODIFIED Requirements

### Requirement: workspace 集成与版本

根 `Cargo.toml`：`members` 追加 `"crates/agents/cloud-coordinator"`（既有 70 成员零改动），`[workspace.package] version = "0.95.0"`。`Makefile` / `ci.yml` 版本注释同步。`ci/src/gate.rs` clippy/test 注释串尾追加 v0.95.0 类型清单（Strategy / StrategyContent / ModelRef / EdgeAck / RejectReason / LocalState / CloudChannel / MockCloudChannel / CloudError / StrategyPublisher / validate_strategy）。**既有 crate 全部零改动**。

## REMOVED Requirements

无。

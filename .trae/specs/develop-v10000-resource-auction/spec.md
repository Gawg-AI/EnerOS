# v0.100.0 资源争抢竞价机制 Spec

> **蓝图**：`蓝图/phase2.md` §v0.100.0（5278~5454 行，P2-E 第 4 版）
> **Crate**：`eneros-federation`（`crates/agents/federation/src/{auction,bid_book,matching}.rs`，既有 crate 追加 3 模块）
> **变更 ID**：develop-v10000-resource-auction

## Why

共享馈线容量场景下多个域（Agent）对同一资源报价争抢，需要公平的**统一价格拍卖撮合**机制：报价收集 → 双向排序撮合 → 安全底线校验 → 出清价格 → **经 v0.99.0 联邦共识确认后生效**，解决域间资源争抢公平性与跨域决策可信性（蓝图 §1/§4.3）。

## What Changes

- `crates/agents/federation/src/bid_book.rs` — **新增**：`AgentId`/`Price`/`Qty`/`BidOrder`/`AskOrder`/`OrderBook`/`Match`/`MatchResult`/`AuctionError` + 确定性序列化（`to_bytes`）与 SM3 `match_digest`
- `crates/agents/federation/src/matching.rs` — **新增**：撮合纯函数 `match_book(book, safety_floor) -> MatchResult`（双向排序/部分成交/安全底线/出清价）
- `crates/agents/federation/src/auction.rs` — **新增**：`AuctionEngine`（submit_bid/submit_ask 校验、match_orders、clear_book、4 计数器 + last_clearing_price 可观测）
- `crates/agents/federation/src/lib.rs` — **修改**：3 模块声明 + 重导出 + crate 文档追加 v0.100.0 说明与 D1~D13 偏差表（既有 7 模块零改动）
- `crates/agents/federation/Cargo.toml` — **修改**：description 追加 v0.100.0；依赖不变（仍仅 eneros-crypto，零新增第三方依赖）
- `configs/federation-auction.toml` — **新增**：`[auction]` 段（safety_floor / max_price + 中文注释 ≥6 点）
- `docs/agents/auction-design.md` — **新增**：12 章节设计文档 + ≥2 Mermaid 图
- 根目录 4 文件版本同步 0.99.0 → 0.100.0（`Cargo.toml`/`Makefile`/`ci.yml`/`gate.rs` 注释）
- **30 个单元测试** TB1~TB10（bid_book.rs）/ TM11~TM22（matching.rs）/ TA23~TA30（auction.rs）（src 内嵌 `#[cfg(test)]`，项目惯例）
- **无 BREAKING**：既有全部 crate 公共 API 零改动

## Impact

- Affected specs：v0.99.0 联邦共识（下游消费：MatchResult 经共识确认）；v0.86.0 报价生成（上游报价意图经适配转为 BidOrder/AskOrder）；v0.101.0 孤岛模式（下游解锁）
- Affected code：`crates/agents/federation/`（纯增量 3 模块）、根目录 4 版本文件、`configs/`、`docs/agents/`

## ADDED Requirements

### Requirement: 订单簿与确定性类型（bid_book.rs）

系统 SHALL 提供定点化订单簿类型：价格/数量用 `u64` 定点（D1），Agent 标识用 `u64`（D2），撮合结果可确定性序列化并求 SM3 摘要供共识确认（D10）。

#### Scenario: 订单提交校验

- **WHEN** `OrderBook::submit_bid(BidOrder{ price: 0 })` 或 `qty: 0`
- **THEN** 返回 `Err(AuctionError::InvalidOrder)`，订单不入簿

#### Scenario: 确定性序列化

- **WHEN** 同一 `MatchResult` 在两个节点分别 `to_bytes()` + `match_digest()`
- **THEN** 字节序列与 SM3 摘要逐字节一致（共识跨节点一致性前提）

### Requirement: 统一价格撮合（matching.rs）

系统 SHALL 实现蓝图 §4.5 双向撮合：bids 价格降序、asks 价格升序，价格交叉（bid ≥ ask）即成交，成交价为两者均值（定点向下取整，D13），支持部分成交；成交价低于 `safety_floor` 时停止撮合（蓝图 §4.4 安全底线拒绝）。

#### Scenario: 基本撮合

- **WHEN** bids=[(1, 100, 10)], asks=[(2, 90, 8)]（price 毫元、qty Wh）
- **THEN** 成交 1 笔：buyer=1, seller=2, price=95, qty=8；clearing_price=95

#### Scenario: 安全底线拒绝

- **WHEN** 全部可成交对的成交价 < safety_floor
- **THEN** `MatchResult.matches` 为空，clearing_price=0（无成交兜底，蓝图 §4.4/§9 可靠）

#### Scenario: 部分成交

- **WHEN** bid qty=10、ask qty=6
- **THEN** 成交 qty=6，bid 剩余 4 继续参与后续撮合（快照语义不修改原簿，D12）

### Requirement: 拍卖引擎与可观测（auction.rs）

系统 SHALL 提供 `AuctionEngine`：持有 OrderBook + safety_floor + max_price 限价（D8，蓝图 §8.5 坑点对策），submit 校验、快照撮合、轮次清簿，并暴露 4 计数器与最近出清价（D9，蓝图 §9 可观测）。

#### Scenario: 限价拒绝

- **WHEN** `max_price = Some(200)` 且提交 price=250 的订单
- **THEN** 返回 `Err(AuctionError::PriceCapExceeded)`，rejected_count+=1

#### Scenario: 计数器

- **WHEN** 一轮 submit 3 bid + 2 ask 后 match_orders 成交 2 笔
- **THEN** bid_count=3, ask_count=2, match_count=2, last_clearing_price=末笔成交价

### Requirement: 共识确认集成（v0.99.0 复用）

系统 SHALL 使撮合结果可经 v0.99.0 联邦共识确认：主节点将 `MatchResult` 序列化作 consensus 请求提交，4 节点对 digest 达成 committed（蓝图 §4.3 末步"共识确认"）。

#### Scenario: 4 节点共识确认撮合结果

- **WHEN** MockConsensusBus 4 节点集群，主节点 `submit(match_result.to_bytes())` 并驱动 poll
- **THEN** 全部 4 节点 `is_committed(1)==true` 且 digest == `match_digest(&result)`

### Requirement: 性能

系统 SHALL 在 1 万笔订单规模下单次撮合 < 100ms（蓝图 §7.2，host 端 `#[cfg(test)]` 用 std Instant 测量）。

## MODIFIED Requirements

### Requirement: eneros-federation crate 定位

crate 文档升级为四版本说明：v0.97.0 联邦发现 + v0.98.0 跨域通信通道 + v0.98.1 纵向加密 + v0.99.0 联邦共识 + **v0.100.0 资源争抢竞价**；`Cargo.toml` description 同步。

## REMOVED Requirements

无。

---

## 偏差声明（D1~D13，相对蓝图 §3/§4.1/§4.4/§4.5）

| 编号 | 偏差 | 理由 |
|------|------|------|
| **D1** | 蓝图 f32 price/qty → **定点 u64**（Price=毫元 1e-3 元，Qty=Wh 1e-3 kWh） | 撮合结果须经 v0.99.0 共识确认（蓝图 §4.3 末步），跨节点需逐字节一致——IEEE 浮点存在平台/编译非确定风险；定点无 NaN、字节稳定，且消除 `partial_cmp().unwrap()` panic 路径（no_std 禁 panic 惯例） |
| **D2** | `AgentId = u64`（蓝图 `agent.clone()` 暗示 String） | 项目无堆值类型惯例（v0.97.0 NodeId=u64，电力调度确定性可复现审计） |
| **D3** | 蓝图 `crates/federation/src/` → `crates/agents/federation/src/` | 记忆 §2.3.1 强制：所有 crate 归 `crates/<subsystem>/`；eneros-federation 既有 crate 增量扩展（v0.98.0~v0.99.0 同例） |
| **D4** | 蓝图 `docs/phase2/auction.md` → `docs/agents/auction-design.md` | 记忆 §2.3.3 强制：文档按方向分类，agents 子系统文档归 `docs/agents/` |
| **D5** | 蓝图 `tests/auction.rs` → src 内嵌 `#[cfg(test)]` | v0.87.0~v0.99.0 项目惯例，不新增 tests/ 文件 |
| **D6** | 撮合算法独立 `matching.rs` 纯函数 `match_book` | 蓝图文件名保留；纯函数（无引擎状态）独立可测，AuctionEngine 仅做簿管理/校验/计数 |
| **D7** | `submit_bid/submit_ask` 返回 `Result<(), AuctionError>`（蓝图为空返回） | price=0/qty=0/超限价需入簿前拒绝；蓝图 §4.4 错误处理"安全底线违反→拒绝"扩展至提交侧 |
| **D8** | `AuctionEngine` 增 `max_price: Option<Price>` 限价 | 蓝图 §8.5 坑点"价格波动大需限价"的直接对策 |
| **D9** | 增 4 计数器（bid_count/ask_count/match_count/rejected_count）+ `last_clearing_price` | 蓝图 §9 可观测"成交记录 metric"；no_std 无 log crate，metric 字段化（v0.99.0 D12 同例） |
| **D10** | 增 `MatchResult::to_bytes()` + `match_digest()`（SM3） | 蓝图 §4.3 末步"共识确认"的落地 seam；auction 模块不持有 ConsensusEngine，序列化字节交由上层 submit，保持模块独立可测 |
| **D11** | 不复用 v0.86.0 `Bid` 类型，新建 `BidOrder/AskOrder` | eneros-federation 保持仅依赖 eneros-crypto（SBOM 不变）；v0.86.0 报价意图由上层适配转换，避免 agents 子系统内横向耦合 |
| **D12** | `match_orders(&self)` 保持蓝图快照语义（不消耗簿），增 `clear_book()` 轮次重置 | 蓝图 §4.2 签名为 `&self`；轮次制拍卖需在共识确认后清簿开新轮 |
| **D13** | 成交价 `(bid+ask)/2` 定点 u64 向下取整 | 定点化配套确定性规则；取整方向全网点一致方可逐字节一致 |

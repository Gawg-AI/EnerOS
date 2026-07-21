//! # eneros-coordinator — v0.92.0 Edge Coordinator 域内仲裁 + v0.93.0 域级优化 + v0.94.0 VPP 聚合
//!
//! ## 版本目标
//!
//! Edge Coordinator（边缘协调器）的**域内仲裁**核心：对域内多个 Agent 的资源请求
//! 执行"**竞价为主 + 安全底线**"的**三级仲裁**（蓝图 §7.3）：
//!
//! ```text
//! 优先级（不可逾越）：安全(safety_critical) > deadline 紧急 > 竞价(bid)
//! ```
//!
//! 1. **安全第一级**：任一 `safety_critical` 请求存在时，安全请求内部按
//!    [`Priority`] 最高者胜出，压制一切竞价与 deadline（安全底线）。
//! 2. **deadline 第二级**：无安全请求时，紧急请求（deadline 落入 urgent 窗口，
//!    [`Claim::is_urgent`]）中最早 deadline 者胜出。
//! 3. **竞价第三级**（常态主路径）：前两级均空时，按 [`cmp_bid`] 全序比较
//!    `bid` 最高者胜出。
//!
//! 本 crate 为 Phase 2 多机联邦 **P2-D（域内协调）起点**，纯计算、零总线副作用，
//! 确定性可重放（同输入同输出），no_std 兼容。
//!
//! ## 核心类型清单
//!
//! | 模块 | 导出 | 说明 |
//! |------|------|------|
//! | [`bid`] | [`Priority`] / [`Claim`] / [`cmp_bid`] | 优先级枚举、资源请求、f32 全序比较 |
//! | [`arbiter`] | [`DomainArbiter`] / [`ArbiterPolicy`] / [`ArbitrationRequest`] / [`ArbitrationResult`] / [`ArbitrationReason`] | 三级仲裁器与请求/结果/原因类型 |
//! | [`conflict`] | [`has_safety_conflict`] / [`detect_deadlock`] | 安全冲突检测、wait-for 图死锁检测 |
//! | [`domain_optimizer`]（v0.93.0） | [`DomainOptimizer`] / [`EdgeBoxState`] / [`DomainPlan`] / [`OptError`] | 域级 LP 优化器（域平衡 + 各 box 容量约束，损耗最小 + 容量比例兜底） |
//! | [`vpp_aggregator`]（v0.94.0） | [`VppAggregator`] / [`VppResource`] / [`VppProfile`] / [`AggregatedDispatch`] / [`Allocation`] / [`VppError`] / [`ResourceType`] | VPP 聚合（容量聚合 + 出力分配 + 市场申报，复用 DomainOptimizer 与 v0.86.0 Bid 族） |
//!
//! ## v0.94.0 VPP 聚合
//!
//! 扩展本 crate 新增 `vpp_aggregator` 模块：聚合域内 DER 容量形成 [`VppProfile`]
//! （容量/爬坡，D12 sanitize 防御）→ 聚合出力控制（sync_boxes 将每在线资源映射为单设备
//! box，复用 v0.93.0 [`DomainOptimizer`] 分配 + 容量比例兜底，D8）→ 市场申报（复用
//! v0.86.0 `Bid` 族生成 Sell 报价）。资源不足确定性拒绝（D10），离线资源即时排除聚合
//! 与分配（D6）。Surgical 追加：v0.92.0 + v0.93.0 既有公共 API 全部保留。
//!
//! ## v0.94.0 偏差记录（D1~D12）
//!
//! - **D1**：模块位于既有 `crates/agents/coordinator/`（§2.3.1 硬规则，同 crate 追加）。
//! - **D2**：`resource_id: u64` + `BTreeMap<u64, _>`（无堆字符串 + 聚合/分配顺序可重放）。
//! - **D3**：sync `dispatch(&mut self, market, target_mw, now_ms)`（no_std 无 async）。
//! - **D4**：`AggregatedDispatch.timestamp = now_ms`；`aggregate(&mut self)` 因计数器更新
//!   （内部 profile 计算为私有 `&self` 免计数）。
//! - **D5**：`MarketData` 未派生 Default → dispatch 显式注入 `market: &MarketData`。
//! - **D6**：`VppResource.online` + `set_online`/`set_available`（离线排除聚合与分配，状态保留）。
//! - **D7**：`VppResource` 增加 `efficiency: f32`（损耗最小目标可区分高效/低效 DER）。
//! - **D8**：复用 `DomainOptimizer`：sync_boxes 单设备 box 映射（box_id=device_id=resource_id，
//!   `soc=1.0`）；"重新优化"落地为内建容量比例兜底。
//! - **D9**：3 个 pub 计数器 `aggregate_count`/`dispatch_count`/`reject_count`。
//! - **D10**：资源不足确定性拒绝——`|target| > available` → `Err(InsufficientCapacity)`（abs 判定）。
//! - **D11**：`ramp_down = ramp_up` 对称；ramp 非有限或 <0 → 按 0 计入（不阻断调度）。
//! - **D12**：NaN 防御——capacity 非有限/≤0 排除资源；available 非有限→0 clamp [0,cap]；
//!   eff NaN→0.5 clamp [0,1]；price 非有限→0+margin。复用 domain_optimizer sanitize（pub(crate)）。
//!
//! ## v0.93.0 域级优化
//!
//! 扩展本 crate 新增 `domain_optimizer` 模块：收集域内 Edge Box 状态（BTreeMap 确定性，
//! D2）→ 构建域级 LP（行 0 域平衡 `Σp = target_mw`、每参与 box 一行容量约束
//! `Σ_{i∈box} p_i ≤ capacity_mw`、目标 Minimize `Σ(1−eff_i)·p_i`，D7）→ Solver 求解
//! 按 box 聚合 [`DispatchPlan`] 下发；Solver Err / Infeasible / 解长度不符回退容量比例
//! 分摊兜底（box 间按 `capacity_mw` 比例 + box 内复用 v0.87.0 `equal_split`，D10）。
//! 净收益 `total_revenue = price × (total_power − total_loss)`（D12），支撑蓝图 §7.2
//! "收益优于单机"可判定。Surgical 追加：v0.92.0 既有公共 API 全部保留。
//!
//! ## v0.93.0 偏差记录（D1~D12）
//!
//! - **D1**：模块位于既有 `crates/agents/coordinator/`（§2.3.1 硬规则，同 crate 追加）。
//! - **D2**：`box_id: u64` + `BTreeMap<u64, _>`（无堆字符串 + LP 列映射/下发顺序可重放）。
//! - **D3**：`socs: BTreeMap<u64, f32>`（DeviceId=u64，同 v0.87.0 dispatch 签名）。
//! - **D4**：`Box<dyn Solver>`（no_std 单线程无共享所有权，v0.87.0 D5 惯例）。
//! - **D5**：sync `optimize(&mut self, market, target_mw, now_ms)`（no_std 无 async）。
//! - **D6**：`now_ms: u64` 外部时间注入；`DomainPlan.timestamp = now_ms`。
//! - **D7**：有实际域级耦合的 LP（域平衡行 + 各 box 容量行）；蓝图 `optimize(market)`
//!   无 target → 增加 `target_mw: f32` 参数注入。
//! - **D8**：`EdgeBoxState.online` + `set_online`（离线从 LP 与 DomainPlan 排除，状态保留）。
//! - **D9**：3 个 pub 计数器 `optimize_count`/`fallback_count`/`empty_count`（v0.92.0 D9 惯例）。
//! - **D10**：确定性容量比例兜底（不迭代重试 LP）；`objective_value = 0.0`（失败为兜底非错误）。
//! - **D11**：`target_mw > Σ在线 capacity` → clamp 后建 LP（构造保证不超发）；target 非有限
//!   → `Err(InvalidTarget)`。
//! - **D12**：净收益公式 + NaN 防御（soc NaN/≤0 跳过设备；capacity 非有限/≤0 排除 box；
//!   eff NaN→0.5 clamp [0,1]；price 非有限→0.0）。
//!
//! ## v0.92.0 偏差记录（D1~D12）
//!
//! - **D1**：零外部依赖（仅 `eneros-agent` 提供 [`AgentId`]），纯计算无总线副作用。
//! - **D2**：`resource_id` 等标识字符串全部 `&'static str`（无堆分配，同 v0.90.0/v0.91.0 D2 惯例）。
//! - **D3**：`deadline` / `timestamp` 统一 `u64` ms，外部时间注入（全 crate 统一 u64 ms 惯例），不读系统时钟。
//! - **D4**：[`DomainArbiter`] 不要求 Send + Sync（no_std 单线程惯例）。
//! - **D5**：确定性仲裁——同 priority / 同 deadline / 同 bid 时取**输入序首个**（手写循环，不用 `max_by_key`，保证可重放）。
//! - **D6**：`timestamp` 一律回显调用方传入的 `now_ms`（结果时间戳与仲裁时刻一致，便于审计重放）。
//! - **D7**：[`Priority`] 变体按 `Low < Normal < High < Critical < Safety` **升序声明**，
//!   使派生 `Ord` 序即优先级序（`Safety` 最大），默认 `Normal`。
//! - **D8**：[`ArbitrationResult`] 携带 `reason` + `conflict` 标记（仲裁可解释性，蓝图 §7.3 审计要求）。
//! - **D9**：[`DomainArbiter`] 6 个计数器字段全 `pub`（仲裁路径可观测：total/safety/deadline/bid/empty/conflict）。
//! - **D10**：安全冲突定义——`safety_critical` 请求数 ≥ 2 即 `conflict = true`
//!   （仍返回胜出者，冲突仅作标记与计数，不阻断仲裁）。
//! - **D11**：[`cmp_bid`] 为 f32 **全序**比较：双 NaN → Equal；NaN 恒最低；
//!   ±Inf 保留偏序。解决 `f32: !Ord` 无法直接 `max` 的问题，NaN 永不胜出。
//! - **D12**：urgent 判定窗口——`deadline < now_ms.saturating_add(window_ms)`（**严格 <**，
//!   过去 deadline 必 urgent；边界 `deadline == now + window` 不 urgent）；
//!   默认窗口 [`ArbiterPolicy`]::urgent_window_ms = 1000ms；saturating_add 防 u64 溢出 panic。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod arbiter;
pub mod bid;
pub mod conflict;
pub mod domain_optimizer;
pub mod vpp_aggregator;

pub use arbiter::{
    ArbiterPolicy, ArbitrationReason, ArbitrationRequest, ArbitrationResult, DomainArbiter,
};
pub use bid::{cmp_bid, Claim, Priority};
pub use conflict::{detect_deadlock, has_safety_conflict};
pub use domain_optimizer::{DomainOptimizer, DomainPlan, EdgeBoxState, OptError};
pub use vpp_aggregator::{
    AggregatedDispatch, Allocation, ResourceType, VppAggregator, VppError, VppProfile, VppResource,
};

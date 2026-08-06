//! EnerOS v0.124.0 Agent 热更新（P2-K OTA 与 Recovery 第 2 版）.
//!
//! 在不停止系统运行的情况下实现 Agent 模块的热加载、热替换、热卸载，支持新旧版本
//! 平滑过渡（请求排空 + 原子切换 + 健康验证 + 失败回滚），满足电力控制 7×24 不间断
//! 运行下的「无感升级」，为 v0.126.0 VPP 出口提供在线升级能力（蓝图 §1）。
//!
//! # 核心类型
//!
//! - [`ArcHotSwap`] — 基于 `alloc::sync::Arc` + `core::sync::atomic::AtomicPtr` 的
//!   真实原子热切换器（D3）：`load` 读当前实例，`swap` 原子换入新实例并返回旧 `Arc`，
//!   旧实例在全部持有方释放后引用计数归零自动卸载（零中断）
//! - [`AgentHotUpdater`] / [`AgentHotUpdaterImpl`] — 热更新状态机：
//!   prepare（ABI 真实比对 + 模块字节登记）→ drain（单步排空评估）→ swap（原子切换）
//!   → verify（SM3 模块哈希 + 健康探针）→ 失败 rollback
//! - [`HotUpdateState`] / [`AgentVersion`] / [`HotUpdateContext`] / [`HotUpdateError`]
//!   — 七态状态机与数据模型
//!
//! # 偏差声明（D1~D7，相对蓝图 `phase2.md` §v0.124.0）
//!
//! | 编号 | 偏差 | 理由 |
//! |------|------|------|
//! | **D1** | 蓝图 `crate::current_time_ms()` 全局时间函数 → `now_ms` 参数传入（prepare/drain/start_ms） | no_std 无系统时间；集成层由 v0.12.0 RTC 供给 |
//! | **D2** | 蓝图 drain 忙等 `loop`（内「短暂休眠（简化）」）→ 单步评估语义：活跃=0 → Ok 可 swap；`now_ms - start_ms > timeout` → `DrainTimeout{active}`；否则保持 Draining 由调用方重试 | no_std 无休眠原语且忙等不可测试；调用方持有重试节奏 |
//! | **D3** | 蓝图 `ArcHotSwap<T>` stub（`_phantom` + 原样返回）→ 真实 `Arc` + `AtomicPtr` 原子交换（`load`/`swap`/`Drop` 完整生命周期） | 蓝图占位实现不满足「热切换机制（真实原子交换，非 stub）」需求 |
//! | **D4** | 蓝图 verify `healthy = true` 简化 → 真实 SM3 模块哈希校验（复用 eneros-crypto `sm3_hash` + `ct_eq` 常时比较）+ 可注册健康探针 `fn(u32) -> bool`（`with_probe`，默认恒 true） | 验收 §7.3 要求模块哈希验证；禁止重复造轮子（记忆 §5.5） |
//! | **D5** | 蓝图 `crates/ota/` → `crates/agents/hot-update/`（eneros-agent-hot-update） | 记忆 §2.3.1 强制 crate 归 `crates/<subsystem>/`；与 v0.35.0 Agent 生命周期、v0.111.0 model-ota 同子系统 |
//! | **D6** | 蓝图 prepare 以合成 old_version（abi 复制自 new_version）比对导致 AbiIncompatible 永不触发 → 新增 `register_agent(agent_id, current)` 登记当前版本，ABI 与登记版本真实比对 | 蓝图代码缺陷修复，使 ABI 兼容检查真实生效 |
//! | **D7** | 蓝图活跃请求计数为私有 increment/decrement → 公开 `request_started`/`request_finished`（AtomicU32，Relaxed） | 路由层需在请求进出时调用，私有方法不可达 |
//! | **D8** | `slot_load` 由私有 → 公开（返回 `Option<Arc<AgentVersion>>`） | 路由层把新请求路由到当前版本、集成测试断言 swap 前后路由目标均须经此读取；蓝图未给出路由读取入口 |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。仅使用
//! `core::*` 与 `alloc::*`，不调用 `panic!` / `todo!` / `unimplemented!` /
//! `unwrap()`，时间戳一律经 `now_ms` 参数传入，可交叉编译到
//! `aarch64-unknown-none`。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod agent_hot;
pub mod hot_swap;

pub use agent_hot::{
    AgentHotUpdater, AgentHotUpdaterImpl, AgentVersion, HotUpdateContext, HotUpdateError,
    HotUpdateState,
};
pub use hot_swap::ArcHotSwap;

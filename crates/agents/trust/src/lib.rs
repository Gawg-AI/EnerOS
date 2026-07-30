//! EnerOS Agent 信誉体系 (v0.168.0) — P3-K Agent 信任与认证.
//!
//! 基于行为历史动态计算 Trust Score，支持信誉分级与自动降级。
//! 高信任 Agent 获得更多资源，低信任 Agent 被限制/封禁。
//!
//! # 核心模块
//!
//! - [`types`] — AgentId、TrustScore、TrustGrade、AgentAction 等核心类型
//! - [`scoring`] — 评分规则（可配置，7 种行为类型）
//! - [`history`] — 行为历史记录与信任分维护
//! - [`adjuster`] — 动态调整器（时间衰减 + 手动调整）
//! - [`manager`] — TrustSystem trait 实现（统一管理）
//!
//! # 评分规则（蓝图 §4.2）
//!
//! | 行为类型 | 分数变化 | 说明 |
//! |---------|---------|------|
//! | 任务完成 | +5 | 成功完成任务 |
//! | 任务失败 | -3 | 任务失败 |
//! | 越权访问 | -50 | 尝试越权 |
//! | 恶意行为 | -200 | 恶意操作 |
//! | 崩溃 | -10 | Agent 崩溃 |
//! | 恢复 | +2 | 成功恢复 |
//! | 协作 | +3 | 协助其他 Agent |
//!
//! # 信誉等级
//!
//! | 等级 | 分数范围 | 权限 |
//! |------|---------|------|
//! | Platinum | 900-1000 | 全部能力 |
//! | Gold | 700-899 | 标准能力 |
//! | Silver | 400-699 | 基础能力（初始） |
//! | Bronze | 100-399 | 受限能力 |
//! | Blocked | 0-99 | 封禁 |
//!
//! # 偏差声明
//!
//! | 编号 | 偏差 | 理由 |
//! |------|------|------|
//! | **D1** | 蓝图 `kernel/src/trust/` → `crates/agents/trust/`（eneros-trust） | 记忆 §2.3.1 强制 crate 归 `crates/<subsystem>/`；信誉体系属 Agent 子系统 |
//! | **D2** | 蓝图 `HashMap` → `BTreeMap` | no_std 兼容（alloc::collections::BTreeMap），无 std HashMap |
//! | **D3** | 蓝图测试 `kernel/tests/trust/` → src 内嵌 `#[cfg(test)]` | 项目惯例 |
//! | **D4** | 蓝图 `sys::get_time()` → 外部注入 `current_time` | no_std 无系统时钟，时间由集成层供给 |
//! | **D5** | 新增时间衰减机制 | 蓝图 §5 技术交底要求"防刷分"，衰减向初始分回归 |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 零第三方依赖，零 unsafe，可交叉编译到 `aarch64-unknown-none`。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod adjuster;
pub mod history;
pub mod manager;
pub mod scoring;
pub mod types;

pub use adjuster::{DecayConfig, TrustAdjuster};
pub use history::{TrustHistory, MAX_HISTORY_SIZE};
pub use manager::{TrustSystem, TrustSystemImpl};
pub use scoring::ScoringRules;
pub use types::{
    ActionImpact, ActionType, AgentAction, AgentId, TrustEvent, TrustGrade, TrustScore, TrustStats,
    TrustTrend, BLOCK_THRESHOLD, INITIAL_SCORE, MAX_SCORE, MIN_SCORE,
};

/// 信誉体系错误枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustError {
    /// Agent 未注册.
    AgentNotRegistered,
    /// Agent 已被封禁.
    AgentBlocked,
    /// 分数超出范围.
    ScoreOutOfRange,
    /// 无效操作.
    InvalidOperation,
}

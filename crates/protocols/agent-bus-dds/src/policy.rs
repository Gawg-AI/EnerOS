//! EnerOS v0.77.0 路由策略层（routing policy & capability verification）.
//!
//! 在 v0.76.0 语义层（TopicRegistry/QosPolicy）之上引入消息路由策略：
//! - 定义 Agent 唯一标识（[`AgentId`]）、权限（[`Permission`]）、丢弃原因（[`DropReason`]）
//! - 定义路由策略（[`RoutingPolicy`]）控制是否要求 token、是否启用优先级抢占、速率限制
//! - 定义能力验证器 trait（[`CapabilityVerifier`]），为 v0.39.0 能力 Token 模型解耦
//!
//! # 偏差声明
//!
//! - **D7**：`CapabilityVerifier` trait 不带 `Send + Sync` bound（no_std 单线程场景，
//!   且回调由路由器在 `&mut self` 下同步调用，无需跨线程传递）
//! - **D10**：使用 trait 抽象解耦能力模型（避免在 v0.77.0 引入对 v0.39.0 `eneros-agent` crate 的依赖；
//!   默认提供 [`MockCapabilityVerifier`] 始终放行，真实能力校验由后续版本注入）
//! - **D12**：`AgentId` 为本地 `u64` newtype（非 v0.39.0 的 `u128`），避免引入 `eneros-agent` crate 依赖

use alloc::format;
use alloc::string::{String, ToString};
use core::fmt;

/// Agent 唯一标识.
///
/// **D12**：本地 `u64` newtype（非 v0.39.0 的 `u128`），避免引入 `eneros-agent` crate 依赖。
/// 在 v0.39.0 能力 Token 模型上线后，可通过 wrapper 适配 `u128` 全局 ID。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AgentId(pub u64);

/// 权限类型.
///
/// 用于 [`CapabilityVerifier`] 校验 agent 对某 topic pattern 的访问权限。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    /// 发布权限.
    Publish,
    /// 订阅权限.
    Subscribe,
}

/// 消息丢弃原因.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropReason {
    /// 未授权（capability 校验失败）.
    Unauthorized,
    /// 触发速率限制.
    RateLimited,
    /// Topic 非法.
    InvalidTopic,
    /// 能力 Token 过期.
    TokenExpired,
}

impl DropReason {
    /// 返回丢弃原因的静态字符串名（用于统计聚合）.
    pub fn reason_name(&self) -> &'static str {
        match self {
            DropReason::Unauthorized => "Unauthorized",
            DropReason::RateLimited => "RateLimited",
            DropReason::InvalidTopic => "InvalidTopic",
            DropReason::TokenExpired => "TokenExpired",
        }
    }
}

/// 路由策略.
///
/// 控制消息路由器的行为：是否要求发布/订阅 token、是否启用优先级抢占、是否启用速率限制。
#[derive(Debug, Clone, Default)]
pub struct RoutingPolicy {
    /// 是否要求发布消息时校验 publish token.
    pub require_publish_token: bool,
    /// 是否要求订阅消息时校验 subscribe token.
    pub require_subscribe_token: bool,
    /// 是否启用优先级抢占（高优先级消息可抢占低优先级队列）.
    pub priority_preempt: bool,
    /// 每个 agent 的速率限制（每秒消息数），`None` 表示不限制.
    pub rate_limit_per_agent: Option<u32>,
}

impl RoutingPolicy {
    /// 严格策略：所有校验启用，速率限制 100/s.
    pub fn strict() -> Self {
        Self {
            require_publish_token: true,
            require_subscribe_token: true,
            priority_preempt: true,
            rate_limit_per_agent: Some(100),
        }
    }
}

/// 路由错误.
#[derive(Debug)]
pub enum RouteError {
    /// Topic pattern 非法.
    InvalidPattern(String),
    /// 消息被丢弃（含原因）.
    Dropped(DropReason),
    /// Topic 未注册或非法.
    InvalidTopic(String),
}

impl fmt::Display for RouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RouteError::InvalidPattern(msg) => write!(f, "topic pattern 非法: {}", msg),
            RouteError::Dropped(reason) => write!(f, "消息被丢弃: {}", reason.reason_name()),
            RouteError::InvalidTopic(msg) => write!(f, "topic 非法: {}", msg),
        }
    }
}

impl core::error::Error for RouteError {}

/// 路由决策.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteDecision {
    /// 投递消息（携带优先级）.
    Deliver { priority: i32 },
    /// 丢弃消息（携带原因）.
    Drop { reason: DropReason },
}

/// 能力验证器 trait.
///
/// 抽象 v0.39.0 能力 Token 模型，避免在 v0.77.0 引入对 `eneros-agent` crate 的依赖。
///
/// **D7**：不带 `Send + Sync` bound（no_std 单线程场景，回调由路由器在 `&mut self` 下同步调用）。
/// **D10**：trait 抽象解耦能力模型，默认实现 [`MockCapabilityVerifier`] 始终放行。
pub trait CapabilityVerifier {
    /// 校验 agent 对某 topic pattern 的权限.
    ///
    /// 返回 `Ok(())` 表示允许，`Err(reason)` 表示拒绝并附带丢弃原因。
    fn verify(&self, perm: Permission, agent: AgentId, pattern: &str) -> Result<(), DropReason>;
}

/// Mock 能力验证器（始终放行）.
///
/// **D10**：默认实现，用于 v0.77.0 路由器在不依赖 v0.39.0 能力 Token 模型时运行。
/// 真实能力校验由 [`CapabilityTicketVerifier`]（v0.208.0 P0-11）提供。
///
/// 21-B B2：mock 仅测试/显式 feature（agent-bus-dds 已降级为
/// out-of-v1.0，agent 总线由 MQTT 承担——ADR-021）。
#[cfg(any(test, feature = "mock"))]
#[derive(Debug, Default)]
pub struct MockCapabilityVerifier;

#[cfg(any(test, feature = "mock"))]
impl CapabilityVerifier for MockCapabilityVerifier {
    fn verify(&self, _perm: Permission, _agent: AgentId, _pattern: &str) -> Result<(), DropReason> {
        Ok(())
    }
}

/// 一条能力授权（agent 对 topic pattern 的权限）。
///
/// v0.208.0（P0-11）：真实能力票据校验——授权表由系统配置注入。
/// `pattern` 支持前缀通配：`"sensor/#"` 匹配 `"sensor/a"`、`"sensor/a/b"` 等。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityGrant {
    /// 被授权 agent。
    pub agent: AgentId,
    /// 权限（Publish / Subscribe）。
    pub permission: Permission,
    /// topic pattern（`#` 通配前缀）。
    pub pattern: alloc::string::String,
}

impl CapabilityGrant {
    /// 创建授权条目。
    pub fn new(agent: AgentId, permission: Permission, pattern: &str) -> Self {
        Self {
            agent,
            permission,
            pattern: pattern.to_string(),
        }
    }

    /// 判断 topic 是否匹配本授权的 pattern。
    pub fn matches(&self, topic: &str) -> bool {
        if let Some(base) = self.pattern.strip_suffix('#') {
            // "sensor/#"：匹配 "sensor" 前缀的任意子路径。
            let base = base.trim_end_matches('/');
            topic == base || topic.starts_with(&format!("{base}/"))
        } else {
            self.pattern == topic
        }
    }
}

/// 真实能力验证器（授权票据表驱动，P0-11）。
///
/// 基于显式 [`CapabilityGrant`] 授权表做真实决策：agent 未获授权访问
/// topic → `Err(Unauthorized)`。空授权表默认拒绝所有（fail-closed）。
#[derive(Debug, Clone, Default)]
pub struct CapabilityTicketVerifier {
    grants: alloc::vec::Vec<CapabilityGrant>,
}

impl CapabilityTicketVerifier {
    /// 创建验证器（空授权表：默认拒绝所有——fail-closed）。
    pub fn new() -> Self {
        Self {
            grants: alloc::vec::Vec::new(),
        }
    }

    /// 注入授权条目。
    pub fn grant(&mut self, grant: CapabilityGrant) {
        self.grants.push(grant);
    }

    /// 批量注入授权。
    pub fn with_grants(mut self, grants: alloc::vec::Vec<CapabilityGrant>) -> Self {
        self.grants = grants;
        self
    }
}

impl CapabilityVerifier for CapabilityTicketVerifier {
    fn verify(&self, perm: Permission, agent: AgentId, pattern: &str) -> Result<(), DropReason> {
        let granted = self
            .grants
            .iter()
            .any(|g| g.agent == agent && g.permission == perm && g.matches(pattern));
        if granted {
            Ok(())
        } else {
            Err(DropReason::Unauthorized)
        }
    }
}

//! EnerOS v0.77.0 消息路由器（message router）.
//!
//! 在 v0.76.0 语义层（TopicRegistry/TopicSpec/QosPolicy）与 v0.77.0 路由策略层
//! （RoutingPolicy/CapabilityVerifier）之上实现消息路由：
//! - 订阅管理（subscribe/unsubscribe，按 pattern 索引）
//! - 消息分发（dispatch，按 pattern 匹配多订阅者回调）
//! - 路由统计（RouterStats，按丢弃原因聚合）
//!
//! # 偏差声明
//!
//! - **D5**：订阅表使用 `alloc::collections::BTreeMap<String, Vec<Subscription>>` 替代 `HashMap`（no_std 兼容）
//! - **D7**：订阅回调 `Box<dyn Fn(&DdsSample)>` 不带 `Send + Sync` bound（no_std 单线程场景）
//! - **D8**：路由器方法使用 `&mut self`（无 `spin::Mutex`），由调用方保证互斥
//! - **D9**：`route` / `dispatch` 接收 `topic: &str` 作为独立参数（`DdsSample` 无 `topic` 字段）
//! - **D11**：`SubId` 为简单 `u64` 计数器 newtype（不引入 `slotmap` 依赖到订阅管理）

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use crate::policy::{
    AgentId, CapabilityVerifier, MockCapabilityVerifier, Permission, RouteDecision, RouteError,
    RoutingPolicy,
};
use crate::registry::TopicRegistry;
use crate::topic::validate_topic_name;
use crate::types::DdsSample;

/// 订阅 ID.
///
/// **D11**：简单 `u64` 计数器 newtype（由 [`MessageRouter::next_sub_id`] 单调递增分配），
/// 不引入 `slotmap` 依赖到订阅管理（slotmap 已用于 DDS 句柄，订阅生命周期由路由器管理）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SubId(pub u64);

/// 订阅条目.
///
/// **D7**：`callback` 为 `Box<dyn Fn(&DdsSample)>`（不带 `Send + Sync` bound），
/// 由路由器在 `&mut self` 下同步调用。
pub struct Subscription {
    /// 订阅 ID（由路由器分配）.
    pub id: SubId,
    /// 订阅者 Agent ID.
    pub subscriber_id: AgentId,
    /// 订阅 pattern（`*` 后缀通配）.
    pub pattern: String,
    /// 消息回调（收到匹配消息时调用）.
    pub callback: Box<dyn Fn(&DdsSample)>,
}

impl fmt::Debug for Subscription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Subscription")
            .field("id", &self.id)
            .field("subscriber_id", &self.subscriber_id)
            .field("pattern", &self.pattern)
            .field("callback", &"<callback>")
            .finish()
    }
}

/// 路由统计.
///
/// **D6**：`dropped_by_reason` 使用 `BTreeMap<&'static str, u64>` 替代 `HashMap`（no_std 兼容）。
#[derive(Debug, Default)]
pub struct RouterStats {
    /// 总路由消息数（含投递与丢弃）.
    pub total_routed: u64,
    /// 总丢弃消息数.
    pub total_dropped: u64,
    /// 按丢弃原因聚合（key 为 [`DropReason::reason_name`] 返回的静态字符串）.
    pub dropped_by_reason: BTreeMap<&'static str, u64>,
}

/// 通配符匹配.
///
/// **D4**：仅支持 `*` 后缀通配（与 `TopicRegistry::match_pattern` 一致）。
///
/// # 行为
///
/// - `pattern` 以 `*` 结尾：返回 `topic` 是否以 `pattern` 去掉 `*` 后的前缀开头
/// - 否则：精确匹配 `pattern == topic`
pub fn pattern_matches(pattern: &str, topic: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix('*') {
        topic.starts_with(prefix)
    } else {
        pattern == topic
    }
}

/// 消息路由器.
///
/// 管理订阅、路由消息到匹配订阅者的回调。无锁设计（**D8**：`&mut self`，无 `Mutex`），
/// 由调用方保证互斥。默认使用 [`MockCapabilityVerifier`]（**D10**），可通过
/// [`MessageRouter::with_verifier`] 注入真实能力验证器。
pub struct MessageRouter {
    registry: TopicRegistry,
    subscriptions: BTreeMap<String, Vec<Subscription>>,
    policy: RoutingPolicy,
    stats: RouterStats,
    next_sub_id: u64,
    verifier: Box<dyn CapabilityVerifier>,
}

impl MessageRouter {
    /// 创建路由器（使用 [`MockCapabilityVerifier`] 作为默认能力验证器）.
    ///
    /// `next_sub_id` 从 1 开始单调递增。
    pub fn new(registry: TopicRegistry, policy: RoutingPolicy) -> Self {
        Self {
            registry,
            subscriptions: BTreeMap::new(),
            policy,
            stats: RouterStats::default(),
            next_sub_id: 1,
            verifier: Box::new(MockCapabilityVerifier),
        }
    }

    /// 创建路由器并注入自定义能力验证器.
    pub fn with_verifier(
        registry: TopicRegistry,
        policy: RoutingPolicy,
        verifier: Box<dyn CapabilityVerifier>,
    ) -> Self {
        Self {
            registry,
            subscriptions: BTreeMap::new(),
            policy,
            stats: RouterStats::default(),
            next_sub_id: 1,
            verifier,
        }
    }

    /// 订阅 topic pattern.
    ///
    /// # 行为
    ///
    /// 1. 校验 pattern 合法性（[`validate_topic_name`]，允许尾部 `*` 通配符）→ 失败返回 [`RouteError::InvalidPattern`]
    /// 2. 若策略要求 subscribe token，调用 [`CapabilityVerifier::verify`] → 失败返回 [`RouteError::Dropped`]
    /// 3. 分配 [`SubId`]（单调递增），插入订阅表，返回 ID
    pub fn subscribe(
        &mut self,
        pattern: &str,
        subscriber_id: AgentId,
        callback: Box<dyn Fn(&DdsSample)>,
    ) -> Result<SubId, RouteError> {
        // D4：仅支持 `*` 后缀通配。校验时去掉尾部 `*`，复用 validate_topic_name 校验前缀部分。
        let validated = pattern.strip_suffix('*').unwrap_or(pattern);
        if validate_topic_name(validated).is_err() {
            return Err(RouteError::InvalidPattern(String::from(pattern)));
        }
        if self.policy.require_subscribe_token {
            if let Err(reason) = self
                .verifier
                .verify(Permission::Subscribe, subscriber_id, pattern)
            {
                return Err(RouteError::Dropped(reason));
            }
        }
        let sub_id = SubId(self.next_sub_id);
        self.next_sub_id += 1;
        let sub = Subscription {
            id: sub_id,
            subscriber_id,
            pattern: String::from(pattern),
            callback,
        };
        self.subscriptions
            .entry(String::from(pattern))
            .or_default()
            .push(sub);
        Ok(sub_id)
    }

    /// 取消订阅.
    ///
    /// 遍历所有 pattern 下的订阅，移除匹配 ID 的订阅。
    ///
    /// # 返回
    ///
    /// - `Ok(())`：成功移除
    /// - `Err(RouteError::InvalidPattern)`：未找到匹配 ID 的订阅（复用 `InvalidPattern` 变体，因无 `NotFound` 变体）
    pub fn unsubscribe(&mut self, id: SubId) -> Result<(), RouteError> {
        let mut found = false;
        for subs in self.subscriptions.values_mut() {
            let before = subs.len();
            subs.retain(|s| s.id != id);
            if subs.len() < before {
                found = true;
            }
        }
        if found {
            Ok(())
        } else {
            Err(RouteError::InvalidPattern(format!(
                "subscription {:?} not found",
                id
            )))
        }
    }

    /// 路由决策（不触发回调）.
    ///
    /// 根据 topic 查询注册表获取默认 QoS 优先级，返回 [`RouteDecision`]。
    /// 未注册的 topic 优先级为 0。
    ///
    /// **D9**：`topic` 作为独立参数传入（`DdsSample` 无 `topic` 字段）。
    pub fn route(&self, topic: &str, _sample: &DdsSample) -> RouteDecision {
        let priority = self
            .registry
            .lookup(topic)
            .map(|spec| spec.default_qos.priority)
            .unwrap_or(0);
        RouteDecision::Deliver { priority }
    }

    /// 分发消息到匹配订阅者的回调.
    ///
    /// # 行为
    ///
    /// 1. 调用 [`Self::route`] 获取决策
    /// 2. 若 `Drop`：更新统计并返回 `Err(RouteError::Dropped)`
    /// 3. 若 `Deliver`：遍历订阅表，对每个 `pattern_matches(pattern, topic)` 的订阅调用回调，
    ///    返回投递计数
    pub fn dispatch(&mut self, topic: &str, sample: &DdsSample) -> Result<usize, RouteError> {
        let decision = self.route(topic, sample);
        match decision {
            RouteDecision::Drop { reason } => {
                self.stats.total_dropped += 1;
                *self
                    .stats
                    .dropped_by_reason
                    .entry(reason.reason_name())
                    .or_insert(0) += 1;
                Err(RouteError::Dropped(reason))
            }
            RouteDecision::Deliver { .. } => {
                let mut count = 0usize;
                for (pattern, subs) in &self.subscriptions {
                    if pattern_matches(pattern, topic) {
                        for sub in subs {
                            (sub.callback)(sample);
                            count += 1;
                        }
                    }
                }
                self.stats.total_routed += 1;
                Ok(count)
            }
        }
    }

    /// 获取路由统计的不可变引用.
    pub fn stats(&self) -> &RouterStats {
        &self.stats
    }
}

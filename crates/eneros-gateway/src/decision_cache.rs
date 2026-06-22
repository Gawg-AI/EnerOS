//! 决策管线结果缓存（T029-15）。
//!
//! 实现 LRU（Least Recently Used）+ TTL（Time To Live）双策略缓存，
//! 对决策管线的计算结果进行复用。相同输入（动作 + 上下文关键字段）
//! 在 TTL 内直接返回缓存结果，避免重复执行 precondition / projection /
//! validation / decomposition / execution / postcondition 全流程。
//!
//! ## 缓存键
//!
//! 缓存键基于以下字段的哈希值（使用 `ahash`）：
//! - `StructuredAction` 的变体与所有字段
//! - `AuthorityLevel`（权限等级）
//! - `Jurisdiction`（管辖范围：zone_ids / voltage_levels / device_ids）
//! - `SystemOperatingState`（系统运行状态）
//! - `agent_id`（决策发起者 ID）
//!
//! 以下字段**不参与**缓存键计算：
//! - `observation`：SCADA 实时遥测，每次变化，纳入将导致缓存失效
//! - `device_states`：interlocking 实时状态，HashMap 结构且频繁变化
//! - `reasoning`：文本说明，不影响决策逻辑
//!
//! 调用方应确保 TTL 足够短，以避免在 device_states 变化后命中过期决策。
//!
//! ## LRU 淘汰
//!
//! 当缓存条目数达到 `max_size` 时，淘汰 `last_accessed_at` 最旧的条目。
//! 淘汰在 `insert()` 时同步执行，O(n) 复杂度（n = max_size）。
//! 对于工业级电力系统，`max_size` 通常为 256~1024，O(n) 淘汰可接受。
//!
//! ## TTL 过期
//!
//! 每次访问时检查 `inserted_at + ttl < now`，过期条目返回 miss 并删除。
//! 采用惰性过期策略（lazy expiration），不启动后台清理任务，避免
//! 引入额外的 tokio task 依赖。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use std::hash::Hasher;

use ahash::AHasher;
use dashmap::DashMap;
use eneros_core::{AuthorityLevel, Jurisdiction, StructuredAction, SystemOperatingState};

use crate::pipeline_types::{DecisionContext, EnhancedPipelineDecision, PipelineAuditEntry};

/// 缓存条目：存储决策结果 + 插入时间 + 最后访问时间。
#[derive(Debug, Clone)]
struct CacheEntry {
    /// 缓存的决策结果
    result: EnhancedPipelineDecision,
    /// 条目插入时间（用于 TTL 过期判断）
    inserted_at: Instant,
    /// 最后访问时间（用于 LRU 淘汰）
    last_accessed_at: Instant,
}

/// 决策结果缓存统计快照。
///
/// 由 [`DecisionCache::stats()`] 产生，包含命中率等可观测指标。
#[derive(Debug, Clone, Default)]
pub struct DecisionCacheStats {
    /// 当前缓存条目数
    pub len: usize,
    /// 缓存最大容量
    pub max_size: usize,
    /// TTL（微秒）
    pub ttl_us: u64,
    /// 命中次数
    pub hits: u64,
    /// 未命中次数
    pub misses: u64,
    /// 过期淘汰次数（TTL 触发）
    pub expirations: u64,
    /// LRU 淘汰次数（容量满触发）
    pub evictions: u64,
    /// 命中率（0.0 ~ 1.0），无请求时为 0.0
    pub hit_rate: f64,
}

/// 决策结果缓存（LRU + TTL）。
///
/// 线程安全：内部使用 `DashMap` + `AtomicU64`，可安全并发访问。
///
/// ## 使用示例
///
/// ```ignore
/// use std::time::Duration;
/// use std::sync::Arc;
/// use eneros_gateway::decision_cache::DecisionCache;
/// use eneros_gateway::decision_pipeline::ConstrainedDecisionPipeline;
///
/// let cache = Arc::new(DecisionCache::new(256, Duration::from_secs(5)));
/// let pipeline = ConstrainedDecisionPipeline::new(/* ... */)
///     .with_cache(cache.clone());
/// ```
pub struct DecisionCache {
    /// 缓存存储：键 → 条目
    cache: DashMap<u64, CacheEntry>,
    /// 最大条目数（LRU 淘汰阈值）
    max_size: usize,
    /// 生存时间（TTL）
    ttl: Duration,
    /// 命中计数
    hits: AtomicU64,
    /// 未命中计数
    misses: AtomicU64,
    /// TTL 过期淘汰计数
    expirations: AtomicU64,
    /// LRU 容量淘汰计数
    evictions: AtomicU64,
}

impl DecisionCache {
    /// 创建新的决策缓存。
    ///
    /// # 参数
    /// - `max_size`：最大缓存条目数。超过时按 LRU 策略淘汰最旧条目。
    /// - `ttl`：条目生存时间。超过此时间的条目在访问时被惰性删除。
    pub fn new(max_size: usize, ttl: Duration) -> Self {
        Self {
            cache: DashMap::with_capacity(max_size.max(1)),
            max_size: max_size.max(1),
            ttl,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            expirations: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
        }
    }

    /// 查询缓存。
    ///
    /// 如果键存在且未过期，返回决策结果并更新 `last_accessed_at`，
    /// 命中计数 +1。如果键不存在或已过期，未命中计数 +1 并返回 `None`
    /// （过期条目会被删除，过期计数 +1）。
    pub fn get(&self, key: u64) -> Option<EnhancedPipelineDecision> {
        let now = Instant::now();
        // 先尝试获取条目
        if let Some(mut entry) = self.cache.get_mut(&key) {
            // 检查 TTL 过期
            if now.duration_since(entry.inserted_at) >= self.ttl {
                // 过期：drop guard 后删除条目
                let expired_key = key;
                drop(entry);
                self.cache.remove(&expired_key);
                self.expirations.fetch_add(1, Ordering::Relaxed);
                self.misses.fetch_add(1, Ordering::Relaxed);
                return None;
            }
            // 命中：更新最后访问时间
            entry.last_accessed_at = now;
            let result = entry.result.clone();
            self.hits.fetch_add(1, Ordering::Relaxed);
            Some(result)
        } else {
            // 键不存在
            self.misses.fetch_add(1, Ordering::Relaxed);
            None
        }
    }

    /// 插入决策结果到缓存。
    ///
    /// 如果缓存已满（`len >= max_size`），先按 LRU 策略淘汰最旧条目，
    /// 再插入新条目。如果键已存在，覆盖旧条目（不触发淘汰计数）。
    ///
    /// 并发安全：由于 `DashMap` 的容量检查与插入不是原子操作，并发插入
    /// 可能导致 `len` 短暂超过 `max_size`。插入后再次检查并淘汰超额条目。
    pub fn insert(&self, key: u64, result: EnhancedPipelineDecision) {
        let now = Instant::now();

        // 如果键已存在，直接覆盖（更新时间和结果）
        if self.cache.contains_key(&key) {
            if let Some(mut entry) = self.cache.get_mut(&key) {
                entry.result = result;
                entry.inserted_at = now;
                entry.last_accessed_at = now;
            }
            return;
        }

        // 容量检查：如果已满，先淘汰 LRU 条目（减少并发超额概率）
        if self.cache.len() >= self.max_size {
            self.evict_lru();
        }

        self.cache.insert(
            key,
            CacheEntry {
                result,
                inserted_at: now,
                last_accessed_at: now,
            },
        );

        // 并发安全：插入后再次检查，处理并发插入导致的短暂超额
        while self.cache.len() > self.max_size {
            if !self.evict_lru() {
                break;
            }
        }
    }

    /// LRU 淘汰：找到 `last_accessed_at` 最旧的条目并删除。
    ///
    /// 在 `insert()` 中当容量满时同步调用。O(n) 复杂度。
    ///
    /// 返回 `true` 表示成功淘汰一个条目，`false` 表示缓存为空或淘汰失败
    ///（其他线程已删除目标条目）。
    fn evict_lru(&self) -> bool {
        // 遍历所有条目，找到 last_accessed_at 最小的键
        let lru_key = self
            .cache
            .iter()
            .min_by_key(|entry| entry.value().last_accessed_at)
            .map(|entry| *entry.key());

        if let Some(key) = lru_key {
            if self.cache.remove(&key).is_some() {
                self.evictions.fetch_add(1, Ordering::Relaxed);
                return true;
            }
        }
        false
    }

    /// 清空缓存。统计计数器不受影响。
    pub fn clear(&self) {
        self.cache.clear();
    }

    /// 当前缓存条目数。
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    /// 缓存是否为空。
    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }

    /// 命中次数。
    pub fn hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }

    /// 未命中次数。
    pub fn misses(&self) -> u64 {
        self.misses.load(Ordering::Relaxed)
    }

    /// LRU 容量淘汰次数。
    pub fn evictions(&self) -> u64 {
        self.evictions.load(Ordering::Relaxed)
    }

    /// TTL 过期淘汰次数。
    pub fn expirations(&self) -> u64 {
        self.expirations.load(Ordering::Relaxed)
    }

    /// 命中率（0.0 ~ 1.0）。无请求时返回 0.0。
    pub fn hit_rate(&self) -> f64 {
        let hits = self.hits.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);
        let total = hits + misses;
        if total == 0 {
            0.0
        } else {
            hits as f64 / total as f64
        }
    }

    /// 获取缓存统计快照。
    pub fn stats(&self) -> DecisionCacheStats {
        DecisionCacheStats {
            len: self.cache.len(),
            max_size: self.max_size,
            ttl_us: self.ttl.as_micros() as u64,
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            expirations: self.expirations.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
            hit_rate: self.hit_rate(),
        }
    }

    /// 计算缓存键。
    ///
    /// 基于决策输入（动作 + 上下文关键字段）计算 64 位哈希。
    /// 相同输入必然产生相同哈希，不同输入极大概率产生不同哈希。
    ///
    /// # 纳入哈希的字段
    /// - `action`：`StructuredAction` 变体 + 所有字段
    /// - `ctx.authority`：权限等级
    /// - `ctx.jurisdiction`：zone_ids / voltage_levels / device_ids
    /// - `ctx.system_state`：系统运行状态
    /// - `ctx.agent_id`：决策发起者 ID
    ///
    /// # 排除的字段（见模块文档说明）
    /// - `ctx.observation`、`ctx.device_states`、`ctx.reasoning`
    pub fn compute_key(action: &StructuredAction, ctx: &DecisionContext) -> u64 {
        let mut hasher = AHasher::default();
        hash_action(&mut hasher, action);
        hash_authority(&mut hasher, ctx.authority);
        hash_jurisdiction(&mut hasher, &ctx.jurisdiction);
        hash_system_state(&mut hasher, ctx.system_state);
        hasher.write(ctx.agent_id.as_bytes());
        hasher.finish()
    }
}

/// 哈希 `StructuredAction` 的所有字段。
///
/// 使用变体标签（u8）+ 各字段字节写入哈希器，
/// 确保不同变体或不同字段值产生不同哈希。
fn hash_action(hasher: &mut AHasher, action: &StructuredAction) {
    match action {
        StructuredAction::ExecuteDevice { device_id, operation, value } => {
            hasher.write_u8(0);
            hasher.write(&device_id.to_le_bytes());
            hasher.write(operation.as_bytes());
            hasher.write(&value.to_le_bytes());
        }
        StructuredAction::ShedLoad { zone_id, amount_mw } => {
            hasher.write_u8(1);
            hasher.write(&zone_id.to_le_bytes());
            hasher.write(&amount_mw.to_le_bytes());
        }
        StructuredAction::StartGenerator { gen_id, target_mw } => {
            hasher.write_u8(2);
            hasher.write(&gen_id.to_le_bytes());
            hasher.write(&target_mw.to_le_bytes());
        }
        StructuredAction::NotifyAgent { agent_id, message } => {
            hasher.write_u8(3);
            hasher.write(agent_id.as_bytes());
            hasher.write(message.as_bytes());
        }
        StructuredAction::IsolateFault { upstream_switch, downstream_switch } => {
            hasher.write_u8(4);
            hasher.write(&upstream_switch.to_le_bytes());
            hasher.write(&downstream_switch.to_le_bytes());
        }
        StructuredAction::CloseTieSwitch { switch_id } => {
            hasher.write_u8(5);
            hasher.write(&switch_id.to_le_bytes());
        }
    }
}

/// 哈希 `AuthorityLevel`（使用变体序号）。
fn hash_authority(hasher: &mut AHasher, authority: AuthorityLevel) {
    let discriminant: u8 = match authority {
        AuthorityLevel::Observer => 0,
        AuthorityLevel::Operator => 1,
        AuthorityLevel::Supervisor => 2,
        AuthorityLevel::Emergency => 3,
    };
    hasher.write_u8(discriminant);
}

/// 哈希 `SystemOperatingState`（使用变体序号）。
fn hash_system_state(hasher: &mut AHasher, state: SystemOperatingState) {
    let discriminant: u8 = match state {
        SystemOperatingState::Normal => 0,
        SystemOperatingState::Alert => 1,
        SystemOperatingState::Emergency => 2,
        SystemOperatingState::Blackout => 3,
        SystemOperatingState::Restoration => 4,
    };
    hasher.write_u8(discriminant);
}

/// 哈希 `Jurisdiction` 的所有字段。
///
/// 为保证相同语义的 Jurisdiction 产生相同哈希，
/// 对 Vec 字段先排序再哈希（避免顺序差异导致哈希不同）。
fn hash_jurisdiction(hasher: &mut AHasher, jurisdiction: &Jurisdiction) {
    // zone_ids：排序后哈希
    let mut zones: Vec<u32> = jurisdiction.zone_ids.clone();
    zones.sort_unstable();
    for z in &zones {
        hasher.write(&z.to_le_bytes());
    }
    hasher.write_u8(0xFF); // 分隔符，防止不同长度字段混淆

    // voltage_levels：排序后哈希
    let mut voltages: Vec<u64> = jurisdiction
        .voltage_levels
        .iter()
        .map(|v| v.to_bits())
        .collect();
    voltages.sort_unstable();
    for v in &voltages {
        hasher.write(&v.to_le_bytes());
    }
    hasher.write_u8(0xFF);

    // device_ids：排序后哈希
    let mut devices: Vec<u64> = jurisdiction.device_ids.clone();
    devices.sort_unstable();
    for d in &devices {
        hasher.write(&d.to_le_bytes());
    }
    hasher.write_u8(0xFF);
}

/// 构建缓存命中审计条目。
///
/// 在缓存命中时，向返回的决策结果追加一条 `cache_hit` 审计记录，
/// 并更新 `total_latency_us` 为缓存命中延迟（通常 < 100µs）。
pub(crate) fn mark_cache_hit(
    mut decision: EnhancedPipelineDecision,
    hit_latency_us: u64,
) -> EnhancedPipelineDecision {
    decision.audit.push(PipelineAuditEntry {
        stage: "cache_hit".to_string(),
        description: format!(
            "Decision served from cache (hit latency: {}µs)",
            hit_latency_us
        ),
        duration_us: hit_latency_us,
        passed: true,
    });
    decision.total_latency_us = hit_latency_us;
    decision
}

#[cfg(test)]
mod tests {
    use super::*;
    use eneros_core::Jurisdiction;
    use std::sync::Arc;
    use std::thread;

    /// 构造一个最小化的决策结果用于测试。
    fn make_test_decision(gen_id: u64, target_mw: f64) -> EnhancedPipelineDecision {
        use eneros_constraint::projector::ProjectionResult;
        use crate::pipeline_types::PreConditionResult;

        let action = StructuredAction::StartGenerator { gen_id, target_mw };
        EnhancedPipelineDecision {
            executed_action: Some(action.clone()),
            original_action: action,
            decomposition: None,
            projection: ProjectionResult::Feasible(StructuredAction::StartGenerator {
                gen_id,
                target_mw,
            }),
            pre_conditions: PreConditionResult::passed(),
            post_conditions: None,
            verdict: eneros_core::ActionVerdict::Approved,
            rollback_plan: None,
            rollback_executed: None,
            audit: vec![],
            total_latency_us: 1000,
        }
    }

    fn make_ctx(authority: AuthorityLevel) -> DecisionContext {
        DecisionContext::new(
            authority,
            Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        )
    }

    // ── 基本功能测试 ──

    #[test]
    fn test_cache_hit_same_input() {
        let cache = DecisionCache::new(16, Duration::from_secs(60));
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let ctx = make_ctx(AuthorityLevel::Supervisor);
        let key = DecisionCache::compute_key(&action, &ctx);

        // 首次查询：未命中
        assert!(cache.get(key).is_none());
        assert_eq!(cache.misses(), 1);
        assert_eq!(cache.hits(), 0);

        // 插入决策结果
        let decision = make_test_decision(1, 100.0);
        cache.insert(key, decision.clone());

        // 再次查询：命中
        let cached = cache.get(key).expect("should hit after insert");
        assert!(cached.executed_action.is_some());
        assert_eq!(cache.hits(), 1);
        assert_eq!(cache.misses(), 1);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_cache_miss_different_input() {
        let cache = DecisionCache::new(16, Duration::from_secs(60));
        let action1 = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let action2 = StructuredAction::StartGenerator { gen_id: 2, target_mw: 100.0 };
        let ctx = make_ctx(AuthorityLevel::Supervisor);

        let key1 = DecisionCache::compute_key(&action1, &ctx);
        let key2 = DecisionCache::compute_key(&action2, &ctx);

        // 不同 gen_id 应产生不同键
        assert_ne!(key1, key2, "different actions must produce different keys");

        cache.insert(key1, make_test_decision(1, 100.0));

        // 查询 key1：命中
        assert!(cache.get(key1).is_some());
        // 查询 key2：未命中
        assert!(cache.get(key2).is_none());
        assert_eq!(cache.hits(), 1);
        assert_eq!(cache.misses(), 1);
    }

    #[test]
    fn test_cache_key_different_authority() {
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let ctx_supervisor = make_ctx(AuthorityLevel::Supervisor);
        let ctx_operator = make_ctx(AuthorityLevel::Operator);

        let key1 = DecisionCache::compute_key(&action, &ctx_supervisor);
        let key2 = DecisionCache::compute_key(&action, &ctx_operator);

        assert_ne!(
            key1, key2,
            "different authority levels must produce different keys"
        );
    }

    #[test]
    fn test_cache_key_different_system_state() {
        let action = StructuredAction::ShedLoad { zone_id: 1, amount_mw: 50.0 };
        let ctx_normal = DecisionContext::new(
            AuthorityLevel::Supervisor,
            Jurisdiction::unrestricted(),
            SystemOperatingState::Normal,
        );
        let ctx_emergency = DecisionContext::new(
            AuthorityLevel::Supervisor,
            Jurisdiction::unrestricted(),
            SystemOperatingState::Emergency,
        );

        let key1 = DecisionCache::compute_key(&action, &ctx_normal);
        let key2 = DecisionCache::compute_key(&action, &ctx_emergency);

        assert_ne!(
            key1, key2,
            "different system states must produce different keys"
        );
    }

    #[test]
    fn test_cache_key_same_jurisdiction_different_order() {
        // 相同 zone_ids 但顺序不同，应产生相同哈希
        let action = StructuredAction::ShedLoad { zone_id: 1, amount_mw: 50.0 };

        let ctx1 = DecisionContext::new(
            AuthorityLevel::Supervisor,
            Jurisdiction::for_zones(vec![3, 1, 2]),
            SystemOperatingState::Normal,
        );
        let ctx2 = DecisionContext::new(
            AuthorityLevel::Supervisor,
            Jurisdiction::for_zones(vec![1, 2, 3]),
            SystemOperatingState::Normal,
        );

        let key1 = DecisionCache::compute_key(&action, &ctx1);
        let key2 = DecisionCache::compute_key(&action, &ctx2);

        assert_eq!(
            key1, key2,
            "same jurisdiction with different order must produce same key"
        );
    }

    #[test]
    fn test_cache_key_different_agent_id() {
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let ctx1 = make_ctx(AuthorityLevel::Supervisor).with_agent_id("agent-A");
        let ctx2 = make_ctx(AuthorityLevel::Supervisor).with_agent_id("agent-B");

        let key1 = DecisionCache::compute_key(&action, &ctx1);
        let key2 = DecisionCache::compute_key(&action, &ctx2);

        assert_ne!(key1, key2, "different agent_id must produce different keys");
    }

    // ── TTL 过期测试 ──

    #[test]
    fn test_ttl_expiration() {
        // TTL = 50ms，过期后应未命中
        let cache = DecisionCache::new(16, Duration::from_millis(50));
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let ctx = make_ctx(AuthorityLevel::Supervisor);
        let key = DecisionCache::compute_key(&action, &ctx);

        cache.insert(key, make_test_decision(1, 100.0));

        // 立即查询：命中
        assert!(cache.get(key).is_some());
        assert_eq!(cache.hits(), 1);

        // 等待 TTL 过期
        std::thread::sleep(Duration::from_millis(60));

        // 过期后查询：未命中，条目被删除
        assert!(cache.get(key).is_none());
        assert_eq!(cache.misses(), 1);
        assert_eq!(cache.expirations(), 1);
        assert_eq!(cache.len(), 0, "expired entry should be removed");
    }

    #[test]
    fn test_ttl_not_expired_still_hits() {
        // TTL = 500ms，短时间内应持续命中
        let cache = DecisionCache::new(16, Duration::from_millis(500));
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let ctx = make_ctx(AuthorityLevel::Supervisor);
        let key = DecisionCache::compute_key(&action, &ctx);

        cache.insert(key, make_test_decision(1, 100.0));

        // 连续查询 5 次，都应命中
        for _ in 0..5 {
            assert!(cache.get(key).is_some(), "should hit within TTL");
        }
        assert_eq!(cache.hits(), 5);
        assert_eq!(cache.misses(), 0);
    }

    // ── LRU 淘汰测试 ──

    #[test]
    fn test_lru_eviction_when_full() {
        // max_size = 3，插入 4 个不同键后应淘汰最旧条目
        let cache = DecisionCache::new(3, Duration::from_secs(60));
        let ctx = make_ctx(AuthorityLevel::Supervisor);

        // 插入 key1, key2, key3
        let action1 = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let key1 = DecisionCache::compute_key(&action1, &ctx);
        cache.insert(key1, make_test_decision(1, 100.0));

        let action2 = StructuredAction::StartGenerator { gen_id: 2, target_mw: 100.0 };
        let key2 = DecisionCache::compute_key(&action2, &ctx);
        cache.insert(key2, make_test_decision(2, 100.0));

        let action3 = StructuredAction::StartGenerator { gen_id: 3, target_mw: 100.0 };
        let key3 = DecisionCache::compute_key(&action3, &ctx);
        cache.insert(key3, make_test_decision(3, 100.0));

        assert_eq!(cache.len(), 3);

        // 访问 key1，使其成为最近使用
        std::thread::sleep(Duration::from_millis(1));
        assert!(cache.get(key1).is_some());

        // 访问 key3，使其成为最近使用
        std::thread::sleep(Duration::from_millis(1));
        assert!(cache.get(key3).is_some());

        // key2 现在是最久未访问的。插入 key4 触发淘汰
        std::thread::sleep(Duration::from_millis(1));
        let action4 = StructuredAction::StartGenerator { gen_id: 4, target_mw: 100.0 };
        let key4 = DecisionCache::compute_key(&action4, &ctx);
        cache.insert(key4, make_test_decision(4, 100.0));

        assert_eq!(cache.len(), 3, "cache size should remain at max_size");
        assert_eq!(cache.evictions(), 1, "one LRU eviction should occur");

        // key2 应被淘汰（最久未访问）
        assert!(
            cache.get(key2).is_none() || cache.misses() > 0,
            "key2 (LRU) should have been evicted"
        );

        // key1, key3, key4 应仍然存在（在 TTL 内）
        let hits_before = cache.hits();
        assert!(cache.get(key1).is_some());
        assert!(cache.get(key3).is_some());
        assert!(cache.get(key4).is_some());
        assert_eq!(cache.hits(), hits_before + 3);
    }

    #[test]
    fn test_lru_eviction_order() {
        // 验证淘汰的是最久未访问的条目，而非最早插入的
        let cache = DecisionCache::new(2, Duration::from_secs(60));
        let ctx = make_ctx(AuthorityLevel::Supervisor);

        let key1 = DecisionCache::compute_key(
            &StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 },
            &ctx,
        );
        let key2 = DecisionCache::compute_key(
            &StructuredAction::StartGenerator { gen_id: 2, target_mw: 100.0 },
            &ctx,
        );

        cache.insert(key1, make_test_decision(1, 100.0));
        cache.insert(key2, make_test_decision(2, 100.0));

        // 访问 key1，使 key2 成为 LRU
        std::thread::sleep(Duration::from_millis(1));
        let _ = cache.get(key1);

        // 插入 key3，应淘汰 key2（最久未访问）
        std::thread::sleep(Duration::from_millis(1));
        let key3 = DecisionCache::compute_key(
            &StructuredAction::StartGenerator { gen_id: 3, target_mw: 100.0 },
            &ctx,
        );
        cache.insert(key3, make_test_decision(3, 100.0));

        assert_eq!(cache.evictions(), 1);

        // key1 应仍存在（最近被访问过）
        let hits_before = cache.hits();
        assert!(cache.get(key1).is_some(), "key1 was recently accessed, should not be evicted");
        assert_eq!(cache.hits(), hits_before + 1);

        // key2 应被淘汰
        let misses_before = cache.misses();
        assert!(cache.get(key2).is_none(), "key2 should have been evicted as LRU");
        assert_eq!(cache.misses(), misses_before + 1);
    }

    // ── 统计测试 ──

    #[test]
    fn test_hit_rate_calculation() {
        let cache = DecisionCache::new(16, Duration::from_secs(60));
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let ctx = make_ctx(AuthorityLevel::Supervisor);
        let key = DecisionCache::compute_key(&action, &ctx);

        // 无请求时命中率为 0
        assert_eq!(cache.hit_rate(), 0.0);

        // 1 次未命中
        assert!(cache.get(key).is_none());
        assert_eq!(cache.hit_rate(), 0.0);

        // 插入后 1 次命中
        cache.insert(key, make_test_decision(1, 100.0));
        assert!(cache.get(key).is_some());
        assert_eq!(cache.hit_rate(), 0.5); // 1 hit / 2 total

        // 再 1 次命中
        assert!(cache.get(key).is_some());
        assert!((cache.hit_rate() - 2.0 / 3.0).abs() < 1e-9); // 2 hits / 3 total
    }

    #[test]
    fn test_stats_snapshot() {
        let cache = DecisionCache::new(8, Duration::from_millis(500));
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let ctx = make_ctx(AuthorityLevel::Supervisor);
        let key = DecisionCache::compute_key(&action, &ctx);

        cache.insert(key, make_test_decision(1, 100.0));
        let _ = cache.get(key);
        let _ = cache.get(99999); // miss

        let stats = cache.stats();
        assert_eq!(stats.len, 1);
        assert_eq!(stats.max_size, 8);
        assert_eq!(stats.ttl_us, 500_000);
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
        assert!((stats.hit_rate - 0.5).abs() < 1e-9);
    }

    #[test]
    fn test_clear_cache() {
        let cache = DecisionCache::new(16, Duration::from_secs(60));
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let ctx = make_ctx(AuthorityLevel::Supervisor);
        let key = DecisionCache::compute_key(&action, &ctx);

        cache.insert(key, make_test_decision(1, 100.0));
        assert_eq!(cache.len(), 1);

        cache.clear();
        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());

        // 清空后查询应未命中
        assert!(cache.get(key).is_none());
    }

    #[test]
    fn test_insert_overwrites_existing_key() {
        let cache = DecisionCache::new(16, Duration::from_secs(60));
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let ctx = make_ctx(AuthorityLevel::Supervisor);
        let key = DecisionCache::compute_key(&action, &ctx);

        // 插入第一个决策
        let decision1 = make_test_decision(1, 100.0);
        cache.insert(key, decision1);

        // 插入第二个决策（相同键，不同 gen_id 的结果）
        let decision2 = make_test_decision(1, 200.0);
        cache.insert(key, decision2);

        // 应只保留最新结果
        assert_eq!(cache.len(), 1, "overwrite should not increase size");
        assert_eq!(cache.evictions(), 0, "overwrite should not trigger eviction");

        let cached = cache.get(key).expect("should hit");
        // 验证返回的是最新结果（target_mw = 200.0）
        if let Some(StructuredAction::StartGenerator { target_mw, .. }) = cached.executed_action {
            assert!((target_mw - 200.0).abs() < 1e-9, "should return overwritten value");
        } else {
            panic!("expected StartGenerator action");
        }
    }

    // ── 并发测试 ──

    #[test]
    fn test_concurrent_access() {
        let cache = Arc::new(DecisionCache::new(64, Duration::from_secs(60)));
        let ctx = make_ctx(AuthorityLevel::Supervisor);

        // 预填充缓存
        for i in 0..32u64 {
            let action = StructuredAction::StartGenerator { gen_id: i, target_mw: 100.0 };
            let key = DecisionCache::compute_key(&action, &ctx);
            cache.insert(key, make_test_decision(i, 100.0));
        }

        let mut handles = Vec::new();
        for thread_id in 0..8 {
            let cache_clone = Arc::clone(&cache);
            let ctx_clone = ctx.clone();
            handles.push(thread::spawn(move || {
                // 每个线程对相同 32 个键查询 100 次
                for _ in 0..100 {
                    for i in 0..32u64 {
                        let action = StructuredAction::StartGenerator {
                            gen_id: i,
                            target_mw: 100.0,
                        };
                        let key = DecisionCache::compute_key(&action, &ctx_clone);
                        let _ = cache_clone.get(key);
                    }
                }
                // 每个线程也插入一些新键
                for i in 0..10u64 {
                    let action = StructuredAction::StartGenerator {
                        gen_id: 1000 + thread_id * 100 + i,
                        target_mw: 100.0,
                    };
                    let key = DecisionCache::compute_key(&action, &ctx_clone);
                    cache_clone.insert(key, make_test_decision(i, 100.0));
                }
            }));
        }

        for h in handles {
            h.join().expect("thread should not panic");
        }

        // 验证统计一致性：所有查询都命中预填充的键
        let stats = cache.stats();
        assert!(stats.hits > 0, "should have hits");
        assert_eq!(stats.misses, 0, "all queries are on pre-populated keys, should have 0 misses");
        // 8 线程 × 100 次 × 32 键 = 25600 次查询
        assert_eq!(
            stats.hits + stats.misses,
            25600,
            "total queries should match"
        );
    }

    #[test]
    fn test_concurrent_insert_and_get() {
        // 并发插入和查询相同键集合，验证不 panic 且数据一致
        let cache = Arc::new(DecisionCache::new(128, Duration::from_secs(60)));
        let ctx = make_ctx(AuthorityLevel::Supervisor);

        let mut handles = Vec::new();
        for thread_id in 0..4 {
            let cache_clone = Arc::clone(&cache);
            let ctx_clone = ctx.clone();
            handles.push(thread::spawn(move || {
                for i in 0..50u64 {
                    let action = StructuredAction::StartGenerator {
                        gen_id: thread_id * 1000 + i,
                        target_mw: 100.0,
                    };
                    let key = DecisionCache::compute_key(&action, &ctx_clone);
                    cache_clone.insert(key, make_test_decision(i, 100.0));
                    // 立即查询
                    let _ = cache_clone.get(key);
                }
            }));
        }

        for h in handles {
            h.join().expect("thread should not panic");
        }

        // 缓存大小不应超过 max_size
        assert!(
            cache.len() <= 128,
            "cache size {} should not exceed max_size 128",
            cache.len()
        );
    }

    // ── mark_cache_hit 测试 ──

    #[test]
    fn test_mark_cache_hit_adds_audit_entry() {
        let decision = make_test_decision(1, 100.0);
        let original_audit_len = decision.audit.len();
        let original_latency = decision.total_latency_us;

        let marked = mark_cache_hit(decision, 50);

        assert_eq!(marked.audit.len(), original_audit_len + 1);
        let last_audit = marked.audit.last().expect("audit entry should exist");
        assert_eq!(last_audit.stage, "cache_hit");
        assert_eq!(last_audit.duration_us, 50);
        assert!(last_audit.description.contains("cache"));
        assert_eq!(marked.total_latency_us, 50);
        assert_ne!(marked.total_latency_us, original_latency);
    }
}

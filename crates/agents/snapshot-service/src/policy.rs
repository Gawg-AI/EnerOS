//! 快照政策注册表（[08] §3.2 / §3.4）：按 purpose 定义 required/optional
//! 集合、freshness、lateness、quality、integrity、substitution。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

/// 替代值规则（[08] §3.4：必须显式配置；禁止用 0、默认 SOC、上次值
/// 无限续期或 LLM 猜值）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubstitutionRule {
    /// 允许的替代来源（优先级序：前=高）。
    pub source_priority: Vec<u32>,
    /// 最大持续时长（超时 → 拒绝替代）。
    pub max_duration_ns: u64,
    /// 允许的用途（purpose 白名单）。
    pub allowed_purposes: Vec<u32>,
    /// 不确定度扩张（替代值 setpoint 的包络界扩张系数 ≥1）。
    pub uncertainty_expansion: u32,
    /// reason code（审计/降级行）。
    pub reason_code: u8,
}

/// 单 purpose 政策。
#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotPolicy {
    pub purpose_id: u32,
    pub policy_version: u64,
    /// 必需点集（缺失 → 拒绝或按 degraded_ok 降级）。
    pub required: Vec<u32>,
    /// 可选点集（缺席不影响完整性）。
    pub optional: Vec<u32>,
    /// 允许的迟到窗（watermark 判定）。
    pub allowed_lateness_ns: u64,
    /// 新鲜度上限（`cut - event_time > max_age_ns` → stale）。
    pub max_age_ns: u64,
    /// 质量合格判定（false = 任何非 Good 都算 bad；true = 允许
    /// uncertain/substituted 之外位）。
    pub min_quality_bits_ok: bool,
    /// 完整性要求（Authenticated 才合格）。
    pub require_integrity: bool,
    /// 仅 policy 明确的 degraded purpose 可返回降级快照。
    pub degraded_ok: bool,
    /// 来源白名单（None = 不设白名单；Some = 快照输入只接受列出来源——
    /// 未注册来源只能进 quarantine，[02] §4.2）。
    pub allowed_sources: Option<Vec<u32>>,
    pub substitution: Option<SubstitutionRule>,
    /// 目录版本绑定（policy 与 catalog 一起激活）。
    pub catalog_version: u64,
}

impl SnapshotPolicy {
    /// 是否必需某点。
    pub fn requires(&self, point: u32) -> bool {
        self.required.contains(&point)
    }
}

/// 政策注册表（purpose → policy；确定性 BTreeMap）。
pub struct SnapshotPolicyRegistry {
    by_purpose: BTreeMap<u32, SnapshotPolicy>,
}

impl SnapshotPolicyRegistry {
    pub fn new() -> Self {
        Self {
            by_purpose: BTreeMap::new(),
        }
    }

    /// 注册/替换（同 purpose 覆盖——版本化由调用方管理 policy_version）。
    pub fn register(&mut self, policy: SnapshotPolicy) {
        self.by_purpose.insert(policy.purpose_id, policy);
    }

    pub fn get(&self, purpose_id: u32) -> Option<&SnapshotPolicy> {
        self.by_purpose.get(&purpose_id)
    }

    pub fn purposes(&self) -> impl Iterator<Item = u32> + '_ {
        self.by_purpose.keys().copied()
    }

    pub fn len(&self) -> usize {
        self.by_purpose.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_purpose.is_empty()
    }
}

impl Default for SnapshotPolicyRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_query() {
        let mut r = SnapshotPolicyRegistry::new();
        let p = SnapshotPolicy {
            purpose_id: 9,
            policy_version: 3,
            required: alloc::vec![1, 2],
            optional: alloc::vec![5],
            allowed_lateness_ns: 1000,
            max_age_ns: 5000,
            min_quality_bits_ok: true,
            require_integrity: true,
            degraded_ok: false,
            substitution: None,
            allowed_sources: None,
            catalog_version: 1,
        };
        r.register(p.clone());
        assert_eq!(r.get(9).unwrap().policy_version, 3);
        assert!(r.get(9).unwrap().requires(1));
        assert!(!r.get(9).unwrap().requires(5));
        assert_eq!(r.purposes().collect::<Vec<_>>(), alloc::vec![9]);
    }
}

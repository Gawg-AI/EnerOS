//! 告警管理器（生成/抑制/确认/升级/清除/查询/统计）.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::error::AlarmError;
use crate::escalation::EscalationPolicy;
use crate::level::AlarmLevel;
use crate::record::{AlarmId, AlarmRecord, AlarmState};
use crate::suppression::{SuppressionRule, SuppressionWindow};

/// 告警统计.
#[derive(Debug, Clone, Default)]
pub struct AlarmStats {
    /// 累计生成数.
    pub total_raised: u64,
    /// 累计确认数.
    pub total_acknowledged: u64,
    /// 累计清除数.
    pub total_cleared: u64,
    /// 累计升级数.
    pub total_escalated: u64,
    /// 累计抑制数.
    pub total_suppressed: u64,
    /// 当前活跃数.
    pub active_count: usize,
}

/// 告警管理器.
pub struct AlarmManager {
    /// 活跃告警表（D20: `BTreeMap`，no_std 无 HashMap）.
    active: BTreeMap<AlarmId, AlarmRecord>,
    /// 历史告警（已清除，归档）.
    history: Vec<AlarmRecord>,
    /// 抑制规则.
    suppression_rules: Vec<SuppressionRule>,
    /// 升级策略.
    escalation_policies: Vec<EscalationPolicy>,
    /// 抑制窗口（按源字符串索引）.
    suppression_windows: BTreeMap<String, SuppressionWindow>,
    /// 下一个告警 ID（从 1 自增）.
    next_id: AlarmId,
    /// 累计抑制数.
    suppressed_count: u64,
    /// 累计升级数.
    escalated_count: u64,
}

impl AlarmManager {
    /// 构造管理器（`next_id = 1`，空表）.
    pub fn new() -> Self {
        Self {
            active: BTreeMap::new(),
            history: Vec::new(),
            suppression_rules: Vec::new(),
            escalation_policies: Vec::new(),
            suppression_windows: BTreeMap::new(),
            next_id: 1,
            suppressed_count: 0,
            escalated_count: 0,
        }
    }

    /// 添加抑制规则.
    pub fn add_suppression_rule(&mut self, rule: SuppressionRule) {
        self.suppression_rules.push(rule);
    }

    /// 添加升级策略.
    pub fn add_escalation_policy(&mut self, policy: EscalationPolicy) {
        self.escalation_policies.push(policy);
    }

    /// 生成告警.
    ///
    /// 先检查抑制规则；若被抑制返回 `Err(Suppressed)`，否则生成告警并返回 ID。
    pub fn raise(
        &mut self,
        level: AlarmLevel,
        source: &str,
        description: &str,
        now_ms: u64,
    ) -> Result<AlarmId, AlarmError> {
        // 检查抑制规则（匹配第一条）
        for rule in &self.suppression_rules {
            if rule.matches_source(source) {
                let window = self
                    .suppression_windows
                    .entry(String::from(source))
                    .or_insert_with(|| SuppressionWindow::new(rule.max_count, rule.duration_ms));
                if window.should_suppress(now_ms) {
                    self.suppressed_count += 1;
                    return Err(AlarmError::Suppressed);
                }
                break;
            }
        }
        // 生成告警
        let id = self.next_id;
        self.next_id += 1;
        let record = AlarmRecord::new(id, level, source, description, now_ms);
        self.active.insert(id, record);
        Ok(id)
    }

    /// 确认告警（Active → Acknowledged）.
    pub fn acknowledge(&mut self, id: AlarmId, now_ms: u64) -> Result<(), AlarmError> {
        let record = self.active.get_mut(&id).ok_or(AlarmError::NotFound)?;
        if record.state != AlarmState::Active {
            return Err(AlarmError::AlreadyAcknowledged);
        }
        record.state = AlarmState::Acknowledged;
        record.acknowledged_at_ms = Some(now_ms);
        Ok(())
    }

    /// 清除告警（任意状态 → Cleared，转入 history）.
    pub fn clear(&mut self, id: AlarmId, now_ms: u64) -> Result<(), AlarmError> {
        let mut record = self.active.remove(&id).ok_or(AlarmError::NotFound)?;
        if record.state == AlarmState::Cleared {
            return Err(AlarmError::AlreadyCleared);
        }
        record.state = AlarmState::Cleared;
        record.cleared_at_ms = Some(now_ms);
        // 重置对应抑制窗口（允许后续告警重新进入）
        if let Some(window) = self.suppression_windows.get_mut(&record.source) {
            window.reset();
        }
        self.history.push(record);
        Ok(())
    }

    /// 升级告警（手动触发，查找匹配策略）.
    pub fn escalate(&mut self, id: AlarmId, _now_ms: u64) -> Result<(), AlarmError> {
        let record = self.active.get_mut(&id).ok_or(AlarmError::NotFound)?;
        if record.escalated_from.is_some() {
            return Err(AlarmError::AlreadyEscalated);
        }
        let new_level = self
            .escalation_policies
            .iter()
            .find(|p| p.from_level == record.level)
            .map(|p| p.to_level)
            .ok_or(AlarmError::InvalidLevel)?;
        record.escalated_from = Some(record.level);
        record.level = new_level;
        self.escalated_count += 1;
        Ok(())
    }

    /// 批量检查自动升级.
    ///
    /// 遍历活跃告警，对每条匹配策略且超时的告警执行升级，返回被升级的 ID 列表。
    pub fn check_auto_escalate(&mut self, now_ms: u64) -> Vec<AlarmId> {
        // 先收集需升级的 ID（避免在迭代中可变借用）
        let mut to_escalate: Vec<AlarmId> = Vec::new();
        for record in self.active.values() {
            for policy in &self.escalation_policies {
                if policy.check_escalation(record, now_ms).is_some() {
                    to_escalate.push(record.id);
                    break;
                }
            }
        }
        // 执行升级
        let mut escalated_ids: Vec<AlarmId> = Vec::new();
        for id in to_escalate {
            if self.escalate(id, now_ms).is_ok() {
                escalated_ids.push(id);
            }
        }
        escalated_ids
    }

    /// 查询活跃告警（按 `raised_at_ms` 升序）.
    pub fn query_active(&self) -> Vec<&AlarmRecord> {
        let mut refs: Vec<&AlarmRecord> = self.active.values().collect();
        refs.sort_by_key(|r| r.raised_at_ms);
        refs
    }

    /// 查询历史告警（`raised_at_ms` 在 `[start_ms, end_ms]` 范围内）.
    pub fn query_history(&self, start_ms: u64, end_ms: u64) -> Vec<&AlarmRecord> {
        self.history
            .iter()
            .filter(|r| r.raised_at_ms >= start_ms && r.raised_at_ms <= end_ms)
            .collect()
    }

    /// 计算统计.
    pub fn stats(&self) -> AlarmStats {
        let active_ack = self
            .active
            .values()
            .filter(|r| r.acknowledged_at_ms.is_some())
            .count() as u64;
        let history_ack = self
            .history
            .iter()
            .filter(|r| r.acknowledged_at_ms.is_some())
            .count() as u64;
        AlarmStats {
            total_raised: self.next_id - 1,
            total_acknowledged: active_ack + history_ack,
            total_cleared: self.history.len() as u64,
            total_escalated: self.escalated_count,
            total_suppressed: self.suppressed_count,
            active_count: self.active.len(),
        }
    }
}

impl Default for AlarmManager {
    fn default() -> Self {
        Self::new()
    }
}

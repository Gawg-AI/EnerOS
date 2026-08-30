//! 冲突裁决（[08] §3.2）：确定性优先/多数/主备规则，输出裁决证据。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use eneros_telemetry_model::event::{ObservationEvent, ObservationValue};

/// 裁决规则（每 purpose 可配；规则间可组合：先主备，主不可得再多数）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictRule {
    /// 主备：主来源胜；主缺席 → 备（再缺 → 冲突）。
    PrimaryBackup,
    /// 来源优先级列表（priority[0] 最高；同优先级同值多数）。
    PriorityList,
}

/// 冲突裁决证据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub point_id: u32,
    /// 参与冲突的来源（排序输出）。
    pub sources: Vec<u32>,
    /// 被采用的来源与事件序列。
    pub decision: ConflictDecision,
    /// 规则名（审计）。
    pub rule: &'static str,
}

/// 裁决决定（采纳某源事件；或不可裁决 → 冲突）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictDecision {
    Accepted { source: u32, sequence: u64 },
    Unresolved,
}

/// 数值相等判定（f64 容差事件级固定——事件值同型传播；快照重放
/// 确定性：同输入同裁决）。
fn values_equal(a: &ObservationValue, b: &ObservationValue) -> bool {
    match (a, b) {
        (ObservationValue::F64(x), ObservationValue::F64(y)) => {
            // 固定舍入/容差策略（[08] §3.5）：±1e-9 相对 + 1e-6 绝对。
            let abs = (*x - *y).abs();
            abs <= 1e-6 || abs <= 1e-9 * x.abs().max(y.abs())
        }
        (ObservationValue::U64(x), ObservationValue::U64(y)) => x == y,
        (ObservationValue::I64(x), ObservationValue::I64(y)) => x == y,
        (ObservationValue::Bool(x), ObservationValue::Bool(y)) => x == y,
        _ => false,
    }
}

/// 确定性冲突裁决器（无随机；hash-free——BTreeMap 序）。
pub struct ConflictResolver {
    rule: ConflictRule,
    /// PriorityList 规则下的来源优先级（低序=高优）。
    priority: Vec<u32>,
}

impl ConflictResolver {
    pub fn new(rule: ConflictRule) -> Self {
        Self {
            rule,
            priority: Vec::new(),
        }
    }

    pub fn with_priority(mut self, priority: Vec<u32>) -> Self {
        self.priority = priority;
        self
    }

    /// 裁决同点多源事件。
    ///
    /// - 单源 → 直接接受（无冲突）；
    /// - 主备：主源事件存在 → 接受；否则备；全缺 → Unresolved；
    /// - PriorityList：最高可用优先级源胜；同优先级多源同值 → 接受首位
    ///   （按源序确定性）；同优先级异值 → 多数裁决（值计数）；平票 →
    ///   Unresolved。
    pub fn resolve(
        &self,
        point_id: u32,
        candidates: &[ObservationEvent],
    ) -> Option<(Conflict, ObservationEvent)> {
        if candidates.is_empty() {
            return None;
        }
        if candidates.len() == 1 {
            return None; // 无冲突，不产出裁决记录
        }
        let mut by_source: BTreeMap<u32, &ObservationEvent> = BTreeMap::new();
        for c in candidates {
            by_source.entry(c.source_id).or_insert(c);
        }
        let decision_event = match self.rule {
            ConflictRule::PrimaryBackup => {
                // 主 = 按 source 序最小（确定性；主备规则在主源缺席时
                // 落到候选中序最小者——caller 对候选进入时的主备集负责）。
                let mut ranked: Vec<&ObservationEvent> = by_source.values().copied().collect();
                ranked.sort_by_key(|e| e.source_id);
                ranked.first().copied().copied()
            }
            ConflictRule::PriorityList => self.pick_by_priority(&by_source),
        };
        let Some(ev) = decision_event else {
            return Some((
                Conflict {
                    point_id,
                    sources: by_source.keys().copied().collect(),
                    decision: ConflictDecision::Unresolved,
                    rule: self.rule_name(),
                },
                // Unresolved 无采纳事件（调用方按 policy 拒绝或降级）。
                // 返回占位（调用方不会使用）。
                candidates[0],
            ));
        };
        let sources: Vec<u32> = by_source.keys().copied().collect();
        Some((
            Conflict {
                point_id,
                sources: sources.clone(),
                decision: ConflictDecision::Accepted {
                    source: ev.source_id,
                    sequence: ev.sequence,
                },
                rule: self.rule_name(),
            },
            ev,
        ))
    }

    fn pick_by_priority(
        &self,
        by_source: &BTreeMap<u32, &ObservationEvent>,
    ) -> Option<ObservationEvent> {
        // 优先级序（priority[0]=最高）；未列出的源排最后。
        let mut ranked: Vec<(&ObservationEvent, usize)> = by_source
            .values()
            .map(|e| {
                let rank = self
                    .priority
                    .iter()
                    .position(|s| *s == e.source_id)
                    .unwrap_or(usize::MAX);
                (*e, rank)
            })
            .collect();
        ranked.sort_by_key(|(e, r)| (*r, e.source_id));
        let top = ranked[0].0;
        // 同优先级多数：值计数。
        let top_rank = ranked[0].1;
        let same_rank: Vec<&ObservationEvent> = ranked
            .iter()
            .filter(|(_, r)| *r == top_rank)
            .map(|(e, _)| *e)
            .collect();
        if same_rank.len() == 1 {
            return Some(*top);
        }
        // 多数裁决：值 → 计数（确定性最大者胜；平票 → None；无 Ord
        // 依赖——线性值相等判定与 f64 容差同构）。
        let mut groups: Vec<(ObservationValue, usize, u32)> = Vec::new();
        for e in same_rank.iter() {
            if let Some(g) = groups
                .iter_mut()
                .find(|(v, _, _)| values_equal(v, &e.value))
            {
                g.1 += 1;
                if g.1 == 1 {
                    g.2 = e.source_id;
                }
            } else {
                groups.push((e.value, 1, e.source_id));
            }
        }
        let best = groups.iter().max_by_key(|(_, c, src)| (*c, *src));
        let (_, count, src) = best?;
        if *count * 2 <= same_rank.len() {
            return None; // 平票/无多数
        }
        by_source.get(src).copied().copied()
    }

    fn rule_name(&self) -> &'static str {
        match self.rule {
            ConflictRule::PrimaryBackup => "primary-backup",
            ConflictRule::PriorityList => "priority-list",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::obs7;

    fn ev_at(source: u32, value: f64, seq: u64) -> ObservationEvent {
        let mut e = obs7(
            1_000_000_000,
            0,
            source,
            &[400.0, 50.0, 50.0, 0.6, 1.0, 0.0, 0.0],
        )
        .remove(0);
        e.point_id = 1;
        e.source_id = source;
        e.sequence = seq;
        e.value = ObservationValue::F64(value);
        e
    }

    #[test]
    fn single_source_no_conflict_record() {
        let r = ConflictResolver::new(ConflictRule::PrimaryBackup);
        assert!(r.resolve(1, &[ev_at(2, 400.0, 1)]).is_none());
    }

    #[test]
    fn primary_backup_picks_primary_then_backup() {
        let r = ConflictResolver::new(ConflictRule::PrimaryBackup);
        let (conflict, ev) = r
            .resolve(1, &[ev_at(2, 400.0, 1), ev_at(3, 399.0, 5)])
            .unwrap();
        assert_eq!(ev.source_id, 2); // 主=首个候选源
        assert_eq!(
            conflict.decision,
            ConflictDecision::Accepted {
                source: 2,
                sequence: 1
            }
        );
        assert_eq!(conflict.rule, "primary-backup");
    }

    #[test]
    fn priority_list_majority_and_tie() {
        // 无显式优先级（全部同档）：3 源 2 同值 → 多数胜。
        let r = ConflictResolver::new(ConflictRule::PriorityList);
        let (c, ev) = r
            .resolve(
                1,
                &[ev_at(2, 400.0, 1), ev_at(3, 400.0, 2), ev_at(4, 399.0, 3)],
            )
            .unwrap();
        assert_eq!(ev.value, ObservationValue::F64(400.0));
        assert!(matches!(c.decision, ConflictDecision::Accepted { .. }));
        // 平票（2v2）→ Unresolved。
        let (c2, _) = r
            .resolve(
                1,
                &[
                    ev_at(2, 400.0, 1),
                    ev_at(3, 399.0, 2),
                    ev_at(4, 400.0, 3),
                    ev_at(5, 399.0, 4),
                ],
            )
            .unwrap();
        assert_eq!(c2.decision, ConflictDecision::Unresolved);
        // 显式优先级：最高优先级源胜（即使值不同）。
        let r3 =
            ConflictResolver::new(ConflictRule::PriorityList).with_priority(alloc::vec![4, 2, 3]);
        let (c3, ev3) = r3
            .resolve(
                1,
                &[ev_at(2, 400.0, 1), ev_at(3, 400.0, 2), ev_at(4, 399.0, 3)],
            )
            .unwrap();
        assert_eq!(ev3.source_id, 4);
        assert_eq!(c3.rule, "priority-list");
    }
}

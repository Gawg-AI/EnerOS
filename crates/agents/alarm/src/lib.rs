//! EnerOS 告警管理体系（Alarm Management，v0.53.2）.
//!
//! 提供告警生成、严重度分级（Info/Warning/Critical/Emergency）、抖动抑制（同源
//! 滑动窗口）、运维确认（ACK）、超时升级（Escalation）、故障恢复清除（Clear），
//! 为运维与安全提供告警全生命周期管理。
//!
//! # 核心类型
//! - [`level::AlarmLevel`] — 告警级别（4 级，派生 Ord）
//! - [`record::AlarmRecord`] — 告警记录（含全生命周期时间戳）
//! - [`record::AlarmState`] — 告警状态（Active/Acknowledged/Cleared）
//! - [`suppression::SuppressionRule`] — 抑制规则（源匹配 + 窗口）
//! - [`suppression::SuppressionWindow`] — 滑动窗口（VecDeque 时间戳）
//! - [`escalation::EscalationPolicy`] — 升级策略（超时升级一级）
//! - [`manager::AlarmManager`] — 告警管理器（raise/acknowledge/clear/escalate/query/stats）
//! - [`error::AlarmError`] — 错误类型
//!
//! # 偏差声明（D19~D25）
//!
//! | 偏差 | 说明 |
//! |------|------|
//! | **D19** | crate 放入 `crates/agents/alarm/`（蓝图 §3 明确指定路径） |
//! | **D20** | 仅依赖 `eneros-upa-model`，活跃表使用 `BTreeMap`（no_std 无 HashMap；不直接依赖 soe-engine 避免循环依赖） |
//! | **D21** | 抑制策略使用滑动窗口计数（`VecDeque<u64>` 时间戳队列），不实现依赖抑制（蓝图 §4.3 提及但 MVP 简化） |
//! | **D22** | 升级策略简化为"超时升级一级"（Critical → Emergency），不实现多级升级阶梯 |
//! | **D23** | 配置以结构体注入（不解析 TOML；蓝图 `configs/alarm_rules.toml` 留待 v0.26.0 配置管理集成） |
//! | **D24** | 不要求 `Send + Sync`（no_std 单线程，与 v0.51.0 D2 一致） |
//! | **D25** | 时间戳使用 `u64` 毫秒参数注入（与 v0.50.0~v0.53.0 D1 一致） |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，依赖 `eneros-upa-model`（纯数据模型）。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod error;
pub mod escalation;
pub mod level;
pub mod manager;
pub mod record;
pub mod suppression;

pub use error::AlarmError;
pub use escalation::EscalationPolicy;
pub use level::AlarmLevel;
pub use manager::{AlarmManager, AlarmStats};
pub use record::{AlarmId, AlarmRecord, AlarmState};
pub use suppression::{SuppressionRule, SuppressionWindow};

#[cfg(test)]
mod tests {
    //! 集成测试 — 覆盖告警管理全链路（T1~T15）.

    use super::*;

    // ===== T1：AlarmLevel 排序与转换 =====
    #[test]
    fn test_t1_alarm_level_ordering() {
        assert!(AlarmLevel::Info < AlarmLevel::Warning);
        assert!(AlarmLevel::Warning < AlarmLevel::Critical);
        assert!(AlarmLevel::Critical < AlarmLevel::Emergency);
        assert_eq!(AlarmLevel::Info.as_u8(), 0);
        assert_eq!(AlarmLevel::Warning.as_u8(), 1);
        assert_eq!(AlarmLevel::Critical.as_u8(), 2);
        assert_eq!(AlarmLevel::Emergency.as_u8(), 3);
        assert_eq!(AlarmLevel::from_u8(0), Some(AlarmLevel::Info));
        assert_eq!(AlarmLevel::from_u8(3), Some(AlarmLevel::Emergency));
        assert_eq!(AlarmLevel::from_u8(9), None);
    }

    // ===== T2：AlarmRecord 构造 =====
    #[test]
    fn test_t2_alarm_record_construction() {
        let r = AlarmRecord::new(1, AlarmLevel::Warning, "dev1/temp", "温度高", 1_000);
        assert_eq!(r.id, 1);
        assert_eq!(r.level, AlarmLevel::Warning);
        assert_eq!(r.source, "dev1/temp");
        assert_eq!(r.description, "温度高");
        assert_eq!(r.raised_at_ms, 1_000);
        assert!(r.acknowledged_at_ms.is_none());
        assert!(r.cleared_at_ms.is_none());
        assert!(r.escalated_from.is_none());
        assert_eq!(r.state, AlarmState::Active);
        assert!(r.is_active());
        assert!(!r.is_escalated());
    }

    // ===== T3：AlarmState 状态转换（raise→Active, acknowledge→Acknowledged, clear→Cleared）=====
    #[test]
    fn test_t3_alarm_state_transitions() {
        let mut m = AlarmManager::new();
        let id = m.raise(AlarmLevel::Critical, "s", "d", 0).unwrap();
        // Active
        let active = m.query_active();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].state, AlarmState::Active);
        // Acknowledged
        m.acknowledge(id, 100).unwrap();
        let active = m.query_active();
        assert_eq!(active[0].state, AlarmState::Acknowledged);
        assert_eq!(active[0].acknowledged_at_ms, Some(100));
        // Cleared（转入 history）
        m.clear(id, 200).unwrap();
        assert_eq!(m.query_active().len(), 0);
        let hist = m.query_history(0, u64::MAX);
        assert_eq!(hist.len(), 1);
        assert_eq!(hist[0].state, AlarmState::Cleared);
        assert_eq!(hist[0].cleared_at_ms, Some(200));
    }

    // ===== T4：AlarmManager raise + query_active =====
    #[test]
    fn test_t4_alarm_manager_raise_and_query_active() {
        let mut m = AlarmManager::new();
        m.raise(AlarmLevel::Info, "a", "d1", 100).unwrap();
        m.raise(AlarmLevel::Warning, "b", "d2", 200).unwrap();
        m.raise(AlarmLevel::Critical, "c", "d3", 300).unwrap();
        let active = m.query_active();
        assert_eq!(active.len(), 3);
        // 按 raised_at_ms 升序
        assert_eq!(active[0].raised_at_ms, 100);
        assert_eq!(active[1].raised_at_ms, 200);
        assert_eq!(active[2].raised_at_ms, 300);
    }

    // ===== T5：AlarmManager acknowledge =====
    #[test]
    fn test_t5_alarm_manager_acknowledge() {
        let mut m = AlarmManager::new();
        let id = m.raise(AlarmLevel::Critical, "s", "d", 0).unwrap();
        m.acknowledge(id, 100).unwrap();
        let active = m.query_active();
        assert_eq!(active[0].state, AlarmState::Acknowledged);
        assert_eq!(active[0].acknowledged_at_ms, Some(100));
    }

    // ===== T6：AlarmManager clear 后转入 history =====
    #[test]
    fn test_t6_alarm_manager_clear_transfers_to_history() {
        let mut m = AlarmManager::new();
        let id = m.raise(AlarmLevel::Warning, "s", "d", 0).unwrap();
        m.clear(id, 100).unwrap();
        assert_eq!(m.query_active().len(), 0);
        assert_eq!(m.query_history(0, u64::MAX).len(), 1);
    }

    // ===== T7：未知 ID acknowledge 返回 NotFound =====
    #[test]
    fn test_t7_acknowledge_unknown_id_returns_not_found() {
        let mut m = AlarmManager::new();
        let r = m.acknowledge(999, 0);
        assert!(matches!(r, Err(AlarmError::NotFound)));
    }

    // ===== T8：已清除再次 clear 返回 NotFound（已从 active 移除）=====
    #[test]
    fn test_t8_clear_already_cleared_returns_not_found() {
        let mut m = AlarmManager::new();
        let id = m.raise(AlarmLevel::Warning, "s", "d", 0).unwrap();
        // 第一次 clear 成功
        assert!(m.clear(id, 100).is_ok());
        assert_eq!(m.query_active().len(), 0);
        assert_eq!(m.query_history(0, u64::MAX).len(), 1);
        // 第二次 clear 同一 id → NotFound（已从 active 移除）
        let r = m.clear(id, 200);
        assert!(matches!(r, Err(AlarmError::NotFound)));
    }

    // ===== T9：SuppressionRule 同源窗口内第 2 次抑制 =====
    #[test]
    fn test_t9_suppression_same_source_within_window() {
        let mut m = AlarmManager::new();
        m.add_suppression_rule(SuppressionRule::new("temp", 5_000));
        // 第一次 raise 成功
        let r1 = m.raise(AlarmLevel::Critical, "temp", "高温", 0);
        assert!(r1.is_ok());
        // 5 秒内第二次 raise 被抑制
        let r2 = m.raise(AlarmLevel::Critical, "temp", "高温", 1_000);
        assert!(matches!(r2, Err(AlarmError::Suppressed)));
        assert_eq!(m.stats().total_suppressed, 1);
    }

    // ===== T10：SuppressionRule 超时窗口后允许 =====
    #[test]
    fn test_t10_suppression_after_window_expires() {
        let mut m = AlarmManager::new();
        m.add_suppression_rule(SuppressionRule::new("temp", 5_000));
        // 第一次 raise 成功（t=0）
        assert!(m.raise(AlarmLevel::Critical, "temp", "高温", 0).is_ok());
        // 第二次 raise 被抑制（t=1000）
        assert!(matches!(
            m.raise(AlarmLevel::Critical, "temp", "高温", 1_000),
            Err(AlarmError::Suppressed)
        ));
        // 窗口过期后第三次 raise 成功（t=6000，cutoff=1000，t=0 被驱逐）
        let r3 = m.raise(AlarmLevel::Critical, "temp", "高温", 6_000);
        assert!(r3.is_ok());
    }

    // ===== T11：EscalationPolicy 检测超时升级 =====
    #[test]
    fn test_t11_escalation_policy_check() {
        let policy = EscalationPolicy::new(AlarmLevel::Critical, AlarmLevel::Emergency, 300_000);
        let record = AlarmRecord::new(1, AlarmLevel::Critical, "s", "d", 0);
        // 未超时 → None
        assert!(policy.check_escalation(&record, 299_999).is_none());
        // 超时 → Some(Emergency)
        assert_eq!(
            policy.check_escalation(&record, 300_000),
            Some(AlarmLevel::Emergency)
        );
        // 已 ACK → None（Acknowledged 状态）
        let mut acked = record.clone();
        acked.state = AlarmState::Acknowledged;
        acked.acknowledged_at_ms = Some(100_000);
        assert!(policy.check_escalation(&acked, 400_000).is_none());
        // 级别不匹配 → None
        let other = AlarmRecord::new(2, AlarmLevel::Warning, "s", "d", 0);
        assert!(policy.check_escalation(&other, 400_000).is_none());
    }

    // ===== T12：AlarmManager escalate 手动升级 =====
    #[test]
    fn test_t12_alarm_manager_escalate_manual() {
        let mut m = AlarmManager::new();
        m.add_escalation_policy(EscalationPolicy::new(
            AlarmLevel::Critical,
            AlarmLevel::Emergency,
            300_000,
        ));
        let id = m.raise(AlarmLevel::Critical, "s", "d", 0).unwrap();
        m.escalate(id, 100).unwrap();
        let active = m.query_active();
        assert_eq!(active[0].level, AlarmLevel::Emergency);
        assert_eq!(active[0].escalated_from, Some(AlarmLevel::Critical));
        assert!(active[0].is_escalated());
        // 再次升级 → AlreadyEscalated
        let r = m.escalate(id, 200);
        assert!(matches!(r, Err(AlarmError::AlreadyEscalated)));
    }

    // ===== T13：AlarmManager check_auto_escalate 批量升级 =====
    #[test]
    fn test_t13_alarm_manager_check_auto_escalate() {
        let mut m = AlarmManager::new();
        m.add_escalation_policy(EscalationPolicy::new(
            AlarmLevel::Critical,
            AlarmLevel::Emergency,
            300_000,
        ));
        let id1 = m.raise(AlarmLevel::Critical, "s1", "d1", 0).unwrap();
        let id2 = m.raise(AlarmLevel::Critical, "s2", "d2", 0).unwrap();
        // 未超时 → 空列表
        let ids = m.check_auto_escalate(299_999);
        assert!(ids.is_empty());
        // 超时 → 2 个 id
        let ids = m.check_auto_escalate(300_000);
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&id1));
        assert!(ids.contains(&id2));
        // 验证升级后级别
        let active = m.query_active();
        for r in active {
            assert_eq!(r.level, AlarmLevel::Emergency);
            assert_eq!(r.escalated_from, Some(AlarmLevel::Critical));
        }
        assert_eq!(m.stats().total_escalated, 2);
    }

    // ===== T14：AlarmManager query_history 时间范围 =====
    #[test]
    fn test_t14_alarm_manager_query_history() {
        let mut m = AlarmManager::new();
        // raise + clear 3 个告警，raised_at 分别为 1000, 3000, 10000
        let id1 = m.raise(AlarmLevel::Info, "s1", "d1", 1_000).unwrap();
        m.clear(id1, 1_500).unwrap();
        let id2 = m.raise(AlarmLevel::Warning, "s2", "d2", 3_000).unwrap();
        m.clear(id2, 3_500).unwrap();
        let id3 = m.raise(AlarmLevel::Critical, "s3", "d3", 10_000).unwrap();
        m.clear(id3, 10_500).unwrap();
        // [1000, 5000] → 2 条（id1=1000, id2=3000）
        let hist = m.query_history(1_000, 5_000);
        assert_eq!(hist.len(), 2);
        // [0, MAX] → 3 条
        let hist = m.query_history(0, u64::MAX);
        assert_eq!(hist.len(), 3);
        // [5000, 10000] → 1 条（id3=10000）
        let hist = m.query_history(5_000, 10_000);
        assert_eq!(hist.len(), 1);
    }

    // ===== T15：AlarmManager stats 统计 =====
    #[test]
    fn test_t15_alarm_manager_stats() {
        let mut m = AlarmManager::new();
        m.add_escalation_policy(EscalationPolicy::new(
            AlarmLevel::Critical,
            AlarmLevel::Emergency,
            300_000,
        ));
        // raise 3
        let id1 = m.raise(AlarmLevel::Critical, "s1", "d1", 0).unwrap();
        let id2 = m.raise(AlarmLevel::Warning, "s2", "d2", 0).unwrap();
        let id3 = m.raise(AlarmLevel::Critical, "s3", "d3", 0).unwrap();
        // acknowledge 1
        m.acknowledge(id1, 100).unwrap();
        // clear 1
        m.clear(id2, 200).unwrap();
        // escalate 1
        m.escalate(id3, 300).unwrap();
        let s = m.stats();
        assert_eq!(s.total_raised, 3);
        assert_eq!(s.total_acknowledged, 1);
        assert_eq!(s.total_cleared, 1);
        assert_eq!(s.total_escalated, 1);
        assert_eq!(s.active_count, 2);
        assert_eq!(s.total_suppressed, 0);
    }
}

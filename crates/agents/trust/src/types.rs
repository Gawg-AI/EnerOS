//! 信誉体系核心类型 (workspace v0.192.0).

use alloc::string::String;

/// Agent 唯一标识符.
pub type AgentId = u64;

/// 信任分（0~1000）.
pub type TrustScore = u16;

/// 初始信任分.
pub const INITIAL_SCORE: TrustScore = 500;

/// 最高信任分.
pub const MAX_SCORE: TrustScore = 1000;

/// 最低信任分.
pub const MIN_SCORE: TrustScore = 0;

/// 封禁阈值（低于此值封禁）.
pub const BLOCK_THRESHOLD: TrustScore = 100;

/// 信誉等级.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TrustGrade {
    /// 封禁（0-99）.
    Blocked,
    /// 铜（100-399）.
    Bronze,
    /// 银（400-699）.
    Silver,
    /// 金（700-899）.
    Gold,
    /// 白金（900-1000）.
    Platinum,
}

impl TrustGrade {
    /// 从分数计算信誉等级.
    pub fn from_score(score: TrustScore) -> Self {
        match score {
            900..=1000 => TrustGrade::Platinum,
            700..=899 => TrustGrade::Gold,
            400..=699 => TrustGrade::Silver,
            100..=399 => TrustGrade::Bronze,
            _ => TrustGrade::Blocked,
        }
    }

    /// 是否被封禁.
    pub fn is_blocked(&self) -> bool {
        *self == TrustGrade::Blocked
    }

    /// 等级名称.
    pub fn name(&self) -> &'static str {
        match self {
            TrustGrade::Platinum => "白金",
            TrustGrade::Gold => "金",
            TrustGrade::Silver => "银",
            TrustGrade::Bronze => "铜",
            TrustGrade::Blocked => "封禁",
        }
    }
}

/// Agent 行为类型.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionType {
    /// 任务完成.
    TaskComplete,
    /// 任务失败.
    TaskFailed,
    /// 越权访问.
    Unauthorized,
    /// 恶意行为.
    MaliciousBehavior,
    /// 崩溃.
    Crash,
    /// 恢复.
    Recovery,
    /// 协作.
    Cooperation,
}

/// 行为影响.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionImpact {
    /// 正面影响.
    Positive,
    /// 中性影响.
    Neutral,
    /// 负面影响.
    Negative,
}

/// Agent 行为记录.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentAction {
    /// Agent ID.
    pub agent_id: AgentId,
    /// 行为类型.
    pub action_type: ActionType,
    /// 时间戳.
    pub timestamp: u64,
    /// 是否成功.
    pub success: bool,
    /// 影响程度.
    pub impact: ActionImpact,
    /// 备注.
    pub note: String,
}

impl AgentAction {
    /// 创建新的行为记录.
    pub fn new(agent_id: AgentId, action_type: ActionType, timestamp: u64, success: bool) -> Self {
        let impact = match action_type {
            ActionType::TaskComplete | ActionType::Recovery | ActionType::Cooperation => {
                ActionImpact::Positive
            }
            ActionType::TaskFailed | ActionType::Crash => ActionImpact::Negative,
            ActionType::Unauthorized | ActionType::MaliciousBehavior => ActionImpact::Negative,
        };
        Self {
            agent_id,
            action_type,
            timestamp,
            success,
            impact,
            note: String::new(),
        }
    }

    /// 添加备注.
    pub fn with_note(mut self, note: String) -> Self {
        self.note = note;
        self
    }
}

/// 信任趋势.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustTrend {
    /// 上升.
    Rising,
    /// 稳定.
    Stable,
    /// 下降.
    Declining,
}

/// 信誉变化事件（审计日志）.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustEvent {
    /// Agent ID.
    pub agent_id: AgentId,
    /// 变化前分数.
    pub old_score: TrustScore,
    /// 变化后分数.
    pub new_score: TrustScore,
    /// 变化前等级.
    pub old_grade: TrustGrade,
    /// 变化后等级.
    pub new_grade: TrustGrade,
    /// 触发行为.
    pub action_type: ActionType,
    /// 时间戳.
    pub timestamp: u64,
}

/// 信誉统计.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrustStats {
    /// 注册 Agent 总数.
    pub total_agents: u32,
    /// 被封禁 Agent 数.
    pub blocked_agents: u32,
    /// 累计行为记录数.
    pub total_actions: u32,
    /// 累计等级变更数.
    pub total_grade_changes: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grade_from_score() {
        assert_eq!(TrustGrade::from_score(1000), TrustGrade::Platinum);
        assert_eq!(TrustGrade::from_score(900), TrustGrade::Platinum);
        assert_eq!(TrustGrade::from_score(899), TrustGrade::Gold);
        assert_eq!(TrustGrade::from_score(700), TrustGrade::Gold);
        assert_eq!(TrustGrade::from_score(699), TrustGrade::Silver);
        assert_eq!(TrustGrade::from_score(400), TrustGrade::Silver);
        assert_eq!(TrustGrade::from_score(399), TrustGrade::Bronze);
        assert_eq!(TrustGrade::from_score(100), TrustGrade::Bronze);
        assert_eq!(TrustGrade::from_score(99), TrustGrade::Blocked);
        assert_eq!(TrustGrade::from_score(0), TrustGrade::Blocked);
    }

    #[test]
    fn test_grade_ordering() {
        assert!(TrustGrade::Platinum > TrustGrade::Gold);
        assert!(TrustGrade::Gold > TrustGrade::Silver);
        assert!(TrustGrade::Silver > TrustGrade::Bronze);
        assert!(TrustGrade::Bronze > TrustGrade::Blocked);
    }

    #[test]
    fn test_grade_is_blocked() {
        assert!(TrustGrade::Blocked.is_blocked());
        assert!(!TrustGrade::Bronze.is_blocked());
    }

    #[test]
    fn test_agent_action_creation() {
        let action = AgentAction::new(1, ActionType::TaskComplete, 1000, true);
        assert_eq!(action.impact, ActionImpact::Positive);

        let action = AgentAction::new(1, ActionType::MaliciousBehavior, 1000, false);
        assert_eq!(action.impact, ActionImpact::Negative);
    }

    #[test]
    fn test_grade_name() {
        assert_eq!(TrustGrade::Platinum.name(), "白金");
        assert_eq!(TrustGrade::Blocked.name(), "封禁");
    }
}

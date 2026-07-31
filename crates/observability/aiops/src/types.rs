//! AI 运维核心类型定义。

use alloc::string::String;
use alloc::vec::Vec;

/// 风险等级
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLevel {
    /// 低风险
    Low,
    /// 中风险
    Medium,
    /// 高风险
    High,
    /// 严重风险
    Critical,
}

/// 异常检测结果
#[derive(Debug, Clone)]
pub struct AnomalyResult {
    /// 指标名称
    pub metric: String,
    /// 异常得分 (0.0 ~ 1.0+)
    pub score: f32,
    /// 是否为异常
    pub is_anomaly: bool,
    /// 时间戳
    pub ts: u64,
    /// 解释说明
    pub explanation: String,
}

/// 预测性维护结果
#[derive(Debug, Clone)]
pub struct PredictionResult {
    /// 资产 ID
    pub asset_id: u64,
    /// 剩余使用寿命
    pub remaining_useful_life: f64,
    /// 置信度
    pub confidence: f32,
    /// 风险等级
    pub risk_level: RiskLevel,
}

/// 自愈动作类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealActionType {
    /// 重启组件
    RestartComponent,
    /// 切换到备用
    SwitchToBackup,
    /// 限流
    ThrottleLoad,
    /// 清除缓存
    ClearCache,
    /// 水平扩展
    ScaleOut,
    /// 通知人工
    NotifyHuman,
}

/// 自愈动作
#[derive(Debug, Clone)]
pub struct HealAction {
    /// 动作类型
    pub action_type: HealActionType,
    /// 目标 ID
    pub target: u64,
    /// 描述
    pub description: String,
}

/// 自愈计划
#[derive(Debug, Clone)]
pub struct HealPlan {
    /// 动作列表
    pub actions: Vec<HealAction>,
    /// 是否需要审批
    pub requires_approval: bool,
    /// 回滚动作
    pub rollback: Option<HealAction>,
}

/// 自愈执行结果
#[derive(Debug, Clone)]
pub struct HealOutcome {
    /// 是否成功
    pub success: bool,
    /// 执行耗时 (ms)
    pub duration_ms: u64,
    /// 副作用列表
    pub side_effects: Vec<String>,
}

#[cfg(test)]
mod tests {
    use alloc::format;
    use alloc::vec;

    use super::*;

    #[test]
    fn test_risk_level_equality() {
        assert_eq!(RiskLevel::Low, RiskLevel::Low);
        assert_ne!(RiskLevel::Low, RiskLevel::Critical);
    }

    #[test]
    fn test_anomaly_result_creation() {
        let result = AnomalyResult {
            metric: String::from("cpu_usage"),
            score: 0.85,
            is_anomaly: true,
            ts: 1000,
            explanation: String::from("CPU 使用率异常偏高"),
        };
        assert!(result.is_anomaly);
        assert!(result.score > 0.5);
        assert_eq!(result.ts, 1000);
    }

    #[test]
    fn test_prediction_result_creation() {
        let result = PredictionResult {
            asset_id: 42,
            remaining_useful_life: 120.5,
            confidence: 0.92,
            risk_level: RiskLevel::Medium,
        };
        assert_eq!(result.asset_id, 42);
        assert_eq!(result.risk_level, RiskLevel::Medium);
    }

    #[test]
    fn test_heal_plan_with_rollback() {
        let plan = HealPlan {
            actions: vec![HealAction {
                action_type: HealActionType::RestartComponent,
                target: 1,
                description: format!("重启组件 {}", 1),
            }],
            requires_approval: true,
            rollback: Some(HealAction {
                action_type: HealActionType::SwitchToBackup,
                target: 1,
                description: String::from("回滚到备用组件"),
            }),
        };
        assert!(plan.requires_approval);
        assert!(plan.rollback.is_some());
        assert_eq!(plan.actions.len(), 1);
    }

    #[test]
    fn test_heal_outcome() {
        let outcome = HealOutcome {
            success: true,
            duration_ms: 250,
            side_effects: vec![String::from("短暂服务中断")],
        };
        assert!(outcome.success);
        assert_eq!(outcome.duration_ms, 250);
        assert_eq!(outcome.side_effects.len(), 1);
    }
}

//! 全局数字孪生核心类型定义
//!
//! 定义多域联合仿真所需的领域类型、耦合变量、边界条件及优化结果等。

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;

/// 领域类型枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainKind {
    /// 储能
    Storage,
    /// 光伏
    Pv,
    /// 风电
    Wind,
    /// 负荷
    Load,
    /// 电网
    Grid,
    /// 市场
    Market,
}

/// 领域状态
#[derive(Debug, Clone)]
pub struct DomainState {
    /// 领域 ID
    pub domain_id: u64,
    /// 领域类型
    pub kind: DomainKind,
    /// 功率 (kW)
    pub power_kw: f64,
    /// 荷电状态 (0.0 ~ 1.0)
    pub soc: f64,
    /// 状态描述
    pub status: String,
}

/// 耦合变量 — 跨域交互数据
#[derive(Debug, Clone)]
pub struct CouplingVar {
    /// 变量名称
    pub name: String,
    /// 源领域 ID
    pub source_domain: u64,
    /// 目标领域 ID
    pub target_domain: u64,
    /// 当前值
    pub value: f64,
}

/// 领域边界 — 描述领域间耦合与交换限制
#[derive(Debug, Clone)]
pub struct DomainBoundary {
    /// 耦合变量列表
    pub coupling_vars: Vec<CouplingVar>,
    /// 交换功率上限 (kW)
    pub exchange_limit_kw: f64,
}

/// 领域孪生体 — 单个领域完整的数字孪生实例
#[derive(Debug, Clone)]
pub struct DomainTwin {
    /// 领域 ID
    pub domain_id: u64,
    /// 领域类型
    pub kind: DomainKind,
    /// 当前状态
    pub state: DomainState,
    /// 边界条件
    pub boundary: DomainBoundary,
}

/// 模型融合策略
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FusionStrategy {
    /// 加权平均
    WeightedAverage,
    /// 贝叶斯融合
    BayesianFusion,
    /// 神经代理模型
    NeuralSurrogate,
}

/// 求解器类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SolverKind {
    /// HiGHS 开源求解器
    HiGHS,
    /// Gurobi 商业求解器
    Gurobi,
    /// COIN-OR CBC
    Cbc,
}

/// 优化目标类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectiveKind {
    /// 最大化收益
    MaxRevenue,
    /// 最小化成本
    MinCost,
    /// 最大化自消纳
    MaxSelfConsumption,
    /// 多目标
    MultiObjective,
}

/// 优化结果
#[derive(Debug, Clone)]
pub struct OptimizationResult {
    /// 目标函数值
    pub objective_value: f64,
    /// 相对单域优化提升百分比
    pub improvement_pct: f64,
    /// 是否可行
    pub feasible: bool,
    /// 使用的求解器
    pub solver: SolverKind,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_domain_kind_equality() {
        assert_eq!(DomainKind::Storage, DomainKind::Storage);
        assert_ne!(DomainKind::Pv, DomainKind::Wind);
    }

    #[test]
    fn test_domain_state_creation() {
        let state = DomainState {
            domain_id: 1,
            kind: DomainKind::Storage,
            power_kw: 100.0,
            soc: 0.85,
            status: String::from("running"),
        };
        assert_eq!(state.domain_id, 1);
        assert_eq!(state.kind, DomainKind::Storage);
        assert!((state.power_kw - 100.0).abs() < f64::EPSILON);
        assert!((state.soc - 0.85).abs() < f64::EPSILON);
    }

    #[test]
    fn test_coupling_var() {
        let cv = CouplingVar {
            name: String::from("pcc_flow"),
            source_domain: 1,
            target_domain: 5,
            value: 42.5,
        };
        assert_eq!(cv.source_domain, 1);
        assert_eq!(cv.target_domain, 5);
        assert!((cv.value - 42.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_domain_boundary() {
        let boundary = DomainBoundary {
            coupling_vars: Vec::new(),
            exchange_limit_kw: 500.0,
        };
        assert!(boundary.coupling_vars.is_empty());
        assert!((boundary.exchange_limit_kw - 500.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_domain_twin() {
        let twin = DomainTwin {
            domain_id: 3,
            kind: DomainKind::Wind,
            state: DomainState {
                domain_id: 3,
                kind: DomainKind::Wind,
                power_kw: 250.0,
                soc: 0.0,
                status: String::from("generating"),
            },
            boundary: DomainBoundary {
                coupling_vars: Vec::new(),
                exchange_limit_kw: 300.0,
            },
        };
        assert_eq!(twin.domain_id, 3);
        assert_eq!(twin.kind, DomainKind::Wind);
    }

    #[test]
    fn test_fusion_strategy() {
        assert_eq!(
            FusionStrategy::WeightedAverage,
            FusionStrategy::WeightedAverage
        );
        assert_ne!(
            FusionStrategy::BayesianFusion,
            FusionStrategy::NeuralSurrogate
        );
    }

    #[test]
    fn test_solver_kind() {
        assert_eq!(SolverKind::HiGHS, SolverKind::HiGHS);
        assert_ne!(SolverKind::Gurobi, SolverKind::Cbc);
    }

    #[test]
    fn test_objective_kind() {
        assert_eq!(ObjectiveKind::MaxRevenue, ObjectiveKind::MaxRevenue);
        assert_ne!(ObjectiveKind::MinCost, ObjectiveKind::MultiObjective);
    }

    #[test]
    fn test_optimization_result() {
        let result = OptimizationResult {
            objective_value: 1200.0,
            improvement_pct: 18.5,
            feasible: true,
            solver: SolverKind::HiGHS,
        };
        assert!(result.feasible);
        assert!(result.improvement_pct >= 15.0);
        assert_eq!(result.solver, SolverKind::HiGHS);
    }
}

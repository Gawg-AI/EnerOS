//! # eneros-twin-global — 全局数字孪生
//!
//! EnerOS v0.192.0 Phase 4 出口版：多域联合仿真 + 全局优化。
//!
//! ## 核心能力
//!
//! - **多域联合仿真** (`multi_domain`): 储能/光伏/风电/负荷/电网/市场 6 域协同
//! - **全局优化** (`optimizer`): 跨域联合优化，提升 ≥ 15%（Phase 4 出口指标）
//! - **模型融合** (`fusion`): 加权平均 / 贝叶斯 / 神经代理多策略融合
//! - **基准对比** (`benchmark`): 单域 vs 全局优化效果量化评估
//!
//! ## Phase 4 出口标准
//!
//! 全局优化相较单域优化提升 ≥ 15%，通过 `benchmark::meets_exit_criteria` 验证。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod benchmark;
pub mod fusion;
pub mod multi_domain;
pub mod optimizer;
pub mod types;

pub use benchmark::{compare, meets_exit_criteria, BenchmarkResult};
pub use fusion::ModelFusion;
pub use multi_domain::MultiDomainSimulator;
pub use optimizer::GlobalOptimizer;
pub use types::{
    CouplingVar, DomainBoundary, DomainKind, DomainState, DomainTwin, FusionStrategy,
    ObjectiveKind, OptimizationResult, SolverKind,
};

/// 全局数字孪生错误类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TwinError {
    /// 领域未找到
    DomainNotFound,
    /// 优化失败
    OptimizationFailed,
    /// 不可行解
    Infeasible,
    /// 融合错误
    FusionError,
    /// 仿真错误
    SimulationError,
    /// SEC-003：求解器后端未验证（Mock 后端禁止产生出口指标）
    SolverNotVerified,
    /// SEC-003：真实求解器调用失败
    SolverFailed,
}

impl core::fmt::Display for TwinError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::DomainNotFound => write!(f, "domain not found"),
            Self::OptimizationFailed => write!(f, "optimization failed"),
            Self::Infeasible => write!(f, "infeasible solution"),
            Self::FusionError => write!(f, "fusion error"),
            Self::SimulationError => write!(f, "simulation error"),
            Self::SolverNotVerified => write!(f, "solver backend not verified (SEC-003)"),
            Self::SolverFailed => write!(f, "solver backend call failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_twin_error_display() {
        assert_eq!(
            alloc::format!("{}", TwinError::DomainNotFound),
            "domain not found"
        );
        assert_eq!(
            alloc::format!("{}", TwinError::Infeasible),
            "infeasible solution"
        );
    }

    #[test]
    fn test_phase4_exit_integration() {
        // 集成测试: 仿真 → 优化 → 基准对比 → 验证出口标准
        let mut sim = MultiDomainSimulator::new();
        sim.simulate_step();

        let mut optimizer = GlobalOptimizer::new(SolverKind::HiGHS, ObjectiveKind::MaxRevenue);
        // SEC-003：Mock 后端拒绝产生出口指标——出口测试必须能对 Mock 失败。
        let err = optimizer.optimize(&sim.domains).unwrap_err();
        assert!(matches!(err, TwinError::SolverNotVerified));
    }
}

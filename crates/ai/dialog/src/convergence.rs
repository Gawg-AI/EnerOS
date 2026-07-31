//! 收敛判定.
//!
//! 提供 [`ConvergenceChecker`] trait 及两种实现：
//! - [`FeasibilityChecker`]：基于可行性判定
//! - [`ObjectiveChecker`]：基于目标函数改善量判定

use alloc::string::String;

use crate::types::{ConvergenceResult, Round};

/// 收敛检查器 trait.
pub trait ConvergenceChecker {
    /// 根据历史轮次判断是否收敛.
    fn check(&self, rounds: &[Round]) -> ConvergenceResult;
}

/// 可行性收敛检查器.
///
/// 当最后一轮的 `solver_feedback.feasible == true` 时判定收敛。
#[derive(Debug, Clone, Copy, Default)]
pub struct FeasibilityChecker;

impl ConvergenceChecker for FeasibilityChecker {
    fn check(&self, rounds: &[Round]) -> ConvergenceResult {
        let total = rounds.len() as u32;
        if let Some(last) = rounds.last() {
            if last.solver_feedback.feasible {
                return ConvergenceResult {
                    converged: true,
                    rounds: total,
                    reason: String::from("solver reports feasible solution"),
                };
            }
        }
        ConvergenceResult {
            converged: false,
            rounds: total,
            reason: String::from("solver has not found feasible solution yet"),
        }
    }
}

/// 目标函数收敛检查器.
///
/// 当最近两轮的目标函数改善量小于 epsilon 时判定收敛。
#[derive(Debug, Clone, Copy)]
pub struct ObjectiveChecker {
    /// 收敛阈值（目标函数改善量下限）.
    pub epsilon: f64,
}

impl ObjectiveChecker {
    /// 创建指定 epsilon 的检查器.
    #[must_use]
    pub fn new(epsilon: f64) -> Self {
        Self { epsilon }
    }
}

impl Default for ObjectiveChecker {
    fn default() -> Self {
        Self { epsilon: 1e-6 }
    }
}

impl ConvergenceChecker for ObjectiveChecker {
    fn check(&self, rounds: &[Round]) -> ConvergenceResult {
        let total = rounds.len() as u32;

        if rounds.len() < 2 {
            return ConvergenceResult {
                converged: false,
                rounds: total,
                reason: String::from("insufficient rounds for objective comparison"),
            };
        }

        let prev = &rounds[rounds.len() - 2];
        let last = &rounds[rounds.len() - 1];
        let improvement = (prev.solver_feedback.objective - last.solver_feedback.objective).abs();

        if improvement < self.epsilon {
            ConvergenceResult {
                converged: true,
                rounds: total,
                reason: String::from("objective improvement below epsilon"),
            }
        } else {
            ConvergenceResult {
                converged: false,
                rounds: total,
                reason: String::from("objective still improving"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use super::*;
    use crate::types::{LlmProposal, SolverFeedback};

    fn make_round(num: u32, feasible: bool, objective: f64) -> Round {
        Round {
            round_num: num,
            llm_proposal: LlmProposal {
                intent: String::from("test"),
                rationale: String::from("test"),
                confidence: 0.9,
            },
            solver_feedback: SolverFeedback {
                feasible,
                objective,
                violations: Vec::new(),
                suggested_adjustment: None,
            },
            converged: feasible,
        }
    }

    #[test]
    fn test_feasibility_checker_converged() {
        let checker = FeasibilityChecker;
        let rounds = vec![make_round(1, false, 100.0), make_round(2, true, 80.0)];
        let result = checker.check(&rounds);
        assert!(result.converged);
        assert_eq!(result.rounds, 2);
    }

    #[test]
    fn test_feasibility_checker_not_converged() {
        let checker = FeasibilityChecker;
        let rounds = vec![make_round(1, false, 100.0), make_round(2, false, 90.0)];
        let result = checker.check(&rounds);
        assert!(!result.converged);
    }

    #[test]
    fn test_feasibility_checker_empty() {
        let checker = FeasibilityChecker;
        let result = checker.check(&[]);
        assert!(!result.converged);
        assert_eq!(result.rounds, 0);
    }

    #[test]
    fn test_objective_checker_converged() {
        let checker = ObjectiveChecker::new(0.01);
        let rounds = vec![make_round(1, false, 100.0), make_round(2, false, 99.995)];
        let result = checker.check(&rounds);
        assert!(result.converged);
        assert_eq!(result.rounds, 2);
    }

    #[test]
    fn test_objective_checker_not_converged() {
        let checker = ObjectiveChecker::new(0.01);
        let rounds = vec![make_round(1, false, 100.0), make_round(2, false, 90.0)];
        let result = checker.check(&rounds);
        assert!(!result.converged);
    }

    #[test]
    fn test_objective_checker_insufficient_rounds() {
        let checker = ObjectiveChecker::new(0.01);
        let rounds = vec![make_round(1, false, 100.0)];
        let result = checker.check(&rounds);
        assert!(!result.converged);
        assert_eq!(result.rounds, 1);
    }

    #[test]
    fn test_objective_checker_default_epsilon() {
        let checker = ObjectiveChecker::default();
        assert_eq!(checker.epsilon, 1e-6);
    }
}

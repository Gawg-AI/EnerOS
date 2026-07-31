//! 对话协商核心类型定义.
//!
//! 包含 LLM↔Solver 多轮对话所需的所有数据结构：
//! 提案、反馈、轮次、问题、解、收敛结果等。

use alloc::string::String;
use alloc::vec::Vec;

/// 对话状态枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogState {
    /// 对话进行中.
    Running,
    /// 已收敛（找到可行解）.
    Converged,
    /// 对话失败.
    Failed,
    /// 超时（达到最大轮次）.
    Timeout,
}

/// LLM 生成的提案.
#[derive(Debug, Clone, PartialEq)]
pub struct LlmProposal {
    /// 意图描述.
    pub intent: String,
    /// 推理依据.
    pub rationale: String,
    /// 置信度 [0.0, 1.0].
    pub confidence: f32,
}

/// Solver 反馈.
#[derive(Debug, Clone, PartialEq)]
pub struct SolverFeedback {
    /// 是否可行.
    pub feasible: bool,
    /// 目标函数值.
    pub objective: f64,
    /// 约束违反列表.
    pub violations: Vec<String>,
    /// 建议调整方向.
    pub suggested_adjustment: Option<String>,
}

/// 单轮对话记录.
#[derive(Debug, Clone, PartialEq)]
pub struct Round {
    /// 轮次编号（从 1 开始）.
    pub round_num: u32,
    /// LLM 提案.
    pub llm_proposal: LlmProposal,
    /// Solver 反馈.
    pub solver_feedback: SolverFeedback,
    /// 本轮是否收敛.
    pub converged: bool,
}

/// 待求解问题描述.
#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    /// 问题描述.
    pub description: String,
    /// 约束条件列表.
    pub constraints: Vec<String>,
    /// 最大对话轮次.
    pub max_rounds: u32,
}

/// 最终求解结果.
#[derive(Debug, Clone, PartialEq)]
pub struct Solution {
    /// 求解方案（步骤列表）.
    pub plan: Vec<String>,
    /// 目标函数值.
    pub objective: f64,
    /// 使用的轮次数.
    pub rounds_used: u32,
}

/// 收敛判定结果.
#[derive(Debug, Clone, PartialEq)]
pub struct ConvergenceResult {
    /// 是否收敛.
    pub converged: bool,
    /// 经历的轮次数.
    pub rounds: u32,
    /// 收敛/未收敛原因.
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dialog_state_eq() {
        assert_eq!(DialogState::Running, DialogState::Running);
        assert_ne!(DialogState::Converged, DialogState::Failed);
        assert_ne!(DialogState::Timeout, DialogState::Running);
    }

    #[test]
    fn test_llm_proposal_creation() {
        let proposal = LlmProposal {
            intent: String::from("reduce peak load"),
            rationale: String::from("shift flexible loads to valley"),
            confidence: 0.85,
        };
        assert_eq!(proposal.confidence, 0.85);
        assert_eq!(proposal.intent, "reduce peak load");
    }

    #[test]
    fn test_solver_feedback_feasible() {
        let feedback = SolverFeedback {
            feasible: true,
            objective: 42.0,
            violations: Vec::new(),
            suggested_adjustment: None,
        };
        assert!(feedback.feasible);
        assert!(feedback.violations.is_empty());
        assert!(feedback.suggested_adjustment.is_none());
    }

    #[test]
    fn test_solver_feedback_infeasible() {
        let feedback = SolverFeedback {
            feasible: false,
            objective: -1.0,
            violations: alloc::vec![String::from("voltage limit exceeded")],
            suggested_adjustment: Some(String::from("reduce generation by 10%")),
        };
        assert!(!feedback.feasible);
        assert_eq!(feedback.violations.len(), 1);
        assert!(feedback.suggested_adjustment.is_some());
    }

    #[test]
    fn test_round_creation() {
        let round = Round {
            round_num: 1,
            llm_proposal: LlmProposal {
                intent: String::from("optimize dispatch"),
                rationale: String::from("cost minimization"),
                confidence: 0.9,
            },
            solver_feedback: SolverFeedback {
                feasible: true,
                objective: 100.5,
                violations: Vec::new(),
                suggested_adjustment: None,
            },
            converged: true,
        };
        assert_eq!(round.round_num, 1);
        assert!(round.converged);
    }

    #[test]
    fn test_problem_creation() {
        let problem = Problem {
            description: String::from("economic dispatch"),
            constraints: alloc::vec![String::from("power balance"), String::from("ramp rate"),],
            max_rounds: 10,
        };
        assert_eq!(problem.max_rounds, 10);
        assert_eq!(problem.constraints.len(), 2);
    }

    #[test]
    fn test_solution_creation() {
        let solution = Solution {
            plan: alloc::vec![String::from("step1"), String::from("step2")],
            objective: 99.9,
            rounds_used: 3,
        };
        assert_eq!(solution.rounds_used, 3);
        assert_eq!(solution.plan.len(), 2);
    }

    #[test]
    fn test_convergence_result() {
        let result = ConvergenceResult {
            converged: true,
            rounds: 5,
            reason: String::from("feasible solution found"),
        };
        assert!(result.converged);
        assert_eq!(result.rounds, 5);
    }
}

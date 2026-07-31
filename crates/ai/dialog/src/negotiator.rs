//! 对话协商器.
//!
//! 定义 [`DialogNegotiator`] trait 及 [`MockNegotiator`] 实现，
//! 模拟 LLM↔Solver 多轮迭代对话直至收敛或超时。

use alloc::format;
use alloc::vec;
use alloc::vec::Vec;

use crate::session::DialogSession;
use crate::types::{LlmProposal, Problem, Round, Solution, SolverFeedback};
use crate::DialogError;

/// 对话协商器 trait.
///
/// 实现者负责驱动 LLM↔Solver 多轮对话并返回最终解。
pub trait DialogNegotiator {
    /// 执行对话协商.
    ///
    /// # 错误
    ///
    /// 当达到最大轮次仍未收敛时返回 `DialogError::MaxRoundsExceeded`，
    /// 当 Solver 始终不可行时返回 `DialogError::SolverInfeasible`。
    fn run(&mut self, problem: &Problem) -> Result<Solution, DialogError>;
}

/// Mock 协商器：模拟 3 轮后收敛.
///
/// 前 2 轮 Solver 返回不可行，第 3 轮返回可行解。
#[derive(Debug, Default)]
pub struct MockNegotiator {
    /// 收敛所需轮次（默认 3）.
    pub converge_after: u32,
}

impl MockNegotiator {
    /// 创建 MockNegotiator，指定收敛轮次.
    #[must_use]
    pub fn new(converge_after: u32) -> Self {
        Self { converge_after }
    }

    /// 模拟 LLM 生成提案.
    fn mock_proposal(&self, round_num: u32) -> LlmProposal {
        LlmProposal {
            intent: format!("optimize round {}", round_num),
            rationale: format!("iterative refinement step {}", round_num),
            confidence: 0.5 + 0.15 * round_num as f32,
        }
    }

    /// 模拟 Solver 反馈.
    fn mock_feedback(&self, round_num: u32) -> SolverFeedback {
        let feasible = round_num >= self.converge_after;
        SolverFeedback {
            feasible,
            objective: 100.0 - (round_num as f64) * 10.0,
            violations: if feasible {
                Vec::new()
            } else {
                vec![format!("constraint violated at round {}", round_num)]
            },
            suggested_adjustment: if feasible {
                None
            } else {
                Some(format!("adjust parameters in round {}", round_num + 1))
            },
        }
    }
}

impl DialogNegotiator for MockNegotiator {
    fn run(&mut self, problem: &Problem) -> Result<Solution, DialogError> {
        if problem.max_rounds == 0 {
            return Err(DialogError::InvalidProblem);
        }

        let max_rounds = problem
            .max_rounds
            .min(self.converge_after.max(problem.max_rounds));
        let mut session = DialogSession::new(max_rounds);

        for round_num in 1..=problem.max_rounds {
            let proposal = self.mock_proposal(round_num);
            let feedback = self.mock_feedback(round_num);
            let converged = feedback.feasible;

            let round = Round {
                round_num,
                llm_proposal: proposal,
                solver_feedback: feedback,
                converged,
            };
            session.add_round(round);

            if session.is_complete() {
                break;
            }
        }

        match session.state {
            crate::types::DialogState::Converged => {
                let last = session.rounds.last().ok_or(DialogError::SolverInfeasible)?;
                Ok(Solution {
                    plan: vec![format!(
                        "solved '{}' in {} rounds",
                        problem.description, last.round_num
                    )],
                    objective: last.solver_feedback.objective,
                    rounds_used: last.round_num,
                })
            }
            crate::types::DialogState::Timeout => Err(DialogError::MaxRoundsExceeded),
            _ => Err(DialogError::SolverInfeasible),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_problem(max_rounds: u32) -> Problem {
        Problem {
            description: String::from("test dispatch"),
            constraints: vec![String::from("power balance")],
            max_rounds,
        }
    }

    #[test]
    fn test_mock_convergence() {
        let mut negotiator = MockNegotiator::new(3);
        let problem = make_problem(10);
        let result = negotiator.run(&problem);
        assert!(result.is_ok());
        let solution = result.unwrap();
        assert_eq!(solution.rounds_used, 3);
        assert_eq!(solution.objective, 70.0);
    }

    #[test]
    fn test_mock_timeout() {
        let mut negotiator = MockNegotiator::new(5);
        let problem = make_problem(3);
        let result = negotiator.run(&problem);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), DialogError::MaxRoundsExceeded);
    }

    #[test]
    fn test_invalid_problem() {
        let mut negotiator = MockNegotiator::new(3);
        let problem = make_problem(0);
        let result = negotiator.run(&problem);
        assert_eq!(result.unwrap_err(), DialogError::InvalidProblem);
    }

    #[test]
    fn test_converge_first_round() {
        let mut negotiator = MockNegotiator::new(1);
        let problem = make_problem(5);
        let result = negotiator.run(&problem);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().rounds_used, 1);
    }
}

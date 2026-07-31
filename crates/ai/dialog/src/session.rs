//! 对话会话管理.
//!
//! [`DialogSession`] 跟踪一次完整的 LLM↔Solver 对话过程，
//! 记录所有轮次并判断对话是否完成。

use alloc::vec::Vec;

use crate::types::{DialogState, Round};

/// 对话会话.
///
/// 管理一次 LLM↔Solver 多轮协商的完整生命周期。
#[derive(Debug, Clone)]
pub struct DialogSession {
    /// 会话唯一标识.
    pub session_id: u64,
    /// 已完成的对话轮次.
    pub rounds: Vec<Round>,
    /// 最大允许轮次.
    pub max_rounds: u32,
    /// 当前对话状态.
    pub state: DialogState,
}

impl DialogSession {
    /// 创建新会话.
    ///
    /// # 参数
    ///
    /// - `max_rounds`: 最大对话轮次上限
    #[must_use]
    pub fn new(max_rounds: u32) -> Self {
        Self {
            session_id: 0,
            rounds: Vec::new(),
            max_rounds,
            state: DialogState::Running,
        }
    }

    /// 添加一轮对话记录.
    ///
    /// 如果该轮标记为收敛，则更新状态为 `Converged`；
    /// 如果达到最大轮次仍未收敛，则标记为 `Timeout`。
    pub fn add_round(&mut self, round: Round) {
        let is_converged = round.converged;
        self.rounds.push(round);

        if is_converged {
            self.state = DialogState::Converged;
        } else if self.rounds.len() as u32 >= self.max_rounds {
            self.state = DialogState::Timeout;
        }
    }

    /// 判断对话是否已完成（收敛或超时）.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        matches!(self.state, DialogState::Converged | DialogState::Timeout)
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::String;

    use super::*;
    use crate::types::{LlmProposal, SolverFeedback};

    fn make_round(num: u32, converged: bool) -> Round {
        Round {
            round_num: num,
            llm_proposal: LlmProposal {
                intent: String::from("test intent"),
                rationale: String::from("test rationale"),
                confidence: 0.8,
            },
            solver_feedback: SolverFeedback {
                feasible: converged,
                objective: 50.0,
                violations: Vec::new(),
                suggested_adjustment: None,
            },
            converged,
        }
    }

    #[test]
    fn test_new_session() {
        let session = DialogSession::new(10);
        assert_eq!(session.max_rounds, 10);
        assert_eq!(session.state, DialogState::Running);
        assert!(session.rounds.is_empty());
        assert!(!session.is_complete());
    }

    #[test]
    fn test_add_round_converges() {
        let mut session = DialogSession::new(5);
        session.add_round(make_round(1, false));
        assert!(!session.is_complete());
        assert_eq!(session.state, DialogState::Running);

        session.add_round(make_round(2, true));
        assert!(session.is_complete());
        assert_eq!(session.state, DialogState::Converged);
        assert_eq!(session.rounds.len(), 2);
    }

    #[test]
    fn test_add_round_timeout() {
        let mut session = DialogSession::new(3);
        session.add_round(make_round(1, false));
        session.add_round(make_round(2, false));
        assert!(!session.is_complete());

        session.add_round(make_round(3, false));
        assert!(session.is_complete());
        assert_eq!(session.state, DialogState::Timeout);
    }

    #[test]
    fn test_session_id() {
        let mut session = DialogSession::new(5);
        session.session_id = 42;
        assert_eq!(session.session_id, 42);
    }
}

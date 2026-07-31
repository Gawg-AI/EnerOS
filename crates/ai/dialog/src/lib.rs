//! EnerOS LLM↔Solver 对话协商 (v0.189.0).
//!
//! 实现 LLM 与 Solver 之间的迭代对话式协商框架：
//! 多轮迭代推理 + 收敛判定，用于复杂能源优化问题的协同求解。
//!
//! # 核心模块
//!
//! - [`types`] — DialogState, LlmProposal, SolverFeedback, Round, Problem, Solution 等
//! - [`session`] — DialogSession 会话管理
//! - [`negotiator`] — DialogNegotiator trait + MockNegotiator
//! - [`convergence`] — ConvergenceChecker trait + FeasibilityChecker / ObjectiveChecker
//!
//! # 协商流程
//!
//! ```text
//! Problem → LLM 提案 → Solver 评估 → 反馈 → LLM 调整 → ... → 收敛/超时
//! ```
//!
//! # 收敛策略
//!
//! - 可行性收敛：Solver 报告可行解
//! - 目标收敛：连续两轮目标函数改善 < epsilon
//!
//! # no_std 合规
//!
//! `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 零第三方依赖，零 unsafe。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod convergence;
pub mod negotiator;
pub mod session;
pub mod types;

pub use convergence::{ConvergenceChecker, FeasibilityChecker, ObjectiveChecker};
pub use negotiator::{DialogNegotiator, MockNegotiator};
pub use session::DialogSession;
pub use types::{
    ConvergenceResult, DialogState, LlmProposal, Problem, Round, Solution, SolverFeedback,
};

/// 对话协商错误枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogError {
    /// 达到最大轮次仍未收敛.
    MaxRoundsExceeded,
    /// Solver 报告不可行.
    SolverInfeasible,
    /// LLM 推理错误.
    LlmError,
    /// 无效问题定义.
    InvalidProblem,
}

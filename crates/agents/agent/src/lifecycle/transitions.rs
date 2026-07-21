//! Agent 生命周期合法状态转换表 — TRANSITIONS / can_transition
//!
//! 定义 7 个生命周期状态（Created / Ready / Running / Suspended / Error / Recovering / Dead）
//! 之间的 12 条合法转换。
//!
//! # 设计
//! - 转换表为 `const` 静态切片，零运行时开销
//! - `can_transition` 通过线性扫描 `TRANSITIONS.contains` 实现（12 条，性能可接受）
//!
//! # 不变量
//! - **Dead 不可逆**：`Dead` 没有任何合法的传出转换
//! - **Error → Running 非法**：从 Error 恢复必须经过 `Error → Recovering → Ready → Running` 路径
//! - **自转换非法**：所有 `(s, s)` 对均不在表中（如 Created→Created）
//! - **不能回到 Created**：无任何转换的目标为 Created

use crate::types::AgentState;

/// 合法状态转换表（12 条）.
///
/// 顺序遵循蓝图 §4.1。
pub const TRANSITIONS: &[(AgentState, AgentState)] = &[
    (AgentState::Created, AgentState::Ready),
    (AgentState::Ready, AgentState::Running),
    (AgentState::Running, AgentState::Suspended),
    (AgentState::Running, AgentState::Error),
    (AgentState::Suspended, AgentState::Running),
    (AgentState::Suspended, AgentState::Error),
    (AgentState::Error, AgentState::Recovering),
    (AgentState::Recovering, AgentState::Ready),
    (AgentState::Recovering, AgentState::Dead),
    (AgentState::Error, AgentState::Dead),
    (AgentState::Running, AgentState::Dead),
    (AgentState::Ready, AgentState::Dead),
];

/// 查询状态转换是否合法.
///
/// 返回 `TRANSITIONS.contains(&(from, to))`。
pub fn can_transition(from: AgentState, to: AgentState) -> bool {
    TRANSITIONS.contains(&(from, to))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AgentState;

    #[test]
    fn test_all_12_legal_transitions() {
        // 每条 TRANSITIONS 表中的转换 must be accepted by can_transition
        for (from, to) in TRANSITIONS {
            assert!(
                can_transition(*from, *to),
                "legal transition {:?} -> {:?} should be allowed",
                from,
                to
            );
        }
    }

    #[test]
    fn test_created_to_running_illegal() {
        // 必须经过 Ready，不能 Created -> Running
        assert!(!can_transition(AgentState::Created, AgentState::Running));
    }

    #[test]
    fn test_error_to_running_illegal() {
        // 蓝图 §8.5: Error -> Running 非法（必须经过 Recovering -> Ready -> Running）
        assert!(!can_transition(AgentState::Error, AgentState::Running));
    }

    #[test]
    fn test_dead_to_anything_illegal() {
        // Dead 是终态，没有任何合法的传出转换
        let non_dead = [
            AgentState::Created,
            AgentState::Ready,
            AgentState::Running,
            AgentState::Suspended,
            AgentState::Error,
            AgentState::Recovering,
        ];
        for s in non_dead {
            assert!(
                !can_transition(AgentState::Dead, s),
                "Dead -> {:?} must be illegal (Dead is irreversible)",
                s
            );
        }
    }

    #[test]
    fn test_self_transitions_illegal() {
        // 自转换（s -> s）均不在合法转换表中
        let all_states = [
            AgentState::Created,
            AgentState::Ready,
            AgentState::Running,
            AgentState::Suspended,
            AgentState::Error,
            AgentState::Recovering,
            AgentState::Dead,
        ];
        for s in all_states {
            assert!(
                !can_transition(s, s),
                "self transition {:?} -> {:?} must be illegal",
                s,
                s
            );
        }
    }

    #[test]
    fn test_no_legal_transition_to_created() {
        // Created 是起始状态，无任何转换的目标为 Created（不能回到 Created）
        assert!(!can_transition(AgentState::Ready, AgentState::Created));
    }

    #[test]
    fn test_transitions_count() {
        // 蓝图 §4.1 定义 12 条合法转换
        assert_eq!(TRANSITIONS.len(), 12);
    }
}

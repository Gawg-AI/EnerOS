//! 冲突与死锁检测 — [`has_safety_conflict`] / [`detect_deadlock`]（v0.92.0）.
//!
//! - **D10**：安全冲突定义——`safety_critical` 请求数 ≥ 2 即冲突（仅标记，不阻断仲裁）。
//! - **D4**：[`detect_deadlock`] 纯计算（no_std 单线程惯例），基于 wait-for 图三色 DFS。
//! - 蓝图 §5.4/§6.5：资源竞争可能导致死锁，死锁检测用于域内协调决策辅助。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use eneros_agent::AgentId;

use crate::bid::Claim;

/// 判定一组请求是否存在安全冲突（D10：safety_critical 计数 ≥ 2）.
///
/// 即使夹杂非安全请求，只要安全请求 ≥ 2 即返回 `true`。
pub fn has_safety_conflict(claims: &[Claim]) -> bool {
    let mut n = 0u64;
    for c in claims {
        if c.safety_critical {
            n += 1;
            if n >= 2 {
                return true;
            }
        }
    }
    false
}

/// wait-for 图死锁检测（蓝图 §5.4/§6.5）.
///
/// 输入 `waits` 为边列表 `(a, b)`，语义：**a 等待 b**（a 正在等待 b 持有的资源）。
/// 检测 wait-for 图中是否存在有向环（含自环、多节点环）。
///
/// 算法：邻接表 + **三色 DFS**（0 白/1 灰/2 黑），
/// 遇到灰色节点即发现环；外层遍历所有节点覆盖不连通分量。
pub fn detect_deadlock(waits: &[(AgentId, AgentId)]) -> bool {
    if waits.is_empty() {
        return false;
    }

    // 邻接表：从源点出发的所有出边（目标节点）
    let mut adj: BTreeMap<AgentId, Vec<AgentId>> = BTreeMap::new();
    for (from, to) in waits {
        adj.entry(*from).or_default().push(*to);
    }
    // 也要把出现在目标位置但无出边的节点加入邻接表（保证外层遍历时覆盖）
    for (_, to) in waits {
        adj.entry(*to).or_default();
    }

    // 三色标记：0 白（未访问），1 灰（在递归栈），2 黑（已处理完，无环）
    let mut color: BTreeMap<AgentId, u8> = BTreeMap::new();

    fn dfs(
        node: AgentId,
        adj: &BTreeMap<AgentId, Vec<AgentId>>,
        color: &mut BTreeMap<AgentId, u8>,
    ) -> bool {
        color.insert(node, 1); // 灰
        if let Some(neighbors) = adj.get(&node) {
            for &next in neighbors {
                let c = *color.get(&next).unwrap_or(&0);
                if c == 1 {
                    // 遇到灰色节点 → 回边 → 环
                    return true;
                }
                if c == 0 && dfs(next, adj, color) {
                    return true;
                }
                // c == 2：黑节点，已处理完，继续
            }
        }
        color.insert(node, 2); // 黑
        false
    }

    for &node in adj.keys() {
        let c = *color.get(&node).unwrap_or(&0);
        if c == 0 && dfs(node, &adj, &mut color) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bid::{Claim, Priority};

    // ===== T31: 0 个 safety → false =====
    #[test]
    fn t31_no_safety_no_conflict() {
        let claims = [
            Claim {
                agent_id: AgentId(1),
                priority: Priority::Normal,
                bid: 1.0,
                safety_critical: false,
                deadline: 1000,
            },
            Claim {
                agent_id: AgentId(2),
                priority: Priority::Normal,
                bid: 2.0,
                safety_critical: false,
                deadline: 2000,
            },
        ];
        assert!(!has_safety_conflict(&claims));
    }

    // ===== T32: 1 个 safety → false =====
    #[test]
    fn t32_one_safety_no_conflict() {
        let claims = [
            Claim {
                agent_id: AgentId(1),
                priority: Priority::Normal,
                bid: 1.0,
                safety_critical: true,
                deadline: 1000,
            },
            Claim {
                agent_id: AgentId(2),
                priority: Priority::Normal,
                bid: 2.0,
                safety_critical: false,
                deadline: 2000,
            },
        ];
        assert!(!has_safety_conflict(&claims));
    }

    // ===== T33: 2 个 safety（夹杂非 safety）→ true =====
    #[test]
    fn t33_two_safety_conflict() {
        let claims = [
            Claim {
                agent_id: AgentId(1),
                priority: Priority::Normal,
                bid: 1.0,
                safety_critical: true,
                deadline: 1000,
            },
            Claim {
                agent_id: AgentId(2),
                priority: Priority::Normal,
                bid: 2.0,
                safety_critical: false,
                deadline: 2000,
            },
            Claim {
                agent_id: AgentId(3),
                priority: Priority::Normal,
                bid: 3.0,
                safety_critical: true,
                deadline: 3000,
            },
        ];
        assert!(has_safety_conflict(&claims));
    }

    // ===== T34: detect_deadlock 空边表 → false =====
    #[test]
    fn t34_deadlock_empty() {
        let waits: &[(AgentId, AgentId)] = &[];
        assert!(!detect_deadlock(waits));
    }

    // ===== T35: 链 a→b→c 无环 → false =====
    #[test]
    fn t35_chain_no_cycle() {
        let waits = [(AgentId(1), AgentId(2)), (AgentId(2), AgentId(3))];
        assert!(!detect_deadlock(&waits));
    }

    // ===== T36: 自环 (a,a) → true =====
    #[test]
    fn t36_self_loop_cycle() {
        let waits = [(AgentId(1), AgentId(1))];
        assert!(detect_deadlock(&waits));
    }

    // ===== T37: 2-cycle (a,b),(b,a) → true =====
    #[test]
    fn t37_two_cycle() {
        let waits = [(AgentId(1), AgentId(2)), (AgentId(2), AgentId(1))];
        assert!(detect_deadlock(&waits));
    }

    // ===== T38: 3-cycle (a,b),(b,c),(c,a) → true =====
    #[test]
    fn t38_three_cycle() {
        let waits = [
            (AgentId(1), AgentId(2)),
            (AgentId(2), AgentId(3)),
            (AgentId(3), AgentId(1)),
        ];
        assert!(detect_deadlock(&waits));
    }

    // ===== T39: 菱形 (a→b, a→c, b→d, c→d) 无环 → false（重复访问黑节点不误报）=====
    #[test]
    fn t39_diamond_no_cycle() {
        let waits = [
            (AgentId(1), AgentId(2)),
            (AgentId(1), AgentId(3)),
            (AgentId(2), AgentId(4)),
            (AgentId(3), AgentId(4)),
        ];
        assert!(!detect_deadlock(&waits));
    }

    // ===== T40: 不连通双分量：分量1 无环 + 分量2 有环 → true =====
    #[test]
    fn t40_disconnected_one_cycle() {
        let waits = [
            // 分量1：x→y（无环）
            (AgentId(10), AgentId(20)),
            // 分量2：p→q→p（有环）
            (AgentId(30), AgentId(40)),
            (AgentId(40), AgentId(30)),
        ];
        assert!(detect_deadlock(&waits));
    }
}

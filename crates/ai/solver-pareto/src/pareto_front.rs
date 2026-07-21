//! Pareto 前沿数据结构与核心算法（v0.104.0 T1）.
//!
//! 多目标问题/目标/解/前沿类型定义 + 支配判定 + 非支配过滤 + 决策者加权选择。
//! 全程统一最小化口径：Maximize 目标由评估出口取负归一（D7），调用方保证
//! `ParetoSolution.objectives` 已归一化。

use alloc::string::String;
use alloc::vec::Vec;

use eneros_solver_core::error::SolverError;

/// 多目标优化问题.
///
/// 由目标列表与决策变量上下界组成；界约束已由 `VariableSpec` 表达，
/// 不引入蓝图未定义的功能约束字段（D6）。
#[derive(Debug, Clone)]
pub struct MultiObjectiveProblem {
    /// 优化目标列表.
    pub objectives: Vec<Objective>,
    /// 决策变量上下界列表.
    pub variables: Vec<VariableSpec>,
}

/// 优化目标.
#[derive(Debug, Clone)]
pub struct Objective {
    /// 目标名称（如 "cost" / "carbon" / "lifespan"）.
    pub name: String,
    /// 优化方向.
    pub direction: OptDirection,
    /// 目标权重（决策者偏好先验）.
    pub weight: f64,
}

/// 优化方向.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OptDirection {
    /// 最小化.
    Minimize,
    /// 最大化（评估出口统一取负归一，D7）.
    Maximize,
}

/// 决策变量上下界.
#[derive(Debug, Clone, Copy)]
pub struct VariableSpec {
    /// 下界.
    pub lower: f64,
    /// 上界.
    pub upper: f64,
}

/// Pareto 解.
#[derive(Debug, Clone)]
pub struct ParetoSolution {
    /// 决策变量取值.
    pub variables: Vec<f64>,
    /// 目标值（统一最小化口径，D7）.
    pub objectives: Vec<f64>,
    /// 非支配层级（0 即非支配）.
    pub rank: usize,
    /// 拥挤度.
    pub crowding: f64,
}

/// Pareto 前沿.
#[derive(Debug, Clone, Default)]
pub struct ParetoFront {
    /// 前沿解集合.
    pub solutions: Vec<ParetoSolution>,
}

/// 多目标 Pareto 求解器 trait（D5：无 Send + Sync bound）.
pub trait ParetoSolver {
    /// 求解多目标问题，输出 Pareto 前沿（NSGA-II 为 rank == 0 集合）.
    ///
    /// 前沿为空时返回空 front（`is_empty()` 可判），LP 兜底由编排层负责（D10）。
    fn solve(
        &self,
        problem: &MultiObjectiveProblem,
        pop_size: usize,
        gen: usize,
    ) -> Result<ParetoFront, SolverError>;
}

/// 支配判定（统一最小化口径，调用方保证 objectives 已归一，D7）.
///
/// `a` 支配 `b` 当且仅当全目标 `a[k] <= b[k]` 且至少一项 `a[k] < b[k]`；
/// 相等向量互不支配。
pub fn dominates(a: &ParetoSolution, b: &ParetoSolution) -> bool {
    let mut strictly_better = false;
    for (x, y) in a.objectives.iter().zip(b.objectives.iter()) {
        if x > y {
            return false;
        }
        if x < y {
            strictly_better = true;
        }
    }
    strictly_better
}

impl ParetoFront {
    /// 返回 rank == 0 的非支配解引用.
    pub fn non_dominated(&self) -> Vec<&ParetoSolution> {
        self.solutions.iter().filter(|s| s.rank == 0).collect()
    }

    /// 前沿是否为空.
    pub fn is_empty(&self) -> bool {
        self.solutions.is_empty()
    }

    /// 前沿解数量.
    pub fn len(&self) -> usize {
        self.solutions.len()
    }

    /// 按权重选择加权和最小的解.
    ///
    /// 权重归一化（蓝图 §4.4）：逐值 `max(0.0)` clamp 负值；权重长度不足目标数
    /// 时缺省补 0（超出截断）；和为 0 时按均匀权重处理（每目标 1.0）。
    /// 空 front 返回 None；最小者选取使用 `f64::total_cmp`（D8），平手取首个。
    pub fn select_by_weight(&self, weights: &[f64]) -> Option<&ParetoSolution> {
        let first = self.solutions.first()?;
        let obj_len = first.objectives.len();
        let mut w: Vec<f64> = (0..obj_len)
            .map(|i| weights.get(i).copied().unwrap_or(0.0).max(0.0))
            .collect();
        // 权重均已 clamp 为非负，sum <= 0.0 即全零（含 NaN 经 max 归 0）.
        if w.iter().sum::<f64>() <= 0.0 {
            w.fill(1.0);
        }
        let score = |s: &ParetoSolution| -> f64 {
            s.objectives
                .iter()
                .zip(w.iter())
                .map(|(o, wi)| o * wi)
                .sum()
        };
        self.solutions
            .iter()
            .min_by(|a, b| score(a).total_cmp(&score(b)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solution(variables: &[f64], objectives: &[f64], rank: usize) -> ParetoSolution {
        ParetoSolution {
            variables: variables.to_vec(),
            objectives: objectives.to_vec(),
            rank,
            crowding: 0.0,
        }
    }

    /// PF1：Objective / OptDirection 构造与 PartialEq.
    #[test]
    fn pf1_objective_construction_and_direction_eq() {
        let obj = Objective {
            name: String::from("cost"),
            direction: OptDirection::Minimize,
            weight: 0.8,
        };
        assert_eq!(obj.name, "cost");
        assert_eq!(obj.direction, OptDirection::Minimize);
        assert_eq!(obj.weight, 0.8);
        assert_eq!(OptDirection::Minimize, OptDirection::Minimize);
        assert_eq!(OptDirection::Maximize, OptDirection::Maximize);
        assert_ne!(OptDirection::Minimize, OptDirection::Maximize);
    }

    /// PF2：VariableSpec 界字段.
    #[test]
    fn pf2_variable_spec_bounds() {
        let spec = VariableSpec {
            lower: 1.5,
            upper: 9.5,
        };
        assert_eq!(spec.lower, 1.5);
        assert_eq!(spec.upper, 9.5);
        let copied = spec;
        assert_eq!(copied.lower, 1.5);
        assert_eq!(copied.upper, 9.5);
    }

    /// PF3：dominates 全劣支配（a=[1,2] 支配 b=[2,3]，反向不支配）.
    #[test]
    fn pf3_dominates_all_worse() {
        let a = solution(&[0.0], &[1.0, 2.0], 0);
        let b = solution(&[0.0], &[2.0, 3.0], 0);
        assert!(dominates(&a, &b));
        assert!(!dominates(&b, &a));
    }

    /// PF4：单项更优即支配（a=[1,5] vs b=[2,5] → 支配）.
    #[test]
    fn pf4_dominates_single_better() {
        let a = solution(&[0.0], &[1.0, 5.0], 0);
        let b = solution(&[0.0], &[2.0, 5.0], 0);
        assert!(dominates(&a, &b));
        assert!(!dominates(&b, &a));
    }

    /// PF5：相等向量互不支配.
    #[test]
    fn pf5_equal_vectors_no_dominance() {
        let a = solution(&[0.0], &[1.0, 2.0], 0);
        let b = solution(&[0.0], &[1.0, 2.0], 0);
        assert!(!dominates(&a, &b));
        assert!(!dominates(&b, &a));
    }

    /// PF6：non_dominated 仅返回 rank == 0（ranks 0/1/0 → 返回 2 个）.
    #[test]
    fn pf6_non_dominated_filters_rank0() {
        let front = ParetoFront {
            solutions: alloc::vec![
                solution(&[0.0], &[1.0, 5.0], 0),
                solution(&[1.0], &[3.0, 3.0], 1),
                solution(&[2.0], &[5.0, 1.0], 0),
            ],
        };
        let nd = front.non_dominated();
        assert_eq!(nd.len(), 2);
        assert!(nd.iter().all(|s| s.rank == 0));
    }

    /// PF7：空 front is_empty / len == 0.
    #[test]
    fn pf7_empty_front() {
        let front = ParetoFront::default();
        assert!(front.is_empty());
        assert_eq!(front.len(), 0);
    }

    /// PF8：select_by_weight 加权最小（[0.8,0.2] → 选 [1.0,5.0]，加权和 1.8）.
    #[test]
    fn pf8_select_by_weight_min() {
        let front = ParetoFront {
            solutions: alloc::vec![
                solution(&[0.0], &[1.0, 5.0], 0),
                solution(&[1.0], &[3.0, 3.0], 0),
                solution(&[2.0], &[5.0, 1.0], 0),
            ],
        };
        // 0.8*1+0.2*5=1.8 < 0.8*3+0.2*3=3.0 < 0.8*5+0.2*1=4.2.
        let selected = front.select_by_weight(&[0.8, 0.2]);
        assert!(selected.is_some());
        if let Some(s) = selected {
            assert_eq!(s.objectives, alloc::vec![1.0, 5.0]);
        }
    }

    /// PF9：空 front select_by_weight 返回 None.
    #[test]
    fn pf9_empty_front_select_none() {
        let front = ParetoFront::default();
        assert!(front.select_by_weight(&[0.8, 0.2]).is_none());
    }

    /// PF10：负权重 clamp（[-1.0,1.0] 视同 [0.0,1.0]）+ 全零权重均匀不 panic.
    #[test]
    fn pf10_negative_clamp_and_zero_uniform() {
        let front = ParetoFront {
            solutions: alloc::vec![
                solution(&[0.0], &[1.0, 5.0], 0),
                solution(&[1.0], &[3.0, 3.0], 0),
                solution(&[2.0], &[5.0, 1.0], 0),
            ],
        };
        // [-1.0, 1.0] clamp 为 [0.0, 1.0] → 纯第二目标最小 → 选 [5.0, 1.0].
        let selected = front.select_by_weight(&[-1.0, 1.0]);
        assert!(selected.is_some());
        if let Some(s) = selected {
            assert_eq!(s.objectives, alloc::vec![5.0, 1.0]);
        }
        // 全零权重 → 均匀权重，不 panic 且返回某解.
        let selected = front.select_by_weight(&[0.0, 0.0]);
        assert!(selected.is_some());
    }
}

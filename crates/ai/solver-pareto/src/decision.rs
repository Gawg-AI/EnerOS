//! 决策者选择（v0.104.0，蓝图 §4.1 DecisionMaker）.
//!
//! 决策者持目标偏好权重，从 Pareto 前沿中选出最终方案；
//! 权重归一化（负值 clamp 0、全零按均匀、长度不足补 0）由
//! `ParetoFront::select_by_weight` 承担，本模块仅做委托。

use alloc::vec::Vec;

use crate::pareto_front::{ParetoFront, ParetoSolution};

/// 决策者（偏好权重 → 前沿加权选择）.
#[derive(Debug, Clone)]
pub struct DecisionMaker {
    /// 目标偏好权重（顺序与 problem.objectives 一致；负值 clamp 0，全零按均匀，长度不足补 0）.
    pub preferences: Vec<f64>,
}

impl DecisionMaker {
    /// 构造决策者.
    pub fn new(preferences: Vec<f64>) -> Self {
        Self { preferences }
    }

    /// 从前沿选择最终方案（归一化委托 `ParetoFront::select_by_weight`；空前沿返回 None）.
    pub fn choose<'a>(&self, front: &'a ParetoFront) -> Option<&'a ParetoSolution> {
        front.select_by_weight(&self.preferences)
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;

    /// 测试辅助：由目标值构造 ParetoSolution（variables 空、rank 0、crowding 0.0）.
    fn sol(objectives: &[f64]) -> ParetoSolution {
        ParetoSolution {
            variables: Vec::new(),
            objectives: objectives.to_vec(),
            rank: 0,
            crowding: 0.0,
        }
    }

    /// 测试辅助：3 解前沿（objectives [1,5] / [3,3] / [5,1]，最小化口径）.
    fn three_solution_front() -> ParetoFront {
        ParetoFront {
            solutions: vec![sol(&[1.0, 5.0]), sol(&[3.0, 3.0]), sol(&[5.0, 1.0])],
        }
    }

    /// DM23：preferences 归一化选择（[0.8,0.2] → 0.8*1+0.2*5=1.8 最小，选 [1.0,5.0]）.
    #[test]
    fn dm23_normalized_preference_select() {
        let front = three_solution_front();
        let dm = DecisionMaker::new(vec![0.8, 0.2]);
        let chosen = dm.choose(&front);
        assert!(chosen.is_some());
        if let Some(s) = chosen {
            assert_eq!(s.objectives, vec![1.0, 5.0]);
        }
    }

    /// DM24：纯成本偏好 [1.0,0.0] 选 [1.0,5.0]，纯碳偏好 [0.0,1.0] 选 [5.0,1.0]，两者不同.
    #[test]
    fn dm24_cost_vs_carbon_preference() {
        let front = three_solution_front();
        let cost_dm = DecisionMaker::new(vec![1.0, 0.0]);
        let carbon_dm = DecisionMaker::new(vec![0.0, 1.0]);
        let cost_chosen = cost_dm.choose(&front);
        let carbon_chosen = carbon_dm.choose(&front);
        assert!(cost_chosen.is_some());
        assert!(carbon_chosen.is_some());
        if let (Some(c), Some(k)) = (cost_chosen, carbon_chosen) {
            assert_eq!(c.objectives, vec![1.0, 5.0]);
            assert_eq!(k.objectives, vec![5.0, 1.0]);
            assert_ne!(c.objectives, k.objectives);
        }
    }

    /// DM25：全零偏好按均匀权重处理，不 panic 且返回 Some.
    #[test]
    fn dm25_zero_preferences_uniform() {
        let front = three_solution_front();
        let dm = DecisionMaker::new(vec![0.0, 0.0]);
        assert!(dm.choose(&front).is_some());
    }

    /// DM26：单目标退化为最小值选择（[3.0]/[1.0]/[2.0] → 选 [1.0]）.
    #[test]
    fn dm26_single_objective_degrades_to_min() {
        let front = ParetoFront {
            solutions: vec![sol(&[3.0]), sol(&[1.0]), sol(&[2.0])],
        };
        let dm = DecisionMaker::new(vec![1.0]);
        let chosen = dm.choose(&front);
        assert!(chosen.is_some());
        if let Some(s) = chosen {
            assert_eq!(s.objectives, vec![1.0]);
        }
    }

    /// DM27：空 front choose 返回 None，不 panic.
    #[test]
    fn dm27_empty_front_returns_none() {
        let front = ParetoFront::default();
        let dm = DecisionMaker::new(vec![0.8, 0.2]);
        assert!(dm.choose(&front).is_none());
    }

    /// DM28：choose 与 front.select_by_weight 直接调用结果一致（同一解引用）.
    #[test]
    fn dm28_consistent_with_select_by_weight() {
        let front = three_solution_front();
        let prefs = vec![0.6, 0.4];
        let dm = DecisionMaker::new(prefs.clone());
        let via_dm = dm.choose(&front);
        let via_front = front.select_by_weight(&prefs);
        assert!(via_dm.is_some());
        assert!(via_front.is_some());
        if let (Some(a), Some(b)) = (via_dm, via_front) {
            assert!(core::ptr::eq(a, b));
            assert_eq!(a.objectives, b.objectives);
        }
    }

    /// DM29：DecisionMaker derive Debug/Clone 可用（clone 后 preferences 相等，Debug 非空）.
    #[test]
    fn dm29_debug_clone() {
        let dm = DecisionMaker::new(vec![0.8, 0.2]);
        let cloned = dm.clone();
        assert_eq!(cloned.preferences, dm.preferences);
        assert!(!alloc::format!("{:?}", dm).is_empty());
    }

    /// DM30：偏好长度 < 目标数缺省补 0（[1.0] 视同 [1.0,0.0]），不 panic 且选择一致.
    #[test]
    fn dm30_short_preferences_zero_padded() {
        let front = three_solution_front();
        let short_dm = DecisionMaker::new(vec![1.0]);
        let full_dm = DecisionMaker::new(vec![1.0, 0.0]);
        let short_chosen = short_dm.choose(&front);
        let full_chosen = full_dm.choose(&front);
        assert!(short_chosen.is_some());
        assert!(full_chosen.is_some());
        if let (Some(a), Some(b)) = (short_chosen, full_chosen) {
            assert_eq!(a.objectives, b.objectives);
            assert_eq!(a.objectives, vec![1.0, 5.0]);
        }
    }
}

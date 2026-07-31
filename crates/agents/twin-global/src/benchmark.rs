//! 基准对比模块
//!
//! 对比单域优化与全局优化效果，验证 Phase 4 出口指标 (≥ 15% 提升)。

/// 基准对比结果
#[derive(Debug, Clone)]
pub struct BenchmarkResult {
    /// 单域优化目标值
    pub single_domain_obj: f64,
    /// 全局优化目标值
    pub global_obj: f64,
    /// 提升百分比
    pub improvement_pct: f64,
}

/// 计算单域 vs 全局优化的提升
///
/// improvement_pct = (global - single) / |single| * 100
pub fn compare(single: f64, global: f64) -> BenchmarkResult {
    let improvement_pct = if single.abs() < f64::EPSILON {
        0.0
    } else {
        (global - single) / single.abs() * 100.0
    };

    BenchmarkResult {
        single_domain_obj: single,
        global_obj: global,
        improvement_pct,
    }
}

/// 判断是否满足 Phase 4 出口标准 (提升 ≥ 15%)
pub fn meets_exit_criteria(result: &BenchmarkResult) -> bool {
    result.improvement_pct >= 15.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_meets_criteria() {
        let result = compare(100.0, 120.0);
        assert!((result.improvement_pct - 20.0).abs() < 1e-9);
        assert!(meets_exit_criteria(&result));
    }

    #[test]
    fn test_exactly_15_percent() {
        let result = compare(200.0, 230.0);
        assert!((result.improvement_pct - 15.0).abs() < 1e-9);
        assert!(meets_exit_criteria(&result));
    }

    #[test]
    fn test_does_not_meet_criteria() {
        let result = compare(100.0, 110.0);
        assert!((result.improvement_pct - 10.0).abs() < 1e-9);
        assert!(!meets_exit_criteria(&result));
    }

    #[test]
    fn test_zero_single() {
        let result = compare(0.0, 50.0);
        assert!((result.improvement_pct - 0.0).abs() < f64::EPSILON);
        assert!(!meets_exit_criteria(&result));
    }

    #[test]
    fn test_negative_single() {
        // 单域亏损 -100，全局 -80 → 提升 20%
        let result = compare(-100.0, -80.0);
        assert!((result.improvement_pct - 20.0).abs() < 1e-9);
        assert!(meets_exit_criteria(&result));
    }

    #[test]
    fn test_large_improvement() {
        let result = compare(50.0, 100.0);
        assert!((result.improvement_pct - 100.0).abs() < 1e-9);
        assert!(meets_exit_criteria(&result));
    }
}

//! EnerOS v0.70.0 实时快速路径引擎.
//!
//! 双脑架构的快路径（Solver only，<500ms）：当电价/负荷/SOC 变化在预定义阈值内时，
//! 跳过 LLM 推理，直接从实时状态生成 LP 参数并求解。
//!
//! # 核心类型
//!
//! - [`PathType`] — 路径类型（SlowPath / FastPath）
//! - [`RealtimeState`] — 实时状态（包装 v0.67.0 SystemState + current_price + load_demand）
//! - [`PathSelector`] — 路径选择器（根据状态变化阈值决定快/慢路径）
//! - [`StrategyTable`] — 预计算策略表（3×3=9 种电价×SOC 组合）
//! - [`RealtimePathEngine`] — 快速路径引擎（查表 → 微调 → 编译 → 求解 → 校验）
//!
//! # 偏差声明（D1~D12，Karpathy "Think Before Coding"）
//!
//! | 偏差 | 蓝图原文 | 本版本处理 | 理由 |
//! |------|---------|-----------|------|
//! | **D1** | `use std::time::{Duration, Instant}` | `use core::time::Duration` | no_std 合规：`std::time` 不可用 |
//! | **D2** | `solver: HighsSolver`（直接使用） | `RealtimePathEngine<S: Solver>`（泛型） | v0.64.0 `HighsSolver` feature-gated，默认用 `MockSolver` |
//! | **D3** | `state.soc` | `state.system.soc_pct` | v0.67.0 `SystemState` 字段名是 `soc_pct` |
//! | **D4** | 蓝图 `SystemState` 含 `current_price` / `load_demand` | 本地定义 `RealtimeState` 包装 v0.67.0 `SystemState` | v0.67.0 `SystemState` 仅含电气字段，无电价/负荷 |
//! | **D5** | `solver.set_time_limit(0.3)` | 不调用（Mock 不支持） | v0.64.0 `Solver` trait 用 `set_param`；快路径测试用 Mock 不需超时 |
//! | **D6** | `Instant::now()` | `now_ms: u64` 参数 | no_std 无 `Instant`，参考 v0.57.0/v0.64.0 `now_ms` 模式 |
//! | **D7** | `price_levels = vec![0.3, 0.6, 1.0]`（3 bounds → 4 桶，矩阵越界） | `vec![0.3, 0.7]`（2 bounds → 3 桶，3×3=9 策略） | 修复蓝图 bounds bug：`unwrap_or(len)` 会访问 `strategies[len]` 越界 panic |
//! | **D8** | 派生 `Debug` | `PathType`/`RealtimeState`/`FastPathResult` 派生 `Debug + Clone`；`PathType` 额外派生 `PartialEq` | 测试需要 `==` 比较 |
//! | **D9** | `FastPathError` 派生 `Debug + Clone` | 仅 `Debug` | Karpathy 简化原则，与 v0.68.0/v0.69.0 一致 |
//! | **D10** | `last_slow_path_time: Option<Instant>` | `last_slow_path_ms: Option<u64>` | no_std 无 `Instant`，用 ms 时间戳 |
//! | **D11** | `Duration::from_secs(300)` | `core::time::Duration::from_secs(300)` | no_std：`core::time::Duration` 可用 |
//! | **D12** | `validator.validate(&schedule, state)` | `validator.validate(&schedule, &state.system)` | v0.67.0 `SafetyValidator::validate` 接收 `&SystemState`，传 `state.system` |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，可交叉编译到 `aarch64-unknown-none`。

#![cfg_attr(not(test), no_std)]
extern crate alloc;

pub mod engine;
pub mod error;
pub mod selector;
pub mod state;
pub mod strategy;

pub use engine::{FastPathResult, RealtimePathEngine};
pub use error::FastPathError;
pub use selector::{PathSelector, PathType};
pub use state::RealtimeState;
pub use strategy::StrategyTable;

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use eneros_energy_lp_model::config::ScheduleConfig;
    use eneros_safety_validator::state::SystemState;
    use eneros_solver_core::mock::MockSolver;
    use eneros_solver_core::solver::Solver;

    use super::*;

    // T1: PathType PartialEq（D8）— SlowPath != FastPath，FastPath == FastPath
    #[test]
    fn t1_path_type_partial_eq() {
        assert_ne!(PathType::SlowPath, PathType::FastPath);
        assert_eq!(PathType::FastPath, PathType::FastPath);
        assert_eq!(PathType::SlowPath, PathType::SlowPath);
    }

    // T2: RealtimeState 显式构造
    #[test]
    fn t2_realtime_state_explicit_construction() {
        let state = RealtimeState {
            system: SystemState::default(),
            current_price: 0.5,
            load_demand: None,
        };
        assert!((state.current_price - 0.5).abs() < 1e-9);
        assert!((state.system.soc_pct - 0.5).abs() < 1e-9);
        assert!(state.load_demand.is_none());
    }

    // T3: RealtimeState::default() 等价显式构造
    #[test]
    fn t3_realtime_state_default() {
        let default_state = RealtimeState::default();
        let explicit = RealtimeState {
            system: SystemState::default(),
            current_price: 0.5,
            load_demand: None,
        };
        assert!((default_state.current_price - explicit.current_price).abs() < 1e-9);
        assert!((default_state.system.soc_pct - explicit.system.soc_pct).abs() < 1e-9);
        assert_eq!(
            default_state.system.timestamp_ms,
            explicit.system.timestamp_ms
        );
        assert_eq!(
            default_state.load_demand.is_none(),
            explicit.load_demand.is_none()
        );
    }

    // T4: PathSelector::new() 默认阈值
    #[test]
    fn t4_path_selector_default_thresholds() {
        let selector = PathSelector::new();
        assert!((selector.price_change_threshold - 0.1).abs() < 1e-9);
        assert!((selector.soc_change_threshold - 5.0).abs() < 1e-9);
        assert!((selector.load_change_threshold - 20.0).abs() < 1e-9);
        assert_eq!(
            selector.min_slow_path_interval,
            core::time::Duration::from_secs(300)
        );
        assert!(selector.last_slow_path_ms.is_none());
        assert!(selector.last_state.is_none());
    }

    // T5: 首次调用 select() 返回 SlowPath
    #[test]
    fn t5_select_first_call_returns_slow_path() {
        let mut selector = PathSelector::new();
        let state = RealtimeState::default();
        let path = selector.select(&state, 0);
        assert_eq!(path, PathType::SlowPath);
        assert_eq!(selector.last_slow_path_ms, Some(0));
    }

    // T6: 第一次走慢路径，第二次间隔内走 FastPath
    #[test]
    fn t6_select_within_interval_returns_fast_path() {
        let mut selector = PathSelector::new();
        let state = RealtimeState::default();
        // 第一次：now_ms=0 → SlowPath
        let p1 = selector.select(&state, 0);
        assert_eq!(p1, PathType::SlowPath);
        // 第二次：now_ms=1000，间隔内（< 300000ms）→ FastPath
        let p2 = selector.select(&state, 1000);
        assert_eq!(p2, PathType::FastPath);
    }

    // T7: 第一次走慢路径，第二次间隔超过 300s 走 SlowPath
    #[test]
    fn t7_select_interval_exceeded_returns_slow_path() {
        let mut selector = PathSelector::new();
        let state = RealtimeState::default();
        // 第一次：now_ms=0 → SlowPath
        let p1 = selector.select(&state, 0);
        assert_eq!(p1, PathType::SlowPath);
        // 第二次：now_ms=301000，间隔 301s > 300s → SlowPath
        let p2 = selector.select(&state, 301_000);
        assert_eq!(p2, PathType::SlowPath);
    }

    // T8: 电价变化超过阈值走 SlowPath
    #[test]
    fn t8_select_price_change_returns_slow_path() {
        let mut selector = PathSelector::new();
        let state1 = RealtimeState {
            current_price: 0.5,
            ..Default::default()
        };
        // 第一次：now_ms=0 → SlowPath
        let p1 = selector.select(&state1, 0);
        assert_eq!(p1, PathType::SlowPath);
        // 第二次：now_ms=1000，电价 0.7（变化 0.2 > 0.1）→ SlowPath
        let state2 = RealtimeState {
            current_price: 0.7,
            ..Default::default()
        };
        let p2 = selector.select(&state2, 1000);
        assert_eq!(p2, PathType::SlowPath);
    }

    // T9: SOC 变化超过阈值走 SlowPath
    #[test]
    fn t9_select_soc_change_returns_slow_path() {
        let mut selector = PathSelector::new();
        let state1 = RealtimeState {
            system: SystemState {
                soc_pct: 0.5,
                ..Default::default()
            },
            ..Default::default()
        };
        // 第一次：now_ms=0 → SlowPath
        let p1 = selector.select(&state1, 0);
        assert_eq!(p1, PathType::SlowPath);
        // 第二次：now_ms=1000，soc_pct=0.6（变化 10% > 5%）→ SlowPath
        let state2 = RealtimeState {
            system: SystemState {
                soc_pct: 0.6,
                ..Default::default()
            },
            ..Default::default()
        };
        let p2 = selector.select(&state2, 1000);
        assert_eq!(p2, PathType::SlowPath);
    }

    // T10: StrategyTable::new 生成 3×3=9 策略
    #[test]
    fn t10_strategy_table_3x3() {
        let table = StrategyTable::new(ScheduleConfig::default());
        assert_eq!(table.price_levels.len(), 2);
        assert_eq!(table.soc_levels.len(), 2);
        assert_eq!(table.strategies.len(), 3);
        for row in &table.strategies {
            assert_eq!(row.len(), 3);
        }
    }

    // T11: 谷时低 SOC → soc_final == Some(0.8)
    #[test]
    fn t11_strategy_valley_low_soc() {
        let table = StrategyTable::new(ScheduleConfig::default());
        let state = RealtimeState {
            current_price: 0.2,
            system: SystemState {
                soc_pct: 0.2,
                ..Default::default()
            },
            ..Default::default()
        };
        let config = table.get_config(&state);
        assert_eq!(config.soc_final, Some(0.8));
    }

    // T12: 峰时高 SOC → soc_final == Some(0.3)
    #[test]
    fn t12_strategy_peak_high_soc() {
        let table = StrategyTable::new(ScheduleConfig::default());
        let state = RealtimeState {
            current_price: 0.8,
            system: SystemState {
                soc_pct: 0.8,
                ..Default::default()
            },
            ..Default::default()
        };
        let config = table.get_config(&state);
        assert_eq!(config.soc_final, Some(0.3));
    }

    // T13: 平时中 SOC → soc_final == None
    #[test]
    fn t13_strategy_flat_mid_soc() {
        let table = StrategyTable::new(ScheduleConfig::default());
        let state = RealtimeState {
            current_price: 0.5,
            system: SystemState {
                soc_pct: 0.5,
                ..Default::default()
            },
            ..Default::default()
        };
        let config = table.get_config(&state);
        assert!(config.soc_final.is_none());
    }

    // T14: 边界 price=0.29 < 0.3 → 谷时策略
    #[test]
    fn t14_strategy_boundary_price() {
        let table = StrategyTable::new(ScheduleConfig::default());
        let state = RealtimeState {
            current_price: 0.29,
            system: SystemState {
                soc_pct: 0.2,
                ..Default::default()
            },
            ..Default::default()
        };
        let config = table.get_config(&state);
        assert_eq!(config.soc_final, Some(0.8));
    }

    // T15: RealtimePathEngine::new 构造成功
    #[test]
    fn t15_engine_new_construction() {
        let engine = RealtimePathEngine::new(ScheduleConfig::default(), MockSolver::new());
        assert_eq!(engine.solver.name(), "MockSolver");
        assert_eq!(engine.solver.version(), "0.1.0");
        assert_eq!(engine.strategy_table.strategies.len(), 3);
    }

    // T16: engine.execute 返回 Ok(FastPathResult)
    #[test]
    fn t16_engine_execute_ok() {
        let mut engine = RealtimePathEngine::new(ScheduleConfig::default(), MockSolver::new());
        let state = RealtimeState::default();
        let result = engine.execute(&state, 0);
        assert!(result.is_ok());
    }

    // T17: 返回结果 path_type == FastPath
    #[test]
    fn t17_engine_execute_path_type_fast() {
        let mut engine = RealtimePathEngine::new(ScheduleConfig::default(), MockSolver::new());
        let state = RealtimeState::default();
        let result = engine.execute(&state, 0).unwrap();
        assert_eq!(result.path_type, PathType::FastPath);
    }

    // T18: soc_pct=0.7 执行不 panic
    #[test]
    fn t18_engine_execute_soc_07_no_panic() {
        let mut engine = RealtimePathEngine::new(ScheduleConfig::default(), MockSolver::new());
        let state = RealtimeState {
            system: SystemState {
                soc_pct: 0.7,
                ..Default::default()
            },
            ..Default::default()
        };
        let result = engine.execute(&state, 0);
        assert!(result.is_ok());
    }

    // T19: load_demand=Some(vec![50.0; 96]) 执行成功
    #[test]
    fn t19_engine_execute_with_load_demand() {
        let mut engine = RealtimePathEngine::new(ScheduleConfig::default(), MockSolver::new());
        let state = RealtimeState {
            load_demand: Some(vec![50.0; 96]),
            ..Default::default()
        };
        let result = engine.execute(&state, 0);
        assert!(result.is_ok());
    }

    // T20: default_with_mock() 等价 new(ScheduleConfig::default(), MockSolver::new())
    #[test]
    fn t20_default_with_mock_equivalent() {
        let engine1 = RealtimePathEngine::default_with_mock();
        let engine2 = RealtimePathEngine::new(ScheduleConfig::default(), MockSolver::new());
        // 验证 solver 等价
        assert_eq!(engine1.solver.name(), engine2.solver.name());
        assert_eq!(engine1.solver.version(), engine2.solver.version());
        // 验证策略表等价
        assert_eq!(
            engine1.strategy_table.strategies.len(),
            engine2.strategy_table.strategies.len()
        );
    }

    // T21: 端到端 — PathSelector 走 FastPath → engine.execute 成功
    #[test]
    fn t21_end_to_end_selector_fast_path_engine_execute() {
        let mut selector = PathSelector::new();
        let state = RealtimeState::default();
        // 第一次：now_ms=0 → SlowPath（初始化基线）
        let p1 = selector.select(&state, 0);
        assert_eq!(p1, PathType::SlowPath);
        // 第二次：now_ms=1000，状态未变 → FastPath
        let p2 = selector.select(&state, 1000);
        assert_eq!(p2, PathType::FastPath);
        // engine.execute 成功
        let mut engine = RealtimePathEngine::new(ScheduleConfig::default(), MockSolver::new());
        let result = engine.execute(&state, 1000);
        assert!(result.is_ok());
    }

    // T22: result.validation.passed 是 bool，result.validation.violations 是 Vec
    #[test]
    fn t22_validation_fields_accessible() {
        let mut engine = RealtimePathEngine::new(ScheduleConfig::default(), MockSolver::new());
        let state = RealtimeState::default();
        let result = engine.execute(&state, 0).unwrap();
        // validation.passed 是 bool（MockSolver 返回空解 → SOC=0.0 可能触发 Fatal，不假设值）
        let passed: bool = result.validation.passed;
        // validation.violations 是 Vec
        let violations: &Vec<_> = &result.validation.violations;
        // 验证字段类型可访问：passed 已作为 bool 绑定，violations 已作为 Vec 绑定
        let _ = (passed, violations.len());
    }
}

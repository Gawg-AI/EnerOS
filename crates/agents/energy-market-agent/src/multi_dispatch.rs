//! EnerOS v0.87.0 多设备调度器.
//!
//! 实现多设备（储能+光伏+充电桩）功率分配：构建 LP 问题（容量/爬坡/SOC 约束）
//! → Solver 求解 → 失败回退 `equal_split` 平均分配兜底。
//!
//! # 偏差声明
//! | 偏差 | 说明 |
//! |------|------|
//! | **D1** | sync `dispatch(&mut self, target, socs, now_ms)` — no_std 无 async runtime；`&mut` 因 Solver::solve 需 &mut + last_setpoints 更新 |
//! | **D5** | `Box<dyn Solver>` 直接复用 eneros-solver-core trait，无需本地抽象 |
//! | **D6** | 直接构建既有 LpProblem CSR 结构，蓝图 DSL 不存在 |
//! | **D8** | `DispatchError` 2 变体：EmptyPool / InvalidTarget；Solver 失败为回退非错误 |
//! | **D9** | 爬坡约束为 `prev - ramp <= p <= prev + ramp`（相对上次设定点），非蓝图过紧 `p <= ramp` |
//! | **D10** | SOC 过滤规则：`soc <= 0.0` → 跳过（确定性可用性过滤） |
//! | **D11** | `now_ms: u64` 参数注入，no_std 无 Instant::now() |
//! | **D13** | `total_power = Σ setpoints`（clamp 后实际值），非直接赋值 target |
//! | **D14** | 目标函数 `Minimize Σ (1.0 - efficiency_i) * p_i`（损耗最小） |

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use eneros_solver_core::{
    problem::{ConstraintMatrix, LpProblem, ObjectiveSense, VarType},
    result::SolveStatus,
    solver::Solver,
};

use crate::device_pool::{DeviceCapability, DeviceMode, DevicePool};

/// 设备功率分配指令.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DeviceAssignment {
    /// 设备 ID.
    pub device_id: u64,
    /// 设定功率（MW）.
    pub setpoint: f32,
    /// 运行模式.
    pub mode: DeviceMode,
}

/// 调度计划.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DispatchPlan {
    /// 时间戳（毫秒）.
    pub timestamp: u64,
    /// 各设备分配指令.
    pub assignments: Vec<DeviceAssignment>,
    /// 实际总功率（MW）.
    pub total_power: f32,
    /// 目标函数值（兜底路径为 0.0）.
    pub objective_value: f32,
}

/// 调度错误.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchError {
    /// 无可调度设备（空池或全部 SOC 耗尽）.
    EmptyPool,
    /// 目标功率非法（NaN / ±∞）.
    InvalidTarget,
}

/// 平均分配兜底.
pub fn equal_split(target: f32, caps: &[(u64, DeviceCapability)]) -> Vec<DeviceAssignment> {
    if caps.is_empty() {
        return Vec::new();
    }
    let n = caps.len() as f32;
    let share = target / n;
    let mut assignments = Vec::with_capacity(caps.len());
    for (id, cap) in caps.iter() {
        let setpoint = share.max(cap.p_min).min(cap.p_max);
        assignments.push(DeviceAssignment {
            device_id: *id,
            setpoint,
            mode: DeviceMode::Auto,
        });
    }
    assignments
}

/// 多设备调度器.
pub struct MultiDeviceDispatcher {
    /// 设备池.
    pub pool: DevicePool,
    /// 求解器.
    pub solver: Box<dyn Solver>,
    /// 上次设定点（设备 ID → 功率）.
    pub last_setpoints: BTreeMap<u64, f32>,
}

impl MultiDeviceDispatcher {
    /// 创建调度器.
    pub fn new(pool: DevicePool, solver: Box<dyn Solver>) -> Self {
        Self {
            pool,
            solver,
            last_setpoints: BTreeMap::new(),
        }
    }

    /// 执行多设备功率分配.
    pub fn dispatch(
        &mut self,
        target: f32,
        socs: &BTreeMap<u64, f32>,
        now_ms: u64,
    ) -> Result<DispatchPlan, DispatchError> {
        // 1. 目标校验
        if !target.is_finite() {
            return Err(DispatchError::InvalidTarget);
        }

        // 2. 陈旧清理
        self.last_setpoints
            .retain(|id, _| self.pool.devices.contains_key(id));

        // 3. SOC 过滤
        let mut eligible: Vec<(u64, DeviceCapability)> = Vec::new();
        for (id, cap) in self.pool.devices.iter() {
            if let Some(soc) = socs.get(id) {
                if *soc <= 0.0 {
                    continue;
                }
            }
            eligible.push((*id, *cap));
        }

        // 4. 空池校验
        if eligible.is_empty() {
            return Err(DispatchError::EmptyPool);
        }

        // 5. 构建 LP
        let problem = build_lp_problem(&eligible, target, &self.last_setpoints);
        let n = eligible.len();

        // 6. 求解（Optimal 且解长度匹配 → 采用；其余全部回退平均分配）
        let (assignments, objective_value) = match self.solver.solve(&problem, now_ms) {
            Ok(result) if result.status == SolveStatus::Optimal && result.solution.len() == n => {
                let mut assignments = Vec::with_capacity(n);
                for (i, (id, cap)) in eligible.iter().enumerate() {
                    let sp = result.solution[i]
                        .max(cap.p_min as f64)
                        .min(cap.p_max as f64) as f32;
                    assignments.push(DeviceAssignment {
                        device_id: *id,
                        setpoint: sp,
                        mode: DeviceMode::Auto,
                    });
                }
                (assignments, result.objective_value as f32)
            }
            _ => (equal_split(target, &eligible), 0.0),
        };

        // 7. 更新 last_setpoints
        for a in &assignments {
            self.last_setpoints.insert(a.device_id, a.setpoint);
        }

        // 8. 返回（D13：total_power 为 clamp 后实际设定值之和）
        let total_power: f32 = assignments.iter().map(|a| a.setpoint).sum();
        Ok(DispatchPlan {
            timestamp: now_ms,
            assignments,
            total_power,
            objective_value,
        })
    }
}

fn build_lp_problem(
    eligible: &[(u64, DeviceCapability)],
    target: f32,
    last_setpoints: &BTreeMap<u64, f32>,
) -> LpProblem {
    let n = eligible.len();
    let mut variables = Vec::with_capacity(n);
    let mut lower_bounds = Vec::with_capacity(n);
    let mut upper_bounds = Vec::with_capacity(n);
    let mut var_types = Vec::with_capacity(n);
    let mut objective = Vec::with_capacity(n);

    for (id, cap) in eligible.iter() {
        variables.push(alloc::format!("p_{}", id));
        lower_bounds.push(cap.p_min as f64);
        upper_bounds.push(cap.p_max as f64);
        var_types.push(VarType::Continuous);
        objective.push(1.0 - cap.efficiency as f64);
    }

    // 平衡行 + 每个有上次设定点的设备 1 条爬坡行
    let num_ramp = eligible
        .iter()
        .filter(|(id, _)| last_setpoints.contains_key(id))
        .count();
    let num_rows = 1 + num_ramp;
    let num_nz = n + num_ramp;

    let mut row_start = Vec::with_capacity(num_rows + 1);
    let mut col_index = Vec::with_capacity(num_nz);
    let mut values = Vec::with_capacity(num_nz);
    let mut rhs_lower = Vec::with_capacity(num_rows);
    let mut rhs_upper = Vec::with_capacity(num_rows);

    // 平衡行：Σ p_i = target
    row_start.push(0);
    for i in 0..n {
        col_index.push(i as i32);
        values.push(1.0);
    }
    row_start.push(n as i32);
    rhs_lower.push(target as f64);
    rhs_upper.push(target as f64);

    // 爬坡行（D9）：prev - ramp <= p_i <= prev + ramp，每行 1 个非零
    for (i, (id, cap)) in eligible.iter().enumerate() {
        if let Some(prev) = last_setpoints.get(id) {
            col_index.push(i as i32);
            values.push(1.0);
            row_start.push(col_index.len() as i32);
            rhs_lower.push((*prev as f64) - (cap.ramp_rate as f64));
            rhs_upper.push((*prev as f64) + (cap.ramp_rate as f64));
        }
    }

    LpProblem {
        variables,
        lower_bounds,
        upper_bounds,
        var_types,
        objective,
        sense: ObjectiveSense::Minimize,
        constraints: ConstraintMatrix::new(num_rows, num_nz, row_start, col_index, values),
        rhs_lower,
        rhs_upper,
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use eneros_solver_core::{error::SolverError, result::SolveResult};

    use super::*;

    // ===== RecordingSolver 测试辅助 =====
    //
    // LP 结构验证直接调用同模块私有 `build_lp_problem`（T107~T111），
    // 因此本桩仅提供固定结果 / 失败两种行为，不记录问题。
    struct RecordingSolver {
        result: Option<SolveResult>,
        fail: bool,
    }

    impl RecordingSolver {
        fn new() -> Self {
            Self {
                result: None,
                fail: false,
            }
        }
        fn with_result(result: SolveResult) -> Self {
            Self {
                result: Some(result),
                fail: false,
            }
        }
        fn failing() -> Self {
            Self {
                result: None,
                fail: true,
            }
        }
    }

    impl Solver for RecordingSolver {
        fn solve(
            &mut self,
            _problem: &LpProblem,
            _now_ms: u64,
        ) -> Result<SolveResult, SolverError> {
            if self.fail {
                return Err(SolverError::RunFailed(-1));
            }
            match &self.result {
                Some(r) => Ok(r.clone()),
                None => Ok(SolveResult::optimal(0.0, vec![])),
            }
        }
        fn name(&self) -> &'static str {
            "RecordingSolver"
        }
        fn version(&self) -> &'static str {
            "0.1.0"
        }
        fn set_param(&mut self, _key: &str, _value: &str) -> Result<(), SolverError> {
            Ok(())
        }
        fn status(&self) -> eneros_solver_core::solver::SolverStatus {
            eneros_solver_core::solver::SolverStatus::Idle
        }
    }

    /// 辅助：构造标准设备（p_min=0, p_max, ramp_rate=1.0, efficiency）.
    fn cap(p_max: f32, efficiency: f32) -> DeviceCapability {
        DeviceCapability {
            p_min: 0.0,
            p_max,
            ramp_rate: 1.0,
            efficiency,
        }
    }

    /// 辅助：构造 2 设备池（ID 1/2）+ 对应 SOC 表.
    fn two_device_setup() -> (DevicePool, BTreeMap<u64, f32>) {
        let mut pool = DevicePool::new();
        pool.add_device(1, cap(5.0, 0.9));
        pool.add_device(2, cap(5.0, 0.8));
        let mut socs = BTreeMap::new();
        socs.insert(1u64, 0.5f32);
        socs.insert(2u64, 0.5f32);
        (pool, socs)
    }

    // ===== T93~T96：数据结构默认值与派生 =====
    #[test]
    fn t93_device_assignment_default() {
        let a = DeviceAssignment::default();
        assert_eq!(a.device_id, 0);
        assert_eq!(a.setpoint, 0.0);
        assert_eq!(a.mode, DeviceMode::Auto);
    }

    #[test]
    fn t94_device_assignment_copy() {
        let a = DeviceAssignment {
            device_id: 1,
            setpoint: 3.5,
            mode: DeviceMode::Auto,
        };
        let b = a;
        assert_eq!(a, b);
    }

    #[test]
    fn t95_dispatch_plan_default() {
        let p = DispatchPlan::default();
        assert_eq!(p.timestamp, 0);
        assert!(p.assignments.is_empty());
        assert_eq!(p.total_power, 0.0);
        assert_eq!(p.objective_value, 0.0);
    }

    #[test]
    fn t96_dispatch_plan_clone() {
        let p = DispatchPlan {
            timestamp: 1000,
            assignments: vec![DeviceAssignment {
                device_id: 1,
                setpoint: 2.0,
                mode: DeviceMode::Auto,
            }],
            total_power: 2.0,
            objective_value: 0.1,
        };
        let q = p.clone();
        assert_eq!(p, q);
    }

    // ===== T97：DispatchError =====
    #[test]
    fn t97_dispatch_error_variants() {
        assert_eq!(DispatchError::EmptyPool, DispatchError::EmptyPool);
        assert_eq!(DispatchError::InvalidTarget, DispatchError::InvalidTarget);
        assert_ne!(DispatchError::EmptyPool, DispatchError::InvalidTarget);
        let _ = format!("{:?}", DispatchError::EmptyPool);
    }

    // ===== T98~T101：equal_split =====
    #[test]
    fn t98_equal_split_two_devices() {
        let caps = vec![(1u64, cap(10.0, 0.9)), (2u64, cap(10.0, 0.8))];
        let a = equal_split(10.0, &caps);
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].setpoint, 5.0);
        assert_eq!(a[1].setpoint, 5.0);
        assert_eq!(a[0].mode, DeviceMode::Auto);
    }

    #[test]
    fn t99_equal_split_clamp_pmax() {
        let caps = vec![(1u64, cap(3.0, 0.9)), (2u64, cap(5.0, 0.8))];
        let a = equal_split(10.0, &caps);
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].setpoint, 3.0);
        assert_eq!(a[1].setpoint, 5.0);
    }

    #[test]
    fn t100_equal_split_clamp_pmin_negative() {
        let caps = vec![
            (
                1u64,
                DeviceCapability {
                    p_min: -2.0,
                    p_max: 0.0,
                    ramp_rate: 0.5,
                    efficiency: 0.9,
                },
            ),
            (
                2u64,
                DeviceCapability {
                    p_min: -2.0,
                    p_max: 0.0,
                    ramp_rate: 0.5,
                    efficiency: 0.8,
                },
            ),
        ];
        let a = equal_split(-10.0, &caps);
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].setpoint, -2.0);
        assert_eq!(a[1].setpoint, -2.0);
    }

    #[test]
    fn t101_equal_split_empty() {
        let caps: Vec<(u64, DeviceCapability)> = vec![];
        let a = equal_split(10.0, &caps);
        assert!(a.is_empty());
    }

    // ===== T102~T104：dispatch 校验 =====
    #[test]
    fn t102_dispatch_invalid_target() {
        let pool = DevicePool::new();
        let solver: Box<dyn Solver> = Box::new(RecordingSolver::new());
        let mut d = MultiDeviceDispatcher::new(pool, solver);
        let socs = BTreeMap::new();
        assert_eq!(
            d.dispatch(f32::NAN, &socs, 1000).unwrap_err(),
            DispatchError::InvalidTarget
        );
        assert_eq!(
            d.dispatch(f32::INFINITY, &socs, 1000).unwrap_err(),
            DispatchError::InvalidTarget
        );
    }

    #[test]
    fn t103_dispatch_empty_pool() {
        let pool = DevicePool::new();
        let solver: Box<dyn Solver> = Box::new(RecordingSolver::new());
        let mut d = MultiDeviceDispatcher::new(pool, solver);
        let mut socs = BTreeMap::new();
        socs.insert(1, 0.5);
        assert_eq!(
            d.dispatch(5.0, &socs, 1000).unwrap_err(),
            DispatchError::EmptyPool
        );
    }

    #[test]
    fn t104_dispatch_all_soc_zero() {
        let mut pool = DevicePool::new();
        pool.add_device(1, cap(5.0, 0.9));
        let solver: Box<dyn Solver> = Box::new(RecordingSolver::new());
        let mut d = MultiDeviceDispatcher::new(pool, solver);
        let mut socs = BTreeMap::new();
        socs.insert(1, 0.0);
        assert_eq!(
            d.dispatch(5.0, &socs, 1000).unwrap_err(),
            DispatchError::EmptyPool
        );
    }

    // ===== T105~T106：dispatch SOC 过滤与正常路径 =====
    #[test]
    fn t105_dispatch_soc_filter_one_skipped() {
        let (pool, mut socs) = two_device_setup();
        socs.insert(1, 0.0); // 设备 1 SOC 耗尽 → 跳过
        let solver: Box<dyn Solver> = Box::new(RecordingSolver::with_result(SolveResult::optimal(
            0.0,
            vec![4.0],
        )));
        let mut d = MultiDeviceDispatcher::new(pool, solver);
        let plan = d.dispatch(4.0, &socs, 1000).unwrap();
        assert_eq!(plan.assignments.len(), 1);
        assert_eq!(plan.assignments[0].device_id, 2);
        assert_eq!(plan.assignments[0].setpoint, 4.0);
    }

    #[test]
    fn t106_dispatch_happy_path() {
        let (pool, socs) = two_device_setup();
        let solver: Box<dyn Solver> = Box::new(RecordingSolver::with_result(SolveResult::optimal(
            0.5,
            vec![3.0, 2.0],
        )));
        let mut d = MultiDeviceDispatcher::new(pool, solver);
        let plan = d.dispatch(5.0, &socs, 2000).unwrap();
        assert_eq!(plan.assignments.len(), 2);
        assert_eq!(plan.assignments[0].device_id, 1);
        assert_eq!(plan.assignments[0].setpoint, 3.0);
        assert_eq!(plan.assignments[1].device_id, 2);
        assert_eq!(plan.assignments[1].setpoint, 2.0);
        assert_eq!(plan.total_power, 5.0);
        assert_eq!(plan.objective_value, 0.5);
        assert_eq!(plan.timestamp, 2000);
        assert_eq!(plan.assignments[0].mode, DeviceMode::Auto);
        // last_setpoints 更新
        assert_eq!(d.last_setpoints.get(&1), Some(&3.0));
        assert_eq!(d.last_setpoints.get(&2), Some(&2.0));
    }

    // ===== T107~T111：LP 问题结构（直接调用 build_lp_problem）=====
    #[test]
    fn t107_lp_variables_and_bounds() {
        let eligible = vec![
            (1u64, cap(5.0, 0.9)),
            (
                2u64,
                DeviceCapability {
                    p_min: -1.0,
                    p_max: 3.0,
                    ramp_rate: 0.5,
                    efficiency: 0.8,
                },
            ),
        ];
        let p = build_lp_problem(&eligible, 5.0, &BTreeMap::new());
        assert_eq!(p.variables.len(), 2);
        assert_eq!(p.variables[0], "p_1");
        assert_eq!(p.variables[1], "p_2");
        assert_eq!(p.lower_bounds, vec![0.0, -1.0]);
        assert_eq!(p.upper_bounds, vec![5.0, 3.0]);
        assert_eq!(p.var_types, vec![VarType::Continuous, VarType::Continuous]);
        assert_eq!(p.sense, ObjectiveSense::Minimize);
    }

    #[test]
    fn t108_lp_balance_row_first_dispatch() {
        let eligible = vec![(1u64, cap(5.0, 0.9)), (2u64, cap(5.0, 0.8))];
        let p = build_lp_problem(&eligible, 5.0, &BTreeMap::new());
        // 首次调度（无 last_setpoints）：仅 1 条平衡行
        assert_eq!(p.constraints.num_rows, 1);
        assert_eq!(p.constraints.num_nz, 2);
        assert_eq!(p.constraints.row_start, vec![0, 2]);
        assert_eq!(p.constraints.col_index, vec![0, 1]);
        assert_eq!(p.constraints.values, vec![1.0, 1.0]);
        assert_eq!(p.rhs_lower, vec![5.0]);
        assert_eq!(p.rhs_upper, vec![5.0]);
    }

    #[test]
    fn t109_lp_objective_loss_coefficients() {
        let eligible = vec![(1u64, cap(5.0, 0.9)), (2u64, cap(5.0, 0.8))];
        let p = build_lp_problem(&eligible, 5.0, &BTreeMap::new());
        // D14：目标系数 = 1.0 - efficiency（f32→f64 精度容差 1e-6）
        assert_eq!(p.objective.len(), 2);
        assert!((p.objective[0] - 0.1).abs() < 1e-6);
        assert!((p.objective[1] - 0.2).abs() < 1e-6);
    }

    #[test]
    fn t110_lp_ramp_rows_added_with_last_setpoints() {
        let eligible = vec![
            (1u64, cap(5.0, 0.9)),
            (
                2u64,
                DeviceCapability {
                    p_min: 0.0,
                    p_max: 5.0,
                    ramp_rate: 2.0,
                    efficiency: 0.8,
                },
            ),
        ];
        // 首次：num_rows = 1
        let first = build_lp_problem(&eligible, 5.0, &BTreeMap::new());
        assert_eq!(first.constraints.num_rows, 1);
        assert_eq!(first.constraints.num_nz, 2);
        // 第二次：2 设备均有上次设定点 → 1 平衡行 + 2 爬坡行
        let mut last = BTreeMap::new();
        last.insert(1u64, 3.0f32);
        last.insert(2u64, 2.0f32);
        let second = build_lp_problem(&eligible, 5.0, &last);
        assert_eq!(second.constraints.num_rows, 3);
        assert_eq!(second.constraints.num_nz, 4);
        assert_eq!(second.constraints.row_start, vec![0, 2, 3, 4]);
        assert_eq!(second.constraints.col_index, vec![0, 1, 0, 1]);
        assert_eq!(second.constraints.values, vec![1.0, 1.0, 1.0, 1.0]);
        // 平衡行 rhs=5.0；设备1: [3-1, 3+1]=[2,4]；设备2: [2-2, 2+2]=[0,4]
        assert_eq!(second.rhs_lower, vec![5.0, 2.0, 0.0]);
        assert_eq!(second.rhs_upper, vec![5.0, 4.0, 4.0]);
    }

    #[test]
    fn t111_lp_ramp_row_semantics() {
        let eligible = vec![
            (1u64, cap(5.0, 0.9)),
            (
                2u64,
                DeviceCapability {
                    p_min: 0.0,
                    p_max: 5.0,
                    ramp_rate: 2.0,
                    efficiency: 0.8,
                },
            ),
        ];
        // 仅设备 1 有上次设定点 → 仅设备 1 产生爬坡行
        let mut last = BTreeMap::new();
        last.insert(1u64, 3.0f32);
        let p = build_lp_problem(&eligible, 5.0, &last);
        assert_eq!(p.constraints.num_rows, 2);
        assert_eq!(p.constraints.row_start, vec![0, 2, 3]);
        // 行 1 的唯一非零在列 0（设备 1 在 eligible 中的位置）
        assert_eq!(p.constraints.col_index[2], 0);
        assert_eq!(p.constraints.values[2], 1.0);
        // D9：prev=3.0, ramp=1.0 → [2.0, 4.0]
        assert_eq!(p.rhs_lower[1], 2.0);
        assert_eq!(p.rhs_upper[1], 4.0);
    }

    // ===== T112：last_setpoints 更新 =====
    #[test]
    fn t112_dispatch_updates_last_setpoints() {
        let (pool, socs) = two_device_setup();
        let solver: Box<dyn Solver> = Box::new(RecordingSolver::with_result(SolveResult::optimal(
            0.3,
            vec![3.0, 2.0],
        )));
        let mut d = MultiDeviceDispatcher::new(pool, solver);
        assert!(d.last_setpoints.is_empty());
        let _ = d.dispatch(5.0, &socs, 1000).unwrap();
        assert_eq!(d.last_setpoints.get(&1), Some(&3.0));
        assert_eq!(d.last_setpoints.get(&2), Some(&2.0));
        // 第二次调度后仍为最新设定点（固定解覆盖）
        let plan2 = d.dispatch(4.0, &socs, 2000).unwrap();
        assert_eq!(plan2.assignments.len(), 2);
        assert_eq!(d.last_setpoints.get(&1), Some(&3.0));
        assert_eq!(d.last_setpoints.get(&2), Some(&2.0));
    }

    // ===== T113~T115：回退路径 =====
    #[test]
    fn t113_dispatch_solver_error_fallback_equal_split() {
        let (pool, socs) = two_device_setup();
        let solver: Box<dyn Solver> = Box::new(RecordingSolver::failing());
        let mut d = MultiDeviceDispatcher::new(pool, solver);
        let plan = d.dispatch(10.0, &socs, 1000).unwrap();
        assert_eq!(plan.assignments.len(), 2);
        assert_eq!(plan.assignments[0].setpoint, 5.0);
        assert_eq!(plan.assignments[1].setpoint, 5.0);
        assert_eq!(plan.objective_value, 0.0);
        assert_eq!(plan.total_power, 10.0);
    }

    #[test]
    fn t114_dispatch_infeasible_fallback() {
        let (pool, socs) = two_device_setup();
        let result = SolveResult {
            status: SolveStatus::Infeasible,
            objective_value: 0.0,
            solution: Vec::new(),
            elapsed_ms: 0,
            dual_solution: None,
        };
        let solver: Box<dyn Solver> = Box::new(RecordingSolver::with_result(result));
        let mut d = MultiDeviceDispatcher::new(pool, solver);
        let plan = d.dispatch(8.0, &socs, 1000).unwrap();
        // 回退平均分配：8.0 / 2 = 4.0
        assert_eq!(plan.assignments.len(), 2);
        assert_eq!(plan.assignments[0].setpoint, 4.0);
        assert_eq!(plan.assignments[1].setpoint, 4.0);
        assert_eq!(plan.objective_value, 0.0);
        assert_eq!(plan.total_power, 8.0);
    }

    #[test]
    fn t115_dispatch_solution_length_mismatch_fallback() {
        let (pool, socs) = two_device_setup();
        // Optimal 但解长度 1 != 设备数 2 → 回退
        let solver: Box<dyn Solver> = Box::new(RecordingSolver::with_result(SolveResult::optimal(
            0.0,
            vec![6.0],
        )));
        let mut d = MultiDeviceDispatcher::new(pool, solver);
        let plan = d.dispatch(6.0, &socs, 1000).unwrap();
        assert_eq!(plan.assignments.len(), 2);
        assert_eq!(plan.assignments[0].setpoint, 3.0);
        assert_eq!(plan.assignments[1].setpoint, 3.0);
        assert_eq!(plan.objective_value, 0.0);
    }

    // ===== T116~T117：clamp 与 total_power =====
    #[test]
    fn t116_dispatch_clamps_solution_to_bounds() {
        let mut pool = DevicePool::new();
        pool.add_device(1, cap(3.0, 0.9));
        pool.add_device(
            2,
            DeviceCapability {
                p_min: 1.0,
                p_max: 5.0,
                ramp_rate: 1.0,
                efficiency: 0.8,
            },
        );
        let mut socs = BTreeMap::new();
        socs.insert(1u64, 0.5f32);
        socs.insert(2u64, 0.5f32);
        // 求解器返回越界解：设备 1 超 p_max，设备 2 低于 p_min
        let solver: Box<dyn Solver> = Box::new(RecordingSolver::with_result(SolveResult::optimal(
            0.0,
            vec![4.5, 0.5],
        )));
        let mut d = MultiDeviceDispatcher::new(pool, solver);
        let plan = d.dispatch(5.0, &socs, 1000).unwrap();
        assert_eq!(plan.assignments[0].setpoint, 3.0); // clamp 到 p_max
        assert_eq!(plan.assignments[1].setpoint, 1.0); // clamp 到 p_min
                                                       // total_power 为 clamp 后实际值之和（D13），非 target
        assert_eq!(plan.total_power, 4.0);
        // last_setpoints 记录 clamp 后值
        assert_eq!(d.last_setpoints.get(&1), Some(&3.0));
        assert_eq!(d.last_setpoints.get(&2), Some(&1.0));
    }

    #[test]
    fn t117_total_power_equals_sum_of_setpoints() {
        let mut pool = DevicePool::new();
        pool.add_device(1, cap(10.0, 0.9));
        pool.add_device(2, cap(10.0, 0.8));
        pool.add_device(3, cap(10.0, 0.85));
        let mut socs = BTreeMap::new();
        socs.insert(1u64, 0.5f32);
        socs.insert(2u64, 0.5f32);
        socs.insert(3u64, 0.5f32);
        // 解之和 7.5 != target 7.0：total_power 取实际解之和（D13）
        let solver: Box<dyn Solver> = Box::new(RecordingSolver::with_result(SolveResult::optimal(
            0.0,
            vec![1.5, 2.5, 3.5],
        )));
        let mut d = MultiDeviceDispatcher::new(pool, solver);
        let plan = d.dispatch(7.0, &socs, 1000).unwrap();
        let sum: f32 = plan.assignments.iter().map(|a| a.setpoint).sum();
        assert_eq!(plan.total_power, sum);
        assert_eq!(plan.total_power, 7.5);
    }

    // ===== T118~T120：多设备协同 / 设备增减 / 离线重分配 =====
    #[test]
    fn t118_dispatch_five_devices_coordination() {
        let mut pool = DevicePool::new();
        let effs = [0.95f32, 0.9, 0.85, 0.8, 0.75];
        let mut socs = BTreeMap::new();
        for (i, eff) in effs.iter().enumerate() {
            let id = (i + 1) as u64;
            pool.add_device(
                id,
                DeviceCapability {
                    p_min: 0.0,
                    p_max: 10.0,
                    ramp_rate: 2.0,
                    efficiency: *eff,
                },
            );
            socs.insert(id, 0.6f32);
        }
        let solver: Box<dyn Solver> = Box::new(RecordingSolver::with_result(SolveResult::optimal(
            0.6,
            vec![4.0, 3.0, 2.0, 1.0, 0.5],
        )));
        let mut d = MultiDeviceDispatcher::new(pool, solver);
        let plan = d.dispatch(10.5, &socs, 5000).unwrap();
        assert_eq!(plan.assignments.len(), 5);
        // 设备按 ID 升序分配（BTreeMap 有序，D3）
        for (i, a) in plan.assignments.iter().enumerate() {
            assert_eq!(a.device_id, (i + 1) as u64);
            assert_eq!(a.mode, DeviceMode::Auto);
        }
        assert_eq!(plan.assignments[0].setpoint, 4.0);
        assert_eq!(plan.assignments[4].setpoint, 0.5);
        assert_eq!(plan.total_power, 10.5);
        assert_eq!(plan.objective_value, 0.6);
        assert_eq!(plan.timestamp, 5000);
        assert_eq!(d.last_setpoints.len(), 5);
    }

    #[test]
    fn t119_dispatch_device_add_remove_compatibility() {
        let (pool, mut socs) = two_device_setup();
        // RecordingSolver::new() 返回 Optimal + 空解 → 长度不匹配 → 回退路径
        let solver: Box<dyn Solver> = Box::new(RecordingSolver::new());
        let mut d = MultiDeviceDispatcher::new(pool, solver);
        // 第一次：2 设备，平均分配 10/2 = 5
        let plan1 = d.dispatch(10.0, &socs, 1000).unwrap();
        assert_eq!(plan1.assignments.len(), 2);
        assert_eq!(d.last_setpoints.len(), 2);
        // 移除设备 2：last_setpoints 陈旧条目清理
        assert!(d.pool.remove_device(2));
        let plan2 = d.dispatch(4.0, &socs, 2000).unwrap();
        assert_eq!(plan2.assignments.len(), 1);
        assert_eq!(plan2.assignments[0].device_id, 1);
        assert_eq!(plan2.assignments[0].setpoint, 4.0);
        assert_eq!(d.last_setpoints.get(&2), None);
        assert_eq!(d.last_setpoints.len(), 1);
        // 新增设备 3：正常纳入调度
        d.pool.add_device(3, cap(10.0, 0.85));
        socs.insert(3u64, 0.5f32);
        let plan3 = d.dispatch(6.0, &socs, 3000).unwrap();
        assert_eq!(plan3.assignments.len(), 2);
        assert_eq!(plan3.assignments[0].device_id, 1);
        assert_eq!(plan3.assignments[1].device_id, 3);
        assert_eq!(plan3.assignments[0].setpoint, 3.0);
        assert_eq!(plan3.assignments[1].setpoint, 3.0);
        assert_eq!(d.last_setpoints.len(), 2);
        assert!(d.last_setpoints.contains_key(&3));
    }

    #[test]
    fn t120_dispatch_device_offline_reallocation() {
        let mut pool = DevicePool::new();
        pool.add_device(1, cap(10.0, 0.9));
        pool.add_device(2, cap(10.0, 0.8));
        pool.add_device(3, cap(10.0, 0.85));
        let mut socs = BTreeMap::new();
        socs.insert(1u64, 0.5f32);
        socs.insert(2u64, 0.5f32);
        socs.insert(3u64, 0.5f32);
        // 回退路径（空解 → 长度不匹配）
        let solver: Box<dyn Solver> = Box::new(RecordingSolver::new());
        let mut d = MultiDeviceDispatcher::new(pool, solver);
        // 3 设备在线：12/3 = 4
        let plan1 = d.dispatch(12.0, &socs, 1000).unwrap();
        assert_eq!(plan1.assignments.len(), 3);
        assert_eq!(plan1.total_power, 12.0);
        // 设备 2 SOC 耗尽（离线）→ 仅 1/3 参与，12/2 = 6
        socs.insert(2u64, 0.0f32);
        let plan2 = d.dispatch(12.0, &socs, 2000).unwrap();
        assert_eq!(plan2.assignments.len(), 2);
        assert_eq!(plan2.assignments[0].device_id, 1);
        assert_eq!(plan2.assignments[1].device_id, 3);
        assert_eq!(plan2.assignments[0].setpoint, 6.0);
        assert_eq!(plan2.assignments[1].setpoint, 6.0);
        assert_eq!(plan2.total_power, 12.0);
        // 设备 2 仍在池中（未移除），last_setpoint 保留旧值不再更新
        assert_eq!(d.last_setpoints.get(&2), Some(&4.0));
    }
}

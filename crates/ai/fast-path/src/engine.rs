//! 快速路径引擎（D2：泛型 Solver，默认 MockSolver）.

use eneros_energy_lp_model::config::ScheduleConfig;
use eneros_energy_lp_model::model::EnergyScheduleModel;
use eneros_energy_lp_model::result::ScheduleResult;
use eneros_safety_validator::result::ValidationResult;
use eneros_safety_validator::validator::SafetyValidator;
use eneros_solver_core::result::SolveResult;
use eneros_solver_core::solver::Solver;

use crate::error::FastPathError;
use crate::selector::PathType;
use crate::state::RealtimeState;
use crate::strategy::StrategyTable;

/// 快速路径结果（D8：派生 Debug + Clone）.
#[derive(Debug, Clone)]
pub struct FastPathResult {
    /// 调度方案（截断后优先）.
    pub schedule: ScheduleResult,
    /// 求解结果.
    pub solve_result: SolveResult,
    /// 安全校验结果.
    pub validation: ValidationResult,
    /// 执行耗时（ms，D6: 简化，用 solve_result.elapsed_ms 代替）.
    pub elapsed_ms: u64,
    /// 路径类型.
    pub path_type: PathType,
}

/// 快速路径引擎（D2：泛型 `<S: Solver>`，默认 MockSolver）.
///
/// 执行流程：查表 → 微调 → 编译 LP → 求解 → 解析 → 安全校验。
pub struct RealtimePathEngine<S: Solver> {
    /// 求解器.
    pub solver: S,
    /// 默认调度配置.
    pub default_config: ScheduleConfig,
    /// 安全校验器.
    pub validator: SafetyValidator,
    /// 预计算策略表.
    pub strategy_table: StrategyTable,
}

impl<S: Solver> RealtimePathEngine<S> {
    /// 创建快速路径引擎.
    pub fn new(config: ScheduleConfig, solver: S) -> Self {
        Self {
            solver,
            default_config: config.clone(),
            validator: SafetyValidator::new(),
            strategy_table: StrategyTable::new(config),
        }
    }

    /// 快速路径执行（D6：now_ms 参数替代 Instant::now()）.
    ///
    /// 流程：
    /// 1. 查表获取基础配置
    /// 2. 微调：soc_init + load_demand
    /// 3. 编译 LP
    /// 4. 求解 LP
    /// 5. 解析结果
    /// 6. 安全校验（D12: 传 state.system 给 v0.67.0 SafetyValidator）
    /// 7. 返回结果
    pub fn execute(
        &mut self,
        state: &RealtimeState,
        now_ms: u64,
    ) -> Result<FastPathResult, FastPathError> {
        // 1. 查表获取基础配置
        let mut config = self.strategy_table.get_config(state);

        // 2. 微调：soc_init + load_demand
        config.soc_init = state.system.soc_pct; // D3: 使用 soc_pct
        config.load_demand = state.load_demand.clone();

        // 3. 编译 LP
        let model = EnergyScheduleModel::new(config.clone());
        let problem = model
            .compile()
            .map_err(|e| FastPathError::CompileError(alloc::format!("{:?}", e)))?;

        // 4. 求解 LP
        let solve_result = self
            .solver
            .solve(&problem, now_ms)
            .map_err(|e| FastPathError::SolveError(alloc::format!("{:?}", e)))?;

        // 5. 解析结果
        let schedule = model.parse_result(&solve_result);
        // 61 v0.314.25 §3.4：solve status gate——非成功状态不得解析成计划
        //（Infeasible/Unbounded/Timeout/Error 与变量缺失 default-fill 全拒）。
        match solve_result.status {
            eneros_solver_core::result::SolveStatus::Optimal
            | eneros_solver_core::result::SolveStatus::Suboptimal => {}
            ref other => {
                return Err(FastPathError::SolveError(alloc::format!(
                    "solve status {other:?} 不接受（非成功状态拒绝）"
                )))
            }
        }

        // 6. 安全校验（D12: 传 state.system 给 v0.67.0 SafetyValidator）
        let validation = self.validator.validate(&schedule, &state.system);
        // 32 AI-03：Fatal 闸对称化——慢路径 P0-17（coordinator Fatal →
        // Err）的闭环；快路径 Fatal 必须 Err（不得把越限当建议——任何
        // 未来把快路径接 sink 的改动都不得破封焊）。
        if !validation.passed {
            return Err(FastPathError::SafetyFatal(validation.violations.len()));
        }

        // 7. clamp 建议处理（复查 FIX 61 §3.4）：passed 且有 clamp 建议——
        // 截断结果作为新候选重跑全部规则（revalidate_clamped，≤4 轮）；
        // 收敛干净才采信，否则回落原解（原解已通过全部规则——clamp 只是
        // 建议，不是可以跳过认证的豁免。注：mutex 后置门只查 clamp 终态、
        // 原解不经该门；生产规则全 hard，钳制必伴 hard Critical 使
        // passed=false 在上方闸门拒出——「passed 且带建议」生产不可表示，
        // 本分支仅自定义软钳规则可及；软钳可把一侧钳到恰 0（74 复查轮
        // 订正：旧句「生产规则不会钳没同时充放对」失实），但钳没使终态
        // 更干净。REG-74-Q-R1）。
        let (schedule, validation) = match validation.clamped_schedule.clone() {
            None => (schedule, validation),
            Some(clamped) => self
                .validator
                .revalidate_clamped(clamped, &state.system)
                .unwrap_or((schedule, validation)),
        };

        // 8. 返回结果（elapsed_ms 暂用 0，D6）
        Ok(FastPathResult {
            schedule,
            solve_result,
            validation,
            elapsed_ms: 0, // D6: 简化，用 solve_result.elapsed_ms 代替
            path_type: PathType::FastPath,
        })
    }
}

#[cfg(any(test, feature = "mock"))]
impl RealtimePathEngine<eneros_solver_core::mock::MockSolver> {
    /// 默认构造（使用 MockSolver，D2）.
    ///
    /// v0.205.0（P0-27）：仅 test/mock feature 可用；生产用
    /// [`RealtimePathEngine::new`] 构造器注入真实 Solver。
    pub fn default_with_mock() -> Self {
        Self::new(
            ScheduleConfig::default(),
            eneros_solver_core::mock::MockSolver::new(),
        )
    }
}

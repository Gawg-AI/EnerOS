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

        // 6. 安全校验（D12: 传 state.system 给 v0.67.0 SafetyValidator）
        let validation = self.validator.validate(&schedule, &state.system);

        // 7. 返回结果（elapsed_ms 暂用 0，D6）
        Ok(FastPathResult {
            schedule: validation.clamped_schedule.clone().unwrap_or(schedule),
            solve_result,
            validation,
            elapsed_ms: 0, // D6: 简化，用 solve_result.elapsed_ms 代替
            path_type: PathType::FastPath,
        })
    }
}

impl RealtimePathEngine<eneros_solver_core::mock::MockSolver> {
    /// 默认构造（使用 MockSolver，D2）.
    pub fn default_with_mock() -> Self {
        Self::new(
            ScheduleConfig::default(),
            eneros_solver_core::mock::MockSolver::new(),
        )
    }
}

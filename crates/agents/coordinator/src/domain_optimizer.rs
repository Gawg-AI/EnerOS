//! EnerOS v0.93.0 Edge Coordinator 域级优化.
//!
//! 收集域内 Edge Box 状态 → 构建域级 LP（域平衡 + 各 box 容量约束，损耗最小目标）
//! → Solver 求解 → 按 box 聚合 [`DispatchPlan`] 下发；Solver 失败回退容量比例分摊
//! 兜底（D10）。复用 v0.87.0 `DevicePool`/`DeviceCapability`/`DispatchPlan`/`equal_split`
//! 与 v0.64.0 `Solver`/`LpProblem`，使园区级整体收益优于单机独立调度，为 v0.94.0
//! VPP 聚合提供优化基础。
//!
//! # 偏差声明
//! | 偏差 | 说明 |
//! |------|------|
//! | **D1** | 模块位于既有 `crates/agents/coordinator/`（工作区 §2.3.1 硬规则，v0.92.0 D1 惯例；同 crate 追加） |
//! | **D2** | `box_id: u64` + `BTreeMap<u64, _>` — 无堆字符串 + 确定性迭代，LP 列映射与计划下发顺序可重放（v0.87.0 D3 惯例） |
//! | **D3** | `socs: BTreeMap<u64, f32>` — DeviceId=u64，同 v0.87.0 dispatch 签名 |
//! | **D4** | `Box<dyn Solver>` — no_std 单线程无共享所有权（v0.87.0 D5 惯例；Solver 本就 `&mut self`） |
//! | **D5** | sync `optimize(&mut self, market, target_mw, now_ms)` — no_std 无 async runtime；`&mut` 因 `Solver::solve` 需 `&mut` + 计数器更新（v0.87.0 D1 惯例） |
//! | **D6** | `now_ms: u64` 外部时间注入 — no_std 无 Instant；`DomainPlan.timestamp = now_ms` |
//! | **D7** | 有实际域级耦合的 LP：变量 `p_{box}_{dev}`（bounds [p_min, p_max]）；行 0 域平衡 `Σp = target_mw`；每参与 box 一行容量约束 `Σ_{i∈box} p_i ≤ capacity_mw`；目标 `Minimize Σ(1−eff_i)·p_i`（v0.87.0 D14 一致）；蓝图 `optimize(market)` 无 target → 增加 `target_mw` 参数注入 |
//! | **D8** | `EdgeBoxState.online` + `set_online` — 离线 box 从 LP 与 DomainPlan 排除，不删除状态便于恢复 |
//! | **D9** | 3 个 pub 计数器 `optimize_count`/`fallback_count`/`empty_count`（v0.92.0 D9 惯例；收益经 `DomainPlan.total_revenue` 可观测） |
//! | **D10** | 确定性容量比例兜底（不迭代重试 LP）：solver Err / Infeasible / 解长度不符 → 参与 box 按 `capacity_mw` 比例分摊 + 复用 `equal_split`；`objective_value = 0.0`（失败为兜底非错误） |
//! | **D11** | `target_mw > Σ在线 capacity` → clamp 到总在线容量后再建 LP（构造保证不超发，不报错中断调度）；target 非有限 → `Err(InvalidTarget)` |
//! | **D12** | 净收益 `total_revenue = price × (total_power − total_loss)`（`total_loss = Σ(1−eff_i)·p_i`）；NaN 防御：soc NaN/≤0 → 按耗尽跳过设备；capacity 非有限/≤0 → 排除 box；eff NaN → 0.5 且 clamp [0,1]；price 非有限 → 0.0 |

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use eneros_energy_market_agent::{
    equal_split, DeviceAssignment, DeviceCapability, DeviceMode, DevicePool, DispatchPlan,
    MarketData,
};
use eneros_solver_core::{
    problem::{ConstraintMatrix, LpProblem, ObjectiveSense, VarType},
    result::SolveStatus,
    solver::Solver,
};

/// Edge Box 域内状态（D2/D3/D8；PartialEq 供 v0.96.0 云端汇聚 `DomainData` 比较）.
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeBoxState {
    /// Edge Box ID.
    pub box_id: u64,
    /// 盒内设备池（复用 v0.87.0 [`DevicePool`]）.
    pub devices: DevicePool,
    /// 设备 SOC 表（dev_id → soc；无记录视为合格，v0.87.0 惯例）.
    pub socs: BTreeMap<u64, f32>,
    /// Box 容量上限（MW；非有限或 ≤ 0 → box 排除，D12）.
    pub capacity_mw: f32,
    /// 在线标记（D8：离线从优化排除，状态保留便于恢复）.
    pub online: bool,
}

/// 域级优化计划（D2/D6/D12）.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DomainPlan {
    /// 各 Edge Box 下发计划（box_id → [`DispatchPlan`]，BTreeMap 确定性顺序）.
    pub box_plans: BTreeMap<u64, DispatchPlan>,
    /// 域级净收益（D12：`price × (total_power − total_loss)`）.
    pub total_revenue: f32,
    /// 优化时刻时间戳（u64 ms，回显 `now_ms`，D6）.
    pub timestamp: u64,
}

/// 域级优化错误（Solver 失败为兜底非错误，D10）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptError {
    /// 无合格 box/设备（空域、全离线或全部 SOC 耗尽）.
    EmptyDomain,
    /// 优化目标非法（NaN / ±∞，D11）.
    InvalidTarget,
}

/// Edge Coordinator 域级优化器（D9：计数器全 pub 可观测）.
pub struct DomainOptimizer {
    /// 域内 Edge Box 状态表（box_id 升序，BTreeMap 确定性，D2）.
    pub edge_boxes: BTreeMap<u64, EdgeBoxState>,
    /// 求解器（D4：Box 单线程所有权）.
    pub solver: Box<dyn Solver>,
    /// optimize 调用次数.
    pub optimize_count: u64,
    /// 兜底路径次数（D10）.
    pub fallback_count: u64,
    /// 空域次数.
    pub empty_count: u64,
}

impl DomainOptimizer {
    /// 创建优化器（计数器全零，无 box）.
    pub fn new(solver: Box<dyn Solver>) -> Self {
        Self {
            edge_boxes: BTreeMap::new(),
            solver,
            optimize_count: 0,
            fallback_count: 0,
            empty_count: 0,
        }
    }

    /// 添加/更新 Edge Box（同 id 覆盖）.
    pub fn add_box(&mut self, box_id: u64, state: EdgeBoxState) {
        self.edge_boxes.insert(box_id, state);
    }

    /// 移除 Edge Box；存在返回 true，不存在返回 false.
    pub fn remove_box(&mut self, box_id: u64) -> bool {
        self.edge_boxes.remove(&box_id).is_some()
    }

    /// 设置在线标记（D8）；box 不存在返回 false，离线不删除状态便于恢复.
    pub fn set_online(&mut self, box_id: u64, online: bool) -> bool {
        if let Some(b) = self.edge_boxes.get_mut(&box_id) {
            b.online = online;
            true
        } else {
            false
        }
    }

    /// 执行域级优化（D5/D7/D10/D11/D12）.
    ///
    /// 流程：计数 → target 校验 → clamp 域容量（D11）→ 建 LP（D7）→ solve：
    /// Optimal 且解长度匹配 → 逐设备 clamp 按 box 聚合；其余 → 容量比例兜底（D10）；
    /// 净收益按 D12 计算。
    pub fn optimize(
        &mut self,
        market: &MarketData,
        target_mw: f32,
        now_ms: u64,
    ) -> Result<DomainPlan, OptError> {
        self.optimize_count += 1;
        // D11：目标非法直接拒绝（不计 empty/fallback）
        if !target_mw.is_finite() {
            return Err(OptError::InvalidTarget);
        }
        // D11：target clamp 到总在线容量（构造保证不超发）
        let mut total_cap = 0.0f32;
        for b in self.edge_boxes.values().filter(|b| b.online) {
            if let Some(c) = sanitize_capacity(b.capacity_mw) {
                total_cap += c;
            }
        }
        let clamped = target_mw.min(total_cap);
        // D7：构建域级 LP
        let (problem, cols) = match build_domain_lp(&self.edge_boxes, clamped) {
            Some(v) => v,
            None => {
                self.empty_count += 1;
                return Err(OptError::EmptyDomain);
            }
        };
        let n = cols.len();
        let mut box_plans: BTreeMap<u64, DispatchPlan> = BTreeMap::new();
        let mut total_loss = 0.0f32;
        match self.solver.solve(&problem, now_ms) {
            Ok(result) if result.status == SolveStatus::Optimal && result.solution.len() == n => {
                // 优化路径：逐设备 clamp [p_min, p_max] 后按 box 聚合
                for (i, (box_id, dev_id, cap)) in cols.iter().enumerate() {
                    let sp = result.solution[i]
                        .max(cap.p_min as f64)
                        .min(cap.p_max as f64) as f32;
                    let loss = (1.0 - sanitize_efficiency(cap.efficiency)) * sp;
                    let plan = box_plans.entry(*box_id).or_insert_with(|| DispatchPlan {
                        timestamp: now_ms,
                        assignments: Vec::new(),
                        total_power: 0.0,
                        objective_value: 0.0,
                    });
                    plan.assignments.push(DeviceAssignment {
                        device_id: *dev_id,
                        setpoint: sp,
                        mode: DeviceMode::Auto,
                    });
                    plan.total_power += sp;
                    plan.objective_value += loss;
                    total_loss += loss;
                }
            }
            _ => {
                // D10：容量比例兜底（确定性，不迭代重试 LP）
                self.fallback_count += 1;
                // cols 按 (box_id, dev_id) 升序、同 box 段连续 → 分组还原参与 box
                #[allow(clippy::type_complexity)]
                let mut groups: Vec<(u64, f32, Vec<(u64, DeviceCapability)>)> = Vec::new();
                for (box_id, dev_id, cap) in cols.iter() {
                    let same_box = matches!(groups.last(), Some((id, _, _)) if id == box_id);
                    if !same_box {
                        let box_cap = self
                            .edge_boxes
                            .get(box_id)
                            .and_then(|b| sanitize_capacity(b.capacity_mw))
                            .unwrap_or(0.0);
                        groups.push((*box_id, box_cap, Vec::new()));
                    }
                    if let Some((_, _, devs)) = groups.last_mut() {
                        devs.push((*dev_id, *cap));
                    }
                }
                let part_cap: f32 = groups.iter().map(|(_, c, _)| c).sum();
                for (box_id, cap_b, devs) in &groups {
                    let box_target = clamped * cap_b / part_cap;
                    let assignments = equal_split(box_target, devs);
                    let mut total_power = 0.0f32;
                    for (a, (_, dev_cap)) in assignments.iter().zip(devs.iter()) {
                        total_power += a.setpoint;
                        total_loss += (1.0 - sanitize_efficiency(dev_cap.efficiency)) * a.setpoint;
                    }
                    box_plans.insert(
                        *box_id,
                        DispatchPlan {
                            timestamp: now_ms,
                            assignments,
                            total_power,
                            objective_value: 0.0,
                        },
                    );
                }
            }
        }
        // D12：净收益 = price × (total_power − total_loss)
        let total_power: f32 = box_plans.values().map(|p| p.total_power).sum();
        let total_revenue =
            sanitize_price(market.current_price as f32) * (total_power - total_loss);
        Ok(DomainPlan {
            box_plans,
            total_revenue,
            timestamp: now_ms,
        })
    }
}

/// SOC 过滤（D12）：NaN 或 ≤ 0.0 → None（按耗尽跳过该设备）；否则原样.
fn sanitize_soc(soc: f32) -> Option<f32> {
    if soc.is_nan() || soc <= 0.0 {
        None
    } else {
        Some(soc)
    }
}

/// 容量过滤（D12）：非有限或 ≤ 0.0 → None（排除该 box）；否则原样.
pub(crate) fn sanitize_capacity(cap: f32) -> Option<f32> {
    if !cap.is_finite() || cap <= 0.0 {
        None
    } else {
        Some(cap)
    }
}

/// 效率过滤（D12）：NaN → 0.5 中性；否则 clamp 到 [0.0, 1.0]（±Inf 自然被 clamp）.
pub(crate) fn sanitize_efficiency(eff: f32) -> f32 {
    if eff.is_nan() {
        0.5
    } else {
        eff.clamp(0.0, 1.0)
    }
}

/// 电价过滤（D12）：非有限 → 0.0；否则原样（负电价合法）.
pub(crate) fn sanitize_price(price: f32) -> f32 {
    if price.is_finite() {
        price
    } else {
        0.0
    }
}

/// 构建域级 LP（D7）.
///
/// 合格 box：`online && sanitize_capacity(capacity_mw).is_some()`（box_id 升序）；
/// 合格设备：box 内 dev_id 升序，有 soc 记录且 sanitize 为 None → 跳过，无记录视为合格。
/// 返回 `(LpProblem, 列映射)`，列映射顺序 = (box_id, dev_id) 升序（D2）；无合格设备 → None。
#[allow(clippy::type_complexity)]
fn build_domain_lp(
    boxes: &BTreeMap<u64, EdgeBoxState>,
    target_mw: f32,
) -> Option<(LpProblem, Vec<(u64, u64, DeviceCapability)>)> {
    // 合格设备列映射 + 各参与 box（≥1 台合格设备）的 (capacity, 起始列, 列数)
    let mut cols: Vec<(u64, u64, DeviceCapability)> = Vec::new();
    let mut spans: Vec<(f32, usize, usize)> = Vec::new();
    for (box_id, b) in boxes.iter() {
        if !b.online {
            continue;
        }
        let box_cap = match sanitize_capacity(b.capacity_mw) {
            Some(c) => c,
            None => continue,
        };
        let start = cols.len();
        for (dev_id, cap) in b.devices.devices.iter() {
            if b.socs
                .get(dev_id)
                .is_some_and(|soc| sanitize_soc(*soc).is_none())
            {
                continue;
            }
            cols.push((*box_id, *dev_id, *cap));
        }
        let count = cols.len() - start;
        if count > 0 {
            spans.push((box_cap, start, count));
        }
    }
    if cols.is_empty() {
        return None;
    }
    let n = cols.len();
    let mut variables = Vec::with_capacity(n);
    let mut lower_bounds = Vec::with_capacity(n);
    let mut upper_bounds = Vec::with_capacity(n);
    let mut var_types = Vec::with_capacity(n);
    let mut objective = Vec::with_capacity(n);
    for (box_id, dev_id, cap) in &cols {
        variables.push(alloc::format!("p_{}_{}", box_id, dev_id));
        lower_bounds.push(cap.p_min as f64);
        upper_bounds.push(cap.p_max as f64);
        var_types.push(VarType::Continuous);
        objective.push(1.0 - sanitize_efficiency(cap.efficiency) as f64);
    }
    // 行 0 域平衡 + 每参与 box 1 行容量约束（非零：平衡 n + 容量行合计 n）
    let num_rows = 1 + spans.len();
    let num_nz = 2 * n;
    let mut row_start = Vec::with_capacity(num_rows + 1);
    let mut col_index = Vec::with_capacity(num_nz);
    let mut values = Vec::with_capacity(num_nz);
    let mut rhs_lower = Vec::with_capacity(num_rows);
    let mut rhs_upper = Vec::with_capacity(num_rows);
    // 行 0：Σp = target（rhs 上下界相等）
    row_start.push(0);
    for i in 0..n {
        col_index.push(i as i32);
        values.push(1.0);
    }
    row_start.push(n as i32);
    rhs_lower.push(target_mw as f64);
    rhs_upper.push(target_mw as f64);
    // 每参与 box：Σ_{i∈box} p_i ≤ capacity（rhs_lower = -∞）
    for (box_cap, start, count) in &spans {
        for i in *start..(*start + *count) {
            col_index.push(i as i32);
            values.push(1.0);
        }
        row_start.push(col_index.len() as i32);
        rhs_lower.push(f64::NEG_INFINITY);
        rhs_upper.push(*box_cap as f64);
    }
    let problem = LpProblem {
        variables,
        lower_bounds,
        upper_bounds,
        var_types,
        objective,
        sense: ObjectiveSense::Minimize,
        constraints: ConstraintMatrix::new(num_rows, num_nz, row_start, col_index, values),
        rhs_lower,
        rhs_upper,
    };
    Some((problem, cols))
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use eneros_energy_market_agent::MarketSignal;
    use eneros_solver_core::{error::SolverError, result::SolveResult};

    use super::*;

    // ===== RecordingSolver 测试辅助（模仿 multi_dispatch.rs）=====
    //
    // LP 结构验证直接调用同模块私有 `build_domain_lp`，
    // 本桩仅提供固定结果 / 指定状态 / 失败三种行为，不记录问题。
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

    /// 辅助：构造 p_min=0 的标准设备.
    fn cap(p_max: f32, eff: f32) -> DeviceCapability {
        cap_full(0.0, p_max, eff)
    }

    /// 辅助：构造完整参数设备（ramp_rate=1.0）.
    fn cap_full(p_min: f32, p_max: f32, eff: f32) -> DeviceCapability {
        DeviceCapability {
            p_min,
            p_max,
            ramp_rate: 1.0,
            efficiency: eff,
        }
    }

    /// 辅助：构造在线 EdgeBoxState（devs 为 (dev_id, p_max, eff)，soc 一律 0.5）.
    fn box_with_devs(box_id: u64, capacity: f32, devs: &[(u64, f32, f32)]) -> EdgeBoxState {
        let mut devices = DevicePool::new();
        let mut socs = BTreeMap::new();
        for (id, p_max, eff) in devs {
            devices.add_device(*id, cap(*p_max, *eff));
            socs.insert(*id, 0.5f32);
        }
        EdgeBoxState {
            box_id,
            devices,
            socs,
            capacity_mw: capacity,
            online: true,
        }
    }

    /// 辅助：构造在线 EdgeBoxState（显式能力参数，无 soc 记录 → 视为合格）.
    fn box_state(box_id: u64, capacity: f32, devs: &[(u64, DeviceCapability)]) -> EdgeBoxState {
        let mut devices = DevicePool::new();
        for (id, c) in devs {
            devices.add_device(*id, *c);
        }
        EdgeBoxState {
            box_id,
            devices,
            socs: BTreeMap::new(),
            capacity_mw: capacity,
            online: true,
        }
    }

    /// 辅助：构造仅含 current_price 的 MarketData.
    fn market(price: f64) -> MarketData {
        MarketData {
            timestamp: 0,
            price_forecast: Vec::new(),
            current_price: price,
            load_forecast: None,
            signal_type: MarketSignal::RealtimePrice,
        }
    }

    // ===== T1~T8：数据结构与构造 =====
    #[test]
    fn t01_edge_box_state_field_echo() {
        let b = box_with_devs(7, 5.0, &[(1, 10.0, 0.9), (2, 8.0, 0.8)]);
        assert_eq!(b.box_id, 7);
        assert_eq!(b.capacity_mw, 5.0);
        assert!(b.online);
        assert_eq!(b.devices.len(), 2);
        assert_eq!(b.devices.get(1).unwrap().p_max, 10.0);
        assert_eq!(b.socs.get(&2), Some(&0.5));
    }

    #[test]
    fn t02_edge_box_state_clone() {
        let b = box_with_devs(3, 4.0, &[(1, 10.0, 0.9)]);
        let c = b.clone();
        assert_eq!(c.box_id, 3);
        assert_eq!(c.capacity_mw, 4.0);
        assert_eq!(c.devices.get(1), b.devices.get(1));
        assert_eq!(c.socs.get(&1), b.socs.get(&1));
        assert!(c.online);
    }

    #[test]
    fn t03_edge_box_state_debug_smoke() {
        let b = box_with_devs(1, 5.0, &[(1, 10.0, 0.9)]);
        let _ = format!("{:?}", b);
    }

    #[test]
    fn t04_domain_plan_default() {
        let p = DomainPlan::default();
        assert!(p.box_plans.is_empty());
        assert_eq!(p.total_revenue, 0.0);
        assert_eq!(p.timestamp, 0);
    }

    #[test]
    fn t05_domain_plan_clone_and_eq() {
        let mut box_plans = BTreeMap::new();
        box_plans.insert(
            1u64,
            DispatchPlan {
                timestamp: 1000,
                assignments: vec![DeviceAssignment {
                    device_id: 1,
                    setpoint: 2.0,
                    mode: DeviceMode::Auto,
                }],
                total_power: 2.0,
                objective_value: 0.1,
            },
        );
        let p = DomainPlan {
            box_plans,
            total_revenue: 1.8,
            timestamp: 1000,
        };
        let q = p.clone();
        assert_eq!(p, q);
        assert_eq!(q.box_plans.get(&1).unwrap().total_power, 2.0);
    }

    #[test]
    fn t06_opt_error_eq_ne_copy() {
        assert_eq!(OptError::EmptyDomain, OptError::EmptyDomain);
        assert_eq!(OptError::InvalidTarget, OptError::InvalidTarget);
        assert_ne!(OptError::EmptyDomain, OptError::InvalidTarget);
        let e = OptError::EmptyDomain;
        let e2 = e; // Copy 语义
        assert_eq!(e, e2);
    }

    #[test]
    fn t07_opt_error_debug_smoke() {
        let _ = format!("{:?}", OptError::EmptyDomain);
        let _ = format!("{:?}", OptError::InvalidTarget);
    }

    #[test]
    fn t08_optimizer_new_counters_zero() {
        let solver: Box<dyn Solver> = Box::new(RecordingSolver::new());
        let opt = DomainOptimizer::new(solver);
        assert_eq!(opt.optimize_count, 0);
        assert_eq!(opt.fallback_count, 0);
        assert_eq!(opt.empty_count, 0);
        assert!(opt.edge_boxes.is_empty());
    }

    // ===== T9~T16：NaN 防御 sanitize（D12）=====
    #[test]
    fn t09_sanitize_soc_normal() {
        assert_eq!(sanitize_soc(0.5), Some(0.5));
        assert_eq!(sanitize_soc(1.0), Some(1.0));
    }

    #[test]
    fn t10_sanitize_soc_nan() {
        assert_eq!(sanitize_soc(f32::NAN), None);
    }

    #[test]
    fn t11_sanitize_soc_zero_and_negative() {
        assert_eq!(sanitize_soc(0.0), None);
        assert_eq!(sanitize_soc(-0.5), None);
    }

    #[test]
    fn t12_sanitize_capacity_normal() {
        assert_eq!(sanitize_capacity(5.0), Some(5.0));
        assert_eq!(sanitize_capacity(0.001), Some(0.001));
    }

    #[test]
    fn t13_sanitize_capacity_nonfinite() {
        assert_eq!(sanitize_capacity(f32::NAN), None);
        assert_eq!(sanitize_capacity(f32::INFINITY), None);
        assert_eq!(sanitize_capacity(f32::NEG_INFINITY), None);
    }

    #[test]
    fn t14_sanitize_capacity_zero_and_negative() {
        assert_eq!(sanitize_capacity(0.0), None);
        assert_eq!(sanitize_capacity(-1.0), None);
    }

    #[test]
    fn t15_sanitize_efficiency_clamp() {
        assert_eq!(sanitize_efficiency(f32::NAN), 0.5);
        assert_eq!(sanitize_efficiency(-0.5), 0.0);
        assert_eq!(sanitize_efficiency(1.5), 1.0);
        assert_eq!(sanitize_efficiency(f32::INFINITY), 1.0);
        assert_eq!(sanitize_efficiency(0.8), 0.8);
    }

    #[test]
    fn t16_sanitize_price() {
        assert_eq!(sanitize_price(f32::NAN), 0.0);
        assert_eq!(sanitize_price(f32::INFINITY), 0.0);
        assert_eq!(sanitize_price(f32::NEG_INFINITY), 0.0);
        assert_eq!(sanitize_price(0.6), 0.6);
        assert_eq!(sanitize_price(-1.0), -1.0); // 负电价原样保留
    }

    // ===== T17~T20：盒管理 =====
    #[test]
    fn t17_add_box_insert_and_overwrite() {
        let mut opt = DomainOptimizer::new(Box::new(RecordingSolver::new()));
        opt.add_box(1, box_with_devs(1, 5.0, &[(1, 10.0, 0.9)]));
        assert_eq!(opt.edge_boxes.len(), 1);
        assert_eq!(opt.edge_boxes.get(&1).unwrap().capacity_mw, 5.0);
        // 同 id 覆盖
        opt.add_box(1, box_with_devs(1, 8.0, &[(1, 12.0, 0.8)]));
        assert_eq!(opt.edge_boxes.len(), 1);
        assert_eq!(opt.edge_boxes.get(&1).unwrap().capacity_mw, 8.0);
        assert_eq!(
            opt.edge_boxes
                .get(&1)
                .unwrap()
                .devices
                .get(1)
                .unwrap()
                .p_max,
            12.0
        );
    }

    #[test]
    fn t18_remove_box_true_false() {
        let mut opt = DomainOptimizer::new(Box::new(RecordingSolver::new()));
        opt.add_box(1, box_with_devs(1, 5.0, &[(1, 10.0, 0.9)]));
        assert!(opt.remove_box(1));
        assert!(opt.edge_boxes.is_empty());
        assert!(!opt.remove_box(1)); // 已删除 → false
        assert!(!opt.remove_box(99)); // 不存在 → false
    }

    #[test]
    fn t19_set_online_returns_and_retains_state() {
        let mut opt = DomainOptimizer::new(Box::new(RecordingSolver::new()));
        opt.add_box(1, box_with_devs(1, 5.0, &[(1, 10.0, 0.9)]));
        // 存在的 id → true，离线后状态保留（D8）
        assert!(opt.set_online(1, false));
        assert!(!opt.edge_boxes.get(&1).unwrap().online);
        assert_eq!(opt.edge_boxes.get(&1).unwrap().capacity_mw, 5.0);
        assert_eq!(opt.edge_boxes.get(&1).unwrap().devices.len(), 1);
        // 恢复在线
        assert!(opt.set_online(1, true));
        assert!(opt.edge_boxes.get(&1).unwrap().online);
        // 不存在的 id → false
        assert!(!opt.set_online(99, false));
    }

    #[test]
    fn t20_offline_single_box_empty_domain() {
        let mut opt = DomainOptimizer::new(Box::new(RecordingSolver::new()));
        opt.add_box(1, box_with_devs(1, 5.0, &[(1, 10.0, 0.9)]));
        assert!(opt.set_online(1, false));
        let err = opt.optimize(&market(1.0), 3.0, 1000).unwrap_err();
        assert_eq!(err, OptError::EmptyDomain);
        assert_eq!(opt.empty_count, 1);
        // 状态保留未删除（D8）
        assert!(opt.edge_boxes.contains_key(&1));
        assert!(!opt.edge_boxes.get(&1).unwrap().online);
        assert_eq!(opt.edge_boxes.get(&1).unwrap().devices.len(), 1);
    }

    // ===== T21~T26：build_domain_lp 结构（D7）=====
    #[test]
    fn t21_build_lp_variables_named_and_ordered() {
        let mut boxes = BTreeMap::new();
        boxes.insert(
            1u64,
            box_with_devs(1, 6.0, &[(1, 10.0, 0.9), (2, 10.0, 0.8)]),
        );
        boxes.insert(
            2u64,
            box_with_devs(2, 4.0, &[(1, 10.0, 0.85), (2, 10.0, 0.75)]),
        );
        let (p, cols) = build_domain_lp(&boxes, 8.0).unwrap();
        // 4 变量按 (box_id, dev_id) 升序（D2）
        assert_eq!(p.variables.len(), 4);
        assert_eq!(p.variables, vec!["p_1_1", "p_1_2", "p_2_1", "p_2_2"]);
        assert_eq!(cols.len(), 4);
        assert_eq!((cols[0].0, cols[0].1), (1, 1));
        assert_eq!((cols[1].0, cols[1].1), (1, 2));
        assert_eq!((cols[2].0, cols[2].1), (2, 1));
        assert_eq!((cols[3].0, cols[3].1), (2, 2));
    }

    #[test]
    fn t22_build_lp_bounds_types_objective() {
        let mut boxes = BTreeMap::new();
        boxes.insert(
            1u64,
            box_state(
                1,
                6.0,
                &[(1, cap_full(-1.0, 3.0, 0.9)), (2, cap_full(0.0, 5.0, 0.8))],
            ),
        );
        let (p, _) = build_domain_lp(&boxes, 4.0).unwrap();
        assert_eq!(p.lower_bounds, vec![-1.0, 0.0]);
        assert_eq!(p.upper_bounds, vec![3.0, 5.0]);
        assert_eq!(p.var_types, vec![VarType::Continuous, VarType::Continuous]);
        assert_eq!(p.sense, ObjectiveSense::Minimize);
        // 目标系数 = 1 − sanitize(eff)（f32→f64 容差 1e-6）
        assert_eq!(p.objective.len(), 2);
        assert!((p.objective[0] - 0.1).abs() < 1e-6);
        assert!((p.objective[1] - 0.2).abs() < 1e-6);
    }

    #[test]
    fn t23_build_lp_row_structure() {
        let mut boxes = BTreeMap::new();
        boxes.insert(
            1u64,
            box_with_devs(1, 6.0, &[(1, 10.0, 0.9), (2, 10.0, 0.8)]),
        );
        boxes.insert(
            2u64,
            box_with_devs(2, 4.0, &[(1, 10.0, 0.85), (2, 10.0, 0.75)]),
        );
        let (p, _) = build_domain_lp(&boxes, 8.0).unwrap();
        // 3 行：1 平衡 + 2 容量；非零 = 4（平衡）+ 4（容量）
        assert_eq!(p.constraints.num_rows, 3);
        assert_eq!(p.constraints.num_nz, 8);
        assert_eq!(p.constraints.row_start, vec![0, 4, 6, 8]);
        assert_eq!(p.constraints.col_index, vec![0, 1, 2, 3, 0, 1, 2, 3]);
        assert_eq!(p.constraints.values, vec![1.0; 8]);
        // 平衡行 rhs 上下界相等 == target
        assert_eq!(p.rhs_lower[0], 8.0);
        assert_eq!(p.rhs_upper[0], 8.0);
    }

    #[test]
    fn t24_build_lp_capacity_rows() {
        let mut boxes = BTreeMap::new();
        boxes.insert(
            1u64,
            box_with_devs(1, 6.0, &[(1, 10.0, 0.9), (2, 10.0, 0.8)]),
        );
        boxes.insert(
            2u64,
            box_with_devs(2, 4.0, &[(1, 10.0, 0.85), (2, 10.0, 0.75)]),
        );
        let (p, _) = build_domain_lp(&boxes, 8.0).unwrap();
        // 容量行 rhs_lower = -∞，rhs_upper = 各 box capacity
        assert_eq!(p.rhs_lower[1], f64::NEG_INFINITY);
        assert_eq!(p.rhs_lower[2], f64::NEG_INFINITY);
        assert_eq!(p.rhs_upper[1], 6.0);
        assert_eq!(p.rhs_upper[2], 4.0);
        // CSR 列索引：行 1 覆盖 box1 设备列 0,1；行 2 覆盖 box2 设备列 2,3
        assert_eq!(&p.constraints.col_index[4..6], &[0, 1]);
        assert_eq!(&p.constraints.col_index[6..8], &[2, 3]);
        assert_eq!(&p.constraints.values[4..8], &[1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn t25_build_lp_deterministic_rebuild() {
        let mut boxes = BTreeMap::new();
        boxes.insert(
            2u64,
            box_with_devs(2, 4.0, &[(2, 8.0, 0.75), (1, 6.0, 0.85)]),
        );
        boxes.insert(1u64, box_with_devs(1, 6.0, &[(1, 10.0, 0.9)]));
        let (p1, c1) = build_domain_lp(&boxes, 5.0).unwrap();
        let (p2, c2) = build_domain_lp(&boxes, 5.0).unwrap();
        // 同输入两次构建逐字段一致（确定性可重放，D2）
        assert_eq!(p1.variables, p2.variables);
        assert_eq!(p1.lower_bounds, p2.lower_bounds);
        assert_eq!(p1.upper_bounds, p2.upper_bounds);
        assert_eq!(p1.var_types, p2.var_types);
        assert_eq!(p1.objective, p2.objective);
        assert_eq!(p1.sense, p2.sense);
        assert_eq!(p1.constraints.num_rows, p2.constraints.num_rows);
        assert_eq!(p1.constraints.num_nz, p2.constraints.num_nz);
        assert_eq!(p1.constraints.row_start, p2.constraints.row_start);
        assert_eq!(p1.constraints.col_index, p2.constraints.col_index);
        assert_eq!(p1.constraints.values, p2.constraints.values);
        assert_eq!(p1.rhs_lower, p2.rhs_lower);
        assert_eq!(p1.rhs_upper, p2.rhs_upper);
        assert_eq!(c1.len(), c2.len());
        for (a, b) in c1.iter().zip(c2.iter()) {
            assert_eq!((a.0, a.1), (b.0, b.1));
            assert_eq!(a.2, b.2);
        }
    }

    #[test]
    fn t26_build_lp_no_eligible_devices_none() {
        // 全部 soc 耗尽 → None
        let mut boxes = BTreeMap::new();
        let mut b1 = box_with_devs(1, 5.0, &[(1, 10.0, 0.9), (2, 10.0, 0.8)]);
        b1.socs.insert(1, 0.0);
        b1.socs.insert(2, 0.0);
        boxes.insert(1u64, b1);
        assert!(build_domain_lp(&boxes, 5.0).is_none());
        // 空设备 box → None
        let mut boxes2 = BTreeMap::new();
        boxes2.insert(1u64, box_with_devs(1, 5.0, &[]));
        assert!(build_domain_lp(&boxes2, 5.0).is_none());
    }

    // ===== T27~T29：optimize 校验与计数器 =====
    #[test]
    fn t27_optimize_invalid_target() {
        let mut opt = DomainOptimizer::new(Box::new(RecordingSolver::new()));
        opt.add_box(1, box_with_devs(1, 5.0, &[(1, 10.0, 0.9)]));
        let m = market(1.0);
        assert_eq!(
            opt.optimize(&m, f32::NAN, 1000).unwrap_err(),
            OptError::InvalidTarget
        );
        assert_eq!(
            opt.optimize(&m, f32::INFINITY, 1000).unwrap_err(),
            OptError::InvalidTarget
        );
        assert_eq!(
            opt.optimize(&m, f32::NEG_INFINITY, 1000).unwrap_err(),
            OptError::InvalidTarget
        );
        assert_eq!(opt.optimize_count, 3);
        assert_eq!(opt.empty_count, 0);
        assert_eq!(opt.fallback_count, 0);
    }

    #[test]
    fn t28_optimize_empty_domain() {
        let m = market(1.0);
        // 无 box
        let mut o1 = DomainOptimizer::new(Box::new(RecordingSolver::new()));
        assert_eq!(
            o1.optimize(&m, 5.0, 1000).unwrap_err(),
            OptError::EmptyDomain
        );
        assert_eq!(o1.empty_count, 1);
        // 全离线
        let mut o2 = DomainOptimizer::new(Box::new(RecordingSolver::new()));
        o2.add_box(1, box_with_devs(1, 5.0, &[(1, 10.0, 0.9)]));
        o2.set_online(1, false);
        assert_eq!(
            o2.optimize(&m, 5.0, 1000).unwrap_err(),
            OptError::EmptyDomain
        );
        assert_eq!(o2.empty_count, 1);
        // 全设备 soc NaN
        let mut o3 = DomainOptimizer::new(Box::new(RecordingSolver::new()));
        let mut b = box_with_devs(1, 5.0, &[(1, 10.0, 0.9)]);
        b.socs.insert(1, f32::NAN);
        o3.add_box(1, b);
        assert_eq!(
            o3.optimize(&m, 5.0, 1000).unwrap_err(),
            OptError::EmptyDomain
        );
        assert_eq!(o3.empty_count, 1);
        assert_eq!(o3.optimize_count, 1);
        assert_eq!(o3.fallback_count, 0);
    }

    #[test]
    fn t29_optimize_success_counters() {
        let mut opt = DomainOptimizer::new(Box::new(RecordingSolver::with_result(
            SolveResult::optimal(0.3, vec![3.0]),
        )));
        opt.add_box(1, box_with_devs(1, 5.0, &[(1, 10.0, 0.9)]));
        let plan = opt.optimize(&market(1.0), 3.0, 1000).unwrap();
        assert_eq!(opt.optimize_count, 1);
        assert_eq!(opt.fallback_count, 0);
        assert_eq!(opt.empty_count, 0);
        assert_eq!(plan.timestamp, 1000);
        assert_eq!(plan.box_plans.len(), 1);
    }

    // ===== T30~T32：spec 场景（多 Box 协同分配）=====
    #[test]
    fn t30_two_box_optimal_allocation() {
        // spec 场景：2 在线 box cap 6/4，各 1 设备 eff 0.95/0.75，target=8.0
        let mut opt = DomainOptimizer::new(Box::new(RecordingSolver::with_result(
            SolveResult::optimal(0.8, vec![6.0, 2.0]),
        )));
        opt.add_box(1, box_with_devs(1, 6.0, &[(1, 10.0, 0.95)]));
        opt.add_box(2, box_with_devs(2, 4.0, &[(2, 10.0, 0.75)]));
        let plan = opt.optimize(&market(2.0), 8.0, 5000).unwrap();
        assert_eq!(plan.box_plans.len(), 2);
        let p1 = plan.box_plans.get(&1).unwrap();
        let p2 = plan.box_plans.get(&2).unwrap();
        assert_eq!(p1.total_power, 6.0);
        assert_eq!(p2.total_power, 2.0);
        assert_eq!(p1.timestamp, 5000);
        assert_eq!(p2.timestamp, 5000);
        assert_eq!(plan.timestamp, 5000);
        assert_eq!(p1.assignments[0].device_id, 1);
        assert_eq!(p1.assignments[0].mode, DeviceMode::Auto);
        assert_eq!(p2.assignments[0].device_id, 2);
    }

    #[test]
    fn t31_solution_clamped_and_box_objective() {
        // solver 解越界 → 逐设备 clamp [p_min, p_max]
        let mut opt = DomainOptimizer::new(Box::new(RecordingSolver::with_result(
            SolveResult::optimal(0.0, vec![7.0, 0.5]),
        )));
        opt.add_box(1, box_state(1, 6.0, &[(1, cap_full(0.0, 5.0, 0.9))]));
        opt.add_box(2, box_state(2, 4.0, &[(2, cap_full(1.0, 4.0, 0.8))]));
        let plan = opt.optimize(&market(1.0), 6.0, 1000).unwrap();
        let p1 = plan.box_plans.get(&1).unwrap();
        let p2 = plan.box_plans.get(&2).unwrap();
        // dev1 7.0 → clamp p_max 5.0；dev2 0.5 → clamp p_min 1.0
        assert_eq!(p1.assignments[0].setpoint, 5.0);
        assert_eq!(p2.assignments[0].setpoint, 1.0);
        assert_eq!(p1.total_power, 5.0);
        assert_eq!(p2.total_power, 1.0);
        // box objective_value == Σ(1−eff)·sp（镜像实现 f32 运算次序，位级一致）
        let o1 = (1.0f32 - 0.9f32) * 5.0f32;
        let o2 = (1.0f32 - 0.8f32) * 1.0f32;
        assert_eq!(p1.objective_value, o1);
        assert_eq!(p2.objective_value, o2);
    }

    #[test]
    fn t32_revenue_net_of_losses() {
        // 同 T30 场景：revenue 精确 == price × (total_power − (0.05×6 + 0.25×2))
        let mut opt = DomainOptimizer::new(Box::new(RecordingSolver::with_result(
            SolveResult::optimal(0.8, vec![6.0, 2.0]),
        )));
        opt.add_box(1, box_with_devs(1, 6.0, &[(1, 10.0, 0.95)]));
        opt.add_box(2, box_with_devs(2, 4.0, &[(2, 10.0, 0.75)]));
        let plan = opt.optimize(&market(2.0), 8.0, 5000).unwrap();
        let l1 = (1.0f32 - 0.95f32) * 6.0f32;
        let l2 = (1.0f32 - 0.75f32) * 2.0f32;
        let expected = 2.0f32 * (8.0f32 - (l1 + l2));
        assert_eq!(plan.total_revenue, expected);
        assert!(plan.total_revenue > 0.0);
    }

    // ===== T33~T34：离线 Box 排除重优化（D8）=====
    #[test]
    fn t33_offline_box_excluded_reoptimize() {
        // 3 在线 box（cap 均 3.0，dev p_max 5.0），target=6.0，failing solver 走兜底
        let mut opt = DomainOptimizer::new(Box::new(RecordingSolver::failing()));
        for id in 1..=3u64 {
            opt.add_box(id, box_with_devs(id, 3.0, &[(id, 5.0, 0.9)]));
        }
        let m = market(1.0);
        let plan1 = opt.optimize(&m, 6.0, 1000).unwrap();
        assert_eq!(plan1.box_plans.len(), 3);
        // 各 box 分 6 × 3/9 = 2.0
        assert_eq!(plan1.box_plans.get(&1).unwrap().total_power, 2.0);
        assert_eq!(plan1.box_plans.get(&2).unwrap().total_power, 2.0);
        // box 2 离线 → 重优化不含 box 2，target 全分给 1/3（各 6 × 3/6 = 3.0）
        assert!(opt.set_online(2, false));
        let plan2 = opt.optimize(&m, 6.0, 2000).unwrap();
        assert_eq!(plan2.box_plans.len(), 2);
        assert!(!plan2.box_plans.contains_key(&2));
        assert_eq!(plan2.box_plans.get(&1).unwrap().total_power, 3.0);
        assert_eq!(plan2.box_plans.get(&3).unwrap().total_power, 3.0);
        let total: f32 = plan2.box_plans.values().map(|p| p.total_power).sum();
        assert_eq!(total, 6.0);
    }

    #[test]
    fn t34_online_restore_reincluded() {
        let mut opt = DomainOptimizer::new(Box::new(RecordingSolver::failing()));
        for id in 1..=3u64 {
            opt.add_box(id, box_with_devs(id, 3.0, &[(id, 5.0, 0.9)]));
        }
        let m = market(1.0);
        assert!(opt.set_online(2, false));
        let plan1 = opt.optimize(&m, 6.0, 1000).unwrap();
        assert!(!plan1.box_plans.contains_key(&2));
        // 恢复在线 → 重新纳入
        assert!(opt.set_online(2, true));
        let plan2 = opt.optimize(&m, 6.0, 2000).unwrap();
        assert_eq!(plan2.box_plans.len(), 3);
        assert_eq!(plan2.box_plans.get(&2).unwrap().total_power, 2.0);
        assert!(opt.edge_boxes.get(&2).unwrap().online);
    }

    // ===== T35~T36：D10 容量比例兜底 =====
    #[test]
    fn t35_solver_err_fallback_proportional() {
        // 2 box cap 6/4，target=10 → box1 分 6.0、box2 分 4.0
        let mut opt = DomainOptimizer::new(Box::new(RecordingSolver::failing()));
        opt.add_box(1, box_with_devs(1, 6.0, &[(1, 10.0, 0.9)]));
        opt.add_box(2, box_with_devs(2, 4.0, &[(2, 10.0, 0.8)]));
        let plan = opt.optimize(&market(1.0), 10.0, 1000).unwrap();
        assert_eq!(opt.fallback_count, 1);
        let p1 = plan.box_plans.get(&1).unwrap();
        let p2 = plan.box_plans.get(&2).unwrap();
        assert_eq!(p1.total_power, 6.0);
        assert_eq!(p2.total_power, 4.0);
        assert_eq!(p1.objective_value, 0.0);
        assert_eq!(p2.objective_value, 0.0);
        // revenue 仍按实际分配计算（>0 当 price>0 且 loss<total_power）
        let expected =
            1.0f32 * (10.0f32 - ((1.0f32 - 0.9f32) * 6.0f32 + (1.0f32 - 0.8f32) * 4.0f32));
        assert_eq!(plan.total_revenue, expected);
        // setpoint 来自 equal_split clamp（p_max=10 不触发）
        assert_eq!(p1.assignments[0].setpoint, 6.0);
        assert_eq!(p2.assignments[0].setpoint, 4.0);
    }

    #[test]
    fn t36_infeasible_and_len_mismatch_fallback() {
        // 情形 1：SolveStatus::Infeasible → 兜底
        let infeasible = SolveResult {
            status: SolveStatus::Infeasible,
            objective_value: 0.0,
            solution: vec![],
            elapsed_ms: 0,
            dual_solution: None,
        };
        let mut o1 = DomainOptimizer::new(Box::new(RecordingSolver::with_result(infeasible)));
        o1.add_box(1, box_with_devs(1, 6.0, &[(1, 10.0, 0.9)]));
        o1.add_box(2, box_with_devs(2, 4.0, &[(2, 10.0, 0.8)]));
        let plan = o1.optimize(&market(1.0), 10.0, 1000).unwrap();
        assert_eq!(o1.fallback_count, 1);
        assert_eq!(plan.box_plans.get(&1).unwrap().total_power, 6.0);
        assert_eq!(plan.box_plans.get(&2).unwrap().total_power, 4.0);
        // 情形 2：Optimal 但解长度不符（2 变量解 vs 1 列）→ 兜底
        let mut o2 = DomainOptimizer::new(Box::new(RecordingSolver::with_result(
            SolveResult::optimal(0.0, vec![1.0, 2.0]),
        )));
        o2.add_box(1, box_with_devs(1, 6.0, &[(1, 10.0, 0.9)]));
        let plan2 = o2.optimize(&market(1.0), 3.0, 1000).unwrap();
        assert_eq!(o2.fallback_count, 1);
        assert_eq!(plan2.box_plans.get(&1).unwrap().total_power, 3.0);
        assert_eq!(plan2.box_plans.get(&1).unwrap().objective_value, 0.0);
    }

    // ===== T37：域容量安全（D11，蓝图 §7.3）=====
    #[test]
    fn t37_target_clamped_to_domain_capacity() {
        // target=100 总在线 cap=10 → 不报错，总出力 ≤ 10
        let mut opt = DomainOptimizer::new(Box::new(RecordingSolver::failing()));
        opt.add_box(1, box_with_devs(1, 6.0, &[(1, 10.0, 0.9)]));
        opt.add_box(2, box_with_devs(2, 4.0, &[(2, 10.0, 0.8)]));
        let plan = opt.optimize(&market(1.0), 100.0, 1000).unwrap();
        let total: f32 = plan.box_plans.values().map(|p| p.total_power).sum();
        assert!(total <= 10.0);
        assert_eq!(total, 10.0);
        assert_eq!(plan.box_plans.get(&1).unwrap().total_power, 6.0);
        assert_eq!(plan.box_plans.get(&2).unwrap().total_power, 4.0);
        // 优化路径同样 clamp：LP 平衡行 rhs == clamped 10.0
        let (problem, _) = build_domain_lp(&opt.edge_boxes, 10.0).unwrap();
        assert_eq!(problem.rhs_lower[0], 10.0);
        assert_eq!(problem.rhs_upper[0], 10.0);
    }

    // ===== T38~T39：NaN 风暴（D12）=====
    #[test]
    fn t38_nan_soc_skip_and_nan_capacity_exclude() {
        let mut opt = DomainOptimizer::new(Box::new(RecordingSolver::with_result(
            SolveResult::optimal(0.1, vec![2.0]),
        )));
        // box 1：capacity NaN → 整体排除
        let mut b1 = box_with_devs(1, 5.0, &[(1, 10.0, 0.9)]);
        b1.capacity_mw = f32::NAN;
        opt.add_box(1, b1);
        // box 2：dev 2 soc NaN → 跳过；dev 3 soc 0.5 → 保留（解长度 1 匹配）
        let mut b2 = box_with_devs(2, 5.0, &[(2, 10.0, 0.9), (3, 10.0, 0.9)]);
        b2.socs.insert(2, f32::NAN);
        opt.add_box(2, b2);
        let plan = opt.optimize(&market(1.0), 2.0, 1000).unwrap();
        assert_eq!(plan.box_plans.len(), 1);
        assert!(!plan.box_plans.contains_key(&1));
        let p2 = plan.box_plans.get(&2).unwrap();
        assert_eq!(p2.assignments.len(), 1);
        assert_eq!(p2.assignments[0].device_id, 3);
        assert_eq!(p2.assignments[0].setpoint, 2.0);
        assert_eq!(opt.fallback_count, 0);
    }

    #[test]
    fn t39_nan_efficiency_neutral_and_nan_price_zero_revenue() {
        let mut opt = DomainOptimizer::new(Box::new(RecordingSolver::with_result(
            SolveResult::optimal(0.0, vec![2.0]),
        )));
        // eff NaN → sanitize 0.5 中性
        opt.add_box(1, box_with_devs(1, 5.0, &[(1, 10.0, f32::NAN)]));
        // price NaN → sanitize 0.0 → revenue 归零
        let plan = opt.optimize(&market(f64::NAN), 2.0, 1000).unwrap();
        let p1 = plan.box_plans.get(&1).unwrap();
        let expected_obj = (1.0f32 - 0.5f32) * 2.0f32;
        assert_eq!(p1.objective_value, expected_obj);
        assert_eq!(plan.total_revenue, 0.0);
        // LP 目标系数同样为 1−0.5
        let (problem, _) = build_domain_lp(&opt.edge_boxes, 2.0).unwrap();
        assert_eq!(problem.objective[0], 0.5);
    }

    // ===== T40：5-Box 集成 + 收益优于单机（蓝图 §7.2，D12 净收益可判定）=====
    #[test]
    fn t40_five_box_integration_revenue_beats_fallback() {
        // 5 box：cap 均 2.0，dev p_max 5.0，eff 递降 0.95/0.90/0.85/0.80/0.75
        let effs = [0.95f32, 0.90, 0.85, 0.80, 0.75];
        let build = |opt: &mut DomainOptimizer| {
            for (i, eff) in effs.iter().enumerate() {
                let id = (i + 1) as u64;
                opt.add_box(id, box_with_devs(id, 2.0, &[(id, 5.0, *eff)]));
            }
        };
        // 优化路径：LP 最优解出力集中于高效 box（前 4 各 2.0，box5 0.0）
        let mut opt_lp = DomainOptimizer::new(Box::new(RecordingSolver::with_result(
            SolveResult::optimal(1.0, vec![2.0, 2.0, 2.0, 2.0, 0.0]),
        )));
        build(&mut opt_lp);
        let plan_lp = opt_lp.optimize(&market(1.0), 8.0, 1000).unwrap();
        assert_eq!(plan_lp.box_plans.len(), 5);
        assert_eq!(plan_lp.timestamp, 1000);
        assert_eq!(opt_lp.fallback_count, 0);
        assert_eq!(opt_lp.optimize_count, 1);
        // 兜底路径：同输入 failing solver → 容量比例均分（各 8×2/10=1.6）
        let mut opt_fb = DomainOptimizer::new(Box::new(RecordingSolver::failing()));
        build(&mut opt_fb);
        let plan_fb = opt_fb.optimize(&market(1.0), 8.0, 1000).unwrap();
        assert_eq!(opt_fb.fallback_count, 1);
        for p in plan_fb.box_plans.values() {
            assert_eq!(p.total_power, 1.6);
        }
        // 净收益判定：优化路径（损耗集中低损 box）严格大于兜底路径
        assert!(plan_lp.total_revenue > plan_fb.total_revenue);
        assert!(plan_lp.total_revenue > 0.0);
        // 离线故障注入：box 5 离线后重优化（LP 解长度变为 4）
        let mut opt2 = DomainOptimizer::new(Box::new(RecordingSolver::with_result(
            SolveResult::optimal(0.8, vec![2.0, 2.0, 2.0, 2.0]),
        )));
        build(&mut opt2);
        assert!(opt2.set_online(5, false));
        let plan2 = opt2.optimize(&market(1.0), 8.0, 2000).unwrap();
        assert_eq!(plan2.box_plans.len(), 4);
        assert!(!plan2.box_plans.contains_key(&5));
        let total2: f32 = plan2.box_plans.values().map(|p| p.total_power).sum();
        assert_eq!(total2, 8.0);
    }
}

//! EnerOS v0.93.0 Edge Coordinator 域级优化.
//!
//! 收集域内 Edge Box 状态 → 构建域级 LP（域平衡 + 各 box 容量约束，损耗最小目标）
//! → Solver 求解 → 按 box 聚合 [`eneros_energy_market_agent::DispatchPlan`] 下发；Solver 失败回退容量比例分摊
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
//! | **D12** | 净收益 `total_revenue = price × (total_power − total_loss)`（`total_loss = Σ(1−eff_i)·p_i`）；NaN 防御：soc NaN/≤0 → 按耗尽跳过设备；capacity 非有限/≤0 → 排除 box；eff NaN → 0.5 且 clamp `[0,1]`；price 非有限 → 0.0 |

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use eneros_energy_market_agent::{DeviceCapability, DevicePool, DispatchPlan, MarketData};
use eneros_solver_core::{
    problem::{ConstraintMatrix, LpProblem, ObjectiveSense, VarType},
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
    /// 各 Edge Box 下发计划（box_id → [`eneros_energy_market_agent::DispatchPlan`]，BTreeMap 确定性顺序）.
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
}

/// 域级优化大方法（ARCH-G03 拆分，含 `optimize` 实现与其 doc）。
mod optimize;

#[cfg(test)]
mod tests;

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

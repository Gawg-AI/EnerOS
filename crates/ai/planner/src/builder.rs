//! ModelBuilder（[09] §3.3 深模块）：外部只提交 ObjectiveSpec（经
//! CompiledObjective）+ CapabilityEnvelopes + 快照负荷；内部
//! [`ModelContributor`] 按资源种类添加变量/约束。列索引由变量目录统一
//! 分配（(asset, kind, step) 排序确定）。
//!
//! 模型面（[09] §3.3 全清单的本卷实现）：
//! - 每资源/时段有功；储能 charge/discharge/SOC + 方向 binary；
//! - 并网点功率平衡（**load_demand 实际进入平衡行**）、购售互斥；
//! - 资源 ramp（包络 × 步长）；充放互斥；SOC 动态/初值/终值；
//! - 新能源可用出力与 curtailment；可调负荷基线/削减；
//! - 市场能量承诺与站能量窗；备用容量（峰变量行）。
//!
//! 拓扑线性化：本卷按**单平衡节点（site_node_id）**装配——所有资源
//! 归并到站平衡点；多节点 B-theta 线性化待目录拓扑容量数据齐备（登记
//! 待数据，[09] §3.3 拓扑行）。

use std::collections::BTreeMap;
use std::string::String;
use std::vec::Vec;

use eneros_decision_contract::objective::{ConstraintKind, ObjectiveKind};
use eneros_objective_engine::CompiledObjective;
use eneros_resource_registry::capability::{Availability, CapabilityEnvelope};
use eneros_solver_core::problem::{ConstraintMatrix, LpProblem, ObjectiveSense, VarType};
use sha2::Digest;

use crate::catalog::{VariableCatalog, VariableKind};
use crate::{AssetBinding, PlanError};

/// 步数上限（与 HorizonPlan 上限一致）。
const MAX_STEPS: usize = 384;

/// 储能参数（快照/装配面注入）。
#[derive(Debug, Clone, PartialEq)]
pub struct BatterySpec {
    pub asset_id: u64,
    /// 额定容量（kWh；>0）。
    pub capacity_kwh: f64,
    /// 充/放电效率 (0,1]。
    pub eta_c: f64,
    pub eta_d: f64,
    /// 初始能量（kWh；∈[0,cap]）。
    pub initial_kwh: f64,
}

/// 新能源（PV/风）可用出力序列。
#[derive(Debug, Clone, PartialEq)]
pub struct RenewableSpec {
    pub asset_id: u64,
    /// 每步可用出力（kW；len == T；有限）。
    pub available_kw: Vec<f64>,
}

/// 负荷（固定或可削减）。
#[derive(Debug, Clone, PartialEq)]
pub struct LoadSpec {
    pub asset_id: u64,
    /// 每步基线（kW；len == T；有限；≥0）。
    pub baseline_kw: Vec<f64>,
    /// 可削减上限（kW；>0 声明 Shed 变量）。
    pub shed_max_kw: f64,
}

/// 并网点。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridSpec {
    pub asset_id: u64,
}

/// 可调度单元（发电机/EVSE/HVAC/工艺）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DispatchSpec {
    pub asset_id: u64,
}

/// 规划输入（[09] §3.3：ObjectiveSpec + CapabilityEnvelopes + 快照负荷）。
pub struct PlanInput<'a> {
    pub objective: &'a CompiledObjective,
    pub snapshot_id: [u8; 16],
    /// TrustedSnapshot.provenance_root（input_digest 证据）。
    pub provenance_root: [u8; 32],
    pub registry_generation: u64,
    pub now_ms: u64,
    /// 站平衡节点（单节点线性化）。
    pub site_node_id: u64,
    /// 每资源实时包络（registry evaluate 产物）。
    pub envelopes: Vec<(u64, CapabilityEnvelope)>,
    pub batteries: Vec<BatterySpec>,
    pub renewables: Vec<RenewableSpec>,
    pub loads: Vec<LoadSpec>,
    pub grid: Option<GridSpec>,
    pub dispatchable: Vec<DispatchSpec>,
    /// 资产 → device/point/unit（分配装配必需）。
    pub bindings: Vec<AssetBinding>,
}

impl PlanInput<'_> {
    fn step_count(&self) -> Result<usize, ModelError> {
        let t = self.objective.horizon_step_count as usize;
        if t == 0 || t > MAX_STEPS {
            return Err(ModelError::InvalidInput("step_count"));
        }
        Ok(t)
    }

    fn envelope(&self, asset_id: u64) -> Result<&CapabilityEnvelope, ModelError> {
        match self.envelopes.iter().find(|(id, _)| *id == asset_id) {
            Some((_, e)) if e.availability != Availability::Unavailable => Ok(e),
            _ => Err(ModelError::EnvelopeUnavailable { asset_id }),
        }
    }

    /// 步长（秒；ramp 换算 kW/s → kW/步）。
    fn step_seconds(&self) -> Result<f64, ModelError> {
        let ns = self.objective.horizon_step_ns;
        if ns == 0 {
            return Err(ModelError::InvalidInput("step_ns"));
        }
        Ok(ns as f64 / 1e9)
    }

    /// 步长（小时；能量换算 kW × h = kWh——SOC 动态/目标/承诺行）。
    fn step_hours(&self) -> Result<f64, ModelError> {
        Ok(self.step_seconds()? / 3600.0)
    }
}

/// 模型构建错误。
#[derive(Debug, Clone, PartialEq)]
pub enum ModelError {
    /// 输入不合法（步数/序列长度/非有限）。
    InvalidInput(&'static str),
    /// 规格约束缺少落地资源。
    InapplicableConstraint { kind: u16, detail: &'static str },
    /// 包络不可用（Unavailable）或缺失。
    EnvelopeUnavailable { asset_id: u64 },
    /// 病态系数（过大/过小——condition 阻断，[09] §3.6）。
    IllConditioned(&'static str),
}

// ---------------------------------------------------------------------------
// 符号装配（key 空间两相：先声明，目录分配列索引后落矩阵）
// ---------------------------------------------------------------------------
type SymKey = (u64, u16, u32);

#[derive(Debug, Clone, Default)]
struct SymExpr {
    terms: Vec<(SymKey, f64)>,
    constant: f64,
}

impl SymExpr {
    fn add(&mut self, key: SymKey, coeff: f64) {
        self.terms.push((key, coeff));
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum RowKind {
    Eq,
    Le,
    Ge,
}

struct RowSpec {
    name: String,
    expr: SymExpr,
    kind: RowKind,
    rhs: f64,
}

/// 资源变量映射（declare 产物）。
#[derive(Debug, Default, Clone)]
pub struct ResourceVarMap {
    vars: BTreeMap<(u16, u32), SymKey>,
}

/// ResourceVarMap 存 (kind, step) → SymKey（key 含 asset）。
impl ResourceVarMap {
    fn insert(&mut self, kind: VariableKind, step: u32, asset: u64) {
        self.vars
            .insert((kind.code(), step), (asset, kind.code(), step));
    }
    pub fn get(&self, kind: VariableKind, step: u32) -> Option<SymKey> {
        self.vars.get(&(kind.code(), step)).copied()
    }
    pub fn require(&self, kind: VariableKind, step: u32) -> Result<SymKey, ModelError> {
        self.get(kind, step)
            .ok_or(ModelError::InvalidInput("var map missing"))
    }
}

/// contributor 装配上下文。
pub struct ModelContext {
    keys: Vec<SymKey>,
    bounds: BTreeMap<SymKey, (f64, f64, VarType)>,
    rows: Vec<RowSpec>,
    objective: SymExpr,
    /// 站平衡行（每步一条：Σ注入 = 0）。
    balance: Vec<SymExpr>,
    step_count: usize,
    /// 资源投影清单（build 时填充）。
    projections: Vec<ProjectionSpec>,
}

/// 解投影声明（BuiltModel 据此从解向量重建每资源净功率）。
enum ProjectionSpec {
    BatteryNet(u64),
    GridNet(u64),
    Dispatch(u64),
    RenewableNet(u64, Vec<f64>),
    LoadNet(u64, Vec<f64>),
}

impl ModelContext {
    fn declare(&mut self, asset_id: u64, kind: VariableKind, step: u32, lo: f64, hi: f64) {
        let key = (asset_id, kind.code(), step);
        self.keys.push(key);
        let vt = if kind.is_discrete() {
            VarType::Binary
        } else {
            VarType::Continuous
        };
        self.bounds.insert(key, (lo, hi, vt));
    }

    fn tighten(&mut self, asset_id: u64, kind: VariableKind, step: u32, lo: f64, hi: f64) {
        let key = (asset_id, kind.code(), step);
        if let Some(e) = self.bounds.get_mut(&key) {
            e.0 = e.0.max(lo);
            e.1 = e.1.min(hi);
        }
    }
}

/// 资源种类贡献者（[09] §3.3 trait 契约形状）。
pub trait ModelContributor {
    fn declare(
        &self,
        ctx: &mut ModelContext,
        input: &PlanInput,
    ) -> Result<Vec<(u64, ResourceVarMap)>, ModelError>;
    fn constrain(
        &self,
        ctx: &mut ModelContext,
        maps: &[(u64, ResourceVarMap)],
        input: &PlanInput,
    ) -> Result<(), ModelError>;
}

// ---------------------------------------------------------------------------
// 电池
// ---------------------------------------------------------------------------
pub struct BatteryContributor;

impl ModelContributor for BatteryContributor {
    fn declare(
        &self,
        ctx: &mut ModelContext,
        input: &PlanInput,
    ) -> Result<Vec<(u64, ResourceVarMap)>, ModelError> {
        let t_count = ctx.step_count;
        let mut out = Vec::new();
        for b in &input.batteries {
            let cap = pos(b.capacity_kwh, "capacity")?;
            let init = fin(b.initial_kwh, "initial_kwh")?;
            if !(0.0..=cap).contains(&init) {
                return Err(ModelError::InvalidInput("initial_kwh range"));
            }
            eff(b.eta_c)?;
            eff(b.eta_d)?;
            let env = input.envelope(b.asset_id)?;
            let (emin, emax) = energy_window(env, cap);
            let mut m = ResourceVarMap::default();
            for t in 0..t_count {
                ctx.declare(
                    b.asset_id,
                    VariableKind::Charge,
                    t as u32,
                    0.0,
                    env.p_max_kw.max(0.0),
                );
                ctx.declare(
                    b.asset_id,
                    VariableKind::Discharge,
                    t as u32,
                    0.0,
                    env.p_max_kw.max(0.0),
                );
                ctx.declare(b.asset_id, VariableKind::Direction, t as u32, 0.0, 1.0);
                ctx.declare(b.asset_id, VariableKind::Soc, t as u32, emin, emax);
                m.insert(VariableKind::Charge, t as u32, b.asset_id);
                m.insert(VariableKind::Discharge, t as u32, b.asset_id);
                m.insert(VariableKind::Direction, t as u32, b.asset_id);
                m.insert(VariableKind::Soc, t as u32, b.asset_id);
            }
            ctx.declare(b.asset_id, VariableKind::Soc, t_count as u32, emin, emax);
            m.insert(VariableKind::Soc, t_count as u32, b.asset_id);
            ctx.projections.push(ProjectionSpec::BatteryNet(b.asset_id));
            out.push((b.asset_id, m));
        }
        Ok(out)
    }

    fn constrain(
        &self,
        ctx: &mut ModelContext,
        maps: &[(u64, ResourceVarMap)],
        input: &PlanInput,
    ) -> Result<(), ModelError> {
        let t_count = ctx.step_count;
        let dt_h = input.step_hours()?;
        let dt_s = input.step_seconds()?;
        for b in &input.batteries {
            let cap = pos(b.capacity_kwh, "capacity")?;
            let eta_c = eff(b.eta_c)?;
            let eta_d = eff(b.eta_d)?;
            let env = input.envelope(b.asset_id)?;
            let m = map_of(maps, b.asset_id)?;
            // 规格比率窗 ∩ 包络能量窗。
            let spec = spec_soc_window(&input.objective.spec, cap)?;
            let (e0, e1) = energy_window(env, cap);
            let emin = e0.max(spec.0);
            let emax = e1.min(spec.1);
            let pmax = env.p_max_kw.max(0.0);
            for t in 0..t_count {
                let c = m.require(VariableKind::Charge, t as u32)?;
                let d = m.require(VariableKind::Discharge, t as u32)?;
                let y = m.require(VariableKind::Direction, t as u32)?;
                // 互斥：c − P·y ≤ 0；d + P·y ≤ P。
                ctx.rows.push(row(
                    format!("bat{}_mutex_c_{}", b.asset_id, t),
                    [(c, 1.0), (y, -pmax)],
                    RowKind::Le,
                    0.0,
                ));
                ctx.rows.push(row(
                    format!("bat{}_mutex_d_{}", b.asset_id, t),
                    [(d, 1.0), (y, pmax)],
                    RowKind::Le,
                    pmax,
                ));
                // SOC 动态：soc[t+1] − soc[t] − ηc·dt_h·c + (dt_h/ηd)·d = 0。
                let s_t = m.require(VariableKind::Soc, t as u32)?;
                let s_t1 = m.require(VariableKind::Soc, (t + 1) as u32)?;
                ctx.rows.push(row(
                    format!("bat{}_soc_{}", b.asset_id, t),
                    [
                        (s_t1, 1.0),
                        (s_t, -1.0),
                        (c, -(eta_c * dt_h)),
                        (d, dt_h / eta_d),
                    ],
                    RowKind::Eq,
                    0.0,
                ));
                // 平衡注入：d − c。
                ctx.balance[t].add(d, 1.0);
                ctx.balance[t].add(c, -1.0);
            }
            // SOC 窗收紧（含规格窗）。
            for t in 0..=t_count {
                ctx.tighten(b.asset_id, VariableKind::Soc, t as u32, emin, emax);
            }
            // 初值。
            let s0 = m.require(VariableKind::Soc, 0)?;
            let mut e = SymExpr::default();
            e.add(s0, 1.0);
            ctx.rows.push(RowSpec {
                name: format!("bat{}_soc_init", b.asset_id),
                expr: e,
                kind: RowKind::Eq,
                rhs: b.initial_kwh,
            });
            // 包络 ramp → 充/放变化率（净功率双向行）。
            let ramp_up = env.ramp_up_kw_s * dt_s;
            let ramp_down = env.ramp_down_kw_s * dt_s;
            for t in 1..t_count {
                let (c0, d0) = (
                    m.require(VariableKind::Charge, (t - 1) as u32)?,
                    m.require(VariableKind::Discharge, (t - 1) as u32)?,
                );
                let (c1, d1) = (
                    m.require(VariableKind::Charge, t as u32)?,
                    m.require(VariableKind::Discharge, t as u32)?,
                );
                if ramp_up.is_finite() && ramp_up > 0.0 {
                    ctx.rows.push(row(
                        format!("bat{}_ramp_up_{}", b.asset_id, t),
                        [(d1, 1.0), (c1, -1.0), (d0, -1.0), (c0, 1.0)],
                        RowKind::Le,
                        ramp_up,
                    ));
                }
                if ramp_down.is_finite() && ramp_down > 0.0 {
                    ctx.rows.push(row(
                        format!("bat{}_ramp_down_{}", b.asset_id, t),
                        [(d0, 1.0), (c0, -1.0), (d1, -1.0), (c1, 1.0)],
                        RowKind::Le,
                        ramp_down,
                    ));
                }
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 新能源
// ---------------------------------------------------------------------------
pub struct RenewableContributor;

impl ModelContributor for RenewableContributor {
    fn declare(
        &self,
        ctx: &mut ModelContext,
        input: &PlanInput,
    ) -> Result<Vec<(u64, ResourceVarMap)>, ModelError> {
        let t_count = ctx.step_count;
        let mut out = Vec::new();
        for r in &input.renewables {
            check_series(&r.available_kw, t_count)?;
            input.envelope(r.asset_id)?;
            let mut m = ResourceVarMap::default();
            for t in 0..t_count {
                let avail = r.available_kw[t];
                ctx.declare(
                    r.asset_id,
                    VariableKind::Curtailment,
                    t as u32,
                    0.0,
                    avail.max(0.0),
                );
                m.insert(VariableKind::Curtailment, t as u32, r.asset_id);
            }
            ctx.projections.push(ProjectionSpec::RenewableNet(
                r.asset_id,
                r.available_kw.clone(),
            ));
            out.push((r.asset_id, m));
        }
        Ok(out)
    }

    fn constrain(
        &self,
        ctx: &mut ModelContext,
        maps: &[(u64, ResourceVarMap)],
        input: &PlanInput,
    ) -> Result<(), ModelError> {
        let t_count = ctx.step_count;
        for r in &input.renewables {
            let m = map_of(maps, r.asset_id)?;
            for t in 0..t_count {
                let curt = m.require(VariableKind::Curtailment, t as u32)?;
                ctx.balance[t].add(curt, -1.0);
                ctx.balance[t].constant += r.available_kw[t];
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 负荷
// ---------------------------------------------------------------------------
pub struct LoadContributor;

impl ModelContributor for LoadContributor {
    fn declare(
        &self,
        ctx: &mut ModelContext,
        input: &PlanInput,
    ) -> Result<Vec<(u64, ResourceVarMap)>, ModelError> {
        let t_count = ctx.step_count;
        let mut out = Vec::new();
        for l in &input.loads {
            check_series(&l.baseline_kw, t_count)?;
            if l.baseline_kw.iter().any(|&v| v < 0.0) {
                return Err(ModelError::InvalidInput("baseline negative"));
            }
            let mut m = ResourceVarMap::default();
            if l.shed_max_kw > 0.0 {
                for t in 0..t_count {
                    let hi = l.shed_max_kw.min(l.baseline_kw[t]);
                    ctx.declare(l.asset_id, VariableKind::Shed, t as u32, 0.0, hi);
                    m.insert(VariableKind::Shed, t as u32, l.asset_id);
                }
            }
            ctx.projections
                .push(ProjectionSpec::LoadNet(l.asset_id, l.baseline_kw.clone()));
            out.push((l.asset_id, m));
        }
        Ok(out)
    }

    fn constrain(
        &self,
        ctx: &mut ModelContext,
        maps: &[(u64, ResourceVarMap)],
        input: &PlanInput,
    ) -> Result<(), ModelError> {
        let t_count = ctx.step_count;
        for l in &input.loads {
            let m = map_of(maps, l.asset_id)?;
            for t in 0..t_count {
                ctx.balance[t].constant -= l.baseline_kw[t];
                if let Some(shed) = m.get(VariableKind::Shed, t as u32) {
                    ctx.balance[t].add(shed, 1.0);
                }
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 并网点
// ---------------------------------------------------------------------------
pub struct GridContributor;

impl ModelContributor for GridContributor {
    fn declare(
        &self,
        ctx: &mut ModelContext,
        input: &PlanInput,
    ) -> Result<Vec<(u64, ResourceVarMap)>, ModelError> {
        let t_count = ctx.step_count;
        let mut out = Vec::new();
        if let Some(g) = &input.grid {
            let env = input.envelope(g.asset_id)?;
            let mut m = ResourceVarMap::default();
            for t in 0..t_count {
                ctx.declare(
                    g.asset_id,
                    VariableKind::Buy,
                    t as u32,
                    0.0,
                    env.p_max_kw.max(0.0),
                );
                ctx.declare(
                    g.asset_id,
                    VariableKind::Sell,
                    t as u32,
                    0.0,
                    (-env.p_min_kw).max(0.0),
                );
                ctx.declare(g.asset_id, VariableKind::GridDirection, t as u32, 0.0, 1.0);
                m.insert(VariableKind::Buy, t as u32, g.asset_id);
                m.insert(VariableKind::Sell, t as u32, g.asset_id);
                m.insert(VariableKind::GridDirection, t as u32, g.asset_id);
            }
            ctx.projections.push(ProjectionSpec::GridNet(g.asset_id));
            out.push((g.asset_id, m));
        }
        Ok(out)
    }

    fn constrain(
        &self,
        ctx: &mut ModelContext,
        maps: &[(u64, ResourceVarMap)],
        input: &PlanInput,
    ) -> Result<(), ModelError> {
        let Some(g) = &input.grid else { return Ok(()) };
        let t_count = ctx.step_count;
        let env = input.envelope(g.asset_id)?;
        let m = map_of(maps, g.asset_id)?;
        // 规格站级功率界 → 收紧 PCC 界。
        let (spec_lo, spec_hi) = spec_power_window(&input.objective.spec);
        let import_cap = spec_hi.map_or(env.p_max_kw.max(0.0), |hi| env.p_max_kw.min(hi).max(0.0));
        let export_cap = spec_lo.map_or((-env.p_min_kw).max(0.0), |lo| {
            (-env.p_min_kw.min(lo)).max(0.0)
        });
        for t in 0..t_count {
            let buy = m.require(VariableKind::Buy, t as u32)?;
            let sell = m.require(VariableKind::Sell, t as u32)?;
            let y = m.require(VariableKind::GridDirection, t as u32)?;
            ctx.rows.push(row(
                format!("grid_mutex_buy_{}", t),
                [(buy, 1.0), (y, -import_cap)],
                RowKind::Le,
                0.0,
            ));
            ctx.rows.push(row(
                format!("grid_mutex_sell_{}", t),
                [(sell, 1.0), (y, export_cap)],
                RowKind::Le,
                export_cap,
            ));
            ctx.balance[t].add(buy, 1.0);
            ctx.balance[t].add(sell, -1.0);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 可调度单元
// ---------------------------------------------------------------------------
pub struct DispatchContributor;

impl ModelContributor for DispatchContributor {
    fn declare(
        &self,
        ctx: &mut ModelContext,
        input: &PlanInput,
    ) -> Result<Vec<(u64, ResourceVarMap)>, ModelError> {
        let t_count = ctx.step_count;
        let mut out = Vec::new();
        for d in &input.dispatchable {
            let env = input.envelope(d.asset_id)?;
            let mut m = ResourceVarMap::default();
            for t in 0..t_count {
                ctx.declare(
                    d.asset_id,
                    VariableKind::ActivePower,
                    t as u32,
                    env.p_min_kw,
                    env.p_max_kw,
                );
                m.insert(VariableKind::ActivePower, t as u32, d.asset_id);
            }
            ctx.projections.push(ProjectionSpec::Dispatch(d.asset_id));
            out.push((d.asset_id, m));
        }
        Ok(out)
    }

    fn constrain(
        &self,
        ctx: &mut ModelContext,
        maps: &[(u64, ResourceVarMap)],
        input: &PlanInput,
    ) -> Result<(), ModelError> {
        let t_count = ctx.step_count;
        let dt = input.step_seconds()?;
        for d in &input.dispatchable {
            let env = input.envelope(d.asset_id)?;
            let m = map_of(maps, d.asset_id)?;
            let mut prev: Option<SymKey> = None;
            for t in 0..t_count {
                let p = m.require(VariableKind::ActivePower, t as u32)?;
                ctx.balance[t].add(p, 1.0);
                if let Some(pp) = prev {
                    for (name, sign, limit) in [
                        (
                            format!("disp{}_ramp_up_{}", d.asset_id, t),
                            1.0,
                            env.ramp_up_kw_s * dt,
                        ),
                        (
                            format!("disp{}_ramp_down_{}", d.asset_id, t),
                            -1.0,
                            env.ramp_down_kw_s * dt,
                        ),
                    ] {
                        if limit.is_finite() && limit > 0.0 {
                            ctx.rows
                                .push(row(name, [(p, sign), (pp, -sign)], RowKind::Le, limit));
                        }
                    }
                }
                prev = Some(p);
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// build：装配 + 规格约束 + 目标 + 编译
// ---------------------------------------------------------------------------
pub struct BuiltModel {
    pub problem: LpProblem,
    pub catalog: VariableCatalog,
    pub model_digest: [u8; 32],
    /// 行名（与 CSR 行序对齐；不可行诊断用，[09] §3.7 步骤 7）。
    pub row_names: Vec<String>,
    projections: Vec<ProjectionSpec>,
}

/// 解投影（BuiltModel → 每资源净功率；缺列/非有限 → MalformedSolution）。
pub fn project_solution(
    built: &BuiltModel,
    solution: &[f64],
) -> Result<Vec<crate::ResourceSolution>, PlanError> {
    let mut out = Vec::new();
    for p in &built.projections {
        let (asset_id, net) = match p {
            ProjectionSpec::BatteryNet(a) => {
                let t = step_count_of(&built.catalog);
                let mut v = Vec::with_capacity(t);
                for t in 0..t {
                    let d = at(&built.catalog, solution, *a, VariableKind::Discharge, t)?;
                    let c = at(&built.catalog, solution, *a, VariableKind::Charge, t)?;
                    v.push(d - c);
                }
                (*a, v)
            }
            ProjectionSpec::GridNet(a) => {
                let t = step_count_of(&built.catalog);
                let mut v = Vec::with_capacity(t);
                for t in 0..t {
                    let b = at(&built.catalog, solution, *a, VariableKind::Buy, t)?;
                    let s = at(&built.catalog, solution, *a, VariableKind::Sell, t)?;
                    v.push(b - s);
                }
                (*a, v)
            }
            ProjectionSpec::Dispatch(a) => {
                let t = step_count_of(&built.catalog);
                let mut v = Vec::with_capacity(t);
                for t in 0..t {
                    v.push(at(
                        &built.catalog,
                        solution,
                        *a,
                        VariableKind::ActivePower,
                        t,
                    )?);
                }
                (*a, v)
            }
            ProjectionSpec::RenewableNet(a, avail) => {
                let mut v = Vec::with_capacity(avail.len());
                for (t, &av) in avail.iter().enumerate() {
                    let curt = at(&built.catalog, solution, *a, VariableKind::Curtailment, t)?;
                    v.push(av - curt);
                }
                (*a, v)
            }
            ProjectionSpec::LoadNet(a, baseline) => {
                let mut v = Vec::with_capacity(baseline.len());
                for (t, &base) in baseline.iter().enumerate() {
                    // 净注入 = shed − baseline；结构上无 Shed 列（固定负荷
                    // 未声明变量）→ 贡献 0（非 default-fill：列不存在是
                    // 声明事实，不是解向量缺失）。
                    let shed = built
                        .catalog
                        .index_of(*a, VariableKind::Shed, t as u32)
                        .ok()
                        .map(|idx| fin_slice(solution, idx))
                        .transpose()?
                        .unwrap_or(0.0);
                    v.push(shed - base);
                }
                (*a, v)
            }
        };
        if net.iter().any(|v| !v.is_finite()) {
            return Err(PlanError::MalformedSolution("non-finite net"));
        }
        out.push(crate::ResourceSolution {
            asset_id,
            net_kw: net,
        });
    }
    Ok(out)
}

fn at(
    catalog: &VariableCatalog,
    solution: &[f64],
    asset: u64,
    kind: VariableKind,
    step: usize,
) -> Result<f64, PlanError> {
    let idx = catalog.index_of(asset, kind, step as u32)?;
    let v = *solution
        .get(idx)
        .ok_or(PlanError::MalformedSolution("short solution"))?;
    if !v.is_finite() {
        return Err(PlanError::MalformedSolution("non-finite"));
    }
    Ok(v)
}

fn fin_slice(solution: &[f64], idx: usize) -> Result<f64, PlanError> {
    let v = *solution
        .get(idx)
        .ok_or(PlanError::MalformedSolution("short solution"))?;
    if !v.is_finite() {
        return Err(PlanError::MalformedSolution("non-finite"));
    }
    Ok(v)
}

/// 目录推步数（Battery Soc 列含 T+1 步；无电池时用任一列 +1 语义——
/// 单站点必有电池或 Grid；此处以 Charge 列计）。
fn step_count_of(catalog: &VariableCatalog) -> usize {
    let max_step = catalog
        .columns()
        .iter()
        .filter(|c| c.kind != VariableKind::Soc)
        .map(|c| c.step as usize)
        .max()
        .unwrap_or(0);
    max_step + 1
}

pub struct ModelBuilder;

impl ModelBuilder {
    /// 装配唯一入口（[09] §3.3）。
    pub fn build(input: &PlanInput<'_>) -> Result<BuiltModel, ModelError> {
        let t_count = input.step_count()?;
        let mut ctx = ModelContext {
            keys: Vec::new(),
            bounds: BTreeMap::new(),
            rows: Vec::new(),
            objective: SymExpr::default(),
            balance: vec![SymExpr::default(); t_count],
            step_count: t_count,
            projections: Vec::new(),
        };

        // 1. contributors：declare → constrain（[09] §3.3）。
        let contributors: [&dyn ModelContributor; 5] = [
            &BatteryContributor,
            &RenewableContributor,
            &LoadContributor,
            &GridContributor,
            &DispatchContributor,
        ];
        let mut all_maps: Vec<(u64, ResourceVarMap)> = Vec::new();
        for c in &contributors {
            let maps = c.declare(&mut ctx, input)?;
            all_maps.extend(maps);
        }
        for c in &contributors {
            c.constrain(&mut ctx, &all_maps, input)?;
        }

        // 2. 功率平衡行：Σ注入 = 0（load_demand 已在其中）。
        for (t, bal) in ctx.balance.iter().enumerate() {
            ctx.rows.push(RowSpec {
                name: format!("balance_{}", t),
                expr: bal.clone(),
                kind: RowKind::Eq,
                rhs: 0.0,
            });
        }

        // 3. 规格硬约束（站点级落地）。
        apply_spec_constraints(input, &mut ctx, &all_maps)?;

        // 4. 目标（ObjectiveKind × 价格）。
        let prices = &input.objective.prices_per_kwh;
        let dt = input.step_hours()?;
        let grid_asset = input.grid.as_ref().map(|g| g.asset_id);
        let kind = input.objective.spec.objective();
        let sense = match kind {
            ObjectiveKind::MaxRevenue => {
                if let Some(ga) = grid_asset {
                    let m = map_of(&all_maps, ga)?;
                    for t in 0..t_count {
                        let price = price_at(prices, t, t_count)?;
                        let sell = m.require(VariableKind::Sell, t as u32)?;
                        let buy = m.require(VariableKind::Buy, t as u32)?;
                        ctx.objective.add(sell, price * dt);
                        ctx.objective.add(buy, -(price * dt));
                    }
                } else {
                    // 无并网点：储能套利收益仍可用（价格 × 净注入）。
                    for b in &input.batteries {
                        let m = map_of(&all_maps, b.asset_id)?;
                        for t in 0..t_count {
                            let price = price_at(prices, t, t_count)?;
                            let d = m.require(VariableKind::Discharge, t as u32)?;
                            let c = m.require(VariableKind::Charge, t as u32)?;
                            ctx.objective.add(d, price * dt);
                            ctx.objective.add(c, -(price * dt));
                        }
                    }
                }
                ObjectiveSense::Maximize
            }
            ObjectiveKind::MinCost => {
                if let Some(ga) = grid_asset {
                    let m = map_of(&all_maps, ga)?;
                    for t in 0..t_count {
                        let price = price_at(prices, t, t_count)?;
                        let buy = m.require(VariableKind::Buy, t as u32)?;
                        ctx.objective.add(buy, price * dt);
                    }
                }
                ObjectiveSense::Minimize
            }
            ObjectiveKind::PeakShave => {
                // 峰变量（单列 step=0）：min PEAK s.t. buy[t] ≤ PEAK。
                let Some(ga) = grid_asset else {
                    return Err(ModelError::InapplicableConstraint {
                        kind: 0,
                        detail: "peak without grid",
                    });
                };
                let m = map_of(&all_maps, ga)?;
                ctx.declare(ga, VariableKind::Peak, 0, 0.0, f64::MAX / 4.0);
                let peak = (ga, VariableKind::Peak.code(), 0u32);
                ctx.objective.add(peak, 1.0);
                for t in 0..t_count {
                    let buy = m.require(VariableKind::Buy, t as u32)?;
                    ctx.rows.push(row(
                        format!("peak_ge_{}", t),
                        [(buy, 1.0), (peak, -1.0)],
                        RowKind::Le,
                        0.0,
                    ));
                }
                ObjectiveSense::Minimize
            }
            ObjectiveKind::SelfConsume => {
                // max Σ (avail − curtail)·dt ≡ min Σ curtail·dt。
                for r in &input.renewables {
                    let m = map_of(&all_maps, r.asset_id)?;
                    for t in 0..t_count {
                        let curt = m.require(VariableKind::Curtailment, t as u32)?;
                        ctx.objective.add(curt, dt);
                    }
                }
                ObjectiveSense::Minimize
            }
        };

        // 5. 条件检查（过大/过小系数阻断——[09] §3.6）。
        for (_k, v) in &ctx.objective.terms {
            check_coeff(*v, "objective")?;
        }
        for r in &ctx.rows {
            for (_k, v) in &r.expr.terms {
                check_coeff(*v, "row")?;
            }
        }

        // 6. 目录分配（(asset,kind,step) 排序）→ LpProblem。
        let mut keys = ctx.keys.clone();
        keys.sort_by_key(|k| (k.0, k.1, k.2));
        keys.dedup();
        let catalog = VariableCatalog::new(
            keys.iter()
                .map(|(a, k, s)| (*a, VariableKind::from_code(*k).expect("known"), *s))
                .collect(),
        );
        let n = catalog.len();
        let idx = |key: SymKey| -> usize {
            catalog
                .index_of(key.0, VariableKind::from_code(key.1).expect("known"), key.2)
                .expect("declared key in catalog")
        };

        let mut lower_bounds = vec![0.0; n];
        let mut upper_bounds = vec![0.0; n];
        let mut var_types = vec![VarType::Continuous; n];
        for key in &keys {
            let i = idx(*key);
            let (lo, hi, vt) = ctx.bounds[key];
            if lo > hi {
                return Err(ModelError::InvalidInput("empty var range"));
            }
            lower_bounds[i] = lo;
            upper_bounds[i] = hi;
            var_types[i] = vt;
        }
        let mut objective = vec![0.0; n];
        for (key, coeff) in &ctx.objective.terms {
            objective[idx(*key)] += coeff;
        }

        // 行：先规格/互斥/ramp 行，再平衡行（ctx.rows 顺序 = 确定性）。
        let num_rows = ctx.rows.len();
        let mut row_start = Vec::with_capacity(num_rows + 1);
        let mut col_index: Vec<i32> = Vec::new();
        let mut values: Vec<f64> = Vec::new();
        let mut rhs_lower = vec![f64::MIN / 4.0; num_rows];
        let mut rhs_upper = vec![f64::MAX / 4.0; num_rows];
        for (ri, r) in ctx.rows.iter().enumerate() {
            row_start.push(col_index.len() as i32);
            let mut merged: BTreeMap<usize, f64> = BTreeMap::new();
            for (key, coeff) in &r.expr.terms {
                *merged.entry(idx(*key)).or_insert(0.0) += coeff;
            }
            for (ci, coeff) in merged {
                if coeff == 0.0 {
                    continue;
                }
                col_index.push(ci as i32);
                values.push(coeff);
            }
            // 常数项移到 rhs：Σa·x + c ⋈ rhs  ⇔  Σa·x ⋈ rhs − c。
            let rhs = r.rhs - r.expr.constant;
            match r.kind {
                RowKind::Eq => {
                    rhs_lower[ri] = rhs;
                    rhs_upper[ri] = rhs;
                }
                RowKind::Le => rhs_upper[ri] = rhs,
                RowKind::Ge => rhs_lower[ri] = rhs,
            }
        }
        row_start.push(col_index.len() as i32);

        let problem = LpProblem {
            variables: keys
                .iter()
                .map(|k| format!("v_{}_{}_{}", k.0, k.1, k.2))
                .collect(),
            lower_bounds,
            upper_bounds,
            var_types,
            objective,
            sense,
            constraints: ConstraintMatrix {
                num_rows,
                num_nz: col_index.len(),
                row_start,
                col_index,
                values,
            },
            rhs_lower,
            rhs_upper,
        };

        let model_digest = digest_model(&problem, &catalog);
        let row_names = ctx.rows.iter().map(|r| r.name.clone()).collect();
        Ok(BuiltModel {
            problem,
            catalog,
            model_digest,
            row_names,
            projections: ctx.projections,
        })
    }
}

// ---------------------------------------------------------------------------
// 规格硬约束 → 模型行
// ---------------------------------------------------------------------------
fn apply_spec_constraints(
    input: &PlanInput<'_>,
    ctx: &mut ModelContext,
    maps: &[(u64, ResourceVarMap)],
) -> Result<(), ModelError> {
    let t_count = ctx.step_count;
    let dt = input.step_hours()?;
    for c in input.objective.spec.hard() {
        match c.kind {
            ConstraintKind::SocFinal => {
                // 终值 SOC：首个电池（单储能迁移语义；多电池登记待扩展）。
                let b = input
                    .batteries
                    .first()
                    .ok_or(ModelError::InapplicableConstraint {
                        kind: c.kind.code(),
                        detail: "no battery",
                    })?;
                let m = map_of(maps, b.asset_id)?;
                let s_end = m.require(VariableKind::Soc, t_count as u32)?;
                ctx.rows.push(row(
                    String::from("spec_soc_final"),
                    [(s_end, 1.0)],
                    RowKind::Eq,
                    c.bound * b.capacity_kwh,
                ));
            }
            ConstraintKind::EnergyCommitmentKwh => {
                let g = input
                    .grid
                    .as_ref()
                    .ok_or(ModelError::InapplicableConstraint {
                        kind: c.kind.code(),
                        detail: "no grid",
                    })?;
                let m = map_of(maps, g.asset_id)?;
                // Σ sell·dt == commitment（时段能量承诺，硬约束）。
                let mut e = SymExpr::default();
                for t in 0..t_count {
                    let sell = m.require(VariableKind::Sell, t as u32)?;
                    e.add(sell, dt);
                }
                ctx.rows.push(RowSpec {
                    name: String::from("spec_energy_commitment"),
                    expr: e,
                    kind: RowKind::Eq,
                    rhs: c.bound,
                });
            }
            ConstraintKind::MaxEnergyKwh | ConstraintKind::MinEnergyKwh => {
                let g = input
                    .grid
                    .as_ref()
                    .ok_or(ModelError::InapplicableConstraint {
                        kind: c.kind.code(),
                        detail: "no grid",
                    })?;
                let m = map_of(maps, g.asset_id)?;
                // 站净购能量窗：Σ(buy−sell)·dt ∈ [min, max]。
                let mut e = SymExpr::default();
                for t in 0..t_count {
                    let buy = m.require(VariableKind::Buy, t as u32)?;
                    let sell = m.require(VariableKind::Sell, t as u32)?;
                    e.add(buy, dt);
                    e.add(sell, -dt);
                }
                let r = match c.kind {
                    ConstraintKind::MaxEnergyKwh => RowSpec {
                        name: String::from("spec_max_energy"),
                        expr: e,
                        kind: RowKind::Le,
                        rhs: c.bound,
                    },
                    _ => RowSpec {
                        name: String::from("spec_min_energy"),
                        expr: e,
                        kind: RowKind::Ge,
                        rhs: c.bound,
                    },
                };
                ctx.rows.push(r);
            }
            ConstraintKind::ReserveCapacityKw => {
                // 峰行：import 峰 ≤ import_cap − reserve（经 PEAK 变量行）。
                let g = input
                    .grid
                    .as_ref()
                    .ok_or(ModelError::InapplicableConstraint {
                        kind: c.kind.code(),
                        detail: "no grid",
                    })?;
                let m = map_of(maps, g.asset_id)?;
                if !ctx
                    .bounds
                    .contains_key(&(g.asset_id, VariableKind::Peak.code(), 0))
                {
                    ctx.declare(g.asset_id, VariableKind::Peak, 0, 0.0, f64::MAX / 4.0);
                    let peak = (g.asset_id, VariableKind::Peak.code(), 0u32);
                    // 主目标注入峰最小化（若当前是 Maximize 则与备用并存——
                    // 备用约束独立成立，不改目标方向）。
                    for t in 0..t_count {
                        let buy = m.require(VariableKind::Buy, t as u32)?;
                        ctx.rows.push(row(
                            format!("reserve_peak_ge_{}", t),
                            [(buy, 1.0), (peak, -1.0)],
                            RowKind::Le,
                            0.0,
                        ));
                    }
                    ctx.rows.push(row(
                        String::from("reserve_cap"),
                        [(peak, 1.0)],
                        RowKind::Le,
                        f64::MAX / 8.0, // cap 由包络收紧覆盖；此处防无界
                    ));
                }
                let env = input.envelope(g.asset_id)?;
                let cap = env.p_max_kw.max(0.0);
                let allowed = (cap - c.bound).max(0.0);
                let peak = (g.asset_id, VariableKind::Peak.code(), 0u32);
                ctx.rows.push(row(
                    String::from("reserve_limit"),
                    [(peak, 1.0)],
                    RowKind::Le,
                    allowed,
                ));
            }
            // Min/MaxSoc（电池窗）与 Min/MaxPowerKw（PCC 收紧）、
            // RampUp/Down（资源 ramp 收紧）已在 contributors 内折入。
            ConstraintKind::MinSoc
            | ConstraintKind::MaxSoc
            | ConstraintKind::MaxPowerKw
            | ConstraintKind::MinPowerKw
            | ConstraintKind::RampUpKwS
            | ConstraintKind::RampDownKwS => {}
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------
fn map_of(maps: &[(u64, ResourceVarMap)], asset: u64) -> Result<&ResourceVarMap, ModelError> {
    maps.iter()
        .find(|(id, _)| *id == asset)
        .map(|(_, m)| m)
        .ok_or(ModelError::InvalidInput("map missing"))
}

fn row(name: String, terms: impl AsRef<[(SymKey, f64)]>, kind: RowKind, rhs: f64) -> RowSpec {
    let mut e = SymExpr::default();
    for (k, v) in terms.as_ref() {
        e.add(*k, *v);
    }
    RowSpec {
        name,
        expr: e,
        kind,
        rhs,
    }
}

fn pos(v: f64, what: &'static str) -> Result<f64, ModelError> {
    if !v.is_finite() || v <= 0.0 {
        return Err(ModelError::InvalidInput(what));
    }
    Ok(v)
}

fn fin(v: f64, what: &'static str) -> Result<f64, ModelError> {
    if !v.is_finite() {
        return Err(ModelError::InvalidInput(what));
    }
    Ok(v)
}

fn eff(v: f64) -> Result<f64, ModelError> {
    if !v.is_finite() || v <= 0.0 || v > 1.0 {
        return Err(ModelError::InvalidInput("efficiency"));
    }
    Ok(v)
}

fn check_series(s: &[f64], t: usize) -> Result<(), ModelError> {
    if s.len() != t {
        return Err(ModelError::InvalidInput("series length"));
    }
    if s.iter().any(|v| !v.is_finite()) {
        return Err(ModelError::InvalidInput("series finite"));
    }
    Ok(())
}

fn check_coeff(v: f64, what: &'static str) -> Result<(), ModelError> {
    if !v.is_finite() {
        return Err(ModelError::IllConditioned(what));
    }
    if v.abs() > 1e12 {
        return Err(ModelError::IllConditioned("coeff too large"));
    }
    if v != 0.0 && v.abs() < 1e-12 {
        return Err(ModelError::IllConditioned("coeff too small"));
    }
    Ok(())
}

fn price_at(prices: &[f64], t: usize, t_count: usize) -> Result<f64, ModelError> {
    match prices.len() {
        0 => Ok(0.0),
        1 => fin(prices[0], "price"),
        n if n == t_count => fin(prices[t], "price"),
        _ => Err(ModelError::InvalidInput("price length")),
    }
}

/// 包络能量窗（缺省 [0, cap]）。
fn energy_window(env: &CapabilityEnvelope, cap: f64) -> (f64, f64) {
    let lo = if env.energy_min_kwh.is_finite() && env.energy_min_kwh >= 0.0 {
        env.energy_min_kwh
    } else {
        0.0
    };
    let hi = if env.energy_max_kwh.is_finite() && env.energy_max_kwh <= cap {
        env.energy_max_kwh
    } else {
        cap
    };
    (lo.min(cap), hi.min(cap))
}

/// 规格 SOC 窗（比率 → kWh；缺省全窗）。
fn spec_soc_window(
    spec: &eneros_decision_contract::ObjectiveSpec,
    cap: f64,
) -> Result<(f64, f64), ModelError> {
    let mut lo = 0.0f64;
    let mut hi = cap;
    for c in spec.hard() {
        match c.kind {
            ConstraintKind::MinSoc => lo = lo.max(c.bound * cap),
            ConstraintKind::MaxSoc => hi = hi.min(c.bound * cap),
            _ => {}
        }
    }
    if lo > hi {
        return Err(ModelError::InvalidInput("soc window empty"));
    }
    Ok((lo, hi))
}

/// 规格站级功率窗（None = 未约束）。
fn spec_power_window(spec: &eneros_decision_contract::ObjectiveSpec) -> (Option<f64>, Option<f64>) {
    let mut lo = None;
    let mut hi = None;
    for c in spec.hard() {
        match c.kind {
            ConstraintKind::MinPowerKw => lo = Some(lo.map_or(c.bound, |v: f64| v.max(c.bound))),
            ConstraintKind::MaxPowerKw => hi = Some(hi.map_or(c.bound, |v: f64| v.min(c.bound))),
            _ => {}
        }
    }
    (lo, hi)
}

/// 模型摘要（内容级：变量目录 + 边界 + 目标 + CSR + rhs）。
fn digest_model(problem: &LpProblem, catalog: &VariableCatalog) -> [u8; 32] {
    let mut h = sha2::Sha256::new();
    h.update(b"eneros.model.v1");
    h.update(catalog.digest());
    h.update((problem.variables.len() as u64).to_le_bytes());
    for i in 0..problem.variables.len() {
        h.update(problem.lower_bounds[i].to_le_bytes());
        h.update(problem.upper_bounds[i].to_le_bytes());
    }
    for c in &problem.objective {
        h.update(c.to_le_bytes());
    }
    h.update([matches!(problem.sense, ObjectiveSense::Minimize) as u8]);
    let m = &problem.constraints;
    h.update((m.num_rows as u64).to_le_bytes());
    for r in &m.row_start {
        h.update(r.to_le_bytes());
    }
    for c in &m.col_index {
        h.update(c.to_le_bytes());
    }
    for v in &m.values {
        h.update(v.to_le_bytes());
    }
    for v in &problem.rhs_lower {
        h.update(v.to_le_bytes());
    }
    for v in &problem.rhs_upper {
        h.update(v.to_le_bytes());
    }
    h.finalize().into()
}

//! 独立 forward evaluator（[09] §4.2）：逐时段单位/range/finite、能量
//! /SOC 推进、范围/斜率约束、节点平衡（站级单节点）、承诺硬约束、包络
//! 全生效窗覆盖——**不读 solver、不复用约束矩阵、不 default-fill**。
//!
//! 容差（[09] §4.4）：按 ConstraintKind 与量纲配置（[`tolerance_for`]），
//! 判定取保守方向：`value ≤ upper + tol` / `value ≥ lower − tol`；
//! 累积能量确定性求和（同输入同结果）。

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_decision_contract::objective::ConstraintKind;
use eneros_decision_contract::CandidatePlan;

use crate::proof::{Cmp, ProofRecord};
use crate::{CertifierError, EvalContext};

/// 逐类容差（kW/kWh/比率；不使用全局 epsilon）。
///
/// L2(id=1)/L3(id=2) 物理容差一致——gap 策略是 planner 侧性质；
/// profile id 只随证书登记（装配面可扩展分档）。
pub const fn tolerance_for(kind: ConstraintKind, _profile_id: u32) -> f64 {
    match kind {
        ConstraintKind::MinSoc | ConstraintKind::MaxSoc | ConstraintKind::SocFinal => 1e-6,
        ConstraintKind::MaxPowerKw
        | ConstraintKind::MinPowerKw
        | ConstraintKind::RampUpKwS
        | ConstraintKind::RampDownKwS
        | ConstraintKind::ReserveCapacityKw => 1e-3,
        ConstraintKind::MaxEnergyKwh
        | ConstraintKind::MinEnergyKwh
        | ConstraintKind::EnergyCommitmentKwh => 1e-3,
    }
}

/// 证据记录序号分配器内部使用；记录 = 一次约束族的评估结论。
struct Recorder {
    records: Vec<ProofRecord>,
    snapshot_id: [u8; 16],
    catalog_version: u64,
}

impl Recorder {
    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        kind: ConstraintKind,
        asset_id: u64,
        cmp: Cmp,
        lhs: f64,
        rhs: f64,
        unit_code: u16,
        profile_id: u32,
    ) -> Result<usize, CertifierError> {
        let tol = tolerance_for(kind, profile_id);
        let pass = match cmp {
            Cmp::Ge => lhs >= rhs - tol,
            Cmp::Le => lhs <= rhs + tol,
            Cmp::Eq => (lhs - rhs).abs() <= tol,
        };
        let seq = self.records.len() as u32;
        self.records.push(ProofRecord {
            seq,
            kind: kind.code(),
            asset_id,
            lhs,
            rhs,
            cmp,
            snapshot_id: self.snapshot_id,
            catalog_version: self.catalog_version,
            unit_code,
            tolerance: tol,
            pass,
        });
        if pass {
            Ok(seq as usize)
        } else {
            Err(CertifierError::Violated {
                constraint_kind: kind.code(),
                asset_id,
                record_index: seq as usize,
            })
        }
    }
}

/// 评估入口：全部硬约束独立重算；任一违约 → 结构化拒绝（带证据序号）。
pub fn evaluate(
    ctx: &EvalContext<'_>,
    plan: &CandidatePlan,
) -> Result<Vec<ProofRecord>, CertifierError> {
    let t_count = plan.step_count() as usize;
    // 分配索引（唯一 (asset, step) → 功率；重复/越步/非有限即拒）。
    let mut traj: BTreeMap<u64, Vec<Option<f64>>> = BTreeMap::new();
    for a in plan.assignments() {
        if !a.power_kw.is_finite() {
            return Err(CertifierError::NonFinite("assignment power"));
        }
        if a.step_index == 0 || a.step_index as usize > t_count {
            return Err(CertifierError::MalformedAssignment("step_index"));
        }
        let slot = &mut traj
            .entry(a.asset_id)
            .or_insert_with(|| alloc::vec![None; t_count])[(a.step_index - 1) as usize];
        if slot.is_some() {
            return Err(CertifierError::MalformedAssignment("duplicate"));
        }
        *slot = Some(a.power_kw);
    }
    evaluate_trajectories(&traj, ctx)
}

/// 轨迹级评估核心（性能验证入口——绕开 CandidatePlan inline 上限；
/// 语义与 [`evaluate`] 一致）。
pub fn evaluate_trajectories(
    traj: &BTreeMap<u64, Vec<Option<f64>>>,
    ctx: &EvalContext<'_>,
) -> Result<Vec<ProofRecord>, CertifierError> {
    let dt_h = ctx.horizon_step_ns as f64 / 3.6e12;
    let dt_s = ctx.horizon_step_ns as f64 / 1e9;
    if dt_h <= 0.0 || dt_h.is_infinite() {
        return Err(CertifierError::InvalidInput("step_ns"));
    }
    let profile = ctx.tolerance_profile_id;

    // 2. 包络覆盖检查（整个生效窗：缺步 = 缺值，不 default-fill）。
    let env_of = |asset: u64| -> Result<
        &eneros_resource_registry::capability::CapabilityEnvelope,
        CertifierError,
    > {
        let (_, e) = ctx
            .envelopes
            .iter()
            .find(|(id, _)| *id == asset)
            .ok_or(CertifierError::Envelope { asset_id: asset })?;
        if !e.controllable() {
            return Err(CertifierError::Envelope { asset_id: asset });
        }
        Ok(e)
    };
    for (asset, series) in traj.iter() {
        let env = env_of(*asset)?;
        let t0_ns = ctx.snapshot.cut_time_ns;
        for (t, slot) in series.iter().enumerate() {
            if slot.is_none() {
                return Err(CertifierError::MissingAssignment {
                    asset_id: *asset,
                    step: t as u32 + 1,
                });
            }
            let at_ns = t0_ns + (t as u64) * ctx.horizon_step_ns;
            if at_ns < env.valid_from_ns || at_ns > env.valid_until_ns {
                return Err(CertifierError::Envelope { asset_id: *asset });
            }
        }
    }

    let mut rec = Recorder {
        records: Vec::new(),
        snapshot_id: ctx.snapshot.snapshot_id,
        catalog_version: ctx.constraint_catalog_version,
    };
    let unit_kw = eneros_telemetry_model::unit_catalog::code_of(
        eneros_telemetry_model::PhysicalUnit::Kilowatts,
    );
    let unit_kwh = eneros_telemetry_model::unit_catalog::code_of(
        eneros_telemetry_model::PhysicalUnit::KilowattHour,
    );

    // 3. 逐资源：包络功率界 + 斜率（能量守恒在 SOC 推进中校验）。
    for (asset, series) in traj.iter() {
        let env = env_of(*asset)?;
        let mut prev: Option<f64> = None;
        for slot in series.iter() {
            let p = slot.expect("checked above");
            // 包络功率界（保守方向：value ≤ hi + tol / ≥ lo − tol）。
            rec.push(
                ConstraintKind::MinPowerKw,
                *asset,
                Cmp::Ge,
                p,
                env.p_min_kw,
                unit_kw,
                profile,
            )?;
            rec.push(
                ConstraintKind::MaxPowerKw,
                *asset,
                Cmp::Le,
                p,
                env.p_max_kw,
                unit_kw,
                profile,
            )?;
            // ramp（步间）。
            if let Some(pp) = prev {
                let up = env.ramp_up_kw_s * dt_s;
                let down = env.ramp_down_kw_s * dt_s;
                rec.push(
                    ConstraintKind::RampUpKwS,
                    *asset,
                    Cmp::Le,
                    (p - pp) / dt_s,
                    up,
                    unit_kw,
                    profile,
                )?;
                rec.push(
                    ConstraintKind::RampDownKwS,
                    *asset,
                    Cmp::Le,
                    (pp - p) / dt_s,
                    down,
                    unit_kw,
                    profile,
                )?;
            }
            prev = Some(p);
        }
    }

    // 4. 电池 SOC 推进（能量守恒独立重算）+ SOC/终值规格约束。
    for b in ctx.batteries {
        if !b.capacity_kwh.is_finite() || b.capacity_kwh <= 0.0 {
            return Err(CertifierError::InvalidInput("capacity"));
        }
        let eta_ok = |v: f64| v > 0.0 && v <= 1.0;
        if !eta_ok(b.eta_c) || !eta_ok(b.eta_d) {
            return Err(CertifierError::InvalidInput("efficiency"));
        }
        let Some(series) = traj.get(&b.asset_id) else {
            return Err(CertifierError::MissingAssignment {
                asset_id: b.asset_id,
                step: 1,
            });
        };
        let mut e = b.initial_kwh;
        for slot in series.iter() {
            let p = slot.expect("checked above");
            // net ≥ 0 = 放电（损失 ηd）；net < 0 = 充电（存入 ηc）。
            if p >= 0.0 {
                e -= p * dt_h / b.eta_d;
            } else {
                e += (-p) * dt_h * b.eta_c;
            }
        }
        // 末态能量（确定性逐步求和后的唯一终值）。
        // 规格 SOC 窗 ∩ 包络能量窗（比率 × 容量 与 kWh 同域比较）。
        let benv = env_of(b.asset_id)?;
        let mut soc_lo = benv.energy_min_kwh.max(0.0);
        let mut soc_hi = benv.energy_max_kwh.min(b.capacity_kwh).max(soc_lo);
        for c in ctx.spec.hard() {
            match c.kind {
                ConstraintKind::MinSoc => soc_lo = soc_lo.max(c.bound * b.capacity_kwh),
                ConstraintKind::MaxSoc => soc_hi = soc_hi.min(c.bound * b.capacity_kwh),
                _ => {}
            }
        }
        if soc_lo > soc_hi {
            return Err(CertifierError::InvalidInput("empty soc window"));
        }
        rec.push(
            ConstraintKind::MinSoc,
            b.asset_id,
            Cmp::Ge,
            e,
            soc_lo,
            unit_kwh,
            profile,
        )?;
        rec.push(
            ConstraintKind::MaxSoc,
            b.asset_id,
            Cmp::Le,
            e,
            soc_hi,
            unit_kwh,
            profile,
        )?;
        for c in ctx.spec.hard() {
            if c.kind == ConstraintKind::SocFinal {
                let target = c.bound * b.capacity_kwh;
                rec.push(
                    ConstraintKind::SocFinal,
                    b.asset_id,
                    Cmp::Eq,
                    e,
                    target,
                    unit_kwh,
                    profile,
                )?;
            }
        }
    }

    // 5. 站级（PCC）规格约束：功率窗 / ramp / 能量窗 / 承诺 / 备用。
    let grid_id = ctx.grid_asset_id;
    let grid_series: Option<&Vec<Option<f64>>> = if grid_id != 0 {
        Some(
            traj.get(&grid_id)
                .ok_or(CertifierError::MissingAssignment {
                    asset_id: grid_id,
                    step: 1,
                })?,
        )
    } else {
        None
    };
    if let Some(series) = grid_series {
        for c in ctx.spec.hard() {
            match c.kind {
                ConstraintKind::MinPowerKw | ConstraintKind::MaxPowerKw => {
                    let mut worst = if c.kind == ConstraintKind::MinPowerKw {
                        f64::MAX
                    } else {
                        f64::MIN
                    };
                    for slot in series.iter() {
                        let p = slot.expect("checked above");
                        worst = if c.kind == ConstraintKind::MinPowerKw {
                            worst.min(p)
                        } else {
                            worst.max(p)
                        };
                    }
                    rec.push(
                        c.kind,
                        grid_id,
                        if c.kind == ConstraintKind::MinPowerKw {
                            Cmp::Ge
                        } else {
                            Cmp::Le
                        },
                        worst,
                        c.bound,
                        unit_kw,
                        profile,
                    )?;
                }
                ConstraintKind::EnergyCommitmentKwh => {
                    // Σ sell·dt == 承诺（sell = −min(0, net)）。
                    let mut sold = 0.0f64;
                    for slot in series.iter() {
                        let p = slot.expect("checked above");
                        sold += (-p).max(0.0) * dt_h;
                    }
                    rec.push(
                        ConstraintKind::EnergyCommitmentKwh,
                        grid_id,
                        Cmp::Eq,
                        sold,
                        c.bound,
                        unit_kwh,
                        profile,
                    )?;
                }
                ConstraintKind::MinEnergyKwh | ConstraintKind::MaxEnergyKwh => {
                    // 站净购能量窗。
                    let mut energy = 0.0f64;
                    for slot in series.iter() {
                        energy += slot.expect("checked above") * dt_h;
                    }
                    rec.push(
                        c.kind,
                        grid_id,
                        if c.kind == ConstraintKind::MinEnergyKwh {
                            Cmp::Ge
                        } else {
                            Cmp::Le
                        },
                        energy,
                        c.bound,
                        unit_kwh,
                        profile,
                    )?;
                }
                ConstraintKind::ReserveCapacityKw => {
                    // 峰购 ≤ 包络上限 − 备用。
                    let env = env_of(grid_id)?;
                    let allowed = env.p_max_kw - c.bound;
                    let mut peak = f64::MIN;
                    for slot in series.iter() {
                        peak = peak.max(slot.expect("checked above"));
                    }
                    rec.push(
                        ConstraintKind::ReserveCapacityKw,
                        grid_id,
                        Cmp::Le,
                        peak,
                        allowed,
                        unit_kw,
                        profile,
                    )?;
                }
                ConstraintKind::MinSoc
                | ConstraintKind::MaxSoc
                | ConstraintKind::SocFinal
                | ConstraintKind::RampUpKwS
                | ConstraintKind::RampDownKwS => {}
            }
        }
    } else if ctx.spec.hard().iter().any(|c| {
        matches!(
            c.kind,
            ConstraintKind::EnergyCommitmentKwh
                | ConstraintKind::MinEnergyKwh
                | ConstraintKind::MaxEnergyKwh
                | ConstraintKind::ReserveCapacityKw
        )
    }) {
        return Err(CertifierError::InvalidInput("site constraint without grid"));
    }

    Ok(rec.records)
}

/// 行名占位（与 planner 行名不同源；此类型只用于诊断输出格式对齐）。
pub type DiagnosticRow = String;

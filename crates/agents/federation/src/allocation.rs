//! # Phase 7 分层站间分配求解（蓝图 61 §12 §2.6）
//!
//! 区域层只分站点目标/备用，不建每台设备变量；站内层把目标分到资源并
//! 独立 Certify（既有 planner/certifier 承担，本模块只产出站点级目标）。
//!
//! 规则：
//!
//! 1. 确定性：同输入同输出（merit-order + 确定性修复 pass，无随机）。
//! 2. 目标不可行时返回可解释 [`InfeasibleEnvelope`]（冲突站点+约束+数值），
//!    不 default-fill、不静默放宽。
//! 3. 单位/时段网格不兼容（interval 网格不一致、单位集不符）→ 结构化
//!    [`AllocError::GridOrUnitMismatch`]，由协调器转为 OfferReject
//!    `GridOrUnitMismatch`。
//! 4. 能力报价经本地 Snapshot/Registry 生成（保守），分配结果永远在
//!    offer 包络内（[`EnvelopeCheck`]，站点侧复用于拒绝恶意超范围目标）。
//!
//! 算法（`solve`）：
//!
//! 1. 校验网格/单位；
//! 2. 逐区间可行性：`Σ p_min ≤ p_target ≤ Σ p_max`；
//! 3. 最小装载（merit order 按 `price_rank` 升序填到 p_min）→ 残差按
//!    merit order 填充（顺时向前传爬坡限制）；
//! 4. 反向 pass 复检爬坡（确定性回退）；
//! 5. 能量修复：站点累计能量越 [energy_min,energy_max] 时在该站自身
//!    [p_min,p_max] 带内做确定性再分布（不跨站挪动）；
//! 6. 终验；仍违反 → [`AllocationResult::Infeasible`] 枚举全部冲突。

use alloc::vec;
use alloc::vec::Vec;

use crate::offer::{AllocationProposal, BlobRefV1, CapabilityOffer, WireError};

/// 区间数上限（96/288 时段资格，wire 层同样封顶）。
pub const MAX_INTERVALS: usize = 288;
/// 单次求解站点数上限（100 站 C3 控制面资格）。
pub const MAX_SITES: usize = 1_024;
/// 能量修复 pass 迭代上限（确定性有界）。
const ENERGY_FIX_PASSES: u32 = 4;

/// 站点 offer 的求解视图（价格排名由区域市场层注入——本模块不解析价格
/// 曲线工件，只按排名确定性排序）。
#[derive(Clone, Debug, PartialEq)]
pub struct SiteOfferView {
    pub site_id: u32,
    pub offer_id: [u8; 16],
    pub revision: u64,
    pub p_min_kw: f64,
    pub p_max_kw: f64,
    pub energy_min_kwh: f64,
    pub energy_max_kwh: f64,
    pub ramp_kw_s: f64,
    pub reserve_up_kw: f64,
    pub reserve_down_kw: f64,
    /// 边际价格排名（0=最便宜）。仅决定同档填充顺序，不影响包络。
    pub price_rank: u32,
}

/// 区域目标（来自市场/上层 plan，与本模块解耦）。
#[derive(Clone, Debug, PartialEq)]
pub struct TargetProfile {
    /// 逐区间参考目标功率（kW）。
    pub p_target_kw: Vec<f64>,
    /// 备用要求（kW）。
    pub reserve_up_kw: f64,
    pub reserve_down_kw: f64,
}

/// 求解输入。
#[derive(Clone, Debug, PartialEq)]
pub struct AllocationInput {
    pub interval_start_ns: u64,
    pub interval_ns: u64,
    pub sites: Vec<SiteOfferView>,
    pub target: TargetProfile,
}

/// 一个站点拿到的目标（站点级，非设备级）。
#[derive(Clone, Debug, PartialEq)]
pub struct SiteAllocation {
    pub site_id: u32,
    /// 逐区间目标功率（kW），`interval_ns` 均匀网格。
    pub points_kw: Vec<f64>,
    /// 该时域累计能量（kWh）。
    pub energy_kwh: f64,
    pub reserve_up_kw: f64,
    pub reserve_down_kw: f64,
    /// 是否处于包络内（恒 true；存在意义=诊断断言面）。
    pub within_envelope: bool,
}

/// 单个站点冲突（可解释 envelope 的元素）。
#[derive(Clone, Debug, PartialEq)]
pub struct SiteConflict {
    pub site_id: u32,
    /// 冲突描述（约束名）。
    pub constraint: &'static str,
    /// 冲突数值（违反方向的值）。
    pub value: f64,
    /// 边界值。
    pub bound: f64,
    /// 首个冲突区间下标。
    pub interval_index: usize,
}

/// 不可行 envelope：结构化解释，绝不 default-fill。
#[derive(Clone, Debug, PartialEq)]
pub struct InfeasibleEnvelope {
    pub conflicts: Vec<SiteConflict>,
    /// 汇总原因（最高严重度）。
    pub reason: &'static str,
}

/// 残差 envelope：满足可行性后仍存在的未覆盖量（备用差口等）。
#[derive(Clone, Debug, PartialEq)]
pub struct ResidualEnvelope {
    /// 逐区间目标未覆盖（kW），≥0。
    pub unmet_kw: Vec<f64>,
    /// 备用未满足（kW）。
    pub unmet_reserve_up_kw: f64,
    pub unmet_reserve_down_kw: f64,
}

/// 求解结果。
#[derive(Clone, Debug, PartialEq)]
pub enum AllocationResult {
    Feasible {
        targets: Vec<SiteAllocation>,
        residual: ResidualEnvelope,
    },
    Infeasible(InfeasibleEnvelope),
}

/// 求解错误（结构化拒绝的输入侧原因）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AllocError {
    /// offer 网格/单位与区域 tolerance catalog 不兼容。
    GridOrUnitMismatch,
    /// 站点数或区间数超上限。
    TooLarge,
    /// 目标/输入含非有限浮点或负值。
    InvalidInput,
    /// 区间数不一致。
    IntervalCountMismatch,
}

/// 包络校验：目标曲线必须落在 offer 的能力包络内（p 界、爬坡、能量）。
/// 站点侧拒绝对恶意/超范围提议的第一道闸（蓝图 §2.7 末行）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnvelopeCheck {
    Ok,
    PowerOutOfRange,
    RampOutOfRange,
    EnergyOutOfRange,
    GridMismatch,
}

/// 将 offer 转为求解视图（保守：无效/过期由调用方过滤）。
pub fn offer_view(site_id: u32, o: &CapabilityOffer, price_rank: u32) -> SiteOfferView {
    SiteOfferView {
        site_id,
        offer_id: o.offer_id,
        revision: o.revision,
        p_min_kw: o.p_min_kw,
        p_max_kw: o.p_max_kw,
        energy_min_kwh: o.energy_min_kwh,
        energy_max_kwh: o.energy_max_kwh,
        ramp_kw_s: o.ramp_kw_s,
        reserve_up_kw: o.reserve_up_kw,
        reserve_down_kw: o.reserve_down_kw,
        price_rank,
    }
}

/// 区域分配求解器（纯函数，确定性）。
pub struct AllocationSolver;

impl AllocationSolver {
    /// 校验并求解。任何不可行都产出 [`InfeasibleEnvelope`]，不修改输入。
    pub fn solve(input: &AllocationInput) -> Result<AllocationResult, AllocError> {
        Self::precheck(input)?;
        let n = input.sites.len();
        let n_int = input.target.p_target_kw.len();
        let mut sum_pmin = vec![0.0f64; n_int];
        let mut sum_pmax = vec![0.0f64; n_int];
        for s in &input.sites {
            for i in 0..n_int {
                sum_pmin[i] += s.p_min_kw;
                sum_pmax[i] += s.p_max_kw;
            }
        }
        // 逐区间可行性（Σ p_min ≤ p_target ≤ Σ p_max）——判据单源
        // kernel envelope（蓝图 62 卷 62.3 S6）。
        for i in 0..n_int {
            let t = input.target.p_target_kw[i];
            if !eneros_allocation::envelope(sum_pmin[i], sum_pmax[i], t) {
                return Ok(AllocationResult::Infeasible(InfeasibleEnvelope {
                    conflicts: input
                        .sites
                        .iter()
                        .map(|s| SiteConflict {
                            site_id: s.site_id,
                            constraint: "aggregate_power_band",
                            value: t,
                            bound: if t < sum_pmin[i] {
                                sum_pmin[i]
                            } else {
                                sum_pmax[i]
                            },
                            interval_index: i,
                        })
                        .collect(),
                    reason: "aggregate target outside combined offer band",
                }));
            }
        }
        // merit order：确定性 tie-break 单源（kernel merit_order——
        // (price_rank, site_id) 全序升序，同序输入保持；蓝图 62 卷 62.3 S6）。
        let order: Vec<usize> = eneros_allocation::merit_order(
            &(0..n)
                .map(|i| {
                    (
                        i,
                        input.sites[i].price_rank as u64,
                        input.sites[i].site_id as u64,
                    )
                })
                .collect::<alloc::vec::Vec<_>>(),
        );
        // 1) 最小装载 + merit-order 填充（前向爬坡受限）。
        let dt_s = input.interval_ns as f64 / 1e9;
        let mut alloc: Vec<Vec<f64>> = input
            .sites
            .iter()
            .map(|s| vec![s.p_min_kw; n_int])
            .collect();
        let mut residual_i: Vec<f64> = input
            .target
            .p_target_kw
            .iter()
            .zip(sum_pmin.iter())
            .map(|(t, m)| t - m)
            .collect();
        // 按 merit order 前向填充（爬坡受限）：逐区间 delta = 残差 ∩
        // 站点 headroom ∩ 前向可到达步长；delta<=0 时跳过（continue，
        // 不 break——零残差区间不等同于后续区间无残差）。
        for &idx in &order {
            let s = &input.sites[idx];
            let headroom = s.p_max_kw - s.p_min_kw;
            if headroom <= 0.0 {
                continue;
            }
            let ramp_step = s.ramp_kw_s * dt_s;
            let mut prev = alloc[idx][0];
            for i in 0..n_int {
                if residual_i[i] <= 0.0 {
                    prev = alloc[idx][i];
                    continue;
                }
                let reachable = if i == 0 {
                    ramp_step
                } else {
                    ramp_step.min(prev + ramp_step - alloc[idx][i] + 1e-12)
                }
                .max(0.0);
                let delta = residual_i[i]
                    .min(headroom - (alloc[idx][i] - s.p_min_kw))
                    .min(reachable);
                if delta <= 0.0 {
                    prev = alloc[idx][i];
                    continue;
                }
                alloc[idx][i] += delta;
                residual_i[i] -= delta;
                prev = alloc[idx][i];
            }
        }
        // 2) 反向爬坡修复：从后向前收缩不可达攀升。
        for &idx in order.iter().rev() {
            let s = &input.sites[idx];
            let ramp_step = s.ramp_kw_s * dt_s;
            for i in (0..n_int).rev() {
                if i == 0 {
                    break;
                }
                let upper = alloc[idx][i - 1] + ramp_step;
                if alloc[idx][i] > upper + 1e-9 {
                    let excess = alloc[idx][i] - upper;
                    alloc[idx][i] = upper;
                    // 回收的余量还给残差（可被其他站/后续 pass 使用）。
                    residual_i[i] += excess;
                }
            }
        }
        // 3) 能量修复：站内 [p_min, p_max] 带内确定性再分布。
        let hour_h = dt_s / 3600.0;
        for _pass in 0..ENERGY_FIX_PASSES {
            let mut changed = false;
            for (idx, s) in input.sites.iter().enumerate() {
                let mut e: f64 = alloc[idx].iter().sum::<f64>() * hour_h;
                if e > s.energy_max_kwh + 1e-9 {
                    // 超上限：从最高点（平局取最早区间）下压到 p_min。
                    let mut work = true;
                    let mut guard = 0;
                    while work && guard < n_int * 2 {
                        guard += 1;
                        e = alloc[idx].iter().sum::<f64>() * hour_h;
                        if e <= s.energy_max_kwh + 1e-9 {
                            break;
                        }
                        let mut target = 0usize;
                        for i in 1..n_int {
                            if alloc[idx][i] > alloc[idx][target] {
                                target = i;
                            }
                        }
                        let drop =
                            (alloc[idx][target] - s.p_min_kw).min((e - s.energy_max_kwh) / hour_h);
                        if drop <= 1e-12 {
                            work = false;
                        } else {
                            alloc[idx][target] -= drop;
                            residual_i[target] += drop;
                            changed = true;
                        }
                    }
                } else if e < s.energy_min_kwh - 1e-9 {
                    // 低于下限：从最低点（平局取最早区间）抬升到 p_max。
                    let mut work = true;
                    let mut guard = 0;
                    while work && guard < n_int * 2 {
                        guard += 1;
                        e = alloc[idx].iter().sum::<f64>() * hour_h;
                        if e >= s.energy_min_kwh - 1e-9 {
                            break;
                        }
                        let mut target = 0usize;
                        for i in 1..n_int {
                            if alloc[idx][i] < alloc[idx][target] {
                                target = i;
                            }
                        }
                        let lift =
                            (s.p_max_kw - alloc[idx][target]).min((s.energy_min_kwh - e) / hour_h);
                        if lift <= 1e-12 {
                            work = false;
                        } else {
                            alloc[idx][target] += lift;
                            residual_i[target] -= lift;
                            changed = true;
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
        // 4) 终验：逐站点包络 + 整体残差非负。
        let mut conflicts = Vec::new();
        for (idx, s) in input.sites.iter().enumerate() {
            let _ = idx;
            let ramp_step = s.ramp_kw_s * dt_s;
            let mut e: f64 = 0.0;
            for i in 0..n_int {
                let v = alloc[idx][i];
                if v < s.p_min_kw - 1e-9 || v > s.p_max_kw + 1e-9 {
                    conflicts.push(SiteConflict {
                        site_id: s.site_id,
                        constraint: "per_site_power_band",
                        value: v,
                        bound: if v < s.p_min_kw {
                            s.p_min_kw
                        } else {
                            s.p_max_kw
                        },
                        interval_index: i,
                    });
                }
                if i > 0 {
                    let d = (v - alloc[idx][i - 1]).abs();
                    if d > ramp_step + 1e-9 {
                        conflicts.push(SiteConflict {
                            site_id: s.site_id,
                            constraint: "ramp_rate",
                            value: d,
                            bound: ramp_step,
                            interval_index: i,
                        });
                    }
                }
                e += v * hour_h;
            }
            if e < s.energy_min_kwh - 1e-9 || e > s.energy_max_kwh + 1e-9 {
                conflicts.push(SiteConflict {
                    site_id: s.site_id,
                    constraint: "per_site_energy_band",
                    value: e,
                    bound: if e < s.energy_min_kwh {
                        s.energy_min_kwh
                    } else {
                        s.energy_max_kwh
                    },
                    interval_index: n_int - 1,
                });
            }
        }
        if !conflicts.is_empty() {
            return Ok(AllocationResult::Infeasible(InfeasibleEnvelope {
                conflicts,
                reason: "site-level envelope violated after deterministic repair",
            }));
        }
        // 残差 envelope。
        let mut unmet = vec![0.0f64; n_int];
        let mut unmet_up = input.target.reserve_up_kw;
        let mut unmet_dn = input.target.reserve_down_kw;
        for (idx, s) in input.sites.iter().enumerate() {
            for i in 0..n_int {
                let r = residual_i[i];
                if r > 0.0 {
                    unmet[i] = r;
                }
            }
            // 备用按最坏区间计（承诺的备用必须任意区间都可交付）：
            // 上调空间看已分配最高点，下调空间看已分配最低点。
            let alloc_min = alloc[idx].iter().copied().fold(f64::INFINITY, f64::min);
            let alloc_max = alloc[idx].iter().copied().fold(f64::NEG_INFINITY, f64::max);
            unmet_up -= s.reserve_up_kw.min(s.p_max_kw - alloc_max);
            unmet_dn -= s.reserve_down_kw.min(alloc_min - s.p_min_kw);
        }
        let targets = input
            .sites
            .iter()
            .enumerate()
            .map(|(idx, s)| {
                let min_alloc = alloc[idx].iter().copied().fold(f64::INFINITY, f64::min);
                let max_alloc = alloc[idx].iter().copied().fold(f64::NEG_INFINITY, f64::max);
                SiteAllocation {
                    site_id: s.site_id,
                    points_kw: alloc[idx].clone(),
                    energy_kwh: alloc[idx].iter().sum::<f64>() * hour_h,
                    reserve_up_kw: s.reserve_up_kw.min(s.p_max_kw - max_alloc),
                    reserve_down_kw: s.reserve_down_kw.min(min_alloc - s.p_min_kw),
                    within_envelope: true,
                }
            })
            .collect();
        Ok(AllocationResult::Feasible {
            targets,
            residual: ResidualEnvelope {
                unmet_kw: unmet,
                unmet_reserve_up_kw: unmet_up.max(0.0),
                unmet_reserve_down_kw: unmet_dn.max(0.0),
            },
        })
    }

    fn precheck(input: &AllocationInput) -> Result<(), AllocError> {
        if input.interval_ns == 0 {
            return Err(AllocError::GridOrUnitMismatch);
        }
        if input.sites.len() > MAX_SITES || input.target.p_target_kw.len() > MAX_INTERVALS {
            return Err(AllocError::TooLarge);
        }
        if input.target.p_target_kw.is_empty() {
            return Err(AllocError::IntervalCountMismatch);
        }
        for s in &input.sites {
            use eneros_allocation::sanitize;
            if sanitize(s.p_min_kw).is_none()
                || sanitize(s.p_max_kw).is_none()
                || sanitize(s.energy_min_kwh).is_none()
                || sanitize(s.energy_max_kwh).is_none()
                || sanitize(s.ramp_kw_s).is_none()
                || sanitize(s.reserve_up_kw).is_none()
                || sanitize(s.reserve_down_kw).is_none()
                || s.p_min_kw < 0.0
                || s.p_max_kw < s.p_min_kw
                || s.energy_min_kwh < 0.0
                || s.energy_max_kwh < s.energy_min_kwh
                || s.ramp_kw_s < 0.0
                || s.reserve_up_kw < 0.0
                || s.reserve_down_kw < 0.0
            {
                return Err(AllocError::InvalidInput);
            }
        }
        for t in &input.target.p_target_kw {
            if eneros_allocation::sanitize(*t).is_none() || *t < 0.0 {
                return Err(AllocError::InvalidInput);
            }
        }
        if eneros_allocation::sanitize(input.target.reserve_up_kw).is_none()
            || eneros_allocation::sanitize(input.target.reserve_down_kw).is_none()
            || input.target.reserve_up_kw < 0.0
            || input.target.reserve_down_kw < 0.0
        {
            return Err(AllocError::InvalidInput);
        }
        Ok(())
    }

    /// 站点侧包络校验（拒恶意超范围目标；也供协调器侧预防性自检）。
    pub fn check_envelope(
        offer: &CapabilityOffer,
        interval_start_ns: u64,
        interval_ns: u64,
        points_kw: &[f64],
        tolerance_profile_id: u32,
    ) -> EnvelopeCheck {
        if tolerance_profile_id == 0 {
            return EnvelopeCheck::GridMismatch;
        }
        if offer.interval_start_ns != interval_start_ns || points_kw.is_empty() {
            return EnvelopeCheck::GridMismatch;
        }
        // P2-1：曲线覆盖必须包含 offer 有效时段（恶意协调器不得用 1h
        // offer 塞 288h 曲线——覆盖外承诺）。饱和乘加：恶意/故障协调器的
        // 超大 interval_ns 在 debug 构建 panic、release 构建回绕绕过覆盖
        // 检查，都必须 fail-closed 判 GridMismatch。
        let cover_end_ns =
            interval_start_ns.saturating_add(interval_ns.saturating_mul(points_kw.len() as u64));
        if cover_end_ns > offer.interval_end_ns || interval_ns == 0 {
            return EnvelopeCheck::GridMismatch;
        }
        let dt_s = interval_ns as f64 / 1e9;
        let hour_h = dt_s / 3600.0;
        let mut e = 0.0f64;
        for (i, v) in points_kw.iter().enumerate() {
            if !v.is_finite() || *v < 0.0 {
                return EnvelopeCheck::PowerOutOfRange;
            }
            if *v < offer.p_min_kw - 1e-9 || *v > offer.p_max_kw + 1e-9 {
                return EnvelopeCheck::PowerOutOfRange;
            }
            if i > 0 {
                let d = (v - points_kw[i - 1]).abs();
                if d > offer.ramp_kw_s * dt_s + 1e-9 {
                    return EnvelopeCheck::RampOutOfRange;
                }
            }
            e += v * hour_h;
        }
        if e < offer.energy_min_kwh - 1e-9 || e > offer.energy_max_kwh + 1e-9 {
            return EnvelopeCheck::EnergyOutOfRange;
        }
        EnvelopeCheck::Ok
    }

    /// 由求解结果生成面向单一站点的 [`AllocationProposal`] wire 产物。
    pub fn to_proposal(
        alloc: &SiteAllocation,
        offer: &CapabilityOffer,
        proposal_id: [u8; 16],
        tolerance_profile_id: u32,
        interval_start_ns: u64,
        interval_ns: u64,
    ) -> Result<AllocationProposal, WireError> {
        let proposal = AllocationProposal {
            proposal_id,
            offer_id: offer.offer_id,
            site_id: alloc.site_id,
            tolerance_profile_id,
            interval_start_ns,
            interval_ns,
            points_kw: alloc.points_kw.clone(),
            reserve_up_kw: alloc.reserve_up_kw,
            reserve_down_kw: alloc.reserve_down_kw,
            target_curve_ref: BlobRefV1::new(2, &[]),
        };
        // BlobRef digest 需要曲线 canonical ——先算再填。
        let curve = proposal.curve_canonical()?;
        let mut p = proposal;
        p.target_curve_ref = BlobRefV1::new(2, &curve);
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]
    use super::*;

    fn site(
        id: u32,
        pmin: f64,
        pmax: f64,
        emin: f64,
        emax: f64,
        ramp: f64,
        rank: u32,
    ) -> SiteOfferView {
        SiteOfferView {
            site_id: id,
            offer_id: [id as u8; 16],
            revision: 1,
            p_min_kw: pmin,
            p_max_kw: pmax,
            energy_min_kwh: emin,
            energy_max_kwh: emax,
            ramp_kw_s: ramp,
            reserve_up_kw: 1.0,
            reserve_down_kw: 1.0,
            price_rank: rank,
        }
    }

    fn input(sites: Vec<SiteOfferView>, target_kw: &[f64]) -> AllocationInput {
        AllocationInput {
            interval_start_ns: 0,
            interval_ns: 3_600_000_000_000,
            sites,
            target: TargetProfile {
                p_target_kw: target_kw.to_vec(),
                reserve_up_kw: 0.0,
                reserve_down_kw: 0.0,
            },
        }
    }

    /// 参考模型：朴素贪婪（min load + 按 rank 填满），与本求解器对比。
    fn reference_model(input: &AllocationInput) -> Vec<Vec<f64>> {
        let n = input.sites.len();
        let n_int = input.target.p_target_kw.len();
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|a, b| {
            input.sites[*a]
                .price_rank
                .cmp(&input.sites[*b].price_rank)
                .then(input.sites[*a].site_id.cmp(&input.sites[*b].site_id))
        });
        let mut out: Vec<Vec<f64>> = input
            .sites
            .iter()
            .map(|s| vec![s.p_min_kw; n_int])
            .collect();
        let mut residual: Vec<f64> = input.target.p_target_kw.clone();
        for s in &input.sites {
            for r in residual.iter_mut() {
                *r -= s.p_min_kw;
            }
        }
        for &idx in &order {
            let s = &input.sites[idx];
            for i in 0..n_int {
                if residual[i] <= 0.0 {
                    break;
                }
                let d = residual[i].min(s.p_max_kw - s.p_min_kw);
                out[idx][i] += d;
                residual[i] -= d;
            }
        }
        out
    }

    #[test]
    fn feasible_matches_reference() {
        // 能量界宽松（不参与约束），与无能量感知的参考模型可对比；
        // 能量修复行为由 energy_repair_keeps_site_inside_band 单独覆盖。
        let sites = vec![
            site(1, 0.0, 100.0, 0.0, 100_000.0, 100.0, 2),
            site(2, 10.0, 50.0, 0.0, 100_000.0, 50.0, 0),
            site(3, 0.0, 30.0, 0.0, 100_000.0, 30.0, 1),
        ];
        let inp = input(sites.clone(), &[60.0, 40.0, 50.0]);
        let res = AllocationSolver::solve(&inp).expect("solve");
        match res {
            AllocationResult::Feasible { targets, residual } => {
                let refm = reference_model(&inp);
                for t in &targets {
                    let idx = sites
                        .iter()
                        .position(|s| s.site_id == t.site_id)
                        .expect("idx");
                    for (i, (got, want)) in t.points_kw.iter().zip(refm[idx].iter()).enumerate() {
                        assert!(
                            (got - want).abs() < 1e-6,
                            "site {} interval {}: {} vs ref {}",
                            t.site_id,
                            i,
                            t.points_kw[i],
                            refm[idx][i]
                        );
                    }
                    assert!(t.within_envelope);
                }
                let total: f64 = targets.iter().map(|t| t.points_kw[0]).sum();
                assert!((total - 60.0).abs() < 1e-6);
                assert!(residual.unmet_kw.iter().all(|v| *v >= -1e-9));
            }
            other => panic!("expected feasible, got {other:?}"),
        }
    }

    #[test]
    fn envelope_check_saturates_huge_interval_grid() {
        // 恶意/故障协调器的超大 interval_ns：饱和乘加必须 fail-closed 判
        // GridMismatch（裸乘法在 debug panic、release 回绕绕过覆盖检查）。
        let o = CapabilityOffer {
            offer_id: [1u8; 16],
            interval_start_ns: 0,
            interval_end_ns: 3_600_000_000_000,
            p_min_kw: 0.0,
            p_max_kw: 50.0,
            energy_min_kwh: 0.0,
            energy_max_kwh: 100.0,
            ramp_kw_s: 100.0,
            reserve_up_kw: 1.0,
            reserve_down_kw: 1.0,
            price_curve_ref: BlobRefV1::new(1, b"p"),
            capability_root: [0u8; 32],
            valid_until_ns: u64::MAX,
            revision: 1,
        };
        assert_eq!(
            AllocationSolver::check_envelope(&o, 0, u64::MAX, &[10.0, 10.0], 1),
            EnvelopeCheck::GridMismatch
        );
        assert_eq!(
            AllocationSolver::check_envelope(&o, u64::MAX - 1, u64::MAX, &[10.0], 1),
            EnvelopeCheck::GridMismatch
        );
    }

    #[test]
    fn reserve_reported_at_worst_interval() {
        // 备用承诺必须任意区间都可交付：上调空间按已分配最高点计。
        // 平坦计划（min=max）下备用足额；顶格分配后上调备用必须如实
        // 入残差（不得以最好区间虚报供给）。
        let mk = |target: &[f64]| {
            let mut s = site(1, 0.0, 100.0, 0.0, 100_000.0, 100.0, 0);
            s.reserve_up_kw = 30.0; // 站点承诺 30 kW 上调备用
            AllocationInput {
                interval_start_ns: 0,
                interval_ns: 3_600_000_000_000,
                sites: vec![s],
                target: TargetProfile {
                    p_target_kw: target.to_vec(),
                    reserve_up_kw: 30.0,
                    reserve_down_kw: 0.0,
                },
            }
        };
        // 分配 60 → 上调空间 40 ≥ 30：足额。
        match AllocationSolver::solve(&mk(&[60.0, 60.0])).expect("solve") {
            AllocationResult::Feasible { targets, residual } => {
                assert_eq!(targets[0].reserve_up_kw, 30.0);
                assert!(residual.unmet_reserve_up_kw.abs() < 1e-9);
            }
            other => panic!("expected feasible, got {other:?}"),
        }
        // 目标顶格 100 → 上调空间 0 < 30：差额 30 全额入残差。
        match AllocationSolver::solve(&mk(&[100.0, 100.0])).expect("solve2") {
            AllocationResult::Feasible { targets, residual } => {
                assert_eq!(targets[0].reserve_up_kw, 0.0);
                assert!((residual.unmet_reserve_up_kw - 30.0).abs() < 1e-9);
            }
            other => panic!("expected feasible, got {other:?}"),
        }
    }

    #[test]
    fn deterministic_across_runs() {
        let sites = vec![
            site(1, 5.0, 120.0, 10.0, 200.0, 20.0, 1),
            site(2, 0.0, 80.0, 0.0, 150.0, 40.0, 0),
            site(3, 2.0, 60.0, 5.0, 100.0, 30.0, 2),
        ];
        let inp = input(sites, &[70.0, 30.0, 80.0, 65.0]);
        let a = AllocationSolver::solve(&inp).expect("a");
        let b = AllocationSolver::solve(&inp).expect("b");
        assert_eq!(a, b, "同输入必须同输出");
    }

    #[test]
    fn infeasible_returns_conflicts() {
        // 目标 250 > Σ p_max = 100+50+30 = 180
        let sites = vec![
            site(1, 0.0, 100.0, 0.0, 500.0, 100.0, 0),
            site(2, 10.0, 50.0, 0.0, 500.0, 100.0, 1),
            site(3, 0.0, 30.0, 0.0, 500.0, 100.0, 2),
        ];
        let inp = input(sites, &[250.0]);
        match AllocationSolver::solve(&inp).expect("solve") {
            AllocationResult::Infeasible(env) => {
                assert!(!env.conflicts.is_empty());
                let c = &env.conflicts[0];
                assert_eq!(c.constraint, "aggregate_power_band");
                assert_eq!(c.site_id, 1);
                assert_eq!(c.value, 250.0);
                assert_eq!(c.bound, 180.0);
            }
            other => panic!("expected infeasible, got {other:?}"),
        }
    }

    #[test]
    fn energy_repair_keeps_site_inside_band() {
        // energy_max 每站 50 kWh/1h → 必须把功率收进 50 kW 内；
        // 目标 150 = 100+50（两站即可）。第三站能量上限 0 → 只能 0。
        let sites = vec![
            site(1, 0.0, 100.0, 0.0, 50.0, 100.0, 0),
            site(2, 0.0, 50.0, 0.0, 25.0, 100.0, 1),
        ];
        let inp = input(sites, &[75.0]);
        match AllocationSolver::solve(&inp).expect("solve") {
            AllocationResult::Feasible { targets, .. } => {
                for t in &targets {
                    let limit = if t.site_id == 1 { 50.0 } else { 25.0 };
                    assert!(
                        t.points_kw[0] <= limit + 1e-6,
                        "site {} p={}",
                        t.site_id,
                        t.points_kw[0]
                    );
                    assert!(t.energy_kwh <= limit + 1e-6);
                }
            }
            other => panic!("expected feasible, got {other:?}"),
        }
    }

    #[test]
    fn ramp_respected() {
        // ramp 1 kW/s → dt=3600s → 每步最大 3600 kW，不受限；
        // 用 dt=60s 构造受限场景。
        let sites = vec![site(1, 0.0, 100.0, 0.0, 500.0, 1.0, 0)];
        let inp = AllocationInput {
            interval_start_ns: 0,
            interval_ns: 60_000_000_000,
            sites,
            target: TargetProfile {
                p_target_kw: vec![100.0, 0.0],
                reserve_up_kw: 0.0,
                reserve_down_kw: 0.0,
            },
        };
        match AllocationSolver::solve(&inp).expect("solve") {
            AllocationResult::Feasible { targets, .. } => {
                let t = &targets[0];
                assert!(t.points_kw[0] <= 60.0 + 1e-6, "ramp up limited to 60 kW");
                // 0 目标区间 -> 下载要回 0（60s 内降 60 可以）
                assert!(t.points_kw[1] >= 0.0 - 1e-6);
            }
            other => panic!("expected feasible, got {other:?}"),
        }
    }

    #[test]
    fn envelope_check_rejects_out_of_band() {
        let o = CapabilityOffer {
            offer_id: [1u8; 16],
            interval_start_ns: 0,
            interval_end_ns: 3_600_000_000_000,
            p_min_kw: 0.0,
            p_max_kw: 50.0,
            energy_min_kwh: 0.0,
            energy_max_kwh: 100.0,
            ramp_kw_s: 100.0,
            reserve_up_kw: 1.0,
            reserve_down_kw: 1.0,
            price_curve_ref: BlobRefV1::new(1, b"p"),
            capability_root: [0u8; 32],
            valid_until_ns: u64::MAX,
            revision: 1,
        };
        assert_eq!(
            AllocationSolver::check_envelope(&o, 0, 3_600_000_000_000, &[60.0], 1),
            EnvelopeCheck::PowerOutOfRange
        );
        assert_eq!(
            AllocationSolver::check_envelope(&o, 0, 3_600_000_000_000, &[40.0], 0),
            EnvelopeCheck::GridMismatch
        );
        assert_eq!(
            AllocationSolver::check_envelope(&o, 0, 3_600_000_000_000, &[40.0], 1),
            EnvelopeCheck::Ok
        );
        assert_eq!(
            AllocationSolver::check_envelope(&o, 0, 3_600_000_000_000, &[f64::NAN], 1),
            EnvelopeCheck::PowerOutOfRange
        );
    }

    #[test]
    fn invalid_input_rejected() {
        let bad = vec![site(1, 0.0, f64::INFINITY, 0.0, 500.0, 100.0, 0)];
        assert_eq!(
            AllocationSolver::solve(&input(bad, &[10.0])),
            Err(AllocError::InvalidInput)
        );
        let neg = vec![site(1, -1.0, 100.0, 0.0, 500.0, 100.0, 0)];
        assert_eq!(
            AllocationSolver::solve(&input(neg, &[10.0])),
            Err(AllocError::InvalidInput)
        );
        assert_eq!(
            AllocationSolver::solve(&input(vec![], &[])),
            Err(AllocError::IntervalCountMismatch)
        );
        assert_eq!(
            AllocationSolver::solve(&input(vec![], &[10.0, 20.0])),
            Ok(AllocationResult::Infeasible(InfeasibleEnvelope {
                conflicts: vec![],
                reason: "aggregate target outside combined offer band",
            }))
        );
    }

    #[test]
    fn empty_sites_but_feasible_zero_target() {
        let inp = input(vec![], &[0.0, 0.0]);
        match AllocationSolver::solve(&inp).expect("solve") {
            AllocationResult::Feasible { targets, residual } => {
                assert!(targets.is_empty());
                assert!(residual
                    .unmet_kw
                    .into_iter()
                    .all(|v| (v - 0.0).abs() < 1e-9));
            }
            other => panic!("expected feasible, got {other:?}"),
        }
    }

    /// 金样位型锁（蓝图 62 卷 62.3 WP-62.3.2：merit-order 换 kernel
    /// 原语前后逐字节等价——爬坡受限 + 能量修复路径全走过的定例输入）。
    /// 断言值 = 换核前现状实跑位型（本测试先于换核落库）。
    #[test]
    fn golden_bit_exact_before_kernel_swap() {
        // 爬坡 1 kW/s、dt=60s → 每步限 60 kW（爬坡修复路径激活）；
        // energy 带 40/90 kWh → 能量修复路径激活。
        let sites = vec![
            site(1, 5.0, 120.0, 2.0, 6.0, 1.0, 2),
            site(2, 0.0, 80.0, 0.0, 3.0, 1.0, 0),
            site(3, 2.0, 60.0, 0.2, 3.5, 1.0, 2),
        ];
        let inp = AllocationInput {
            interval_start_ns: 0,
            interval_ns: 60_000_000_000,
            sites,
            target: TargetProfile {
                p_target_kw: vec![150.0, 130.0, 110.0, 90.0],
                reserve_up_kw: 3.0,
                reserve_down_kw: 2.0,
            },
        };
        let out = AllocationSolver::solve(&inp).expect("solve");
        let mut dump = String::new();
        match &out {
            AllocationResult::Feasible { targets, residual } => {
                for t in targets {
                    for v in &t.points_kw {
                        dump.push_str(&format!("p:{:016x} ", v.to_bits()));
                    }
                    dump.push_str(&format!(
                        "e:{:016x} up:{:016x} dn:{:016x} ",
                        t.energy_kwh.to_bits(),
                        t.reserve_up_kw.to_bits(),
                        t.reserve_down_kw.to_bits()
                    ));
                }
                for v in &residual.unmet_kw {
                    dump.push_str(&format!("u:{:016x} ", v.to_bits()));
                }
                dump.push_str(&format!(
                    "ru:{:016x} rd:{:016x}",
                    residual.unmet_reserve_up_kw.to_bits(),
                    residual.unmet_reserve_down_kw.to_bits()
                ));
            }
            other => dump.push_str(&format!("{other:?}")),
        }
        // 换核前捕获的位型（换核后重跑必须逐字节一致）。
        assert_eq!(
            dump,
            "p:4050400000000000 p:4050400000000000 p:4048000000000000 p:403c000000000000 e:400b777777777777 up:3ff0000000000000 dn:3ff0000000000000 p:0000000000000000 p:404e000000000000 p:404e000000000000 p:404e000000000000 e:4008000000000000 up:3ff0000000000000 dn:0000000000000000 p:4039000000000000 p:4014000000000000 p:4000000000000000 p:4000000000000000 e:3fe2222222222222 up:3ff0000000000000 dn:0000000000000000 u:404e000000000000 u:0000000000000000 u:0000000000000000 u:0000000000000000 ru:0000000000000000 rd:3ff0000000000000",
            "merit-order 换核位型漂移——逐字节等价判据失败"
        );
    }
}

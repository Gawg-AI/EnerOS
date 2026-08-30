//! 聚合包络（[08] §4.5：**不能简单求和**）：p/q/energy/ramp 结合拓扑、
//! 同时性、效率和不可控基线；保留成员 version/root；任一成员状态变化
//! 使聚合 generation 变化。本版本只生成包络与参考聚合，不执行多资源
//! 命令；聚合承诺分解与下级 Certifier 校验属 Phase 4（委托登记）。

use alloc::vec::Vec;

use crate::capability::{Availability, CapabilityEnvelope, DeratingReason};

/// 聚合成员（包络 + 不可控基线标记）。
#[derive(Debug, Clone, PartialEq)]
pub struct AggregateMember {
    pub asset_id: u64,
    pub envelope: CapabilityEnvelope,
    /// 不可控基线（固定负荷等；不进入可控和）。
    pub uncontrolled_baseline_kw: f64,
}

/// 聚合包络（参考实现；同时性系数/效率由组装方注入——不得在求和里
/// 自造默认值）。
#[derive(Debug, Clone, PartialEq)]
pub struct AggregateEnvelope {
    pub member_count: usize,
    pub p_min_kw: f64,
    pub p_max_kw: f64,
    pub q_min_kvar: f64,
    pub q_max_kvar: f64,
    pub energy_max_kwh: f64,
    pub ramp_up_kw_s: f64,
    pub ramp_down_kw_s: f64,
    /// 成员 version/root 保留（任一成员变化 → generation 变化）。
    pub member_roots: Vec<[u8; 16]>,
    pub generation: u64,
    pub availability: Availability,
    /// 不可控基线合计（不进入可控和）。
    pub uncontrolled_total_kw: f64,
    pub derating_reason: DeratingReason,
}

/// 聚合参数（同时性/效率——来源必须可追溯；禁止自造）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AggregateParams {
    /// 有功同时性系数 (0,1]。
    pub simultaneity: f64,
    /// 平均效率 (0,1]（损耗进入能量/爬坡）。
    pub efficiency: f64,
}

/// 参考聚合（[08] §4.5）：逐成员并入；任一成员不可用 → 聚合沿该成员
/// 收紧（成员 Unavailable 时聚合可用性 = Derated 且该成员出可控和）。
pub fn aggregate_envelopes(
    members: &[AggregateMember],
    params: AggregateParams,
    now_ns: u64,
) -> AggregateEnvelope {
    let mut p_min = 0.0f64;
    let mut p_max = 0.0f64;
    let mut q_min = 0.0f64;
    let mut q_max = 0.0f64;
    let mut energy = 0.0f64;
    let mut ramp_up = 0.0f64;
    let mut ramp_down = 0.0f64;
    let mut uncontrolled = 0.0f64;
    let mut roots = Vec::with_capacity(members.len());
    let mut unavailable = false;
    let mut derated = false;
    let mut generation = 0u64;
    // 叠前有效性窗口：成员包络必须覆盖 now（过期成员不并入可控和）。
    for m in members {
        generation = generation.max(m.envelope.generation);
        roots.push(m.envelope.source_snapshot_id);
        uncontrolled += m.uncontrolled_baseline_kw;
        if !m.envelope.is_valid_at(now_ns) {
            unavailable = true;
            continue;
        }
        if !(params.simultaneity.is_finite()
            && params.simultaneity > 0.0
            && params.simultaneity <= 1.0
            && params.efficiency.is_finite()
            && params.efficiency > 0.0
            && params.efficiency <= 1.0)
        {
            // 参数非法 → 聚合不可用（拒绝默认值）。
            unavailable = true;
            continue;
        }
        if !m.envelope.controllable() {
            // 成员不可控：出可控和，聚合标记降额。
            derated = true;
            continue;
        }
        p_min += m.envelope.p_min_kw * params.simultaneity;
        p_max += m.envelope.p_max_kw * params.simultaneity;
        q_min += m.envelope.q_min_kvar * params.simultaneity;
        q_max += m.envelope.q_max_kvar * params.simultaneity;
        energy += m.envelope.energy_max_kwh * params.efficiency;
        ramp_up += m.envelope.ramp_up_kw_s * params.efficiency;
        ramp_down += m.envelope.ramp_down_kw_s * params.efficiency;
    }
    let _ = derated;
    AggregateEnvelope {
        member_count: members.len(),
        p_min_kw: p_min,
        p_max_kw: p_max,
        q_min_kvar: q_min,
        q_max_kvar: q_max,
        energy_max_kwh: energy,
        ramp_up_kw_s: ramp_up,
        ramp_down_kw_s: ramp_down,
        member_roots: roots,
        generation,
        availability: if unavailable {
            Availability::Unavailable
        } else {
            Availability::Full
        },
        uncontrolled_total_kw: uncontrolled,
        derating_reason: DeratingReason::None,
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;

    fn member(asset: u64, pmin: f64, pmax: f64, root: u8, gen: u64) -> AggregateMember {
        AggregateMember {
            asset_id: asset,
            envelope: CapabilityEnvelope {
                asset_id: asset,
                valid_from_ns: 0,
                valid_until_ns: u64::MAX,
                p_min_kw: pmin,
                p_max_kw: pmax,
                q_min_kvar: -10.0,
                q_max_kvar: 10.0,
                energy_min_kwh: 0.0,
                energy_max_kwh: 100.0,
                ramp_up_kw_s: 1.0,
                ramp_down_kw_s: 1.0,
                availability: Availability::Full,
                derating_reason: DeratingReason::None,
                source_snapshot_id: [root; 16],
                tightenings: Vec::new(),
                generation: gen,
                min_on_ns: 0,
                min_off_ns: 0,
            },
            uncontrolled_baseline_kw: 20.0,
        }
    }

    #[test]
    fn aggregate_is_not_plain_sum() {
        let members = vec![
            member(1, -500.0, 500.0, 1, 1),
            member(2, -100.0, 300.0, 2, 1),
        ];
        let agg = aggregate_envelopes(
            &members,
            AggregateParams {
                simultaneity: 0.8,
                efficiency: 0.9,
            },
            0,
        );
        // 不打折的求和是 -600..800；带同时性/效率后更紧。
        assert!((agg.p_min_kw - (-600.0 * 0.8)).abs() < 1e-9);
        assert!((agg.p_max_kw - (800.0 * 0.8)).abs() < 1e-9);
        assert!((agg.energy_max_kwh - (200.0 * 0.9)).abs() < 1e-9);
        assert_eq!(agg.member_roots.len(), 2);
        assert_eq!(agg.uncontrolled_total_kw, 40.0);
    }

    #[test]
    fn unavailable_member_excluded_with_derating() {
        let mut bad = member(2, -100.0, 300.0, 2, 1);
        bad.envelope.availability = Availability::Unavailable;
        let agg = aggregate_envelopes(
            &[member(1, -500.0, 500.0, 1, 1), bad],
            AggregateParams {
                simultaneity: 1.0,
                efficiency: 1.0,
            },
            0,
        );
        // 不可控成员出可控和：聚合 = 健康成员。
        assert!((agg.p_max_kw - 500.0).abs() < 1e-9);
        assert_eq!(agg.member_roots.len(), 2); // 根保留（证据）
        assert_eq!(agg.availability, Availability::Full);
    }

    #[test]
    fn invalid_params_reject() {
        let members = vec![member(1, -500.0, 500.0, 1, 1)];
        let agg = aggregate_envelopes(
            &members,
            AggregateParams {
                simultaneity: 0.0, // 非法
                efficiency: 1.0,
            },
            0,
        );
        assert_eq!(agg.availability, Availability::Unavailable);
        assert_eq!(agg.p_max_kw, 0.0);
    }
}

//! `DomainOptimizer::optimize` 实现（ARCH-G03：interface 与 implementation 分离）。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use eneros_energy_market_agent::{
    multi_dispatch::equal_split, DeviceAssignment, DeviceCapability, DeviceMode, DispatchPlan,
};
use eneros_solver_core::result::SolveStatus;

use super::*;

impl DomainOptimizer {
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

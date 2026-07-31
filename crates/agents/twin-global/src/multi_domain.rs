//! 多域联合仿真引擎
//!
//! 管理多个领域孪生体，执行联合仿真步进，收集跨域耦合变量。

extern crate alloc;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::types::{CouplingVar, DomainBoundary, DomainKind, DomainState, DomainTwin};

/// 多域联合仿真器
#[derive(Debug)]
pub struct MultiDomainSimulator {
    /// 所有领域孪生体
    pub domains: Vec<DomainTwin>,
}

impl MultiDomainSimulator {
    /// 创建包含 6 个默认领域的仿真器
    ///
    /// 默认领域: Storage, Pv, Wind, Load, Grid, Market
    pub fn new() -> Self {
        let kinds = [
            DomainKind::Storage,
            DomainKind::Pv,
            DomainKind::Wind,
            DomainKind::Load,
            DomainKind::Grid,
            DomainKind::Market,
        ];

        let domains = kinds
            .iter()
            .enumerate()
            .map(|(i, &kind)| {
                let id = (i + 1) as u64;
                let power = match kind {
                    DomainKind::Storage => 100.0,
                    DomainKind::Pv => 350.0,
                    DomainKind::Wind => 500.0,
                    DomainKind::Load => -200.0,
                    DomainKind::Grid => 0.0,
                    DomainKind::Market => 0.0,
                };
                let soc = match kind {
                    DomainKind::Storage => 0.75,
                    _ => 0.0,
                };
                DomainTwin {
                    domain_id: id,
                    kind,
                    state: DomainState {
                        domain_id: id,
                        kind,
                        power_kw: power,
                        soc,
                        status: String::from("idle"),
                    },
                    boundary: DomainBoundary {
                        coupling_vars: vec![CouplingVar {
                            name: String::from("power_exchange"),
                            source_domain: id,
                            target_domain: if id < 6 { id + 1 } else { 1 },
                            value: power * 0.1,
                        }],
                        exchange_limit_kw: 1000.0,
                    },
                }
            })
            .collect();

        Self { domains }
    }

    /// 添加自定义领域孪生体
    pub fn add_domain(&mut self, domain: DomainTwin) {
        self.domains.push(domain);
    }

    /// 执行一步联合仿真
    ///
    /// Mock 实现: 对每个领域的 power_kw 施加确定性微扰
    pub fn simulate_step(&mut self) {
        for (i, domain) in self.domains.iter_mut().enumerate() {
            let delta = ((i as f64) + 1.0) * 0.5;
            domain.state.power_kw += delta;
            // 更新耦合变量值
            for cv in domain.boundary.coupling_vars.iter_mut() {
                cv.value = domain.state.power_kw * 0.1;
            }
            domain.state.status = String::from("simulating");
        }
    }

    /// 收集所有领域的耦合变量
    pub fn get_coupling_vars(&self) -> Vec<CouplingVar> {
        self.domains
            .iter()
            .flat_map(|d| d.boundary.coupling_vars.iter().cloned())
            .collect()
    }
}

impl Default for MultiDomainSimulator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_creation_default_domains() {
        let sim = MultiDomainSimulator::new();
        assert_eq!(sim.domains.len(), 6);
    }

    #[test]
    fn test_domain_kinds() {
        let sim = MultiDomainSimulator::new();
        assert_eq!(sim.domains[0].kind, DomainKind::Storage);
        assert_eq!(sim.domains[1].kind, DomainKind::Pv);
        assert_eq!(sim.domains[2].kind, DomainKind::Wind);
        assert_eq!(sim.domains[3].kind, DomainKind::Load);
        assert_eq!(sim.domains[4].kind, DomainKind::Grid);
        assert_eq!(sim.domains[5].kind, DomainKind::Market);
    }

    #[test]
    fn test_add_domain() {
        let mut sim = MultiDomainSimulator::new();
        let extra = DomainTwin {
            domain_id: 99,
            kind: DomainKind::Storage,
            state: DomainState {
                domain_id: 99,
                kind: DomainKind::Storage,
                power_kw: 50.0,
                soc: 0.5,
                status: String::from("custom"),
            },
            boundary: DomainBoundary {
                coupling_vars: Vec::new(),
                exchange_limit_kw: 200.0,
            },
        };
        sim.add_domain(extra);
        assert_eq!(sim.domains.len(), 7);
        assert_eq!(sim.domains[6].domain_id, 99);
    }

    #[test]
    fn test_simulate_step_updates_state() {
        let mut sim = MultiDomainSimulator::new();
        let initial_power = sim.domains[0].state.power_kw;
        sim.simulate_step();
        assert!(sim.domains[0].state.power_kw > initial_power);
        assert_eq!(sim.domains[0].state.status, "simulating");
    }

    #[test]
    fn test_simulate_step_deterministic() {
        let mut sim1 = MultiDomainSimulator::new();
        let mut sim2 = MultiDomainSimulator::new();
        sim1.simulate_step();
        sim2.simulate_step();
        for (d1, d2) in sim1.domains.iter().zip(sim2.domains.iter()) {
            assert!((d1.state.power_kw - d2.state.power_kw).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn test_get_coupling_vars() {
        let sim = MultiDomainSimulator::new();
        let vars = sim.get_coupling_vars();
        // 每个默认领域有 1 个耦合变量
        assert_eq!(vars.len(), 6);
        assert_eq!(vars[0].name, "power_exchange");
    }
}

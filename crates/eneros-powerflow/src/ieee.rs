use crate::matrix::YBusMatrix;
use crate::solver::BusTypeNR;
use eneros_core::ElementId;
use std::collections::HashMap;

/// Bus data for the IEEE 14-bus test system
#[derive(Debug, Clone)]
pub struct Ieee14Bus {
    /// Bus ID (1-based as per IEEE convention)
    pub bus_id: u32,
    /// Bus type: 0=Slack, 1=PV, 2=PQ
    pub bus_type: u8,
    /// Active power net injection (MW), positive=generation, negative=load
    pub p_mw: f64,
    /// Reactive power net injection (MVar), positive=generation, negative=load
    pub q_mvar: f64,
    /// Voltage magnitude (p.u.)
    pub v_pu: f64,
    /// Voltage angle (degrees)
    pub angle_deg: f64,
}

/// Branch data for the IEEE 14-bus test system
#[derive(Debug, Clone)]
pub struct Ieee14Branch {
    /// From bus (1-based)
    pub from_bus: u32,
    /// To bus (1-based)
    pub to_bus: u32,
    /// Resistance (p.u.)
    pub r_pu: f64,
    /// Reactance (p.u.)
    pub x_pu: f64,
    /// Total line charging susceptance (p.u.)
    pub b_pu: f64,
    /// Rate (MVA)
    pub rate_mva: f64,
    /// Transformer tap ratio (1.0 for lines, non-1.0 for transformers)
    pub tap_ratio: f64,
}

/// Complete IEEE 14-bus standard test system data
#[derive(Debug, Clone)]
pub struct Ieee14BusData {
    /// Base MVA
    pub base_mva: f64,
    /// Bus data
    pub buses: Vec<Ieee14Bus>,
    /// Branch data
    pub branches: Vec<Ieee14Branch>,
    /// Shunt susceptances: (bus_id, B in p.u.)
    pub shunt_susceptances: Vec<(u32, f64)>,
}

impl Ieee14BusData {
    /// Convert to solver input format: (YBusMatrix, p_spec, q_spec, bus_types)
    ///
    /// - P and Q are converted from MW/MVar to per-unit by dividing by base_mva
    /// - Bus IDs are mapped from 1-based (IEEE) to 0-based (solver)
    pub fn to_solver_input(&self) -> (YBusMatrix, Vec<f64>, Vec<f64>, Vec<BusTypeNR>) {
        let n = self.buses.len();

        // Build bus map: IEEE 1-based -> solver 0-based
        let bus_map: HashMap<ElementId, usize> = self
            .buses
            .iter()
            .enumerate()
            .map(|(idx, bus)| (bus.bus_id as ElementId, idx))
            .collect();

        // Build branch data for YBusMatrix (with tap ratio)
        let branch_data: Vec<(ElementId, ElementId, f64, f64, f64, f64)> = self
            .branches
            .iter()
            .map(|br| {
                (
                    br.from_bus as ElementId,
                    br.to_bus as ElementId,
                    br.r_pu,
                    br.x_pu,
                    br.b_pu,
                    br.tap_ratio,
                )
            })
            .collect();

        let mut ybus = YBusMatrix::from_branches(&branch_data, &bus_map);
        ybus.set_base_mva(self.base_mva);
        for branch in &self.branches {
            if let (Some(&from_idx), Some(&to_idx)) = (
                bus_map.get(&(branch.from_bus as ElementId)),
                bus_map.get(&(branch.to_bus as ElementId)),
            ) {
                ybus.set_branch_rating_mva(from_idx, to_idx, branch.rate_mva);
            }
        }

        // Add shunt susceptances to Y-Bus diagonal
        for &(bus_id, b_shunt) in &self.shunt_susceptances {
            if let Some(&idx) = bus_map.get(&(bus_id as ElementId)) {
                ybus.add_shunt(idx, 0.0, b_shunt);
            }
        }

        // Build p_spec, q_spec in per-unit
        let mut p_spec = vec![0.0; n];
        let mut q_spec = vec![0.0; n];
        let mut bus_types = Vec::with_capacity(n);

        for bus in &self.buses {
            let idx = (bus.bus_id - 1) as usize;
            // p_mw/q_mvar are net injection (positive=generation, negative=load).
            // Convert to per-unit.
            p_spec[idx] = bus.p_mw / self.base_mva;
            q_spec[idx] = bus.q_mvar / self.base_mva;

            bus_types.push(match bus.bus_type {
                0 => BusTypeNR::Slack,
                1 => BusTypeNR::PV,
                _ => BusTypeNR::PQ,
            });
        }

        (ybus, p_spec, q_spec, bus_types)
    }
}

/// Return the IEEE 14-bus standard test system data
pub fn ieee14() -> Ieee14BusData {
    // IEEE 14-bus system with generation and load data.
    // p_mw and q_mvar represent NET injection (generation - load) in MW/MVar.
    // Positive = net generation, Negative = net load.
    Ieee14BusData {
        base_mva: 100.0,
        buses: vec![
            // Bus 1: Slack, V=1.06pu, angle=0. P_inj determined by slack balance.
            Ieee14Bus {
                bus_id: 1,
                bus_type: 0,
                p_mw: 0.0,
                q_mvar: 0.0,
                v_pu: 1.060,
                angle_deg: 0.0,
            },
            // Bus 2: PV, Gen=40MW, Load=21.7+j12.7MVar => P_inj = 40-21.7 = 18.3MW
            Ieee14Bus {
                bus_id: 2,
                bus_type: 1,
                p_mw: 18.3,
                q_mvar: -12.7,
                v_pu: 1.045,
                angle_deg: -4.98,
            },
            // Bus 3: PV (synchronous condenser), Gen=0MW, Load=94.2+j19.0MVar => P_inj = -94.2MW
            Ieee14Bus {
                bus_id: 3,
                bus_type: 1,
                p_mw: -94.2,
                q_mvar: -19.0,
                v_pu: 1.010,
                angle_deg: -12.72,
            },
            // Bus 4: PQ, Load=47.8-j3.9MVar
            Ieee14Bus {
                bus_id: 4,
                bus_type: 2,
                p_mw: -47.8,
                q_mvar: 3.9,
                v_pu: 1.019,
                angle_deg: -10.33,
            },
            // Bus 5: PQ, Load=7.6+j1.6MVar
            Ieee14Bus {
                bus_id: 5,
                bus_type: 2,
                p_mw: -7.6,
                q_mvar: -1.6,
                v_pu: 1.020,
                angle_deg: -8.78,
            },
            // Bus 6: PV, Gen=0MW, Load=11.2+j7.5MVar => P_inj = -11.2MW
            Ieee14Bus {
                bus_id: 6,
                bus_type: 1,
                p_mw: -11.2,
                q_mvar: -7.5,
                v_pu: 1.070,
                angle_deg: -14.22,
            },
            // Bus 7: PQ, no load
            Ieee14Bus {
                bus_id: 7,
                bus_type: 2,
                p_mw: 0.0,
                q_mvar: 0.0,
                v_pu: 1.062,
                angle_deg: -13.37,
            },
            // Bus 8: PV, Gen=0MW, no load
            Ieee14Bus {
                bus_id: 8,
                bus_type: 1,
                p_mw: 0.0,
                q_mvar: 0.0,
                v_pu: 1.090,
                angle_deg: -13.36,
            },
            // Bus 9: PQ, Load=29.5+j16.6MVar
            Ieee14Bus {
                bus_id: 9,
                bus_type: 2,
                p_mw: -29.5,
                q_mvar: -16.6,
                v_pu: 1.056,
                angle_deg: -14.94,
            },
            // Bus 10: PQ, Load=9.0+j5.8MVar
            Ieee14Bus {
                bus_id: 10,
                bus_type: 2,
                p_mw: -9.0,
                q_mvar: -5.8,
                v_pu: 1.051,
                angle_deg: -15.10,
            },
            // Bus 11: PQ, Load=3.5+j1.8MVar
            Ieee14Bus {
                bus_id: 11,
                bus_type: 2,
                p_mw: -3.5,
                q_mvar: -1.8,
                v_pu: 1.057,
                angle_deg: -14.80,
            },
            // Bus 12: PQ, Load=6.1+j1.6MVar
            Ieee14Bus {
                bus_id: 12,
                bus_type: 2,
                p_mw: -6.1,
                q_mvar: -1.6,
                v_pu: 1.055,
                angle_deg: -15.07,
            },
            // Bus 13: PQ, Load=13.5+j5.8MVar
            Ieee14Bus {
                bus_id: 13,
                bus_type: 2,
                p_mw: -13.5,
                q_mvar: -5.8,
                v_pu: 1.050,
                angle_deg: -15.16,
            },
            // Bus 14: PQ, Load=14.9+j5.0MVar
            Ieee14Bus {
                bus_id: 14,
                bus_type: 2,
                p_mw: -14.9,
                q_mvar: -5.0,
                v_pu: 1.036,
                angle_deg: -16.04,
            },
        ],
        branches: vec![
            // Lines (tap_ratio = 1.0)
            Ieee14Branch {
                from_bus: 1,
                to_bus: 2,
                r_pu: 0.01938,
                x_pu: 0.05917,
                b_pu: 0.0528,
                rate_mva: 100.0,
                tap_ratio: 1.0,
            },
            Ieee14Branch {
                from_bus: 1,
                to_bus: 5,
                r_pu: 0.05403,
                x_pu: 0.22304,
                b_pu: 0.0492,
                rate_mva: 100.0,
                tap_ratio: 1.0,
            },
            Ieee14Branch {
                from_bus: 2,
                to_bus: 3,
                r_pu: 0.04699,
                x_pu: 0.19797,
                b_pu: 0.0438,
                rate_mva: 100.0,
                tap_ratio: 1.0,
            },
            Ieee14Branch {
                from_bus: 2,
                to_bus: 4,
                r_pu: 0.05811,
                x_pu: 0.17632,
                b_pu: 0.0340,
                rate_mva: 100.0,
                tap_ratio: 1.0,
            },
            Ieee14Branch {
                from_bus: 2,
                to_bus: 5,
                r_pu: 0.05695,
                x_pu: 0.17388,
                b_pu: 0.0346,
                rate_mva: 100.0,
                tap_ratio: 1.0,
            },
            Ieee14Branch {
                from_bus: 3,
                to_bus: 4,
                r_pu: 0.06701,
                x_pu: 0.17103,
                b_pu: 0.0128,
                rate_mva: 100.0,
                tap_ratio: 1.0,
            },
            Ieee14Branch {
                from_bus: 4,
                to_bus: 5,
                r_pu: 0.01335,
                x_pu: 0.04211,
                b_pu: 0.0,
                rate_mva: 100.0,
                tap_ratio: 1.0,
            },
            // Transformers with off-nominal tap ratios
            Ieee14Branch {
                from_bus: 4,
                to_bus: 7,
                r_pu: 0.0,
                x_pu: 0.20912,
                b_pu: 0.0,
                rate_mva: 100.0,
                tap_ratio: 0.978,
            },
            Ieee14Branch {
                from_bus: 4,
                to_bus: 9,
                r_pu: 0.0,
                x_pu: 0.55618,
                b_pu: 0.0,
                rate_mva: 100.0,
                tap_ratio: 0.969,
            },
            Ieee14Branch {
                from_bus: 5,
                to_bus: 6,
                r_pu: 0.0,
                x_pu: 0.25202,
                b_pu: 0.0,
                rate_mva: 100.0,
                tap_ratio: 0.932,
            },
            Ieee14Branch {
                from_bus: 6,
                to_bus: 11,
                r_pu: 0.09498,
                x_pu: 0.19890,
                b_pu: 0.0,
                rate_mva: 100.0,
                tap_ratio: 1.0,
            },
            Ieee14Branch {
                from_bus: 6,
                to_bus: 12,
                r_pu: 0.12291,
                x_pu: 0.25581,
                b_pu: 0.0,
                rate_mva: 100.0,
                tap_ratio: 1.0,
            },
            Ieee14Branch {
                from_bus: 6,
                to_bus: 13,
                r_pu: 0.06615,
                x_pu: 0.13027,
                b_pu: 0.0,
                rate_mva: 100.0,
                tap_ratio: 1.0,
            },
            Ieee14Branch {
                from_bus: 7,
                to_bus: 8,
                r_pu: 0.0,
                x_pu: 0.17615,
                b_pu: 0.0,
                rate_mva: 100.0,
                tap_ratio: 0.969,
            },
            Ieee14Branch {
                from_bus: 7,
                to_bus: 9,
                r_pu: 0.0,
                x_pu: 0.11001,
                b_pu: 0.0,
                rate_mva: 100.0,
                tap_ratio: 1.0,
            },
            Ieee14Branch {
                from_bus: 9,
                to_bus: 10,
                r_pu: 0.03181,
                x_pu: 0.08450,
                b_pu: 0.0,
                rate_mva: 100.0,
                tap_ratio: 1.0,
            },
            Ieee14Branch {
                from_bus: 9,
                to_bus: 14,
                r_pu: 0.12711,
                x_pu: 0.27038,
                b_pu: 0.0,
                rate_mva: 100.0,
                tap_ratio: 1.0,
            },
            Ieee14Branch {
                from_bus: 10,
                to_bus: 11,
                r_pu: 0.08205,
                x_pu: 0.19207,
                b_pu: 0.0,
                rate_mva: 100.0,
                tap_ratio: 1.0,
            },
            Ieee14Branch {
                from_bus: 12,
                to_bus: 13,
                r_pu: 0.22092,
                x_pu: 0.19988,
                b_pu: 0.0,
                rate_mva: 100.0,
                tap_ratio: 1.0,
            },
            Ieee14Branch {
                from_bus: 13,
                to_bus: 14,
                r_pu: 0.17093,
                x_pu: 0.34802,
                b_pu: 0.0,
                rate_mva: 100.0,
                tap_ratio: 1.0,
            },
        ],
        // Shunt capacitors: Bus 9 has 19.0 MVar capacitor => B = 19.0/100 = 0.19 pu
        shunt_susceptances: vec![(9, 0.19)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solver::BusTypeNR;

    /// 验证 IEEE-14 标准测试系统的基本结构
    #[test]
    fn test_ieee14_bus_count() {
        let data = ieee14();
        assert_eq!(data.buses.len(), 14, "IEEE-14 should have 14 buses");
    }

    #[test]
    fn test_ieee14_branch_count() {
        let data = ieee14();
        assert_eq!(data.branches.len(), 20, "IEEE-14 should have 20 branches");
    }

    #[test]
    fn test_ieee14_base_mva() {
        let data = ieee14();
        assert_eq!(data.base_mva, 100.0);
    }

    #[test]
    fn test_ieee14_bus_ids_sequential() {
        // IEEE-14 节点 ID 应为 1..=14
        let data = ieee14();
        let ids: Vec<u32> = data.buses.iter().map(|b| b.bus_id).collect();
        assert_eq!(ids, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14]);
    }

    #[test]
    fn test_ieee14_bus_type_distribution() {
        // IEEE-14 应有 1 Slack, 4 PV, 9 PQ
        let data = ieee14();
        let slack_count = data.buses.iter().filter(|b| b.bus_type == 0).count();
        let pv_count = data.buses.iter().filter(|b| b.bus_type == 1).count();
        let pq_count = data.buses.iter().filter(|b| b.bus_type == 2).count();
        assert_eq!(slack_count, 1, "should have 1 slack bus");
        assert_eq!(pv_count, 4, "should have 4 PV buses");
        assert_eq!(pq_count, 9, "should have 9 PQ buses");
    }

    #[test]
    fn test_ieee14_initial_voltages_in_range() {
        // 所有初始电压应在合理范围 [0.95, 1.10]
        let data = ieee14();
        for bus in &data.buses {
            assert!(
                bus.v_pu >= 0.95 && bus.v_pu <= 1.10,
                "Bus {} voltage {} pu out of range [0.95, 1.10]",
                bus.bus_id,
                bus.v_pu
            );
        }
    }

    #[test]
    fn test_ieee14_slack_bus_voltage() {
        // Slack 节点（bus 1）电压应为 1.06 pu，相角为 0
        let data = ieee14();
        let slack = data.buses.iter().find(|b| b.bus_type == 0).unwrap();
        assert_eq!(slack.bus_id, 1);
        assert!((slack.v_pu - 1.060).abs() < 1e-6);
        assert!((slack.angle_deg - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_ieee14_shunt_susceptances() {
        // Bus 9 应有 0.19 pu 并联电纳
        let data = ieee14();
        assert_eq!(data.shunt_susceptances.len(), 1);
        assert_eq!(data.shunt_susceptances[0].0, 9);
        assert!((data.shunt_susceptances[0].1 - 0.19).abs() < 1e-6);
    }

    #[test]
    fn test_ieee14_branch_data_valid() {
        // 所有支路的 r_pu、x_pu 应为有限值，rate_mva > 0
        let data = ieee14();
        for br in &data.branches {
            assert!(br.r_pu.is_finite(), "Branch {}->{} r_pu not finite", br.from_bus, br.to_bus);
            assert!(br.x_pu.is_finite(), "Branch {}->{} x_pu not finite", br.from_bus, br.to_bus);
            assert!(br.rate_mva > 0.0, "Branch {}->{} rate_mva should be positive", br.from_bus, br.to_bus);
            assert!(br.tap_ratio > 0.0, "Branch {}->{} tap_ratio should be positive", br.from_bus, br.to_bus);
        }
    }

    #[test]
    fn test_ieee14_transformer_tap_ratios() {
        // 验证变压器支路的非标准变比
        let data = ieee14();
        let transformers: Vec<&Ieee14Branch> = data
            .branches
            .iter()
            .filter(|br| (br.tap_ratio - 1.0).abs() > 1e-6)
            .collect();
        // IEEE-14 有 3 台变压器（4-7, 4-9, 5-6, 7-8 共 4 台）
        assert!(!transformers.is_empty(), "should have at least one transformer");
        for tx in &transformers {
            assert!(tx.tap_ratio > 0.0 && tx.tap_ratio < 1.1, "transformer tap_ratio out of range");
        }
    }

    #[test]
    fn test_ieee14_to_solver_input_bus_types() {
        // to_solver_input 应正确转换 bus_type
        let data = ieee14();
        let (_ybus, _p, _q, bus_types) = data.to_solver_input();
        assert_eq!(bus_types.len(), 14);
        let slack_count = bus_types.iter().filter(|&&t| t == BusTypeNR::Slack).count();
        let pv_count = bus_types.iter().filter(|&&t| t == BusTypeNR::PV).count();
        let pq_count = bus_types.iter().filter(|&&t| t == BusTypeNR::PQ).count();
        assert_eq!(slack_count, 1);
        assert_eq!(pv_count, 4);
        assert_eq!(pq_count, 9);
    }

    #[test]
    fn test_ieee14_to_solver_input_p_q_per_unit() {
        // to_solver_input 应将 P/Q 从 MW/MVar 转换为 per-unit
        let data = ieee14();
        let (_ybus, p_spec, q_spec, _bus_types) = data.to_solver_input();
        // Bus 1 (idx 0) 是 Slack，P/Q 由平衡计算，spec 应为 0
        assert!((p_spec[0] - 0.0).abs() < 1e-10);
        assert!((q_spec[0] - 0.0).abs() < 1e-10);
        // Bus 2 (idx 1) 是 PV，P_inj = 18.3 MW → p_spec = 0.183 pu
        assert!((p_spec[1] - 0.183).abs() < 1e-6);
        // Q_inj = -12.7 MVar → q_spec = -0.127 pu
        assert!((q_spec[1] - (-0.127)).abs() < 1e-6);
    }

    #[test]
    fn test_ieee14_to_solver_input_ybus_size() {
        // Y-bus 矩阵应为 14x14
        let data = ieee14();
        let (ybus, _p, _q, _bus_types) = data.to_solver_input();
        assert_eq!(ybus.size(), 14);
        // 非零元数量应 > 0（至少有对角元素和支路元素）
        assert!(ybus.nnz() > 0);
    }

    #[test]
    fn test_ieee14_to_solver_input_base_mva() {
        let data = ieee14();
        let (ybus, _p, _q, _bus_types) = data.to_solver_input();
        assert_eq!(ybus.base_mva(), 100.0);
    }

    #[test]
    fn test_ieee14_to_solver_input_branch_ratings() {
        // 所有支路应有 rating_mva 设置
        let data = ieee14();
        let (ybus, _p, _q, _bus_types) = data.to_solver_input();
        for br in &data.branches {
            let from_idx = (br.from_bus - 1) as usize;
            let to_idx = (br.to_bus - 1) as usize;
            let rating = ybus.branch_rating_mva(from_idx, to_idx);
            assert!(rating.is_some(), "Branch {}->{} should have rating", br.from_bus, br.to_bus);
            assert_eq!(rating.unwrap(), br.rate_mva);
        }
    }

    #[test]
    fn test_ieee14_data_clone() {
        // Ieee14BusData 应可 Clone
        let data = ieee14();
        let cloned = data.clone();
        assert_eq!(cloned.buses.len(), data.buses.len());
        assert_eq!(cloned.branches.len(), data.branches.len());
        assert_eq!(cloned.base_mva, data.base_mva);
    }

    #[test]
    fn test_ieee14_bus_data_debug_format() {
        // Ieee14Bus 应可 Debug
        let data = ieee14();
        let debug_str = format!("{:?}", data.buses[0]);
        assert!(debug_str.contains("Ieee14Bus"));
        assert!(debug_str.contains("bus_id"));
    }
}

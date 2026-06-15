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

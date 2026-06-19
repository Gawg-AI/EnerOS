use std::collections::HashMap;

use async_trait::async_trait;
use eneros_core::ElementId;
use crate::collector::DataSource;

/// Simulated data source for IEEE 14-bus system
///
/// Provides realistic IEEE 14-bus data without real devices.
/// Used for testing and demonstration purposes.
/// Data is constant (no noise) — for noisy simulation, wrap with a noise-adding decorator.
pub struct SimulatedDataSource {
    data: HashMap<(ElementId, String), f64>,
}

impl SimulatedDataSource {
    /// Create a new SimulatedDataSource with IEEE 14-bus data
    pub fn new() -> Self {
        let mut data = HashMap::new();

        // IEEE 14-bus voltages (p.u.) — typical power flow results with small variations
        let bus_voltages: Vec<(ElementId, f64)> = vec![
            (1, 1.060), (2, 1.045), (3, 1.010), (4, 1.018),
            (5, 1.020), (6, 1.070), (7, 1.062), (8, 1.090),
            (9, 1.056), (10, 1.051), (11, 1.057), (12, 1.055),
            (13, 1.050), (14, 1.036),
        ];
        for (bus_id, v) in &bus_voltages {
            data.insert((*bus_id, "voltage_pu".to_string()), *v);
        }

        // IEEE 14-bus voltage angles (degrees)
        let bus_angles: Vec<(ElementId, f64)> = vec![
            (1, 0.0), (2, -4.98), (3, -12.73), (4, -10.31),
            (5, -8.78), (6, -14.22), (7, -13.37), (8, -13.36),
            (9, -14.94), (10, -15.10), (11, -14.79), (12, -15.07),
            (13, -15.16), (14, -16.04),
        ];
        for (bus_id, a) in &bus_angles {
            data.insert((*bus_id, "angle_deg".to_string()), *a);
        }

        // Generator outputs (MW)
        let gen_outputs: Vec<(ElementId, f64)> = vec![
            (1, 232.4), (2, 40.0), (3, 0.0), (6, 0.0), (8, 17.4),
        ];
        for (gen_id, p) in &gen_outputs {
            data.insert((*gen_id, "gen_p_mw".to_string()), *p);
        }

        // Generator reactive outputs (MVar)
        let gen_q: Vec<(ElementId, f64)> = vec![
            (1, -16.5), (2, 42.4), (3, 23.4), (6, 12.2), (8, 17.4),
        ];
        for (gen_id, q) in &gen_q {
            data.insert((*gen_id, "gen_q_mvar".to_string()), *q);
        }

        // Load consumption (MW) — buses with loads
        let load_p: Vec<(ElementId, f64)> = vec![
            (2, 21.7), (3, 94.2), (4, 47.8), (5, 7.6),
            (6, 11.2), (9, 29.5), (10, 9.0), (11, 3.5),
            (12, 6.1), (13, 13.5), (14, 14.9),
        ];
        for (load_id, p) in &load_p {
            data.insert((*load_id, "load_p_mw".to_string()), *p);
        }

        // Load reactive consumption (MVar)
        let load_q: Vec<(ElementId, f64)> = vec![
            (2, 12.7), (3, 19.0), (4, -3.9), (5, 1.6),
            (6, 7.5), (9, 16.6), (10, 5.8), (11, 1.8),
            (12, 1.6), (13, 5.8), (14, 5.0),
        ];
        for (load_id, q) in &load_q {
            data.insert((*load_id, "load_q_mvar".to_string()), *q);
        }

        // System frequency (Hz) — 50.0 Hz nominal (constant, no noise)
        data.insert((0, "frequency_hz".to_string()), 50.0);

        Self { data }
    }
}

#[async_trait]
impl DataSource for SimulatedDataSource {
    fn read(&self, element_id: ElementId, parameter: &str) -> Option<f64> {
        self.data.get(&(element_id, parameter.to_string())).copied()
    }

    // SimulatedDataSource uses the default no-op `refresh()` — values are
    // constant IEEE 14-bus data, no upstream device to poll.
}

impl Default for SimulatedDataSource {
    fn default() -> Self {
        Self::new()
    }
}

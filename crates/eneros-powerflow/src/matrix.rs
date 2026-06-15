use eneros_core::ElementId;
use std::collections::HashMap;

/// Y-Bus matrix for power flow calculation
#[derive(Clone)]
pub struct YBusMatrix {
    size: usize,
    data: Vec<Vec<(f64, f64)>>, // (G, B) pairs
    bus_map: HashMap<ElementId, usize>,
    base_mva: f64,
    branch_ratings_mva: HashMap<(usize, usize), f64>,
}

impl YBusMatrix {
    /// Create a new Y-Bus matrix
    pub fn new(size: usize) -> Self {
        Self {
            size,
            data: vec![vec![(0.0, 0.0); size]; size],
            bus_map: HashMap::new(),
            base_mva: 1.0,
            branch_ratings_mva: HashMap::new(),
        }
    }

    /// Set the bus index mapping
    pub fn set_bus_map(&mut self, bus_map: HashMap<ElementId, usize>) {
        self.bus_map = bus_map;
    }

    pub fn set_base_mva(&mut self, base_mva: f64) {
        if base_mva.is_finite() && base_mva > 0.0 {
            self.base_mva = base_mva;
        }
    }

    pub fn base_mva(&self) -> f64 {
        self.base_mva
    }

    pub fn set_branch_rating_mva(&mut self, from_idx: usize, to_idx: usize, rating_mva: f64) {
        if from_idx < self.size && to_idx < self.size && rating_mva.is_finite() && rating_mva > 0.0
        {
            let key = ordered_pair(from_idx, to_idx);
            self.branch_ratings_mva.insert(key, rating_mva);
        }
    }

    pub fn branch_rating_mva(&self, from_idx: usize, to_idx: usize) -> Option<f64> {
        self.branch_ratings_mva
            .get(&ordered_pair(from_idx, to_idx))
            .copied()
    }

    /// Get matrix element (G, B)
    pub fn get(&self, i: usize, j: usize) -> (f64, f64) {
        if i < self.size && j < self.size {
            self.data[i][j]
        } else {
            (0.0, 0.0)
        }
    }

    /// Set matrix element
    pub fn set(&mut self, i: usize, j: usize, g: f64, b: f64) {
        if i < self.size && j < self.size {
            self.data[i][j] = (g, b);
        }
    }

    /// Add to matrix element
    pub fn add(&mut self, i: usize, j: usize, g: f64, b: f64) {
        if i < self.size && j < self.size {
            self.data[i][j].0 += g;
            self.data[i][j].1 += b;
        }
    }

    /// Get matrix size
    pub fn size(&self) -> usize {
        self.size
    }

    /// Build Y-Bus from branch data with tap ratios
    /// branches: (from, to, r, x, b, tap_ratio)
    /// tap_ratio = 1.0 for normal lines, non-1.0 for transformers
    pub fn from_branches(
        branches: &[(ElementId, ElementId, f64, f64, f64, f64)],
        bus_map: &HashMap<ElementId, usize>,
    ) -> Self {
        let size = bus_map.len();
        let mut matrix = Self::new(size);
        matrix.set_bus_map(bus_map.clone());

        for &(from, to, r, x, b, tap) in branches {
            if let (Some(&i), Some(&j)) = (bus_map.get(&from), bus_map.get(&to)) {
                let z_sq = r * r + x * x;
                if z_sq > 1e-10 {
                    let g = r / z_sq;
                    let b_line = -x / z_sq;
                    let b_charging = b / 2.0;

                    if (tap - 1.0).abs() < 1e-10 {
                        // Normal line (tap = 1.0)
                        matrix.add(i, i, g, b_line + b_charging);
                        matrix.add(j, j, g, b_line + b_charging);
                        matrix.add(i, j, -g, -b_line);
                        matrix.add(j, i, -g, -b_line);
                    } else {
                        // Transformer with off-nominal tap ratio
                        // Y_ii += y / tap^2, Y_jj += y, Y_ij = Y_ji = -y / tap
                        let tap_sq = tap * tap;
                        matrix.add(i, i, g / tap_sq, (b_line + b_charging) / tap_sq);
                        matrix.add(j, j, g, b_line + b_charging);
                        matrix.add(i, j, -g / tap, -b_line / tap);
                        matrix.add(j, i, -g / tap, -b_line / tap);
                    }
                }
            }
        }

        matrix
    }

    /// Add shunt admittance to a bus diagonal element
    pub fn add_shunt(&mut self, bus_idx: usize, g: f64, b: f64) {
        self.add(bus_idx, bus_idx, g, b);
    }
}

fn ordered_pair(a: usize, b: usize) -> (usize, usize) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

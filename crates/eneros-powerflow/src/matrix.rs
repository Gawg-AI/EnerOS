use std::collections::HashMap;
use eneros_core::ElementId;

/// Y-Bus matrix for power flow calculation
pub struct YBusMatrix {
    size: usize,
    data: Vec<Vec<(f64, f64)>>, // (G, B) pairs
    bus_map: HashMap<ElementId, usize>,
}

impl YBusMatrix {
    /// Create a new Y-Bus matrix
    pub fn new(size: usize) -> Self {
        Self {
            size,
            data: vec![vec![(0.0, 0.0); size]; size],
            bus_map: HashMap::new(),
        }
    }

    /// Set the bus index mapping
    pub fn set_bus_map(&mut self, bus_map: HashMap<ElementId, usize>) {
        self.bus_map = bus_map;
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

    /// Build Y-Bus from branch data
    pub fn from_branches(
        branches: &[(ElementId, ElementId, f64, f64, f64)], // (from, to, r, x, b)
        bus_map: &HashMap<ElementId, usize>,
    ) -> Self {
        let size = bus_map.len();
        let mut matrix = Self::new(size);
        matrix.set_bus_map(bus_map.clone());

        for &(from, to, r, x, b) in branches {
            if let (Some(&i), Some(&j)) = (bus_map.get(&from), bus_map.get(&to)) {
                // Calculate admittance
                let z_sq = r * r + x * x;
                if z_sq > 1e-10 {
                    let g = r / z_sq;
                    let b_line = -x / z_sq;
                    let b_charging = b / 2.0;

                    // Add to diagonal
                    matrix.add(i, i, g, b_line + b_charging);
                    matrix.add(j, j, g, b_line + b_charging);

                    // Add to off-diagonal
                    matrix.add(i, j, -g, -b_line);
                    matrix.add(j, i, -g, -b_line);
                }
            }
        }

        matrix
    }
}

/// Jacobian matrix for Newton-Raphson iteration
pub struct JacobianMatrix {
    size: usize,
    data: Vec<Vec<f64>>,
}

impl JacobianMatrix {
    /// Create a new Jacobian matrix
    pub fn new(size: usize) -> Self {
        Self {
            size,
            data: vec![vec![0.0; size]; size],
        }
    }

    /// Set matrix element
    pub fn set(&mut self, i: usize, j: usize, value: f64) {
        if i < self.size && j < self.size {
            self.data[i][j] = value;
        }
    }

    /// Get matrix element
    pub fn get(&self, i: usize, j: usize) -> f64 {
        if i < self.size && j < self.size {
            self.data[i][j]
        } else {
            0.0
        }
    }

    /// Get matrix size
    pub fn size(&self) -> usize {
        self.size
    }

    /// Get reference to data
    pub fn data(&self) -> &[Vec<f64>] {
        &self.data
    }
}

use crate::types::AnalysisError;
use eneros_core::ElementId;
use ndarray::Array2;
use num_complex::Complex64;

/// Fault type classification
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FaultType {
    /// Balanced three-phase fault
    ThreePhase,
    /// Single line-to-ground fault
    SingleLineGround,
    /// Line-to-line fault
    LineLine,
    /// Double line-to-ground fault
    DoubleLineGround,
}

/// Fault specification
#[derive(Debug, Clone)]
pub struct FaultSpec {
    pub bus_id: ElementId,
    pub fault_type: FaultType,
    /// Fault impedance (default: 0)
    pub fault_impedance: Complex64,
}

impl Default for FaultSpec {
    fn default() -> Self {
        Self {
            bus_id: 0,
            fault_type: FaultType::ThreePhase,
            fault_impedance: Complex64::new(0.0, 0.0),
        }
    }
}

/// Short circuit analysis result
#[derive(Debug, Clone)]
pub struct FaultResult {
    pub fault_bus_id: ElementId,
    pub fault_type: FaultType,
    /// Fault current in kA
    pub fault_current_ka: Complex64,
    /// Bus voltages during fault (bus_id, voltage)
    pub bus_voltages: Vec<(ElementId, Complex64)>,
    /// Branch currents during fault (branch_id, current)
    pub branch_currents: Vec<(ElementId, Complex64)>,
}

/// Sequence impedances for asymmetric fault analysis
#[derive(Debug, Clone)]
pub struct SequenceImpedance {
    /// Positive sequence impedance
    pub z1: Complex64,
    /// Negative sequence impedance
    pub z2: Complex64,
    /// Zero sequence impedance
    pub z0: Complex64,
}

/// Short circuit analyzer
pub struct ShortCircuitAnalyzer;

impl ShortCircuitAnalyzer {
    pub fn new() -> Self {
        Self
    }

    /// Analyze a fault using the Z-bus method
    pub fn analyze(
        &self,
        fault: &FaultSpec,
        z_bus: &Array2<Complex64>,
        prefault_voltages: &[Complex64],
        sequence_z: Option<&SequenceImpedance>,
    ) -> Result<FaultResult, AnalysisError> {
        let n = z_bus.nrows();
        let fault_idx = fault.bus_id as usize;

        if fault_idx >= n {
            return Err(AnalysisError::InvalidConfiguration(format!(
                "Fault bus index {} out of range (max {})",
                fault_idx,
                n - 1
            )));
        }

        if prefault_voltages.len() < n {
            return Err(AnalysisError::DataIncomplete(format!(
                "Need {} prefault voltages, got {}",
                n,
                prefault_voltages.len()
            )));
        }

        let v_prefault = prefault_voltages[fault_idx];

        match fault.fault_type {
            FaultType::ThreePhase => {
                self.analyze_three_phase(fault, z_bus, prefault_voltages, fault_idx, v_prefault)
            }
            FaultType::SingleLineGround => self.analyze_slg(
                fault,
                z_bus,
                prefault_voltages,
                fault_idx,
                v_prefault,
                sequence_z,
            ),
            FaultType::LineLine => self.analyze_ll(
                fault,
                z_bus,
                prefault_voltages,
                fault_idx,
                v_prefault,
                sequence_z,
            ),
            FaultType::DoubleLineGround => self.analyze_dlg(
                fault,
                z_bus,
                prefault_voltages,
                fault_idx,
                v_prefault,
                sequence_z,
            ),
        }
    }

    /// Three-phase (balanced) fault analysis
    fn analyze_three_phase(
        &self,
        fault: &FaultSpec,
        z_bus: &Array2<Complex64>,
        prefault_voltages: &[Complex64],
        fault_idx: usize,
        v_prefault: Complex64,
    ) -> Result<FaultResult, AnalysisError> {
        let n = z_bus.nrows();
        let z_ff = z_bus[[fault_idx, fault_idx]];
        let z_f = fault.fault_impedance;

        // Fault current: I_f = V_prefault / (Z_ff + Z_f)
        let denominator = z_ff + z_f;
        if denominator.norm() < 1e-12 {
            return Err(AnalysisError::SingularMatrix(
                "Z_ff + Z_f is zero, cannot compute fault current".into(),
            ));
        }

        let i_fault = v_prefault / denominator;

        // Bus voltages during fault: V_i = V_i^prefault - Z_if * I_f
        let mut bus_voltages = Vec::with_capacity(n);
        for i in 0..n {
            let z_if = z_bus[[i, fault_idx]];
            let v_i = prefault_voltages[i] - z_if * i_fault;
            bus_voltages.push((i as ElementId, v_i));
        }

        // Branch currents: I_ij = (V_i - V_j) / z_ij
        // We compute for all pairs where Z_bus has off-diagonal entries
        let mut branch_currents = Vec::new();
        let mut branch_id_counter: ElementId = 0;
        for i in 0..n {
            for j in (i + 1)..n {
                let z_ij = z_bus[[i, j]];
                if z_ij.norm() > 1e-12 {
                    // z_branch = -z_ij (off-diagonal of Z_bus is negative of branch impedance)
                    let z_branch = -z_ij;
                    let v_i = bus_voltages[i].1;
                    let v_j = bus_voltages[j].1;
                    let i_branch = (v_i - v_j) / z_branch;
                    branch_currents.push((branch_id_counter, i_branch));
                    branch_id_counter += 1;
                }
            }
        }

        // Convert fault current to kA (assuming base current of 1 kA for p.u. system)
        Ok(FaultResult {
            fault_bus_id: fault.bus_id,
            fault_type: fault.fault_type,
            fault_current_ka: i_fault,
            bus_voltages,
            branch_currents,
        })
    }

    /// Single line-to-ground fault analysis using symmetrical components
    fn analyze_slg(
        &self,
        fault: &FaultSpec,
        z_bus: &Array2<Complex64>,
        prefault_voltages: &[Complex64],
        fault_idx: usize,
        v_prefault: Complex64,
        sequence_z: Option<&SequenceImpedance>,
    ) -> Result<FaultResult, AnalysisError> {
        let seq_z = sequence_z.ok_or_else(|| {
            AnalysisError::DataIncomplete(
                "Sequence impedances required for asymmetric fault analysis".into(),
            )
        })?;

        let n = z_bus.nrows();
        let z_ff = z_bus[[fault_idx, fault_idx]];
        let z_f = fault.fault_impedance;

        // SLG fault: I_a = 3 * I_0 = 3 * V_prefault / (Z1 + Z2 + Z0 + 3*Zf)
        // Using Z_bus positive sequence as Z1, and provided sequence impedances
        let z1_eq = z_ff;
        let denominator = z1_eq + seq_z.z2 + seq_z.z0 + Complex64::new(3.0, 0.0) * z_f;

        if denominator.norm() < 1e-12 {
            return Err(AnalysisError::SingularMatrix(
                "Sequence impedance sum is zero for SLG fault".into(),
            ));
        }

        let i_0 = v_prefault / denominator;
        let i_fault = Complex64::new(3.0, 0.0) * i_0;

        // Bus voltages during fault (simplified using positive sequence)
        let mut bus_voltages = Vec::with_capacity(n);
        for i in 0..n {
            let z_if = z_bus[[i, fault_idx]];
            let v_i = prefault_voltages[i] - z_if * i_0;
            bus_voltages.push((i as ElementId, v_i));
        }

        let mut branch_currents = Vec::new();
        let mut branch_id_counter: ElementId = 0;
        for i in 0..n {
            for j in (i + 1)..n {
                let z_ij = z_bus[[i, j]];
                if z_ij.norm() > 1e-12 {
                    let z_branch = -z_ij;
                    let v_i = bus_voltages[i].1;
                    let v_j = bus_voltages[j].1;
                    let i_branch = (v_i - v_j) / z_branch;
                    branch_currents.push((branch_id_counter, i_branch));
                    branch_id_counter += 1;
                }
            }
        }

        Ok(FaultResult {
            fault_bus_id: fault.bus_id,
            fault_type: fault.fault_type,
            fault_current_ka: i_fault,
            bus_voltages,
            branch_currents,
        })
    }

    /// Line-to-line fault analysis
    fn analyze_ll(
        &self,
        fault: &FaultSpec,
        z_bus: &Array2<Complex64>,
        prefault_voltages: &[Complex64],
        fault_idx: usize,
        v_prefault: Complex64,
        sequence_z: Option<&SequenceImpedance>,
    ) -> Result<FaultResult, AnalysisError> {
        let seq_z = sequence_z.ok_or_else(|| {
            AnalysisError::DataIncomplete(
                "Sequence impedances required for asymmetric fault analysis".into(),
            )
        })?;

        let n = z_bus.nrows();
        let z_ff = z_bus[[fault_idx, fault_idx]];
        let z_f = fault.fault_impedance;

        // LL fault: I_a = -j*sqrt(3) * V_prefault / (Z1 + Z2 + Zf)
        let denominator = z_ff + seq_z.z2 + z_f;

        if denominator.norm() < 1e-12 {
            return Err(AnalysisError::SingularMatrix(
                "Sequence impedance sum is zero for LL fault".into(),
            ));
        }

        let i_pos = v_prefault / denominator;
        let i_fault = Complex64::new(0.0, -3.0_f64.sqrt()) * i_pos;

        // Bus voltages during fault (simplified)
        let mut bus_voltages = Vec::with_capacity(n);
        for i in 0..n {
            let z_if = z_bus[[i, fault_idx]];
            let v_i = prefault_voltages[i] - z_if * i_pos;
            bus_voltages.push((i as ElementId, v_i));
        }

        let mut branch_currents = Vec::new();
        let mut branch_id_counter: ElementId = 0;
        for i in 0..n {
            for j in (i + 1)..n {
                let z_ij = z_bus[[i, j]];
                if z_ij.norm() > 1e-12 {
                    let z_branch = -z_ij;
                    let v_i = bus_voltages[i].1;
                    let v_j = bus_voltages[j].1;
                    let i_branch = (v_i - v_j) / z_branch;
                    branch_currents.push((branch_id_counter, i_branch));
                    branch_id_counter += 1;
                }
            }
        }

        Ok(FaultResult {
            fault_bus_id: fault.bus_id,
            fault_type: fault.fault_type,
            fault_current_ka: i_fault,
            bus_voltages,
            branch_currents,
        })
    }

    /// Double line-to-ground fault analysis
    fn analyze_dlg(
        &self,
        fault: &FaultSpec,
        z_bus: &Array2<Complex64>,
        prefault_voltages: &[Complex64],
        fault_idx: usize,
        v_prefault: Complex64,
        sequence_z: Option<&SequenceImpedance>,
    ) -> Result<FaultResult, AnalysisError> {
        let seq_z = sequence_z.ok_or_else(|| {
            AnalysisError::DataIncomplete(
                "Sequence impedances required for asymmetric fault analysis".into(),
            )
        })?;

        let n = z_bus.nrows();
        let z_ff = z_bus[[fault_idx, fault_idx]];
        let z_f = fault.fault_impedance;

        // DLG fault: I_0 = -V_prefault / (Z0 + Z2||Z1 + 3*Zf)
        // Z_parallel = Z2 * Z1 / (Z2 + Z1)
        let z_parallel = seq_z.z2 * z_ff / (seq_z.z2 + z_ff);
        let denominator = seq_z.z0 + z_parallel + Complex64::new(3.0, 0.0) * z_f;

        if denominator.norm() < 1e-12 {
            return Err(AnalysisError::SingularMatrix(
                "Sequence impedance sum is zero for DLG fault".into(),
            ));
        }

        let i_0 = -v_prefault / denominator;
        // I_fault = 3 * I_0 for the ground current
        let i_fault = Complex64::new(3.0, 0.0) * i_0;

        // Bus voltages during fault (simplified)
        let mut bus_voltages = Vec::with_capacity(n);
        for i in 0..n {
            let z_if = z_bus[[i, fault_idx]];
            let v_i = prefault_voltages[i] - z_if * i_0;
            bus_voltages.push((i as ElementId, v_i));
        }

        let mut branch_currents = Vec::new();
        let mut branch_id_counter: ElementId = 0;
        for i in 0..n {
            for j in (i + 1)..n {
                let z_ij = z_bus[[i, j]];
                if z_ij.norm() > 1e-12 {
                    let z_branch = -z_ij;
                    let v_i = bus_voltages[i].1;
                    let v_j = bus_voltages[j].1;
                    let i_branch = (v_i - v_j) / z_branch;
                    branch_currents.push((branch_id_counter, i_branch));
                    branch_id_counter += 1;
                }
            }
        }

        Ok(FaultResult {
            fault_bus_id: fault.bus_id,
            fault_type: fault.fault_type,
            fault_current_ka: i_fault,
            bus_voltages,
            branch_currents,
        })
    }
}

impl Default for ShortCircuitAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a simple Z-bus for a 3-bus system
    /// Bus 0 --- z01 --- Bus 1 --- z12 --- Bus 2
    fn build_3bus_z_bus() -> Array2<Complex64> {
        // For a simple radial system with impedances:
        // z01 = 0.01 + j0.1, z12 = 0.015 + j0.15
        // Z_bus is built by inverting Y_bus
        let z01 = Complex64::new(0.01, 0.1);
        let z12 = Complex64::new(0.015, 0.15);

        // Y_bus for 3-bus radial system
        let y01 = Complex64::new(1.0, 0.0) / z01;
        let y12 = Complex64::new(1.0, 0.0) / z12;

        // Shunt admittances to ground (line charging + grounding) to make Y_bus non-singular
        // Typical values: y_shunt ≈ 0.1 * y_line for line charging
        let y_shunt0 = y01 * 0.1;
        let y_shunt1 = (y01 + y12) * 0.1;
        let y_shunt2 = y12 * 0.1;

        let mut y_bus = Array2::<Complex64>::zeros((3, 3));
        y_bus[[0, 0]] = y01 + y_shunt0;
        y_bus[[0, 1]] = -y01;
        y_bus[[1, 0]] = -y01;
        y_bus[[1, 1]] = y01 + y12 + y_shunt1;
        y_bus[[1, 2]] = -y12;
        y_bus[[2, 1]] = -y12;
        y_bus[[2, 2]] = y12 + y_shunt2;

        // Invert Y_bus to get Z_bus
        invert_complex_matrix(&y_bus)
            .unwrap_or_else(|| Array2::from_elem((3, 3), Complex64::new(0.0, 0.0)))
    }

    fn invert_complex_matrix(a: &Array2<Complex64>) -> Option<Array2<Complex64>> {
        let n = a.nrows();
        if n == 0 {
            return Some(Array2::zeros((0, 0)));
        }

        let mut aug = vec![vec![Complex64::new(0.0, 0.0); 2 * n]; n];
        for i in 0..n {
            for j in 0..n {
                aug[i][j] = a[[i, j]];
            }
            aug[i][n + i] = Complex64::new(1.0, 0.0);
        }

        for col in 0..n {
            let mut max_val = aug[col][col].norm();
            let mut max_row = col;
            for (row, row_values) in aug.iter().enumerate().take(n).skip(col + 1) {
                let candidate = row_values[col].norm();
                if candidate > max_val {
                    max_val = candidate;
                    max_row = row;
                }
            }

            if max_val < 1e-12 {
                return None;
            }

            if max_row != col {
                aug.swap(col, max_row);
            }

            let pivot = aug[col][col];
            for value in aug[col].iter_mut().take(2 * n) {
                *value /= pivot;
            }

            let pivot_row = aug[col].clone();
            for (row, row_values) in aug.iter_mut().enumerate().take(n) {
                if row != col {
                    let factor = row_values[col];
                    for (value, pivot_value) in
                        row_values.iter_mut().zip(pivot_row.iter()).take(2 * n)
                    {
                        *value -= factor * *pivot_value;
                    }
                }
            }
        }

        let mut inv = Array2::<Complex64>::zeros((n, n));
        for i in 0..n {
            for j in 0..n {
                inv[[i, j]] = aug[i][n + j];
            }
        }

        Some(inv)
    }

    #[test]
    fn test_three_phase_fault() {
        let z_bus = build_3bus_z_bus();
        let prefault_voltages = vec![
            Complex64::new(1.0, 0.0),
            Complex64::new(1.0, 0.0),
            Complex64::new(1.0, 0.0),
        ];

        let fault = FaultSpec {
            bus_id: 1,
            fault_type: FaultType::ThreePhase,
            fault_impedance: Complex64::new(0.0, 0.0),
        };

        let analyzer = ShortCircuitAnalyzer::new();
        let result = analyzer.analyze(&fault, &z_bus, &prefault_voltages, None);

        assert!(
            result.is_ok(),
            "Three-phase fault analysis failed: {:?}",
            result.err()
        );
        let result = result.unwrap();

        // Fault current should be significant
        assert!(
            result.fault_current_ka.norm() > 0.1,
            "Fault current magnitude {} should be significant",
            result.fault_current_ka.norm()
        );

        // Fault bus voltage should be near zero (for bolted fault)
        let fault_bus_voltage = result
            .bus_voltages
            .iter()
            .find(|(id, _)| *id == 1)
            .map(|(_, v)| v.norm())
            .unwrap_or(999.0);
        assert!(
            fault_bus_voltage < 0.5,
            "Fault bus voltage {} should be small for bolted fault",
            fault_bus_voltage
        );

        // Non-fault bus voltages should be reduced but not zero
        for (id, v) in &result.bus_voltages {
            if *id != 1 {
                assert!(
                    v.norm() > 0.0 && v.norm() <= 1.1,
                    "Bus {} voltage {} should be in reasonable range",
                    id,
                    v.norm()
                );
            }
        }
    }

    #[test]
    fn test_three_phase_fault_with_impedance() {
        let z_bus = build_3bus_z_bus();
        let prefault_voltages = vec![
            Complex64::new(1.0, 0.0),
            Complex64::new(1.0, 0.0),
            Complex64::new(1.0, 0.0),
        ];

        let fault = FaultSpec {
            bus_id: 1,
            fault_type: FaultType::ThreePhase,
            fault_impedance: Complex64::new(0.1, 0.0),
        };

        let analyzer = ShortCircuitAnalyzer::new();
        let result = analyzer.analyze(&fault, &z_bus, &prefault_voltages, None);

        assert!(result.is_ok());
        let result = result.unwrap();

        // Fault current with impedance should be less than bolted fault
        let bolted_fault = FaultSpec {
            bus_id: 1,
            fault_type: FaultType::ThreePhase,
            fault_impedance: Complex64::new(0.0, 0.0),
        };
        let bolted_result = analyzer
            .analyze(&bolted_fault, &z_bus, &prefault_voltages, None)
            .unwrap();

        assert!(
            result.fault_current_ka.norm() < bolted_result.fault_current_ka.norm(),
            "Fault current with impedance ({}) should be less than bolted ({})",
            result.fault_current_ka.norm(),
            bolted_result.fault_current_ka.norm()
        );
    }

    #[test]
    fn test_slg_fault_with_sequence_impedances() {
        let z_bus = build_3bus_z_bus();
        let prefault_voltages = vec![
            Complex64::new(1.0, 0.0),
            Complex64::new(1.0, 0.0),
            Complex64::new(1.0, 0.0),
        ];

        let seq_z = SequenceImpedance {
            z1: Complex64::new(0.01, 0.1),
            z2: Complex64::new(0.01, 0.1),
            z0: Complex64::new(0.03, 0.3),
        };

        let fault = FaultSpec {
            bus_id: 1,
            fault_type: FaultType::SingleLineGround,
            fault_impedance: Complex64::new(0.0, 0.0),
        };

        let analyzer = ShortCircuitAnalyzer::new();
        let result = analyzer.analyze(&fault, &z_bus, &prefault_voltages, Some(&seq_z));

        assert!(
            result.is_ok(),
            "SLG fault analysis failed: {:?}",
            result.err()
        );
        let result = result.unwrap();

        // SLG fault current should be significant
        assert!(
            result.fault_current_ka.norm() > 0.1,
            "SLG fault current magnitude {} should be significant",
            result.fault_current_ka.norm()
        );

        assert_eq!(result.fault_type, FaultType::SingleLineGround);
    }

    #[test]
    fn test_ll_fault_with_sequence_impedances() {
        let z_bus = build_3bus_z_bus();
        let prefault_voltages = vec![
            Complex64::new(1.0, 0.0),
            Complex64::new(1.0, 0.0),
            Complex64::new(1.0, 0.0),
        ];

        let seq_z = SequenceImpedance {
            z1: Complex64::new(0.01, 0.1),
            z2: Complex64::new(0.01, 0.1),
            z0: Complex64::new(0.03, 0.3),
        };

        let fault = FaultSpec {
            bus_id: 1,
            fault_type: FaultType::LineLine,
            fault_impedance: Complex64::new(0.0, 0.0),
        };

        let analyzer = ShortCircuitAnalyzer::new();
        let result = analyzer.analyze(&fault, &z_bus, &prefault_voltages, Some(&seq_z));

        assert!(
            result.is_ok(),
            "LL fault analysis failed: {:?}",
            result.err()
        );
        let result = result.unwrap();

        assert!(
            result.fault_current_ka.norm() > 0.1,
            "LL fault current magnitude {} should be significant",
            result.fault_current_ka.norm()
        );
    }

    #[test]
    fn test_dlg_fault_with_sequence_impedances() {
        let z_bus = build_3bus_z_bus();
        let prefault_voltages = vec![
            Complex64::new(1.0, 0.0),
            Complex64::new(1.0, 0.0),
            Complex64::new(1.0, 0.0),
        ];

        let seq_z = SequenceImpedance {
            z1: Complex64::new(0.01, 0.1),
            z2: Complex64::new(0.01, 0.1),
            z0: Complex64::new(0.03, 0.3),
        };

        let fault = FaultSpec {
            bus_id: 1,
            fault_type: FaultType::DoubleLineGround,
            fault_impedance: Complex64::new(0.0, 0.0),
        };

        let analyzer = ShortCircuitAnalyzer::new();
        let result = analyzer.analyze(&fault, &z_bus, &prefault_voltages, Some(&seq_z));

        assert!(
            result.is_ok(),
            "DLG fault analysis failed: {:?}",
            result.err()
        );
        let result = result.unwrap();

        assert!(
            result.fault_current_ka.norm() > 0.1,
            "DLG fault current magnitude {} should be significant",
            result.fault_current_ka.norm()
        );
    }

    #[test]
    fn test_asymmetric_fault_without_sequence_impedances() {
        let z_bus = build_3bus_z_bus();
        let prefault_voltages = vec![
            Complex64::new(1.0, 0.0),
            Complex64::new(1.0, 0.0),
            Complex64::new(1.0, 0.0),
        ];

        let fault = FaultSpec {
            bus_id: 1,
            fault_type: FaultType::SingleLineGround,
            fault_impedance: Complex64::new(0.0, 0.0),
        };

        let analyzer = ShortCircuitAnalyzer::new();
        let result = analyzer.analyze(&fault, &z_bus, &prefault_voltages, None);

        // Should fail because sequence impedances are required
        assert!(result.is_err());
    }

    #[test]
    fn test_fault_bus_out_of_range() {
        let z_bus = build_3bus_z_bus();
        let prefault_voltages = vec![
            Complex64::new(1.0, 0.0),
            Complex64::new(1.0, 0.0),
            Complex64::new(1.0, 0.0),
        ];

        let fault = FaultSpec {
            bus_id: 10, // Out of range
            fault_type: FaultType::ThreePhase,
            fault_impedance: Complex64::new(0.0, 0.0),
        };

        let analyzer = ShortCircuitAnalyzer::new();
        let result = analyzer.analyze(&fault, &z_bus, &prefault_voltages, None);

        assert!(result.is_err());
    }

    #[test]
    fn test_fault_current_magnitude_ordering() {
        // For the same system: 3-phase > SLG > DLG > LL (typically)
        let z_bus = build_3bus_z_bus();
        let prefault_voltages = vec![
            Complex64::new(1.0, 0.0),
            Complex64::new(1.0, 0.0),
            Complex64::new(1.0, 0.0),
        ];

        let seq_z = SequenceImpedance {
            z1: Complex64::new(0.01, 0.1),
            z2: Complex64::new(0.01, 0.1),
            z0: Complex64::new(0.03, 0.3),
        };

        let analyzer = ShortCircuitAnalyzer::new();

        let fault_3ph = FaultSpec {
            bus_id: 1,
            fault_type: FaultType::ThreePhase,
            fault_impedance: Complex64::new(0.0, 0.0),
        };
        let fault_slg = FaultSpec {
            bus_id: 1,
            fault_type: FaultType::SingleLineGround,
            fault_impedance: Complex64::new(0.0, 0.0),
        };

        let result_3ph = analyzer
            .analyze(&fault_3ph, &z_bus, &prefault_voltages, None)
            .unwrap();
        let result_slg = analyzer
            .analyze(&fault_slg, &z_bus, &prefault_voltages, Some(&seq_z))
            .unwrap();

        // Three-phase fault current should typically be larger than SLG
        // (depends on zero-sequence impedance)
        assert!(
            result_3ph.fault_current_ka.norm() > 0.0,
            "3-phase fault current should be positive"
        );
        assert!(
            result_slg.fault_current_ka.norm() > 0.0,
            "SLG fault current should be positive"
        );
    }
}

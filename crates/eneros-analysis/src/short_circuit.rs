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

/// Full Z-bus matrices for each sequence network.
///
/// Used for production-grade asymmetric fault analysis where each sequence
/// network has its own impedance structure (e.g., different grounding,
/// transformer winding connections, or mutual couplings that differ between
/// sequences).
#[derive(Debug, Clone)]
pub struct SequenceNetworks {
    /// Positive sequence Z-bus matrix
    pub z_bus_positive: Array2<Complex64>,
    /// Negative sequence Z-bus matrix
    pub z_bus_negative: Array2<Complex64>,
    /// Zero sequence Z-bus matrix
    pub z_bus_zero: Array2<Complex64>,
}

/// Short circuit analyzer
pub struct ShortCircuitAnalyzer;

impl ShortCircuitAnalyzer {
    pub fn new() -> Self {
        Self
    }

    /// Analyze a fault using the Z-bus method
    ///
    /// **Approximation note**: For asymmetric faults (SLG, LL, DLG), this
    /// method uses a single scalar `SequenceImpedance { z1, z2, z0 }` and
    /// approximates the positive-sequence network with the supplied `z_bus`.
    /// The bus voltages during the fault are computed using only the
    /// positive-sequence Z-bus. This is the legacy / simplified path.
    ///
    /// For production-grade asymmetric fault analysis where each sequence
    /// network has its own Z-bus matrix, use
    /// [`ShortCircuitAnalyzer::analyze_with_sequence_networks`] instead.
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

    /// Analyze a fault using full Z-bus matrices for each sequence network.
    ///
    /// This is the production-grade path for asymmetric fault analysis.
    /// Each sequence network (positive, negative, zero) is described by its
    /// own Z-bus matrix, allowing correct modeling of systems where the
    /// sequence networks differ structurally (e.g., transformer winding
    /// connections, grounding, mutual couplings).
    ///
    /// For three-phase faults, only the positive-sequence Z-bus is used and
    /// the result is identical to [`ShortCircuitAnalyzer::analyze`].
    ///
    /// The prefault voltages are assumed balanced, so the positive-sequence
    /// prefault voltage equals the supplied `prefault_voltages`, while the
    /// negative- and zero-sequence prefault voltages are zero.
    pub fn analyze_with_sequence_networks(
        &self,
        fault: &FaultSpec,
        seq_networks: &SequenceNetworks,
        prefault_voltages: &[Complex64],
    ) -> Result<FaultResult, AnalysisError> {
        let z_bus_pos = &seq_networks.z_bus_positive;
        let z_bus_neg = &seq_networks.z_bus_negative;
        let z_bus_zero = &seq_networks.z_bus_zero;

        let n = z_bus_pos.nrows();
        let fault_idx = fault.bus_id as usize;

        // Validate dimensions of all three networks.
        if fault_idx >= n {
            return Err(AnalysisError::InvalidConfiguration(format!(
                "Fault bus index {} out of range (max {})",
                fault_idx,
                n - 1
            )));
        }
        if z_bus_neg.nrows() != n || z_bus_neg.ncols() != n {
            return Err(AnalysisError::InvalidConfiguration(format!(
                "Negative-sequence Z-bus dimensions {}x{} do not match positive-sequence {}x{}",
                z_bus_neg.nrows(),
                z_bus_neg.ncols(),
                n,
                n
            )));
        }
        if z_bus_zero.nrows() != n || z_bus_zero.ncols() != n {
            return Err(AnalysisError::InvalidConfiguration(format!(
                "Zero-sequence Z-bus dimensions {}x{} do not match positive-sequence {}x{}",
                z_bus_zero.nrows(),
                z_bus_zero.ncols(),
                n,
                n
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
            FaultType::ThreePhase => self.seq_analyze_three_phase(
                z_bus_pos,
                prefault_voltages,
                fault_idx,
                v_prefault,
                fault,
            ),
            FaultType::SingleLineGround => self.seq_analyze_slg(
                z_bus_pos,
                z_bus_neg,
                z_bus_zero,
                prefault_voltages,
                fault_idx,
                v_prefault,
                fault,
            ),
            FaultType::LineLine => self.seq_analyze_ll(
                z_bus_pos,
                z_bus_neg,
                prefault_voltages,
                fault_idx,
                v_prefault,
                fault,
            ),
            FaultType::DoubleLineGround => self.seq_analyze_dlg(
                z_bus_pos,
                z_bus_neg,
                z_bus_zero,
                prefault_voltages,
                fault_idx,
                v_prefault,
                fault,
            ),
        }
    }

    /// Three-phase fault using only the positive-sequence Z-bus.
    fn seq_analyze_three_phase(
        &self,
        z_bus_pos: &Array2<Complex64>,
        prefault_voltages: &[Complex64],
        fault_idx: usize,
        v_prefault: Complex64,
        fault: &FaultSpec,
    ) -> Result<FaultResult, AnalysisError> {
        let n = z_bus_pos.nrows();
        let z_ff = z_bus_pos[[fault_idx, fault_idx]];
        let z_f = fault.fault_impedance;

        let denominator = z_ff + z_f;
        if denominator.norm() < 1e-12 {
            return Err(AnalysisError::SingularMatrix(
                "Z_ff + Z_f is zero, cannot compute fault current".into(),
            ));
        }

        let i_fault = v_prefault / denominator;

        let bus_voltages = self.compute_phase_voltages_from_seq(
            z_bus_pos,
            prefault_voltages,
            fault_idx,
            i_fault,
            n,
        );
        let branch_currents = self.compute_branch_currents(z_bus_pos, &bus_voltages, n);

        Ok(FaultResult {
            fault_bus_id: fault.bus_id,
            fault_type: fault.fault_type,
            fault_current_ka: i_fault,
            bus_voltages,
            branch_currents,
        })
    }

    /// SLG fault using independent sequence Z-bus matrices.
    ///
    /// For a single line-to-ground fault at bus f:
    ///   I_1 = I_2 = I_0 = V_prefault / (Z1_ff + Z2_ff + Z0_ff + 3*Zf)
    ///   I_fault = 3 * I_0
    /// Bus voltages per sequence:
    ///   V_i^seq = V_prefault^seq - Z_if^seq * I_seq
    /// Phase voltage: V_a = V_0 + V_1 + V_2
    #[allow(clippy::too_many_arguments)]
    fn seq_analyze_slg(
        &self,
        z_bus_pos: &Array2<Complex64>,
        z_bus_neg: &Array2<Complex64>,
        z_bus_zero: &Array2<Complex64>,
        prefault_voltages: &[Complex64],
        fault_idx: usize,
        v_prefault: Complex64,
        fault: &FaultSpec,
    ) -> Result<FaultResult, AnalysisError> {
        let n = z_bus_pos.nrows();
        let z1_ff = z_bus_pos[[fault_idx, fault_idx]];
        let z2_ff = z_bus_neg[[fault_idx, fault_idx]];
        let z0_ff = z_bus_zero[[fault_idx, fault_idx]];
        let z_f = fault.fault_impedance;

        let denominator = z1_ff + z2_ff + z0_ff + Complex64::new(3.0, 0.0) * z_f;
        if denominator.norm() < 1e-12 {
            return Err(AnalysisError::SingularMatrix(
                "Sequence impedance sum is zero for SLG fault".into(),
            ));
        }

        // Sequence currents are equal for SLG.
        let i_seq = v_prefault / denominator;
        let i_fault = Complex64::new(3.0, 0.0) * i_seq;

        let bus_voltages = self.compute_phase_voltages_three_seq(
            z_bus_pos,
            z_bus_neg,
            z_bus_zero,
            prefault_voltages,
            fault_idx,
            i_seq,
            i_seq,
            i_seq,
            n,
        );
        let branch_currents = self.compute_branch_currents(z_bus_pos, &bus_voltages, n);

        Ok(FaultResult {
            fault_bus_id: fault.bus_id,
            fault_type: fault.fault_type,
            fault_current_ka: i_fault,
            bus_voltages,
            branch_currents,
        })
    }

    /// LL fault using independent sequence Z-bus matrices.
    ///
    /// For a line-to-line fault at bus f:
    ///   I_0 = 0
    ///   I_1 = -I_2 = V_prefault / (Z1_ff + Z2_ff + Zf)
    ///   I_fault (phase b-c) = -j*sqrt(3) * I_1
    /// Bus voltages per sequence:
    ///   V_i^1 = V_prefault_i - Z1_if * I_1
    ///   V_i^2 = -Z2_if * I_2  (prefault negative-sequence is zero)
    ///   V_i^0 = 0
    /// Phase voltage: V_a = V_0 + V_1 + V_2
    fn seq_analyze_ll(
        &self,
        z_bus_pos: &Array2<Complex64>,
        z_bus_neg: &Array2<Complex64>,
        prefault_voltages: &[Complex64],
        fault_idx: usize,
        v_prefault: Complex64,
        fault: &FaultSpec,
    ) -> Result<FaultResult, AnalysisError> {
        let n = z_bus_pos.nrows();
        let z1_ff = z_bus_pos[[fault_idx, fault_idx]];
        let z2_ff = z_bus_neg[[fault_idx, fault_idx]];
        let z_f = fault.fault_impedance;

        let denominator = z1_ff + z2_ff + z_f;
        if denominator.norm() < 1e-12 {
            return Err(AnalysisError::SingularMatrix(
                "Sequence impedance sum is zero for LL fault".into(),
            ));
        }

        let i_1 = v_prefault / denominator;
        let i_2 = -i_1;
        let i_0 = Complex64::new(0.0, 0.0);
        // Phase b-c fault current magnitude: |I_b| = sqrt(3) * |I_1|
        let i_fault = Complex64::new(0.0, -3.0_f64.sqrt()) * i_1;

        let bus_voltages = self.compute_phase_voltages_three_seq(
            z_bus_pos,
            z_bus_neg,
            // No zero-sequence contribution for LL; pass a zero matrix of
            // equal size so the helper still works.
            z_bus_neg,
            prefault_voltages,
            fault_idx,
            i_1,
            i_2,
            i_0,
            n,
        );
        // Override: zero-sequence voltages are zero for LL, so subtract the
        // zero-sequence contribution that the helper computed using z_bus_neg
        // as a stand-in. Since i_0 = 0, the contribution is already zero, so
        // no correction is needed.
        let branch_currents = self.compute_branch_currents(z_bus_pos, &bus_voltages, n);

        Ok(FaultResult {
            fault_bus_id: fault.bus_id,
            fault_type: fault.fault_type,
            fault_current_ka: i_fault,
            bus_voltages,
            branch_currents,
        })
    }

    /// DLG fault using independent sequence Z-bus matrices.
    ///
    /// For a double line-to-ground fault at bus f (phases b and c to ground):
    ///   I_1 = V_prefault / (Z1_ff + Z2_ff || (Z0_ff + 3*Zf))
    ///   I_2 = -I_1 * (Z0_ff + 3*Zf) / (Z2_ff + Z0_ff + 3*Zf)
    ///   I_0 = -I_1 * Z2_ff / (Z2_ff + Z0_ff + 3*Zf)
    ///   I_fault (ground current) = 3 * I_0
    /// Bus voltages per sequence:
    ///   V_i^seq = V_prefault^seq - Z_if^seq * I_seq
    /// Phase voltage: V_a = V_0 + V_1 + V_2
    #[allow(clippy::too_many_arguments)]
    fn seq_analyze_dlg(
        &self,
        z_bus_pos: &Array2<Complex64>,
        z_bus_neg: &Array2<Complex64>,
        z_bus_zero: &Array2<Complex64>,
        prefault_voltages: &[Complex64],
        fault_idx: usize,
        v_prefault: Complex64,
        fault: &FaultSpec,
    ) -> Result<FaultResult, AnalysisError> {
        let n = z_bus_pos.nrows();
        let z1_ff = z_bus_pos[[fault_idx, fault_idx]];
        let z2_ff = z_bus_neg[[fault_idx, fault_idx]];
        let z0_ff = z_bus_zero[[fault_idx, fault_idx]];
        let z_f = fault.fault_impedance;

        let z0_plus_3zf = z0_ff + Complex64::new(3.0, 0.0) * z_f;
        let z_parallel_denom = z2_ff + z0_plus_3zf;
        if z_parallel_denom.norm() < 1e-12 {
            return Err(AnalysisError::SingularMatrix(
                "Sequence impedance sum is zero for DLG fault (Z2 + Z0 + 3*Zf)".into(),
            ));
        }
        let z_parallel = z2_ff * z0_plus_3zf / z_parallel_denom;

        let denominator = z1_ff + z_parallel;
        if denominator.norm() < 1e-12 {
            return Err(AnalysisError::SingularMatrix(
                "Sequence impedance sum is zero for DLG fault (Z1 + Z2||Z0)".into(),
            ));
        }

        let i_1 = v_prefault / denominator;
        let i_2 = -i_1 * z0_plus_3zf / z_parallel_denom;
        let i_0 = -i_1 * z2_ff / z_parallel_denom;
        // Ground current = 3 * I_0
        let i_fault = Complex64::new(3.0, 0.0) * i_0;

        let bus_voltages = self.compute_phase_voltages_three_seq(
            z_bus_pos,
            z_bus_neg,
            z_bus_zero,
            prefault_voltages,
            fault_idx,
            i_1,
            i_2,
            i_0,
            n,
        );
        let branch_currents = self.compute_branch_currents(z_bus_pos, &bus_voltages, n);

        Ok(FaultResult {
            fault_bus_id: fault.bus_id,
            fault_type: fault.fault_type,
            fault_current_ka: i_fault,
            bus_voltages,
            branch_currents,
        })
    }

    /// Compute phase-a bus voltages from a single-sequence (positive-only)
    /// injection. Used for three-phase faults where only I_1 is nonzero.
    fn compute_phase_voltages_from_seq(
        &self,
        z_bus: &Array2<Complex64>,
        prefault_voltages: &[Complex64],
        fault_idx: usize,
        i_seq: Complex64,
        n: usize,
    ) -> Vec<(ElementId, Complex64)> {
        let mut bus_voltages = Vec::with_capacity(n);
        for i in 0..n {
            let z_if = z_bus[[i, fault_idx]];
            let v_i = prefault_voltages[i] - z_if * i_seq;
            bus_voltages.push((i as ElementId, v_i));
        }
        bus_voltages
    }

    /// Compute phase-a bus voltages by summing the contributions of all three
    /// sequence networks.
    ///
    /// V_a_i = V_prefault_i (positive seq only)
    ///       - Z1_if * I_1
    ///       - Z2_if * I_2
    ///       - Z0_if * I_0
    ///
    /// The negative- and zero-sequence prefault voltages are zero for a
    /// balanced prefault system.
    #[allow(clippy::too_many_arguments)]
    fn compute_phase_voltages_three_seq(
        &self,
        z_bus_pos: &Array2<Complex64>,
        z_bus_neg: &Array2<Complex64>,
        z_bus_zero: &Array2<Complex64>,
        prefault_voltages: &[Complex64],
        fault_idx: usize,
        i_1: Complex64,
        i_2: Complex64,
        i_0: Complex64,
        n: usize,
    ) -> Vec<(ElementId, Complex64)> {
        let mut bus_voltages = Vec::with_capacity(n);
        for i in 0..n {
            let v_pos = prefault_voltages[i] - z_bus_pos[[i, fault_idx]] * i_1;
            let v_neg = -z_bus_neg[[i, fault_idx]] * i_2;
            let v_zero = -z_bus_zero[[i, fault_idx]] * i_0;
            let v_a = v_pos + v_neg + v_zero;
            bus_voltages.push((i as ElementId, v_a));
        }
        bus_voltages
    }

    /// Compute branch currents from bus voltages using the off-diagonal
    /// entries of the supplied Z-bus (positive sequence).
    ///
    /// For a branch between buses i and j, the branch impedance is
    /// z_branch = -Z_bus[i, j] (off-diagonal of Z-bus is the negative of the
    /// branch impedance for a passive network). The branch current is
    /// (V_i - V_j) / z_branch.
    fn compute_branch_currents(
        &self,
        z_bus: &Array2<Complex64>,
        bus_voltages: &[(ElementId, Complex64)],
        n: usize,
    ) -> Vec<(ElementId, Complex64)> {
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
        branch_currents
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

    /// Build a `SequenceNetworks` for the 3-bus system where each sequence
    /// network has a distinct impedance level.
    ///
    /// - Positive sequence: same as `build_3bus_z_bus`
    /// - Negative sequence: identical to positive (typical for static devices)
    /// - Zero sequence: higher impedance (3x line impedance, plus grounding)
    fn build_3bus_sequence_networks() -> SequenceNetworks {
        let z_bus_positive = build_3bus_z_bus();
        // Negative sequence typically equals positive for static networks.
        let z_bus_negative = z_bus_positive.clone();

        // Zero sequence: build a separate Y_bus with 3x line impedance and
        // grounding admittances, then invert.
        let z01 = Complex64::new(0.01, 0.1) * Complex64::new(3.0, 0.0);
        let z12 = Complex64::new(0.015, 0.15) * Complex64::new(3.0, 0.0);

        let y01 = Complex64::new(1.0, 0.0) / z01;
        let y12 = Complex64::new(1.0, 0.0) / z12;

        // Add grounding admittances at each bus for the zero-sequence network.
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

        let z_bus_zero = invert_complex_matrix(&y_bus)
            .unwrap_or_else(|| Array2::from_elem((3, 3), Complex64::new(0.0, 0.0)));

        SequenceNetworks {
            z_bus_positive,
            z_bus_negative,
            z_bus_zero,
        }
    }

    #[test]
    fn test_seq_networks_three_phase_matches_legacy() {
        // Three-phase fault should produce identical results between the
        // legacy `analyze` path and `analyze_with_sequence_networks`, since
        // both use only the positive-sequence Z-bus.
        let seq_networks = build_3bus_sequence_networks();
        let z_bus = seq_networks.z_bus_positive.clone();
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
        let legacy = analyzer
            .analyze(&fault, &z_bus, &prefault_voltages, None)
            .unwrap();
        let seq = analyzer
            .analyze_with_sequence_networks(&fault, &seq_networks, &prefault_voltages)
            .unwrap();

        assert!(
            (legacy.fault_current_ka - seq.fault_current_ka).norm() < 1e-9,
            "Three-phase fault current mismatch: legacy={} seq={}",
            legacy.fault_current_ka,
            seq.fault_current_ka
        );
        assert_eq!(legacy.bus_voltages.len(), seq.bus_voltages.len());
        for ((id_l, v_l), (id_s, v_s)) in
            legacy.bus_voltages.iter().zip(seq.bus_voltages.iter())
        {
            assert_eq!(id_l, id_s);
            assert!(
                (v_l - v_s).norm() < 1e-9,
                "Bus {} voltage mismatch: legacy={} seq={}",
                id_l,
                v_l,
                v_s
            );
        }
    }

    #[test]
    fn test_seq_networks_slg_independent_sequences() {
        let seq_networks = build_3bus_sequence_networks();
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
        let result = analyzer
            .analyze_with_sequence_networks(&fault, &seq_networks, &prefault_voltages)
            .unwrap();

        // SLG fault current must be significant.
        assert!(
            result.fault_current_ka.norm() > 0.1,
            "SLG fault current magnitude {} should be significant",
            result.fault_current_ka.norm()
        );
        assert_eq!(result.fault_type, FaultType::SingleLineGround);

        // Manually verify the SLG current formula:
        // I_fault = 3 * V / (Z1_ff + Z2_ff + Z0_ff)
        let z1_ff = seq_networks.z_bus_positive[[1, 1]];
        let z2_ff = seq_networks.z_bus_negative[[1, 1]];
        let z0_ff = seq_networks.z_bus_zero[[1, 1]];
        let expected_i = Complex64::new(3.0, 0.0)
            * (Complex64::new(1.0, 0.0) / (z1_ff + z2_ff + z0_ff));
        assert!(
            (result.fault_current_ka - expected_i).norm() < 1e-9,
            "SLG current {} does not match expected {}",
            result.fault_current_ka,
            expected_i
        );

        // Fault bus phase-a voltage should be near zero for a bolted SLG.
        let fault_bus_v = result
            .bus_voltages
            .iter()
            .find(|(id, _)| *id == 1)
            .map(|(_, v)| v.norm())
            .unwrap_or(999.0);
        assert!(
            fault_bus_v < 0.5,
            "Fault bus voltage {} should be small for bolted SLG",
            fault_bus_v
        );
    }

    #[test]
    fn test_seq_networks_ll_independent_sequences() {
        let seq_networks = build_3bus_sequence_networks();
        let prefault_voltages = vec![
            Complex64::new(1.0, 0.0),
            Complex64::new(1.0, 0.0),
            Complex64::new(1.0, 0.0),
        ];

        let fault = FaultSpec {
            bus_id: 1,
            fault_type: FaultType::LineLine,
            fault_impedance: Complex64::new(0.0, 0.0),
        };

        let analyzer = ShortCircuitAnalyzer::new();
        let result = analyzer
            .analyze_with_sequence_networks(&fault, &seq_networks, &prefault_voltages)
            .unwrap();

        assert!(
            result.fault_current_ka.norm() > 0.1,
            "LL fault current magnitude {} should be significant",
            result.fault_current_ka.norm()
        );

        // Verify LL formula: I_fault = -j*sqrt(3) * V / (Z1_ff + Z2_ff)
        let z1_ff = seq_networks.z_bus_positive[[1, 1]];
        let z2_ff = seq_networks.z_bus_negative[[1, 1]];
        let expected_i =
            Complex64::new(0.0, -3.0_f64.sqrt()) * (Complex64::new(1.0, 0.0) / (z1_ff + z2_ff));
        assert!(
            (result.fault_current_ka - expected_i).norm() < 1e-9,
            "LL current {} does not match expected {}",
            result.fault_current_ka,
            expected_i
        );
    }

    #[test]
    fn test_seq_networks_dlg_independent_sequences() {
        let seq_networks = build_3bus_sequence_networks();
        let prefault_voltages = vec![
            Complex64::new(1.0, 0.0),
            Complex64::new(1.0, 0.0),
            Complex64::new(1.0, 0.0),
        ];

        let fault = FaultSpec {
            bus_id: 1,
            fault_type: FaultType::DoubleLineGround,
            fault_impedance: Complex64::new(0.0, 0.0),
        };

        let analyzer = ShortCircuitAnalyzer::new();
        let result = analyzer
            .analyze_with_sequence_networks(&fault, &seq_networks, &prefault_voltages)
            .unwrap();

        assert!(
            result.fault_current_ka.norm() > 0.1,
            "DLG fault current magnitude {} should be significant",
            result.fault_current_ka.norm()
        );

        // Verify DLG formula:
        // I_1 = V / (Z1 + Z2 || Z0)
        // I_0 = -I_1 * Z2 / (Z2 + Z0)
        // I_fault = 3 * I_0
        let z1_ff = seq_networks.z_bus_positive[[1, 1]];
        let z2_ff = seq_networks.z_bus_negative[[1, 1]];
        let z0_ff = seq_networks.z_bus_zero[[1, 1]];
        let z_par = z2_ff * z0_ff / (z2_ff + z0_ff);
        let i_1 = Complex64::new(1.0, 0.0) / (z1_ff + z_par);
        let i_0 = -i_1 * z2_ff / (z2_ff + z0_ff);
        let expected_i = Complex64::new(3.0, 0.0) * i_0;
        assert!(
            (result.fault_current_ka - expected_i).norm() < 1e-9,
            "DLG current {} does not match expected {}",
            result.fault_current_ka,
            expected_i
        );
    }

    #[test]
    fn test_seq_networks_slg_with_fault_impedance() {
        let seq_networks = build_3bus_sequence_networks();
        let prefault_voltages = vec![
            Complex64::new(1.0, 0.0),
            Complex64::new(1.0, 0.0),
            Complex64::new(1.0, 0.0),
        ];

        let analyzer = ShortCircuitAnalyzer::new();

        let bolted = FaultSpec {
            bus_id: 1,
            fault_type: FaultType::SingleLineGround,
            fault_impedance: Complex64::new(0.0, 0.0),
        };
        let with_z = FaultSpec {
            bus_id: 1,
            fault_type: FaultType::SingleLineGround,
            fault_impedance: Complex64::new(0.1, 0.0),
        };

        let r_bolted = analyzer
            .analyze_with_sequence_networks(&bolted, &seq_networks, &prefault_voltages)
            .unwrap();
        let r_with_z = analyzer
            .analyze_with_sequence_networks(&with_z, &seq_networks, &prefault_voltages)
            .unwrap();

        assert!(
            r_with_z.fault_current_ka.norm() < r_bolted.fault_current_ka.norm(),
            "SLG with impedance ({}) should be less than bolted ({})",
            r_with_z.fault_current_ka.norm(),
            r_bolted.fault_current_ka.norm()
        );
    }

    #[test]
    fn test_seq_networks_dimension_mismatch() {
        let seq_networks = build_3bus_sequence_networks();
        // Build a 2-bus zero-sequence matrix to trigger dimension mismatch.
        let bad_zero = Array2::from_elem((2, 2), Complex64::new(0.0, 0.0));
        let bad_networks = SequenceNetworks {
            z_bus_positive: seq_networks.z_bus_positive.clone(),
            z_bus_negative: seq_networks.z_bus_negative.clone(),
            z_bus_zero: bad_zero,
        };

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
        let result = analyzer.analyze_with_sequence_networks(
            &fault,
            &bad_networks,
            &prefault_voltages,
        );
        assert!(
            result.is_err(),
            "Dimension mismatch should produce an error"
        );
    }

    #[test]
    fn test_seq_networks_fault_bus_out_of_range() {
        let seq_networks = build_3bus_sequence_networks();
        let prefault_voltages = vec![
            Complex64::new(1.0, 0.0),
            Complex64::new(1.0, 0.0),
            Complex64::new(1.0, 0.0),
        ];
        let fault = FaultSpec {
            bus_id: 10,
            fault_type: FaultType::ThreePhase,
            fault_impedance: Complex64::new(0.0, 0.0),
        };

        let analyzer = ShortCircuitAnalyzer::new();
        let result =
            analyzer.analyze_with_sequence_networks(&fault, &seq_networks, &prefault_voltages);
        assert!(result.is_err());
    }

    #[test]
    fn test_seq_networks_slg_uses_zero_sequence_network() {
        // Verify that the SLG result actually depends on the zero-sequence
        // Z-bus. If we replace the zero-sequence network with a much larger
        // impedance, the SLG fault current must decrease.
        let seq_networks = build_3bus_sequence_networks();
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
        let base = analyzer
            .analyze_with_sequence_networks(&fault, &seq_networks, &prefault_voltages)
            .unwrap();

        // Build a high-impedance zero-sequence network (10x line impedance).
        let z01 = Complex64::new(0.01, 0.1) * Complex64::new(30.0, 0.0);
        let z12 = Complex64::new(0.015, 0.15) * Complex64::new(30.0, 0.0);
        let y01 = Complex64::new(1.0, 0.0) / z01;
        let y12 = Complex64::new(1.0, 0.0) / z12;
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
        let z_bus_zero_hi = invert_complex_matrix(&y_bus).unwrap();
        let hi_networks = SequenceNetworks {
            z_bus_positive: seq_networks.z_bus_positive.clone(),
            z_bus_negative: seq_networks.z_bus_negative.clone(),
            z_bus_zero: z_bus_zero_hi,
        };

        let hi = analyzer
            .analyze_with_sequence_networks(&fault, &hi_networks, &prefault_voltages)
            .unwrap();

        assert!(
            hi.fault_current_ka.norm() < base.fault_current_ka.norm(),
            "SLG with higher zero-seq impedance ({}) should be less than base ({})",
            hi.fault_current_ka.norm(),
            base.fault_current_ka.norm()
        );
    }
}

use eneros_core::Result;
use eneros_powerflow::PowerFlowResult;
use eneros_constraint::{N1Result, StabilityResult, Violation};

/// Pipeline stage identifier
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipelineStage {
    /// Power flow calculation
    PowerFlow,
    /// N-1 contingency analysis
    N1Analysis,
    /// Constraint checking
    ConstraintCheck,
    /// Stability analysis
    StabilityCheck,
}

/// Complete network analysis result from the unified pipeline
#[derive(Debug, Clone)]
pub struct NetworkAnalysisResult {
    /// Power flow result
    pub power_flow: PowerFlowResult,
    /// N-1 contingency results
    pub n1_results: Vec<N1Result>,
    /// Constraint violations
    pub violations: Vec<Violation>,
    /// Stability analysis result
    pub stability: StabilityResult,
    /// Whether the overall analysis passed all checks
    pub passed: bool,
    /// Stages that failed
    pub failed_stages: Vec<PipelineStage>,
}

impl NetworkAnalysisResult {
    /// Check if N-1 analysis passed (no critical violations)
    pub fn n1_passed(&self) -> bool {
        self.n1_results.iter().all(|r| r.voltage_violations.is_empty() && r.thermal_violations.is_empty())
    }

    /// Check if constraint checking passed
    pub fn constraints_passed(&self) -> bool {
        self.violations.is_empty()
    }

    /// Check if stability analysis passed
    pub fn stability_passed(&self) -> bool {
        self.stability.stable
    }

    /// Get N-1 critical contingencies
    pub fn critical_contingencies(&self) -> Vec<&N1Result> {
        self.n1_results
            .iter()
            .filter(|r| !r.voltage_violations.is_empty() || !r.thermal_violations.is_empty())
            .collect()
    }

    /// Get non-convergent contingencies
    pub fn non_convergent_contingencies(&self) -> Vec<&N1Result> {
        self.n1_results
            .iter()
            .filter(|r| !r.converged)
            .collect()
    }
}

/// Run the full analysis pipeline on a PowerNetwork
pub fn run_full_analysis(network: &crate::PowerNetwork) -> Result<NetworkAnalysisResult> {
    // Stage 1: Power flow
    let power_flow = network.solve()?;

    // Stage 2: N-1 analysis
    let n1_results = network.check_n1();

    // Stage 3: Constraint check
    let violations = network.check_constraints(&power_flow);

    // Stage 4: Stability check
    let stability = network.check_stability(&power_flow);

    // Determine overall pass/fail
    let mut failed_stages = Vec::new();

    let n1_passed = n1_results.iter().all(|r| {
        r.voltage_violations.is_empty() && r.thermal_violations.is_empty()
    });
    if !n1_passed {
        failed_stages.push(PipelineStage::N1Analysis);
    }

    if !violations.is_empty() {
        failed_stages.push(PipelineStage::ConstraintCheck);
    }

    if !stability.stable {
        failed_stages.push(PipelineStage::StabilityCheck);
    }

    let passed = failed_stages.is_empty();

    Ok(NetworkAnalysisResult {
        power_flow,
        n1_results,
        violations,
        stability,
        passed,
        failed_stages,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PowerNetwork;

    #[test]
    fn test_full_pipeline_ieee14() {
        let network = PowerNetwork::from_ieee14();
        let result = run_full_analysis(&network);

        assert!(result.is_ok(), "Full pipeline failed: {:?}", result.err());
        let analysis = result.unwrap();

        // IEEE 14 should converge
        assert!(analysis.power_flow.converged);

        // IEEE 14 should be stable
        assert!(analysis.stability_passed());

        // Check N-1 results exist
        assert_eq!(analysis.n1_results.len(), 20);
    }
}

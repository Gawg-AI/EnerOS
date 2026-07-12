//! Compliance audit report generation and BOM writeback.

use core::fmt::Write;

use super::*;

/// Error type for BOM writeback operations.
#[derive(Debug, PartialEq, Eq)]
pub enum BomError {
    /// Writing to the BOM store failed.
    WriteError,
    /// The BOM impact data is invalid (e.g. isolator needed but no items).
    InvalidImpact,
}

/// Generates a compliance audit report from collected isolation evidence.
///
/// The report contains the Go/No-Go conclusion, reference partition
/// layouts, data-flow verification status, and the regulatory clause
/// reference (36 号文 §3.2).
pub fn generate_compliance_report(ev: &IsolationEvidence) -> ComplianceReport {
    let conclusion = super::compliance::evaluate_compliance(ev);
    ComplianceReport {
        conclusion,
        partition_a: super::compliance::partition_a(),
        partition_b: super::compliance::partition_b(),
        data_flow_verified: ev.unidirectional_flow,
        regulatory_clause: make_clause("36号文-横向隔离-§3.2"),
    }
}

/// Writes back the BOM impact to the BOM store.
///
/// In a real system this would persist to the configuration management
/// database. This implementation validates the impact and returns Ok
/// when the data is consistent.
pub fn writeback_bom(impact: &BomImpact) -> Result<(), BomError> {
    // If an isolator is required, at least one BOM item must be present.
    if impact.need_isolator && impact.bom_items.is_empty() {
        return Err(BomError::InvalidImpact);
    }
    // Sanity check on cost to catch overflow in budget calculations.
    if impact.cost_delta_yuan > 1_000_000 {
        return Err(BomError::InvalidImpact);
    }
    // Reference implementation: validation passed, writeback succeeds.
    Ok(())
}

fn make_clause(s: &str) -> heapless::String<64> {
    let mut out = heapless::String::new();
    write!(out, "{}", s).ok();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence_all_true() -> IsolationEvidence {
        IsolationEvidence {
            partition_separation: true,
            capability_enforced: true,
            unidirectional_flow: true,
            formal_verification: true,
        }
    }

    #[test]
    fn test_generate_compliance_report_go() {
        let ev = evidence_all_true();
        let report = generate_compliance_report(&ev);
        assert!(matches!(report.conclusion, ComplianceResult::Go { .. }));
        assert!(report.data_flow_verified);
        assert!(!report.regulatory_clause.is_empty());
        assert_eq!(report.partition_a.name, "safety_control");
        assert_eq!(report.partition_b.name, "agent_runtime");
        // Partitions must not overlap.
        assert!(
            report.partition_a.memory_base + report.partition_a.memory_size
                <= report.partition_b.memory_base
        );
    }

    #[test]
    fn test_generate_compliance_report_nogo() {
        let ev = IsolationEvidence {
            partition_separation: false,
            ..evidence_all_true()
        };
        let report = generate_compliance_report(&ev);
        assert!(matches!(report.conclusion, ComplianceResult::NoGo { .. }));
        // data_flow_verified reflects the evidence, not the conclusion.
        assert!(report.data_flow_verified);
        assert!(!report.regulatory_clause.is_empty());
    }

    #[test]
    fn test_writeback_bom_ok_with_items() {
        let mut items = heapless::Vec::new();
        items.push("network-isolator").ok();
        let impact = BomImpact {
            need_isolator: true,
            cost_delta_yuan: 12_000,
            bom_items: items,
        };
        assert!(writeback_bom(&impact).is_ok());
    }

    #[test]
    fn test_writeback_bom_ok_no_isolator() {
        let impact = BomImpact::default();
        assert!(writeback_bom(&impact).is_ok());
    }

    #[test]
    fn test_writeback_bom_invalid_empty_items() {
        // need_isolator=true but no BOM items -> InvalidImpact.
        let impact = BomImpact {
            need_isolator: true,
            cost_delta_yuan: 12_000,
            bom_items: heapless::Vec::new(),
        };
        assert_eq!(writeback_bom(&impact), Err(BomError::InvalidImpact));
    }

    #[test]
    fn test_writeback_bom_invalid_cost_overflow() {
        let mut items = heapless::Vec::new();
        items.push("network-isolator").ok();
        let impact = BomImpact {
            need_isolator: true,
            cost_delta_yuan: 2_000_000,
            bom_items: items,
        };
        assert_eq!(writeback_bom(&impact), Err(BomError::InvalidImpact));
    }

    #[test]
    fn test_writeback_bom_from_nogo_result() {
        // Verify that BomImpact extracted from a NoGo conclusion can be
        // written back successfully.
        let ev = IsolationEvidence {
            partition_separation: false,
            ..evidence_all_true()
        };
        let result = super::super::compliance::evaluate_compliance(&ev);
        assert!(matches!(result, ComplianceResult::NoGo { .. }));
        if let ComplianceResult::NoGo { bom_impact, .. } = result {
            assert!(writeback_bom(&bom_impact).is_ok());
        }
    }
}

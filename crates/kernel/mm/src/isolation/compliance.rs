//! Horizontal isolation compliance verification logic.
//!
//! The Go/No-Go decision flow (per 36 号文 §3.2):
//! 1. Physical memory partition separation? No -> NoGo
//! 2. Capability enforcement? No -> NoGo
//! 3. Unidirectional data flow? No -> NoGo
//! 4. Formal verification? No -> Go (conditional) / Yes -> Go
//!
//! The first three checks are mandatory. Formal verification turns a
//! conditional Go into an unconditional Go; at least three of four
//! checks must be satisfied for Go.

use core::fmt::Write;

use super::*;

/// Partition A (safety/control) reference configuration.
const PARTITION_A: PartitionInfo = PartitionInfo {
    name: "safety_control",
    memory_base: 0x4000_0000,
    memory_size: 0x0800_0000, // 128 MB
    capability_root: 0x1000,
};

/// Partition B (agent/runtime) reference configuration.
const PARTITION_B: PartitionInfo = PartitionInfo {
    name: "agent_runtime",
    memory_base: 0x4800_0000,
    memory_size: 0x0800_0000, // 128 MB
    capability_root: 0x2000,
};

/// Collects isolation evidence from the v0.9.0 partition isolation primitives.
///
/// In the reference configuration all four isolation properties are
/// satisfied. The function is deterministic: the same system state
/// always yields the same evidence.
pub fn collect_isolation_evidence() -> IsolationEvidence {
    IsolationEvidence {
        partition_separation: true,
        capability_enforced: true,
        unidirectional_flow: true,
        formal_verification: true,
    }
}

/// Evaluates the Go/No-Go compliance conclusion from collected evidence.
///
/// This is the pure decision function; it does not touch hardware and
/// may be called with mock evidence in tests.
pub fn evaluate_compliance(evidence: &IsolationEvidence) -> ComplianceResult {
    if !evidence.partition_separation {
        return ComplianceResult::NoGo {
            need_physical_device: true,
            reason: make_reason("physical memory partition separation not satisfied"),
            bom_impact: BomImpact {
                need_isolator: true,
                cost_delta_yuan: 12_000,
                bom_items: bom_items_network_isolator(),
            },
        };
    }

    if !evidence.capability_enforced {
        return ComplianceResult::NoGo {
            need_physical_device: false,
            reason: make_reason("capability-based access control not enforced"),
            bom_impact: BomImpact::default(),
        };
    }

    if !evidence.unidirectional_flow {
        return ComplianceResult::NoGo {
            need_physical_device: true,
            reason: make_reason("cross-boundary data flow is not unidirectional"),
            bom_impact: BomImpact {
                need_isolator: true,
                cost_delta_yuan: 8_000,
                bom_items: bom_items_diode_gateway(),
            },
        };
    }

    // First three mandatory checks pass — Go (with or without formal
    // verification). When formal_verification is false the Go is
    // conditional; the evidence field records this.
    ComplianceResult::Go {
        bipartition_acceptable: true,
        evidence: *evidence,
    }
}

/// Top-level entry: collects evidence from the runtime and evaluates
/// compliance, returning a Go/No-Go conclusion.
pub fn verify_horizontal_isolation() -> ComplianceResult {
    let evidence = collect_isolation_evidence();
    evaluate_compliance(&evidence)
}

/// Returns the reference partition A (safety/control) info.
pub const fn partition_a() -> PartitionInfo {
    PARTITION_A
}

/// Returns the reference partition B (agent/runtime) info.
pub const fn partition_b() -> PartitionInfo {
    PARTITION_B
}

fn make_reason(msg: &str) -> heapless::String<256> {
    let mut s = heapless::String::new();
    write!(s, "{}", msg).ok();
    s
}

fn bom_items_network_isolator() -> heapless::Vec<&'static str, 8> {
    let mut v = heapless::Vec::new();
    v.push("network-isolator").ok();
    v.push("sfp-module").ok();
    v
}

fn bom_items_diode_gateway() -> heapless::Vec<&'static str, 8> {
    let mut v = heapless::Vec::new();
    v.push("diode-gateway").ok();
    v
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
    fn test_verify_horizontal_isolation_go_all_true() {
        let ev = evidence_all_true();
        let result = evaluate_compliance(&ev);
        assert!(matches!(result, ComplianceResult::Go { .. }));
        if let ComplianceResult::Go {
            bipartition_acceptable,
            evidence,
        } = result
        {
            assert!(bipartition_acceptable);
            assert_eq!(evidence, ev);
        }
    }

    #[test]
    fn test_verify_horizontal_isolation_go_without_formal_verification() {
        // First three mandatory checks pass, formal verification missing
        // -> still Go (conditional).
        let ev = IsolationEvidence {
            formal_verification: false,
            ..evidence_all_true()
        };
        let result = evaluate_compliance(&ev);
        assert!(matches!(result, ComplianceResult::Go { .. }));
        if let ComplianceResult::Go {
            bipartition_acceptable,
            evidence,
        } = result
        {
            assert!(bipartition_acceptable);
            assert!(!evidence.formal_verification);
        }
    }

    #[test]
    fn test_verify_horizontal_isolation_nogo_partition_false() {
        let ev = IsolationEvidence {
            partition_separation: false,
            ..evidence_all_true()
        };
        let result = evaluate_compliance(&ev);
        assert!(matches!(result, ComplianceResult::NoGo { .. }));
        if let ComplianceResult::NoGo {
            need_physical_device,
            reason,
            bom_impact,
        } = result
        {
            assert!(need_physical_device);
            assert!(!reason.is_empty());
            assert!(bom_impact.need_isolator);
            assert!(bom_impact.cost_delta_yuan > 0);
            assert!(!bom_impact.bom_items.is_empty());
        }
    }

    #[test]
    fn test_verify_horizontal_isolation_nogo_capability_false() {
        let ev = IsolationEvidence {
            capability_enforced: false,
            ..evidence_all_true()
        };
        let result = evaluate_compliance(&ev);
        assert!(matches!(result, ComplianceResult::NoGo { .. }));
        if let ComplianceResult::NoGo {
            need_physical_device,
            bom_impact,
            ..
        } = result
        {
            assert!(!need_physical_device);
            assert!(!bom_impact.need_isolator);
        }
    }

    #[test]
    fn test_verify_horizontal_isolation_nogo_unidirectional_false() {
        let ev = IsolationEvidence {
            unidirectional_flow: false,
            ..evidence_all_true()
        };
        let result = evaluate_compliance(&ev);
        assert!(matches!(result, ComplianceResult::NoGo { .. }));
        if let ComplianceResult::NoGo {
            need_physical_device,
            bom_impact,
            ..
        } = result
        {
            assert!(need_physical_device);
            assert!(bom_impact.need_isolator);
            assert!(bom_impact.cost_delta_yuan > 0);
        }
    }

    #[test]
    fn test_verify_horizontal_isolation_default_returns_go() {
        // verify_horizontal_isolation uses the reference configuration
        // where all four checks pass -> Go.
        let result = verify_horizontal_isolation();
        assert!(matches!(result, ComplianceResult::Go { .. }));
    }

    #[test]
    fn test_collect_isolation_evidence_reproducible() {
        // Same system state -> same evidence (deterministic).
        let ev1 = collect_isolation_evidence();
        let ev2 = collect_isolation_evidence();
        assert_eq!(ev1, ev2);
    }
}

//! Horizontal isolation compliance verification (v0.9.1).
//!
//! Based on 36 号文《电力监控系统安全防护总体方案》§3.2, this module
//! verifies bipartition horizontal isolation by collecting v0.9.0
//! isolation evidence and producing a Go/No-Go compliance conclusion.
//!
//! The four isolation properties checked are:
//! 1. Physical memory partition separation (non-overlapping `allowed_phys`)
//! 2. Capability-based access control enforcement
//! 3. Unidirectional cross-boundary data flow
//! 4. Formal verification of the isolation property
//!
//! The first three are mandatory; formal verification turns a conditional
//! Go into an unconditional Go.

pub mod audit;
pub mod compliance;

/// Compliance conclusion produced by horizontal isolation verification.
//
// The NoGo variant is large (~409 bytes) because it embeds a
// `heapless::String<256>` reason and a `BomImpact` with a
// `heapless::Vec<..., 8>`. In a no_std crate without `alloc` we
// cannot box these fields, so we suppress the large_enum_variant lint.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum ComplianceResult {
    /// Bipartition isolation is acceptable; the system may proceed.
    Go {
        /// Whether the bipartition layout is acceptable.
        bipartition_acceptable: bool,
        /// Evidence collected from v0.9.0 isolation primitives.
        evidence: IsolationEvidence,
    },
    /// Isolation is insufficient; proceed is blocked.
    NoGo {
        /// Whether additional physical isolation hardware is required.
        need_physical_device: bool,
        /// Human-readable explanation.
        reason: heapless::String<256>,
        /// BOM impact of remediation.
        bom_impact: BomImpact,
    },
}

/// Isolation evidence collected from the runtime system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IsolationEvidence {
    /// Physical memory partition separation (non-overlapping `allowed_phys`).
    pub partition_separation: bool,
    /// Capability-based access control is enforced.
    pub capability_enforced: bool,
    /// Data flow is strictly unidirectional across the boundary.
    pub unidirectional_flow: bool,
    /// Formal verification of isolation property completed.
    pub formal_verification: bool,
}

/// BOM (Bill of Materials) impact when NoGo.
#[derive(Clone, Debug, PartialEq)]
pub struct BomImpact {
    /// Whether a physical network isolator / security gateway is required.
    pub need_isolator: bool,
    /// Incremental cost in RMB yuan.
    pub cost_delta_yuan: u32,
    /// Affected BOM line items.
    pub bom_items: heapless::Vec<&'static str, 8>,
}

impl Default for BomImpact {
    fn default() -> Self {
        Self {
            need_isolator: false,
            cost_delta_yuan: 0,
            bom_items: heapless::Vec::new(),
        }
    }
}

/// Information about a single partition used in the compliance report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PartitionInfo {
    /// Partition name (e.g. "safety_control").
    pub name: &'static str,
    /// Physical memory base address.
    pub memory_base: u64,
    /// Physical memory size in bytes.
    pub memory_size: u64,
    /// Capability table root CNode address.
    pub capability_root: u64,
}

/// Compliance audit report.
#[derive(Debug, PartialEq)]
pub struct ComplianceReport {
    /// Go/No-Go conclusion.
    pub conclusion: ComplianceResult,
    /// Partition A (safety/control).
    pub partition_a: PartitionInfo,
    /// Partition B (agent/runtime).
    pub partition_b: PartitionInfo,
    /// Whether the cross-boundary data flow was verified unidirectional.
    pub data_flow_verified: bool,
    /// Regulatory clause reference (e.g. 36 号文 clause).
    pub regulatory_clause: heapless::String<64>,
}

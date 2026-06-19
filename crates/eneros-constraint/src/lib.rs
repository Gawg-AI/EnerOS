pub mod engine;
pub mod projector;
pub mod rules;
pub mod violation;
pub mod compliance;
pub mod validation_rules;

pub use engine::ConstraintEngine;
pub use projector::{FeasibilityProjector, ProjectionResult, WhatIfResult, NetworkSimulator, ActionModification};
pub use rules::{Constraint, ConstraintType, ConstraintCategory, ResponseStrategy,
    N1Result, N1Violation, N1ViolationType, StabilityResult, VoltageMargin};
pub use violation::Violation;
pub use compliance::{ComplianceChecker, ComplianceFinding, ComplianceStatus,
    EquipmentSpec, OperatingConditions};
pub use validation_rules::{
    ValidationRuleEngine, ValidationFinding, ValidationStatus, ValidationSummary,
    SystemStateSnapshot, BusVoltageObservation, FrequencyObservation,
    ContingencyObservation, ShortCircuitObservation,
};

// Re-export agentos types used by constraint engine
pub use eneros_core::{ActionFeasibility, SystemOperatingState};

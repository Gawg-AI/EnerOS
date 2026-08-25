//! EnerOS control gateway (CTL-003 + CTL-004 write-command default-deny gate).
//!
//! Two responsibilities:
//! - [`envelope::CommandEnvelope`] — a fixed-size, length-bounded, SM3-HMAC
//!   authorized command envelope codec (v2: carries correlation/sequence/
//!   lease/fencing/policy/config/idempotency). This is the wire format for every
//!   write command that targets a device point.
//! - [`gateway::ControlGateway`] — the **only** path that turns envelope bytes
//!   into an [`gateway::AuthorizedCommand`]. It is default-deny: decode failure,
//!   HMAC mismatch, expiry, future-dating, replay, missing lease/policy and
//!   lease revocation are all hard denies — no write reaches the device.
//!   Protocol adapters must forward write requests here; they are forbidden
//!   from writing directly.
//!
//! # no_std
//!
//! `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`. Uses
//! `eneros-crypto` (SM3-HMAC + constant-time compare) and
//! `eneros-telemetry-model` (ControlCommand). No third-party deps.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod chain;
pub mod commissioning;
pub mod envelope;
pub mod gateway;
pub mod idempotency;
pub mod incident_replay;
pub mod lease;
pub mod override_ctrl;
pub mod policy;
pub mod profile;
pub mod reconcile;
pub mod safety;
pub mod slo;
pub mod virtual_plant;
pub mod wire;

pub use chain::{ChainDenyReason, ChainOutcome, ControlChain, FeedbackBundle};
pub use commissioning::{
    CheckKind, CheckResult, CommissioningReport, CommissioningState, CommissioningValidator,
    SystemSelfCheckInput,
};
pub use envelope::{
    ActorKind, CommandEnvelope, EnvelopeBuilder, EnvelopeError, ENVELOPE_LEN, HEADER_LEN, VERSION,
};
pub use gateway::{
    AuthorizedCommand, ControlGateway, GatewayError, LeaseChecker, LeaseStatus,
    StructuralLeaseChecker,
};
pub use idempotency::{IdempotencyCache, OpOutcome, SboExecuteResult, SboState, SboTracker};
pub use incident_replay::{
    CausalEvent, CausalEventKind, CommandChain, DecisionRecord, ExecutionOutcomeRecord,
    IncidentReplay, RejectionGate, ReplaySummary, SafetyVerdictRecord,
};
pub use lease::{LeaseScope, LeaseService};
pub use override_ctrl::{OverrideAuthority, OverrideDecision, OverrideSession};
pub use policy::{CommandClass, Decision, DenyReason, Obligation, Pdp, Policy, PolicyRule};
pub use profile::{ChainTuning, ControlProfile};
pub use reconcile::FeedbackSample;
pub use reconcile::{reconcile, Reconciled};
pub use safety::{PlantContext, SafetyPoint, SafetyRole, SafetyValidator, SafetyVerdict};
pub use slo::{
    frozen_slo, DeadlineMonitor, DegradationAction, LatencySample, MissClass, SloId, SloSnapshot,
    SloSnapshotEntry, SloSpec, SloStats, WorkloadProfile, FROZEN_SLOS,
};
pub use virtual_plant::{ApplyOutcome, Fault, PlantState, VirtualPlant};
pub use wire::{OutcomeCode, OutcomeFrame, WireError};

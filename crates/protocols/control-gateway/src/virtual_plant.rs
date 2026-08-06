//! Virtual plant (CTL-011).
//!
//! A simulated energy-storage plant that executes verified, safety-validated
//! commands and exposes feedback, with **fault injection** for the three
//! scenarios the MASTER requires on the QEMU virtual platform (no real
//! hardware): actuator stuck, communication loss, and power loss + recovery.
//!
//! The virtual plant is the L2 acceptance harness for the whole control chain:
//! envelope → gateway → PDP → safety validator → plant. It proves the chain
//! closes deterministically under each fault scenario.

use alloc::collections::BTreeMap;
use alloc::string::String;

use crate::gateway::AuthorizedCommand;
use crate::safety::{PlantContext, SafetyValidator, SafetyVerdict};

/// Internal plant state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlantState {
    pub pcc_closed: bool,
    pub inverter_blocked: bool,
    pub soc_pct: f64,
    pub grid_up: bool,
    /// Last-feedback timestamp (epoch secs). Goes stale on comm loss.
    pub last_feedback_epoch: u64,
    /// True while plant is powered down.
    pub powered_down: bool,
}

impl Default for PlantState {
    fn default() -> Self {
        // Fail-safe default: PCC open, inverter blocked, grid up, SOC mid.
        Self {
            pcc_closed: false,
            inverter_blocked: true,
            soc_pct: 50.0,
            grid_up: true,
            last_feedback_epoch: 0,
            powered_down: false,
        }
    }
}

/// Outcome of applying a command to the plant.
#[derive(Debug, Clone, PartialEq)]
pub enum ApplyOutcome {
    /// Command applied; state transitioned.
    Applied,
    /// Plant powered down — no command may execute.
    PoweredDown,
    /// Actuator stuck: command accepted by the chain but the physical state
    /// did not change. The supervisor must detect this mismatch (CTL-010/014).
    ActuatorStuck,
    /// Safety validator rejected the command.
    Unsafe(String),
}

/// Faults that can be injected into the plant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// Actuator on `point_id` is stuck — commands to it have no physical effect.
    ActuatorStuck { point_id: u32 },
    /// Communication link is down — feedback goes stale.
    CommLoss,
    /// Plant has lost auxiliary power.
    PowerLoss,
}

pub struct VirtualPlant {
    pub state: PlantState,
    stuck_points: BTreeMap<u32, bool>,
    comm_loss: bool,
    pub validator: SafetyValidator,
}

impl Default for VirtualPlant {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtualPlant {
    pub fn new() -> Self {
        Self {
            state: PlantState::default(),
            stuck_points: BTreeMap::new(),
            comm_loss: false,
            validator: SafetyValidator::new(),
        }
    }

    /// Inject a fault.
    pub fn inject(&mut self, fault: Fault) {
        match fault {
            Fault::ActuatorStuck { point_id } => {
                self.stuck_points.insert(point_id, true);
            }
            Fault::CommLoss => {
                self.comm_loss = true;
            }
            Fault::PowerLoss => {
                self.state.powered_down = true;
                // Fail-safe: PCC opens, inverter blocks.
                self.state.pcc_closed = false;
                self.state.inverter_blocked = true;
            }
        }
    }

    /// Recover from power loss: re-energize with fail-safe state held.
    pub fn recover_power(&mut self, now: u64) {
        self.state.powered_down = false;
        self.state.last_feedback_epoch = now;
        // Stay fail-safe until operator explicitly re-closes.
        self.state.pcc_closed = false;
        self.state.inverter_blocked = true;
    }

    /// Clear comm loss (link restored); refresh feedback timestamp.
    pub fn restore_comm(&mut self, now: u64) {
        self.comm_loss = false;
        self.state.last_feedback_epoch = now;
    }

    /// Current plant context for the safety validator. On comm loss the
    /// feedback timestamp is frozen (stale); the reconciler (CTL-010) will
    /// reject it.
    pub fn context(&self) -> PlantContext {
        PlantContext {
            grid_up: self.state.grid_up,
            soc_pct: self.state.soc_pct,
            dc_bus_v: 0.0,
            overcurrent: false,
            overtemp: false,
            maintenance_lockout: false,
        }
    }

    /// Apply a verified command. Runs the independent safety validator first,
    /// then mutates state (unless an actuator is stuck or plant is down).
    pub fn apply(&mut self, cmd: &AuthorizedCommand, now: u64) -> ApplyOutcome {
        if self.state.powered_down {
            return ApplyOutcome::PoweredDown;
        }
        let ctx = self.context();
        match self.validator.validate(cmd, &ctx) {
            SafetyVerdict::Unsafe(reason) => return ApplyOutcome::Unsafe(reason),
            SafetyVerdict::Safe => {}
        }

        let stuck = self
            .stuck_points
            .get(&cmd.point_id)
            .copied()
            .unwrap_or(false);
        // Apply the physical effect. If the actuator is stuck, state does NOT change.
        let prev = self.state;
        if !stuck {
            self.mutate_for(cmd);
            self.state.last_feedback_epoch = if self.comm_loss {
                self.state.last_feedback_epoch // frozen
            } else {
                now
            };
        }

        if stuck && self.state == prev {
            ApplyOutcome::ActuatorStuck
        } else {
            ApplyOutcome::Applied
        }
    }

    fn mutate_for(&mut self, cmd: &AuthorizedCommand) {
        use eneros_telemetry_model::command::{ControlCommand, DoubleCommand, SingleCommand};
        // Map (point) → effect by consulting the validator's role table.
        let role = self.validator.role_of(cmd.device_id, cmd.point_id);
        match (role, &cmd.command) {
            (
                Some(crate::safety::SafetyRole::PccBreaker),
                ControlCommand::Single(SingleCommand::On),
            )
            | (
                Some(crate::safety::SafetyRole::PccBreaker),
                ControlCommand::Double(DoubleCommand::On),
            ) => {
                self.state.pcc_closed = true;
            }
            (
                Some(crate::safety::SafetyRole::PccBreaker),
                ControlCommand::Single(SingleCommand::Off),
            )
            | (
                Some(crate::safety::SafetyRole::PccBreaker),
                ControlCommand::Double(DoubleCommand::Off),
            ) => {
                self.state.pcc_closed = false;
            }
            (
                Some(crate::safety::SafetyRole::InverterBlock),
                ControlCommand::Single(SingleCommand::On),
            )
            | (
                Some(crate::safety::SafetyRole::InverterBlock),
                ControlCommand::Double(DoubleCommand::On),
            ) => {
                self.state.inverter_blocked = true;
            }
            (
                Some(crate::safety::SafetyRole::InverterBlock),
                ControlCommand::Single(SingleCommand::Off),
            )
            | (
                Some(crate::safety::SafetyRole::InverterBlock),
                ControlCommand::Double(DoubleCommand::Off),
            ) => {
                self.state.inverter_blocked = false;
            }
            _ => {}
        }
    }
}

// ============================================================================
// Tests — the three required L2 scenarios
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use eneros_telemetry_model::command::{ControlCommand, SingleCommand};
    use eneros_telemetry_model::point_spec::{ControlDirection, PointSpec};
    use eneros_telemetry_model::unit::PhysicalUnit;

    use super::*;
    use crate::envelope::ActorKind;
    use crate::gateway::AuthorizedCommand;
    use crate::safety::{SafetyPoint, SafetyRole};

    fn auth(point: u32, cmd: ControlCommand, actor: ActorKind) -> AuthorizedCommand {
        AuthorizedCommand {
            device_id: 1,
            point_id: point,
            command: cmd,
            actor_kind: actor,
            issuer: [0; 16],
            correlation_id: [0; 16],
            sequence: 1,
            lease_id: [0; 16],
            fencing_token: 1,
            policy_version: 1,
            idempotency_key: [0; 16],
        }
    }

    fn pcc_spec() -> PointSpec {
        PointSpec {
            point_id: 10,
            device_id: 1,
            unit: PhysicalUnit::Dimensionless,
            safe_min: 0.0,
            safe_max: 1.0,
            max_ramp_per_sec: None,
            direction: ControlDirection::Discrete,
            remote_controllable: true,
        }
    }

    fn plant() -> VirtualPlant {
        let mut p = VirtualPlant::new();
        p.validator.register(
            1,
            10,
            SafetyPoint {
                role: SafetyRole::PccBreaker,
                spec: pcc_spec(),
            },
        );
        p
    }

    #[test]
    fn test_normal_close_pcc_when_grid_up() {
        let mut p = plant();
        let cmd = auth(
            10,
            ControlCommand::Single(SingleCommand::On),
            ActorKind::Operator,
        );
        assert_eq!(p.apply(&cmd, 1_000), ApplyOutcome::Applied);
        assert!(p.state.pcc_closed);
    }

    #[test]
    fn test_actuator_stuck_scenario() {
        // CTL-011 required scenario: actuator stuck → command accepted by the
        // chain but state does not change.
        let mut p = plant();
        p.inject(Fault::ActuatorStuck { point_id: 10 });
        let cmd = auth(
            10,
            ControlCommand::Single(SingleCommand::On),
            ActorKind::Operator,
        );
        assert_eq!(p.apply(&cmd, 1_000), ApplyOutcome::ActuatorStuck);
        assert!(!p.state.pcc_closed, "stuck actuator must not change state");
    }

    #[test]
    fn test_comm_loss_makes_feedback_stale() {
        // CTL-011 required scenario: comm loss → feedback timestamp frozen.
        let mut p = plant();
        p.state.last_feedback_epoch = 1_000;
        p.inject(Fault::CommLoss);
        // Advance clock; feedback timestamp must NOT advance.
        let cmd = auth(
            10,
            ControlCommand::Single(SingleCommand::On),
            ActorKind::Operator,
        );
        let _ = p.apply(&cmd, 2_000);
        assert_eq!(
            p.state.last_feedback_epoch, 1_000,
            "feedback must stay stale on comm loss"
        );
    }

    #[test]
    fn test_power_loss_then_recovery_scenario() {
        // CTL-011 required scenario: power loss → fail-safe + commands rejected;
        // recovery → re-energized in fail-safe, operator re-closes.
        let mut p = plant();
        // First close PCC normally.
        let close = auth(
            10,
            ControlCommand::Single(SingleCommand::On),
            ActorKind::Operator,
        );
        assert_eq!(p.apply(&close, 1_000), ApplyOutcome::Applied);
        assert!(p.state.pcc_closed);

        // Power loss → fail-safe (PCC opens), commands rejected.
        p.inject(Fault::PowerLoss);
        assert!(!p.state.pcc_closed, "power loss must open PCC (fail-safe)");
        assert_eq!(p.apply(&close, 1_100), ApplyOutcome::PoweredDown);

        // Recover → still fail-safe until operator re-closes.
        p.recover_power(1_200);
        assert!(!p.state.pcc_closed);
        assert_eq!(p.apply(&close, 1_300), ApplyOutcome::Applied);
        assert!(p.state.pcc_closed);
    }

    #[test]
    fn test_safety_validator_blocks_unsafe_even_in_plant() {
        // Close PCC while grid is down → safety validator denies.
        let mut p = plant();
        p.state.grid_up = false;
        let cmd = auth(
            10,
            ControlCommand::Single(SingleCommand::On),
            ActorKind::Operator,
        );
        assert!(matches!(p.apply(&cmd, 1_000), ApplyOutcome::Unsafe(_)));
        assert!(!p.state.pcc_closed);
    }

    #[test]
    fn test_restore_comm_refreshes_feedback() {
        let mut p = plant();
        p.inject(Fault::CommLoss);
        p.restore_comm(5_000);
        assert_eq!(p.state.last_feedback_epoch, 5_000);
    }
}

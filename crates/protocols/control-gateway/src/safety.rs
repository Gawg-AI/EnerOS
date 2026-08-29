//! Independent safety validator (CTL-008).
//!
//! This is the **independent** safety layer that sits between the gateway
//! (which proved authenticity/freshness/lease) and the device. It recomputes
//! whether a command is **physically safe to execute** given the current plant
//! state, using its **own** point table and constraint set — not the command
//! generator's data. Sharing the generator's failure point is explicitly
//! forbidden (MASTER §9.5): the validator must reach its verdict through an
//! independent code/data path.
//!
//! The constraints directly enforce the STPA unsafe-control-actions in
//! `STPA`:
//! - UCA-PCC-001: close PCC while grid is down / maintenance → deny.
//! - UCA-CHG-001: charge past SOC ceiling → deny.
//! - UCA-DIS-001: discharge below SOC floor → deny.
//! - UCA-PCS-001: inverter unblock on overcurrent → deny.
//! - UCA-PCC-002: PCC must open on grid loss (the validator refuses to *hold*
//!   PCC closed when grid is lost — modelled as denying a "keep closed" while
//!   grid is down).

use alloc::collections::BTreeMap;
use alloc::string::String;

use eneros_telemetry_model::command::{ControlCommand, DoubleCommand, SingleCommand};
use eneros_telemetry_model::point_spec::PointSpec;

use crate::gateway::AuthorizedCommand;

/// Semantic role of a controlled point. The validator maps
/// `(device_id, point_id) → SafetyRole` via its **own** table, independent of
/// whatever the command generator believed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafetyRole {
    /// 并网点断路器（合 = 并网，分 = 解列）。
    PccBreaker,
    /// 电池充电使能（On = 允许充电）。
    BatteryCharge,
    /// 电池放电使能（On = 允许放电）。
    BatteryDischarge,
    /// 变流器封波（On = 封波/停机）。
    InverterBlock,
    /// PV 出力削减（On = 削减）。
    PvCurtail,
    /// 无安全角色的点（默认拒绝写）。
    Unassigned,
}

/// The validator's independent point table entry.
#[derive(Debug, Clone, Copy)]
pub struct SafetyPoint {
    pub role: SafetyRole,
    pub spec: PointSpec,
}

/// Current plant context the validator consults. Provided by the feedback
/// reconciliation layer (CTL-010); the validator does NOT trust the command
/// generator's view of these values.
#[derive(Debug, Clone, Copy, Default)]
pub struct PlantContext {
    /// 电网是否带电（true=带电）。
    pub grid_up: bool,
    /// 电池 SOC（百分比 0..100）。
    pub soc_pct: f64,
    /// 直流母线电压（V）。
    pub dc_bus_v: f64,
    /// 当前是否过流。
    pub overcurrent: bool,
    /// 当前是否过温。
    pub overtemp: bool,
    /// 维护闭锁（true=禁止合闸）。
    pub maintenance_lockout: bool,
}

/// Validator verdict.
#[derive(Debug, Clone, PartialEq)]
pub enum SafetyVerdict {
    Safe,
    Unsafe(String),
}

impl SafetyVerdict {
    pub fn is_safe(&self) -> bool {
        matches!(self, SafetyVerdict::Safe)
    }
}

/// The independent safety validator. Owns the point table; stateless across
/// calls (context is passed in).
pub struct SafetyValidator {
    table: BTreeMap<(u32, u32), SafetyPoint>,
}

impl Default for SafetyValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl SafetyValidator {
    pub fn new() -> Self {
        Self {
            table: BTreeMap::new(),
        }
    }

    /// Register a point's safety role + spec. The validator only allows writes
    /// to points it has registered (default-deny for unassigned points).
    pub fn register(&mut self, device_id: u32, point_id: u32, point: SafetyPoint) {
        self.table.insert((device_id, point_id), point);
    }

    /// Look up the safety role the validator has assigned to a point. This is
    /// the **validator's own** view (independent of the command generator) and
    /// is used by the virtual plant (CTL-011) to map a command to its physical
    /// effect without consulting the command generator's data.
    pub fn role_of(&self, device_id: u32, point_id: u32) -> Option<SafetyRole> {
        self.table.get(&(device_id, point_id)).map(|p| p.role)
    }

    /// Validate a verified, lease-bound command against the current plant state.
    pub fn validate(&self, cmd: &AuthorizedCommand, ctx: &PlantContext) -> SafetyVerdict {
        // （蓝图/24 D4：cmd.device_id() 类型化为 u16，无损加宽作表 key）
        let key = (cmd.device_id() as u32, cmd.point_id());
        let Some(point) = self.table.get(&key) else {
            // Default-deny: unassigned points are never writable.
            return SafetyVerdict::Unsafe(String::from("point not in safety table"));
        };
        if point.role == SafetyRole::Unassigned {
            return SafetyVerdict::Unsafe(String::from("point has no safety role"));
        }

        match point.role {
            SafetyRole::PccBreaker => self.validate_pcc(cmd, ctx),
            SafetyRole::BatteryCharge => self.validate_charge(cmd, ctx, point),
            SafetyRole::BatteryDischarge => self.validate_discharge(cmd, ctx, point),
            SafetyRole::InverterBlock => self.validate_inverter_block(cmd, ctx),
            SafetyRole::PvCurtail => SafetyVerdict::Safe, // curtailing PV is always safe
            SafetyRole::Unassigned => SafetyVerdict::Unsafe(String::from("unassigned")),
        }
    }

    fn validate_pcc(&self, cmd: &AuthorizedCommand, ctx: &PlantContext) -> SafetyVerdict {
        // Closing PCC (Single On / Double On) requires grid up + no maintenance lockout.
        // 61 v0.314.25：模拟量命令无开关形态（None）→ 不构成合闸/使能。
        let is_close = matches!(
            cmd.command(),
            Some(ControlCommand::Single(SingleCommand::On))
                | Some(ControlCommand::Double(DoubleCommand::On))
        );
        if is_close {
            if ctx.maintenance_lockout {
                return SafetyVerdict::Unsafe(String::from(
                    "UCA-PCC-001: close during maintenance lockout",
                ));
            }
            if !ctx.grid_up {
                return SafetyVerdict::Unsafe(String::from(
                    "UCA-PCC-001: close PCC while grid down (islanding)",
                ));
            }
        }
        SafetyVerdict::Safe
    }

    fn validate_charge(
        &self,
        cmd: &AuthorizedCommand,
        ctx: &PlantContext,
        point: &SafetyPoint,
    ) -> SafetyVerdict {
        // Enabling charge (On) when SOC already at/above ceiling → overcharge.
        let enable = matches!(
            cmd.command(),
            Some(ControlCommand::Single(SingleCommand::On))
                | Some(ControlCommand::Double(DoubleCommand::On))
        );
        if enable && ctx.soc_pct >= point.spec.safe_max {
            return SafetyVerdict::Unsafe(String::from("UCA-CHG-001: charge past SOC ceiling"));
        }
        if ctx.overtemp {
            return SafetyVerdict::Unsafe(String::from("HZ-THERM-001: charge while overtemp"));
        }
        SafetyVerdict::Safe
    }

    fn validate_discharge(
        &self,
        cmd: &AuthorizedCommand,
        ctx: &PlantContext,
        point: &SafetyPoint,
    ) -> SafetyVerdict {
        let enable = matches!(
            cmd.command(),
            Some(ControlCommand::Single(SingleCommand::On))
                | Some(ControlCommand::Double(DoubleCommand::On))
        );
        if enable && ctx.soc_pct <= point.spec.safe_min {
            return SafetyVerdict::Unsafe(String::from("UCA-DIS-001: discharge below SOC floor"));
        }
        SafetyVerdict::Safe
    }

    fn validate_inverter_block(
        &self,
        cmd: &AuthorizedCommand,
        ctx: &PlantContext,
    ) -> SafetyVerdict {
        // UNblocking (turning block Off) while overcurrent → UCA-PCS-001.
        let unblock = matches!(
            cmd.command(),
            Some(ControlCommand::Single(SingleCommand::Off))
                | Some(ControlCommand::Double(DoubleCommand::Off))
        );
        if unblock && ctx.overcurrent {
            return SafetyVerdict::Unsafe(String::from(
                "UCA-PCS-001: unblock inverter while overcurrent",
            ));
        }
        SafetyVerdict::Safe
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use eneros_telemetry_model::command::CommandValue;
    use eneros_telemetry_model::command::{ControlCommand, DoubleCommand, SingleCommand};
    use eneros_telemetry_model::point_spec::{ControlDirection, PointSpec};
    use eneros_telemetry_model::unit::PhysicalUnit;

    use super::*;
    use crate::envelope::ActorKind;

    fn auth(device: u16, point: u32, cmd: ControlCommand) -> AuthorizedCommand {
        // 61 v0.314.25：命令类别由 typed value 驱动；开关位 → SwitchPosition。
        let value = match cmd {
            ControlCommand::Single(SingleCommand::On)
            | ControlCommand::Double(DoubleCommand::On) => CommandValue::SwitchPosition(1),
            _ => CommandValue::SwitchPosition(0),
        };
        AuthorizedCommand::from_parts(
            device,
            point,
            ActorKind::Agent,
            [0; 16],
            [0; 16],
            1,
            [0; 16],
            1,
            1,
            [0; 16],
            value,
            [0x11; 32],
        )
    }

    fn soc_spec(min: f64, max: f64) -> PointSpec {
        PointSpec {
            point_id: 0,
            device_id: 0,
            unit: PhysicalUnit::Percent,
            safe_min: min,
            safe_max: max,
            max_ramp_per_sec: Some(1.0),
            direction: ControlDirection::Increase,
            remote_controllable: true,
        }
    }

    fn validator() -> SafetyValidator {
        let mut v = SafetyValidator::new();
        v.register(
            1,
            10,
            SafetyPoint {
                role: SafetyRole::PccBreaker,
                spec: PointSpec {
                    unit: PhysicalUnit::Dimensionless,
                    safe_min: 0.0,
                    safe_max: 1.0,
                    max_ramp_per_sec: None,
                    direction: ControlDirection::Discrete,
                    remote_controllable: true,
                    point_id: 10,
                    device_id: 1,
                },
            },
        );
        v.register(
            2,
            20,
            SafetyPoint {
                role: SafetyRole::BatteryCharge,
                spec: soc_spec(0.0, 90.0),
            },
        );
        v.register(
            2,
            21,
            SafetyPoint {
                role: SafetyRole::BatteryDischarge,
                spec: soc_spec(10.0, 100.0),
            },
        );
        v.register(
            3,
            30,
            SafetyPoint {
                role: SafetyRole::InverterBlock,
                spec: PointSpec {
                    unit: PhysicalUnit::Dimensionless,
                    safe_min: 0.0,
                    safe_max: 1.0,
                    max_ramp_per_sec: None,
                    direction: ControlDirection::Discrete,
                    remote_controllable: true,
                    point_id: 30,
                    device_id: 3,
                },
            },
        );
        v
    }

    #[test]
    fn test_unassigned_point_denied() {
        let v = validator();
        let cmd = auth(99, 99, ControlCommand::Single(SingleCommand::On));
        let ctx = PlantContext::default();
        assert!(!v.validate(&cmd, &ctx).is_safe());
    }

    #[test]
    fn test_pcc_close_safe_when_grid_up() {
        let v = validator();
        let cmd = auth(1, 10, ControlCommand::Single(SingleCommand::On));
        let ctx = PlantContext {
            grid_up: true,
            soc_pct: 50.0,
            ..Default::default()
        };
        assert!(v.validate(&cmd, &ctx).is_safe());
    }

    #[test]
    fn test_pcc_close_denied_when_grid_down() {
        // UCA-PCC-001
        let v = validator();
        let cmd = auth(1, 10, ControlCommand::Single(SingleCommand::On));
        let ctx = PlantContext {
            grid_up: false,
            ..Default::default()
        };
        assert!(!v.validate(&cmd, &ctx).is_safe());
    }

    #[test]
    fn test_pcc_close_denied_during_maintenance() {
        let v = validator();
        let cmd = auth(1, 10, ControlCommand::Double(DoubleCommand::On));
        let ctx = PlantContext {
            grid_up: true,
            maintenance_lockout: true,
            ..Default::default()
        };
        assert!(!v.validate(&cmd, &ctx).is_safe());
    }

    #[test]
    fn test_charge_denied_at_soc_ceiling() {
        // UCA-CHG-001
        let v = validator();
        let cmd = auth(2, 20, ControlCommand::Single(SingleCommand::On));
        let ctx = PlantContext {
            soc_pct: 90.0,
            ..Default::default()
        };
        assert!(!v.validate(&cmd, &ctx).is_safe());
    }

    #[test]
    fn test_charge_safe_below_ceiling() {
        let v = validator();
        let cmd = auth(2, 20, ControlCommand::Single(SingleCommand::On));
        let ctx = PlantContext {
            soc_pct: 50.0,
            ..Default::default()
        };
        assert!(v.validate(&cmd, &ctx).is_safe());
    }

    #[test]
    fn test_charge_denied_overtemp() {
        // HZ-THERM-001
        let v = validator();
        let cmd = auth(2, 20, ControlCommand::Single(SingleCommand::On));
        let ctx = PlantContext {
            soc_pct: 50.0,
            overtemp: true,
            ..Default::default()
        };
        assert!(!v.validate(&cmd, &ctx).is_safe());
    }

    #[test]
    fn test_discharge_denied_at_soc_floor() {
        // UCA-DIS-001
        let v = validator();
        let cmd = auth(2, 21, ControlCommand::Single(SingleCommand::On));
        let ctx = PlantContext {
            soc_pct: 10.0,
            ..Default::default()
        };
        assert!(!v.validate(&cmd, &ctx).is_safe());
    }

    #[test]
    fn test_inverter_unblock_denied_on_overcurrent() {
        // UCA-PCS-001
        let v = validator();
        let cmd = auth(3, 30, ControlCommand::Single(SingleCommand::Off));
        let ctx = PlantContext {
            overcurrent: true,
            ..Default::default()
        };
        assert!(!v.validate(&cmd, &ctx).is_safe());
    }

    #[test]
    fn test_inverter_block_on_is_safe() {
        let v = validator();
        let cmd = auth(3, 30, ControlCommand::Single(SingleCommand::On));
        let ctx = PlantContext {
            overcurrent: true,
            ..Default::default()
        };
        assert!(v.validate(&cmd, &ctx).is_safe());
    }
}

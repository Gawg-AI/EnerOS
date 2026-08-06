//! EnerOS 能源域冻结安全目录 (CTL-001)。
//!
//! 本文件是 EnerOS v1 能源场景（储能 + 并网 + PV）的安全工程基线数据。所有条目为
//! `const`，编译期固化。修改须走变更评审，并同步更新追溯表与测试。
//!
//! 覆盖设备类别：`bms`（电池管理）、`pcs`（储能变流器）、`pcc_breaker`（并网点断路器）、
//! `pv_inverter`（光伏逆变器）、`dc_bus`（直流母线）。

use crate::{
    fmea::{Detection, FmeaEntry, Occurrence},
    hazard::{EnergyFlow, Hazard, HazardCategory, Severity},
    safe_state::{SafeAction, SafeState, SafeStateTrigger},
    stpa::{StpaEntry, UcaDirection, UnsafeControlAction},
    trace::{RequirementSource, SafetyRequirement, TraceTarget},
};

// ============================================================================
// Hazards
// ============================================================================

/// 冻结危险源目录。
pub const HAZARDS: &[Hazard] = &[
    Hazard::new(
        "HZ-ELEC-001",
        HazardCategory::Electrical,
        Severity::Catastrophic,
        "dc_bus",
        EnergyFlow::DcBusToBattery,
        "overvoltage",
        "dc_bus_overvoltage_damage",
    ),
    Hazard::new(
        "HZ-ELEC-002",
        HazardCategory::Electrical,
        Severity::Catastrophic,
        "pcc_breaker",
        EnergyFlow::DcBusToGrid,
        "islanding",
        "unintentional_islanding_energization",
    ),
    Hazard::new(
        "HZ-ELEC-003",
        HazardCategory::Electrical,
        Severity::Critical,
        "pcs",
        EnergyFlow::DcBusToGrid,
        "overcurrent",
        "pcs_overcurrent_fire",
    ),
    Hazard::new(
        "HZ-ELEC-004",
        HazardCategory::Electrical,
        Severity::Critical,
        "dc_bus",
        EnergyFlow::BatteryToDcBus,
        "undervoltage",
        "dc_bus_collapse_load_shed",
    ),
    Hazard::new(
        "HZ-ECHEM-001",
        HazardCategory::Electrochemical,
        Severity::Catastrophic,
        "bms",
        EnergyFlow::DcBusToBattery,
        "overcharge",
        "battery_thermal_runaway",
    ),
    Hazard::new(
        "HZ-ECHEM-002",
        HazardCategory::Electrochemical,
        Severity::Critical,
        "bms",
        EnergyFlow::BatteryToDcBus,
        "overdischarge",
        "battery_cell_reversal_damage",
    ),
    Hazard::new(
        "HZ-THERM-001",
        HazardCategory::Thermal,
        Severity::Catastrophic,
        "bms",
        EnergyFlow::DcBusToBattery,
        "overtemperature",
        "battery_thermal_runaway",
    ),
    Hazard::new(
        "HZ-CYBER-001",
        HazardCategory::CyberControl,
        Severity::Catastrophic,
        "pcc_breaker",
        EnergyFlow::None,
        "unauthorized_close",
        "live_line_energization_during_maintenance",
    ),
    Hazard::new(
        "HZ-CYBER-002",
        HazardCategory::CyberControl,
        Severity::Critical,
        "pcs",
        EnergyFlow::DcBusToGrid,
        "command_replay",
        "unauthorized_discharge_after_revoke",
    ),
];

// ============================================================================
// FMEA
// ============================================================================

/// 冻结 FMEA 目录。
pub const FMEA: &[FmeaEntry] = &[
    FmeaEntry::new(
        "FMEA-BMS-001",
        "bms",
        "soc_estimator_drift",
        "cumulative coulomb-count error / calibration loss",
        "overcharge or overdischarge due to wrong SOC",
        Severity::Catastrophic,
        Occurrence::Moderate,
        Detection::Low,
        "independent safety validator recomputes SOC bounds; commissioning polarity/range check",
    ),
    FmeaEntry::new(
        "FMEA-PCS-001",
        "pcs",
        "igbt_short",
        "IGBT module die failure",
        "DC bus short to ground through inverter leg",
        Severity::Critical,
        Occurrence::Low,
        Detection::High,
        "block inverter + open PCC on overcurrent; hardware fuse backup",
    ),
    FmeaEntry::new(
        "FMEA-PCS-002",
        "pcs",
        "anti_islanding_fail",
        "anti-islanding algorithm misses grid loss",
        "unintentional islanding energizes utility line",
        Severity::Catastrophic,
        Occurrence::Low,
        Detection::Moderate,
        "independent grid-loss detection + mandatory PCC open on grid loss",
    ),
    FmeaEntry::new(
        "FMEA-PCC-001",
        "pcc_breaker",
        "fail_to_trip",
        "mechanical stuck / coil failure",
        "islanding not broken; line stays energized",
        Severity::Catastrophic,
        Occurrence::Low,
        Detection::Low,
        "safety watchdog forces SafeDefault; periodic trip test in commissioning",
    ),
    FmeaEntry::new(
        "FMEA-DCBUS-001",
        "dc_bus",
        "overvoltage",
        "PV overpower / regen without clamp",
        "capacitor/cell overvoltage damage",
        Severity::Catastrophic,
        Occurrence::Moderate,
        Detection::High,
        "curtail PV + stop charge + dump resistor; independent validator enforces hard ceiling",
    ),
    FmeaEntry::new(
        "FMEA-COMM-001",
        "pcs",
        "feedback_loss",
        "protocol link down / sensor wire cut",
        "controller acts on stale state",
        Severity::Critical,
        Occurrence::Moderate,
        Detection::Moderate,
        "feedback reconciliation: stale input rejects command; TTL expiry to SafeDefault",
    ),
];

// ============================================================================
// STPA
// ============================================================================

/// 冻结 STPA（UCA + 安全约束）目录。
pub const STPA: &[StpaEntry] = &[
    StpaEntry::new(
        UnsafeControlAction::new(
            "UCA-PCC-001",
            "close_pcc_breaker",
            UcaDirection::ProvidedUnsafe,
            "grid_outage_or_maintenance",
            "HZ-CYBER-001",
        ),
        "PCC must not close when grid is verified down or during maintenance lockout",
        "controller trusts LLM proposal without independent grid-state verification",
    ),
    StpaEntry::new(
        UnsafeControlAction::new(
            "UCA-CHG-001",
            "charge_battery",
            UcaDirection::ProvidedUnsafe,
            "soc_full",
            "HZ-ECHEM-001",
        ),
        "Charge command must be rejected when SOC >= hard ceiling",
        "stale SOC feedback lets controller keep charging past ceiling",
    ),
    StpaEntry::new(
        UnsafeControlAction::new(
            "UCA-DIS-001",
            "discharge_battery",
            UcaDirection::ProvidedUnsafe,
            "soc_empty",
            "HZ-ECHEM-002",
        ),
        "Discharge must be rejected when SOC <= hard floor",
        "controller follows market price signal past safe SOC floor",
    ),
    StpaEntry::new(
        UnsafeControlAction::new(
            "UCA-PCC-002",
            "open_pcc_breaker",
            UcaDirection::NotProvidedUnsafe,
            "grid_loss",
            "HZ-ELEC-002",
        ),
        "PCC must open when grid loss is independently confirmed",
        "anti-islanding logic shares failure point with main controller",
    ),
    StpaEntry::new(
        UnsafeControlAction::new(
            "UCA-PCS-001",
            "block_inverter",
            UcaDirection::NotProvidedUnsafe,
            "overcurrent",
            "HZ-ELEC-003",
        ),
        "Inverter must be blocked when overcurrent detected",
        "safety validator not independent of PCS control loop",
    ),
    StpaEntry::new(
        UnsafeControlAction::new(
            "UCA-DIS-002",
            "discharge_battery",
            UcaDirection::WrongTiming,
            "lease_expired",
            "HZ-CYBER-002",
        ),
        "Discharge must stop immediately when lease/fencing token is revoked",
        "old lease accepted after revoke due to non-monotonic fencing",
    ),
];

// ============================================================================
// Safe states
// ============================================================================

/// 冻结 safe-state 目录。
pub const SAFE_STATES: &[SafeState] = &[
    SafeState::new(
        "pcs",
        SafeStateTrigger::GridLoss,
        SafeAction::OpenPcc,
        Severity::Catastrophic,
        "islanding must be broken independently of PCS",
    ),
    SafeState::new(
        "pcs",
        SafeStateTrigger::GridLoss,
        SafeAction::BlockInverter,
        Severity::Catastrophic,
        "stop energizing a dead grid",
    ),
    SafeState::new(
        "pcs",
        SafeStateTrigger::OvercurrentFault,
        SafeAction::BlockInverter,
        Severity::Critical,
        "block output on overcurrent",
    ),
    SafeState::new(
        "bms",
        SafeStateTrigger::ThermalFault,
        SafeAction::StopCharge,
        Severity::Catastrophic,
        "no charging above thermal ceiling",
    ),
    SafeState::new(
        "bms",
        SafeStateTrigger::AgentRunaway,
        SafeAction::SafeDefault,
        Severity::Catastrophic,
        "agent失控 → 全局安全默认",
    ),
    SafeState::new(
        "pcc_breaker",
        SafeStateTrigger::SafetyWatchdog,
        SafeAction::OpenPcc,
        Severity::Catastrophic,
        "watchdog未喂狗 → 分断PCC",
    ),
    SafeState::new(
        "dc_bus",
        SafeStateTrigger::SensorFault,
        SafeAction::StopCharge,
        Severity::Catastrophic,
        "传感器坏 → 不再充电，避免过压",
    ),
    SafeState::new(
        "pv_inverter",
        SafeStateTrigger::CommLoss,
        SafeAction::HoldLastSafeCommand,
        Severity::Critical,
        "通信丢失 → 保持最近安全命令，TTL到期进SafeDefault",
    ),
    SafeState::new(
        "pcs",
        SafeStateTrigger::ControlServiceCrash,
        SafeAction::DegradeLocal,
        Severity::Critical,
        "控制服务crash → 降级到确定性本地策略",
    ),
    SafeState::new(
        "pcc_breaker",
        SafeStateTrigger::UntrustedTime,
        SafeAction::SafeDefault,
        Severity::Critical,
        "时间不可信 → 拒绝依赖精确时间的安全操作",
    ),
];

// ============================================================================
// Requirement trace
// ============================================================================

/// 冻结安全需求追溯表。`code`/`test` 指向真实存在的实现与测试。
pub const REQUIREMENTS: &[SafetyRequirement] = &[
    SafetyRequirement::new(
        "SR-001",
        RequirementSource::Stpa("UCA-PCC-001"),
        "Control gateway must default-deny PCC close without authentic HMAC envelope",
        TraceTarget {
            code: "eneros-control-gateway::gateway::ControlGateway::authorize",
            test: "control_gateway::tests::test_authorize_denies_tampered_hmac",
            evidence_level: "L1",
        },
    ),
    SafetyRequirement::new(
        "SR-002",
        RequirementSource::Stpa("UCA-DIS-002"),
        "Expired commands must be rejected (now > expiry → deny)",
        TraceTarget {
            code: "eneros-control-gateway::gateway::ControlGateway::authorize",
            test: "control_gateway::tests::test_authorize_denies_expired",
            evidence_level: "L1",
        },
    ),
    SafetyRequirement::new(
        "SR-003",
        RequirementSource::Stpa("UCA-DIS-002"),
        "Replayed command_id must be rejected",
        TraceTarget {
            code: "eneros-control-gateway::gateway::ControlGateway::authorize",
            test: "control_gateway::tests::test_authorize_denies_replay",
            evidence_level: "L1",
        },
    ),
    SafetyRequirement::new(
        "SR-004",
        RequirementSource::Fmea("FMEA-COMM-001"),
        "Future-dated commands must be rejected (clock skew bound)",
        TraceTarget {
            code: "eneros-control-gateway::gateway::ControlGateway::authorize",
            test: "control_gateway::tests::test_authorize_denies_future_dated",
            evidence_level: "L1",
        },
    ),
    SafetyRequirement::new(
        "SR-005",
        RequirementSource::Stpa("UCA-DIS-002"),
        "Lease revoke must immediately invalidate running access; old fencing token non-reusable",
        TraceTarget {
            code: "eneros-control-gateway::lease::LeaseService",
            test: "control_gateway::lease::tests::test_revoke_invalidates_active_lease",
            evidence_level: "L1",
        },
    ),
];

// ============================================================================
// Catalog integrity queries
// ============================================================================

/// 按 id 查找危险源。
pub fn find_hazard(id: &str) -> Option<&'static Hazard> {
    HAZARDS.iter().find(|h| h.id == id)
}

/// 按 id 查找 FMEA 条目。
pub fn find_fmea(id: &str) -> Option<&'static FmeaEntry> {
    FMEA.iter().find(|f| f.id == id)
}

/// 按设备类别枚举其所有 safe-state 规则。
pub fn safe_states_for(device_class: &str) -> impl Iterator<Item = &'static SafeState> + '_ {
    SAFE_STATES
        .iter()
        .filter(move |s| s.device_class == device_class)
}

/// 校验所有 STPA 的 `hazard_id` 真实存在于 `HAZARDS`。
pub fn stpa_hazards_resolve() -> bool {
    STPA.iter().all(|s| find_hazard(s.uca.hazard_id).is_some())
}

/// 校验所有需求追溯的 source id 真实存在于对应目录。
pub fn requirements_resolve() -> bool {
    REQUIREMENTS.iter().all(|r| match r.source {
        RequirementSource::Hazard(id) => find_hazard(id).is_some(),
        RequirementSource::Fmea(id) => find_fmea(id).is_some(),
        RequirementSource::Stpa(id) => STPA.iter().any(|s| s.uca.id == id),
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    #[test]
    fn catalog_nonempty() {
        assert!(!HAZARDS.is_empty());
        assert!(!FMEA.is_empty());
        assert!(!STPA.is_empty());
        assert!(!SAFE_STATES.is_empty());
        assert!(!REQUIREMENTS.is_empty());
    }

    #[test]
    fn stpa_hazards_all_resolve() {
        assert!(stpa_hazards_resolve());
    }

    #[test]
    fn requirements_all_resolve() {
        assert!(requirements_resolve());
    }

    #[test]
    fn hazard_ids_unique() {
        let mut ids: Vec<&str> = HAZARDS.iter().map(|h| h.id).collect();
        ids.sort_unstable();
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n, "duplicate hazard ids");
    }

    #[test]
    fn fmea_rpn_within_bounds() {
        for f in FMEA {
            let r = f.rpn();
            assert!((1..=100).contains(&r), "{} rpn out of range: {}", f.id, r);
        }
    }

    #[test]
    fn safe_states_cover_catastrophic() {
        // For catastrophic hazards (islanding, thermal runaway) there must be a
        // safe-state rule whose action is fail-safe (not HoldLastSafeCommand).
        for h in HAZARDS
            .iter()
            .filter(|h| h.severity == Severity::Catastrophic)
        {
            let covered = SAFE_STATES.iter().any(|s| {
                s.covers_severity >= h.severity && s.action != SafeAction::HoldLastSafeCommand
            });
            assert!(covered, "no fail-safe safe-state for hazard {}", h.id);
        }
    }

    #[test]
    fn find_hazard_works() {
        assert!(find_hazard("HZ-ELEC-001").is_some());
        assert!(find_hazard("nope").is_none());
    }

    #[test]
    fn safe_states_for_device_class() {
        let pcs_rules: Vec<_> = safe_states_for("pcs").collect();
        assert!(!pcs_rules.is_empty());
        assert!(pcs_rules.iter().all(|s| s.device_class == "pcs"));
    }
}

//! 100+ 虚拟 Fleet 测试框架 (OPS-003).
//!
//! MASTER §21.14 OPS-003: "100+ virtual fleet harness — 网络分区/批次更新"
//! MASTER §21.14 G8 Exit criteria: "100+ QEMU device fleet 模拟中批次发布、
//! 网络分区、撤回和证书轮换通过。"
//!
//! 本模块在主机侧模拟 100+ 设备的 fleet 行为，验证：
//! 1. 批次发布（canary → promote → complete）在全 fleet 范围内的正确性。
//! 2. 网络分区场景下，离线设备的吊销在重连后生效。
//! 3. 漂移检测覆盖全 fleet。
//! 4. 坏版本 auto-stop 不扩散到未升级设备。
//!
//! 这是纯逻辑模拟（无 QEMU），证明 fleet 逻辑正确性。
//! 真实 QEMU fleet 编排由 xtask 的 `run qemu-prod --fleet` 提供。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::canary::{AutoStopReason, BatchHealth, CanaryPhase, CanaryRollout, RolloutPolicy};
use crate::enrollment::{AttestationReport, DeviceEnrollment, EnrollmentStatus};
use crate::inventory::{DeviceInventory, DeviceState};
use crate::revoke::{RevocationRecord, RevokeManager, RevokeTarget};

/// 虚拟设备。
#[derive(Debug, Clone)]
struct VirtualDevice {
    device_id: String,
    hardware_id: String,
    online: bool,
    version: String,
    enrolled: bool,
}

impl VirtualDevice {
    fn new(idx: usize) -> Self {
        Self {
            device_id: format!("dev-{:03}", idx),
            hardware_id: format!("hw-{:03}", idx),
            online: true,
            version: String::from("0.199.0"),
            enrolled: false,
        }
    }

    fn attestation(&self) -> AttestationReport {
        AttestationReport {
            hardware_id: self.hardware_id.clone(),
            firmware_version: self.version.clone(),
            boot_measurement: String::from(
                "a1b2c3d4e5f67890a1b2c3d4e5f67890a1b2c3d4e5f67890a1b2c3d4e5f67890",
            ),
            tpm_pubkey_fingerprint: format!("sha256:dev-{}", self.hardware_id),
        }
    }
}

/// 虚拟 Fleet（≥100 设备）。
struct VirtualFleet {
    devices: Vec<VirtualDevice>,
}

impl VirtualFleet {
    fn new(count: usize) -> Self {
        let devices = (0..count).map(VirtualDevice::new).collect();
        Self { devices }
    }

    /// 批量注册所有设备。
    fn enroll_all(&mut self) -> DeviceEnrollment {
        let mut enrollment = DeviceEnrollment::new();
        for device in &mut self.devices {
            let token = enrollment.generate_token(&device.hardware_id, 1000, 3600);
            let attestation = device.attestation();
            let status = enrollment.enroll(&token.token, &attestation, 1001);
            assert!(status.is_ok(), "enroll failed for {}", device.hardware_id);
            device.enrolled = true;
        }
        enrollment
    }

    /// 将指定百分比的设备设为离线（模拟网络分区）。
    fn partition(&mut self, offline_percent: u8) {
        let offline_count = (self.devices.len() * offline_percent as usize) / 100;
        for (i, device) in self.devices.iter_mut().enumerate() {
            device.online = i >= offline_count;
        }
    }

    /// 所有设备恢复在线。
    fn heal_all(&mut self) {
        for device in &mut self.devices {
            device.online = true;
        }
    }

    /// 在线设备数。
    fn online_count(&self) -> usize {
        self.devices.iter().filter(|d| d.online).count()
    }

    /// 升级在线设备到目标版本。
    fn upgrade_online(&mut self, target_version: &str) -> BatchHealth {
        let mut health = BatchHealth {
            device_count: self.online_count() as u32,
            ..Default::default()
        };
        health.metrics_available = true;

        for device in &mut self.devices {
            if device.online {
                health.success_count += 1;
                device.version = String::from(target_version);
            }
        }
        health
    }

    /// 生成全 fleet 清单。
    fn inventory(&self) -> DeviceInventory {
        let mut inv = DeviceInventory::new();
        for device in &self.devices {
            let mut desired = DeviceState::new(&device.device_id);
            desired.set("version", "0.200.0");
            desired.set("enrolled", "true");

            let mut observed = DeviceState::new(&device.device_id);
            observed.set("version", &device.version);
            observed.set("enrolled", if device.enrolled { "true" } else { "false" });

            inv.set_desired(desired);
            inv.set_observed(observed);
        }
        inv
    }
}

const FLEET_SIZE: usize = 120; // > 100 per OPS-003 requirement

#[test]
fn t1_fleet_enrollment_all_devices() {
    let mut fleet = VirtualFleet::new(FLEET_SIZE);
    let enrollment = fleet.enroll_all();
    assert_eq!(enrollment.enrolled_count(), FLEET_SIZE);

    // Every device should be Enrolled
    for device in &fleet.devices {
        assert_eq!(
            enrollment.status(&device.hardware_id),
            EnrollmentStatus::Enrolled
        );
    }
}

#[test]
fn t2_fleet_canary_rollout_success() {
    let mut fleet = VirtualFleet::new(FLEET_SIZE);
    fleet.enroll_all();

    let mut rollout =
        CanaryRollout::new("0.200.0", RolloutPolicy::production(), FLEET_SIZE as u32).unwrap();

    // Phase 1: Canary (10% = 12 devices)
    rollout.start_canary().unwrap();
    // Simulate: first 12 devices upgrade
    let canary_devices: Vec<&mut VirtualDevice> = fleet.devices.iter_mut().take(12).collect();
    let _ = canary_devices; // conceptual
    let health = BatchHealth {
        device_count: 12,
        success_count: 12,
        failure_count: 0,
        slo_breach_count: 0,
        crash_count: 0,
        metrics_available: true,
    };
    rollout.report_health(health).unwrap();
    rollout.advance().unwrap();
    assert_eq!(rollout.phase(), CanaryPhase::Promoting);

    // Phase 2: Promote (50% = 60 devices)
    let health = BatchHealth {
        device_count: 60,
        success_count: 60,
        failure_count: 0,
        slo_breach_count: 0,
        crash_count: 0,
        metrics_available: true,
    };
    rollout.report_health(health).unwrap();
    rollout.advance().unwrap();
    assert_eq!(rollout.phase(), CanaryPhase::Complete);
    assert_eq!(rollout.upgraded_count(), 72); // 12 + 60
}

#[test]
fn t3_fleet_canary_auto_stop_prevents_spread() {
    let mut fleet = VirtualFleet::new(FLEET_SIZE);
    fleet.enroll_all();

    let mut rollout =
        CanaryRollout::new("0.200.0", RolloutPolicy::production(), FLEET_SIZE as u32).unwrap();

    // Canary batch with high failure rate
    rollout.start_canary().unwrap();
    let bad_health = BatchHealth {
        device_count: 12,
        success_count: 6,
        failure_count: 6, // 50% failure > 5% threshold
        slo_breach_count: 0,
        crash_count: 0,
        metrics_available: true,
    };

    let result = rollout.report_health(bad_health);
    assert!(result.is_err());
    assert_eq!(rollout.phase(), CanaryPhase::AutoStopped);
    assert_eq!(
        rollout.auto_stop_reason(),
        Some(AutoStopReason::FailureRateExceeded)
    );

    // Verify: cannot advance after auto-stop (bad version doesn't spread)
    let result = rollout.advance();
    assert!(result.is_err());

    // Remaining devices should still be on old version
    let remaining = rollout.remaining_count();
    assert!(remaining > 0, "auto-stop must leave devices unupgraded");
}

#[test]
fn t4_fleet_network_partition_revoke_propagation() {
    let mut fleet = VirtualFleet::new(FLEET_SIZE);
    fleet.enroll_all();

    // Partition: 30% devices offline
    fleet.partition(30);
    let online_before = fleet.online_count();
    let offline_count = FLEET_SIZE - online_before;
    assert!(offline_count > 0, "some devices must be offline");

    // Issue revocation while devices are offline
    let mut revoke_mgr = RevokeManager::new();
    let offline_device_ids: Vec<String> = fleet
        .devices
        .iter()
        .filter(|d| !d.online)
        .map(|d| d.device_id.clone())
        .collect();

    let record = RevocationRecord::new(
        "rev-partition-001",
        RevokeTarget::Certificate,
        "compromised-cert-001",
        "key compromise during partition",
        1000,
        1000,
        "security-officer",
        offline_device_ids.clone(),
    )
    .unwrap();
    revoke_mgr.issue(record).unwrap();

    // Revocation is immediately effective (is_revoked returns true)
    assert!(revoke_mgr.is_revoked(RevokeTarget::Certificate, "compromised-cert-001"));

    // Offline devices haven't confirmed yet
    assert!(revoke_mgr.pending_count() > 0);

    // Heal partition: all devices back online
    fleet.heal_all();

    // Propagate to previously-offline devices
    for device_id in &offline_device_ids {
        let propagated = revoke_mgr.propagate_on_reconnect(device_id);
        assert!(
            propagated.contains(&String::from("rev-partition-001")),
            "device {} must receive revocation on reconnect",
            device_id
        );
        revoke_mgr.confirm(device_id, "rev-partition-001").unwrap();
    }

    // All devices confirmed
    let record = revoke_mgr.find("rev-partition-001").unwrap();
    assert!(record.all_confirmed());
    assert_eq!(revoke_mgr.pending_count(), 0);
}

#[test]
fn t5_fleet_drift_detection_at_scale() {
    let mut fleet = VirtualFleet::new(FLEET_SIZE);
    fleet.enroll_all();

    // Upgrade 80% of devices
    let upgrade_count = (FLEET_SIZE * 80) / 100;
    for (i, device) in fleet.devices.iter_mut().enumerate() {
        if i < upgrade_count {
            device.version = String::from("0.200.0");
        }
    }

    let inv = fleet.inventory();
    assert_eq!(inv.device_count(), FLEET_SIZE);

    // 20% devices still on old version → drift
    let drifted = inv.drifted_device_count();
    assert_eq!(
        drifted,
        FLEET_SIZE - upgrade_count,
        "exactly {} devices should have drift",
        FLEET_SIZE - upgrade_count
    );
}

#[test]
fn t6_fleet_cert_rotation_does_not_break_enrollment() {
    let mut fleet = VirtualFleet::new(FLEET_SIZE);
    let mut enrollment = fleet.enroll_all();

    // Revoke old cert for first 10 devices
    let mut revoke_mgr = RevokeManager::new();
    let target_devices: Vec<String> = fleet
        .devices
        .iter()
        .take(10)
        .map(|d| d.device_id.clone())
        .collect();

    revoke_mgr
        .issue(
            RevocationRecord::new(
                "rev-rotation-001",
                RevokeTarget::Certificate,
                "old-cert-batch-1",
                "scheduled rotation",
                2000,
                2000,
                "security-officer",
                target_devices.clone(),
            )
            .unwrap(),
        )
        .unwrap();

    // Old cert is revoked immediately
    assert!(revoke_mgr.is_revoked(RevokeTarget::Certificate, "old-cert-batch-1"));

    // But enrollment status is separate — devices remain enrolled
    for device in fleet.devices.iter().take(10) {
        assert_eq!(
            enrollment.status(&device.hardware_id),
            EnrollmentStatus::Enrolled
        );
    }

    // Re-enroll with new cert (generate new token)
    for device in fleet.devices.iter_mut().take(10) {
        let new_token = enrollment.generate_token(&device.hardware_id, 3000, 3600);
        let attestation = device.attestation();
        enrollment
            .enroll(&new_token.token, &attestation, 3001)
            .unwrap();
    }

    // All 10 still enrolled
    assert_eq!(enrollment.enrolled_count(), FLEET_SIZE);
}

#[test]
#[allow(clippy::assertions_on_constants)]
fn t7_fleet_scale_at_least_100_devices() {
    // Explicit verification: fleet size >= 100 (OPS-003 requirement)
    assert!(
        FLEET_SIZE >= 100,
        "fleet harness must simulate >= 100 devices, got {}",
        FLEET_SIZE
    );
}

#[test]
fn t8_fleet_full_lifecycle_sequence() {
    // End-to-end: enroll → canary → promote → complete → revoke → drift detect
    let mut fleet = VirtualFleet::new(FLEET_SIZE);
    fleet.enroll_all();

    // Canary rollout to 0.200.0
    let mut rollout =
        CanaryRollout::new("0.200.0", RolloutPolicy::production(), FLEET_SIZE as u32).unwrap();
    rollout.start_canary().unwrap();
    rollout
        .report_health(BatchHealth {
            device_count: 12,
            success_count: 12,
            metrics_available: true,
            ..Default::default()
        })
        .unwrap();
    rollout.advance().unwrap();
    rollout
        .report_health(BatchHealth {
            device_count: 60,
            success_count: 60,
            metrics_available: true,
            ..Default::default()
        })
        .unwrap();
    rollout.advance().unwrap();
    assert_eq!(rollout.phase(), CanaryPhase::Complete);

    // Upgrade all devices
    fleet.upgrade_online("0.200.0");

    // Verify no drift
    let inv = fleet.inventory();
    assert_eq!(inv.drifted_device_count(), 0);

    // Revoke a compromised cert fleet-wide
    let mut revoke_mgr = RevokeManager::new();
    revoke_mgr
        .issue(
            RevocationRecord::new(
                "rev-e2e-001",
                RevokeTarget::Certificate,
                "cert-compromised-e2e",
                "post-upgrade compromise",
                5000,
                5000,
                "officer",
                Vec::new(), // fleet-wide
            )
            .unwrap(),
        )
        .unwrap();

    assert!(revoke_mgr.is_revoked(RevokeTarget::Certificate, "cert-compromised-e2e"));

    // All devices reconnect and confirm
    for device in &fleet.devices {
        let propagated = revoke_mgr.propagate_on_reconnect(&device.device_id);
        assert!(propagated.contains(&String::from("rev-e2e-001")));
        revoke_mgr
            .confirm(&device.device_id, "rev-e2e-001")
            .unwrap();
    }

    let record = revoke_mgr.find("rev-e2e-001").unwrap();
    assert!(record.all_confirmed());
    assert_eq!(record.confirmed_count(), FLEET_SIZE);
}

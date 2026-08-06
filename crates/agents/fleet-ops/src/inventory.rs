//! 设备清单与漂移检测 (OPS-002).
//!
//! 维护设备 desired state（期望状态）和 observed state（实际状态），
//! 检测两者之间的漂移（drift）。

use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

/// 设备状态键值对。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceState {
    /// 设备 ID。
    pub device_id: String,
    /// 状态属性（key-value）。
    pub properties: Vec<(String, String)>,
}

impl DeviceState {
    pub fn new(device_id: &str) -> Self {
        Self {
            device_id: String::from(device_id),
            properties: Vec::new(),
        }
    }

    pub fn set(&mut self, key: &str, value: &str) {
        if let Some(entry) = self.properties.iter_mut().find(|(k, _)| k == key) {
            entry.1 = String::from(value);
        } else {
            self.properties
                .push((String::from(key), String::from(value)));
        }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.properties
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// 漂移类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriftType {
    /// 期望值与实际值不同。
    ValueMismatch,
    /// 期望有的属性实际缺失。
    MissingProperty,
    /// 实际有但期望没有的属性。
    UnexpectedProperty,
}

/// 漂移记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateDrift {
    /// 设备 ID。
    pub device_id: String,
    /// 漂移类型。
    pub drift_type: DriftType,
    /// 属性键。
    pub key: String,
    /// 期望值。
    pub desired_value: Option<String>,
    /// 实际值。
    pub observed_value: Option<String>,
}

impl StateDrift {
    pub fn label(&self) -> &'static str {
        match self.drift_type {
            DriftType::ValueMismatch => "VALUE_MISMATCH",
            DriftType::MissingProperty => "MISSING_PROPERTY",
            DriftType::UnexpectedProperty => "UNEXPECTED_PROPERTY",
        }
    }
}

/// 设备清单管理器。
pub struct DeviceInventory {
    /// 期望状态（按设备 ID 索引）。
    desired: Vec<DeviceState>,
    /// 实际状态（按设备 ID 索引）。
    observed: Vec<DeviceState>,
}

impl Default for DeviceInventory {
    fn default() -> Self {
        Self::new()
    }
}

impl DeviceInventory {
    pub fn new() -> Self {
        Self {
            desired: Vec::new(),
            observed: Vec::new(),
        }
    }

    /// 设置设备期望状态。
    pub fn set_desired(&mut self, state: DeviceState) {
        self.desired.retain(|s| s.device_id != state.device_id);
        self.desired.push(state);
    }

    /// 更新设备实际状态。
    pub fn set_observed(&mut self, state: DeviceState) {
        self.observed.retain(|s| s.device_id != state.device_id);
        self.observed.push(state);
    }

    /// 检测指定设备的漂移。
    pub fn detect_drift(&self, device_id: &str) -> Vec<StateDrift> {
        let desired = self.desired.iter().find(|s| s.device_id == device_id);
        let observed = self.observed.iter().find(|s| s.device_id == device_id);

        match (desired, observed) {
            (Some(d), Some(o)) => {
                let mut drifts = Vec::new();

                // 检查期望属性
                for (key, desired_val) in &d.properties {
                    match o.get(key) {
                        Some(observed_val) => {
                            if desired_val != observed_val {
                                drifts.push(StateDrift {
                                    device_id: String::from(device_id),
                                    drift_type: DriftType::ValueMismatch,
                                    key: key.clone(),
                                    desired_value: Some(desired_val.clone()),
                                    observed_value: Some(observed_val.to_string()),
                                });
                            }
                        }
                        None => {
                            drifts.push(StateDrift {
                                device_id: String::from(device_id),
                                drift_type: DriftType::MissingProperty,
                                key: key.clone(),
                                desired_value: Some(desired_val.clone()),
                                observed_value: None,
                            });
                        }
                    }
                }

                // 检查意外属性
                for (key, observed_val) in &o.properties {
                    if d.get(key).is_none() {
                        drifts.push(StateDrift {
                            device_id: String::from(device_id),
                            drift_type: DriftType::UnexpectedProperty,
                            key: key.clone(),
                            desired_value: None,
                            observed_value: Some(observed_val.clone()),
                        });
                    }
                }

                drifts
            }
            _ => Vec::new(),
        }
    }

    /// 检测所有设备的漂移。
    pub fn detect_all_drifts(&self) -> Vec<StateDrift> {
        let mut all = Vec::new();
        for state in &self.desired {
            all.extend(self.detect_drift(&state.device_id));
        }
        all
    }

    /// 返回设备数。
    pub fn device_count(&self) -> usize {
        self.desired.len()
    }

    /// 返回有漂移的设备数。
    pub fn drifted_device_count(&self) -> usize {
        self.desired
            .iter()
            .filter(|s| !self.detect_drift(&s.device_id).is_empty())
            .count()
    }
}

#[cfg(test)]
mod tests {
    use alloc::format;

    use super::*;

    #[test]
    fn no_drift_when_states_match() {
        let mut inv = DeviceInventory::new();
        let mut desired = DeviceState::new("dev-001");
        desired.set("version", "0.200.0");
        desired.set("agent_count", "8");

        let mut observed = DeviceState::new("dev-001");
        observed.set("version", "0.200.0");
        observed.set("agent_count", "8");

        inv.set_desired(desired);
        inv.set_observed(observed);

        assert!(inv.detect_drift("dev-001").is_empty());
    }

    #[test]
    fn value_mismatch_detected() {
        let mut inv = DeviceInventory::new();
        let mut desired = DeviceState::new("dev-001");
        desired.set("version", "0.200.0");

        let mut observed = DeviceState::new("dev-001");
        observed.set("version", "0.199.0");

        inv.set_desired(desired);
        inv.set_observed(observed);

        let drifts = inv.detect_drift("dev-001");
        assert_eq!(drifts.len(), 1);
        assert_eq!(drifts[0].drift_type, DriftType::ValueMismatch);
        assert_eq!(drifts[0].key, "version");
    }

    #[test]
    fn missing_property_detected() {
        let mut inv = DeviceInventory::new();
        let mut desired = DeviceState::new("dev-001");
        desired.set("version", "0.200.0");
        desired.set("config_hash", "abc123");

        let mut observed = DeviceState::new("dev-001");
        observed.set("version", "0.200.0");
        // config_hash 缺失

        inv.set_desired(desired);
        inv.set_observed(observed);

        let drifts = inv.detect_drift("dev-001");
        assert_eq!(drifts.len(), 1);
        assert_eq!(drifts[0].drift_type, DriftType::MissingProperty);
        assert_eq!(drifts[0].key, "config_hash");
    }

    #[test]
    fn unexpected_property_detected() {
        let mut inv = DeviceInventory::new();
        let mut desired = DeviceState::new("dev-001");
        desired.set("version", "0.200.0");

        let mut observed = DeviceState::new("dev-001");
        observed.set("version", "0.200.0");
        observed.set("debug_mode", "true"); // 不应该有的属性

        inv.set_desired(desired);
        inv.set_observed(observed);

        let drifts = inv.detect_drift("dev-001");
        assert_eq!(drifts.len(), 1);
        assert_eq!(drifts[0].drift_type, DriftType::UnexpectedProperty);
        assert_eq!(drifts[0].key, "debug_mode");
    }

    #[test]
    fn multiple_drifts_detected() {
        let mut inv = DeviceInventory::new();
        let mut desired = DeviceState::new("dev-001");
        desired.set("version", "0.200.0");
        desired.set("agent_count", "8");
        desired.set("config_hash", "abc123");

        let mut observed = DeviceState::new("dev-001");
        observed.set("version", "0.199.0"); // mismatch
        observed.set("agent_count", "8"); // ok
                                          // config_hash missing
        observed.set("debug_mode", "true"); // unexpected

        inv.set_desired(desired);
        inv.set_observed(observed);

        let drifts = inv.detect_drift("dev-001");
        assert_eq!(drifts.len(), 3);
    }

    #[test]
    fn detect_all_drifts_across_devices() {
        let mut inv = DeviceInventory::new();

        let mut d1 = DeviceState::new("dev-001");
        d1.set("version", "0.200.0");
        let mut o1 = DeviceState::new("dev-001");
        o1.set("version", "0.199.0");

        let mut d2 = DeviceState::new("dev-002");
        d2.set("version", "0.200.0");
        let mut o2 = DeviceState::new("dev-002");
        o2.clone_from(&d2); // no drift

        inv.set_desired(d1);
        inv.set_observed(o1);
        inv.set_desired(d2);
        inv.set_observed(o2);

        let all = inv.detect_all_drifts();
        assert_eq!(all.len(), 1); // only dev-001 has drift
        assert_eq!(all[0].device_id, "dev-001");
    }

    #[test]
    fn drifted_device_count() {
        let mut inv = DeviceInventory::new();

        for i in 0..5 {
            let id = format!("dev-{:03}", i);
            let mut desired = DeviceState::new(&id);
            desired.set("version", "0.200.0");
            let mut observed = DeviceState::new(&id);
            observed.set("version", if i < 2 { "0.199.0" } else { "0.200.0" });
            inv.set_desired(desired);
            inv.set_observed(observed);
        }

        assert_eq!(inv.device_count(), 5);
        assert_eq!(inv.drifted_device_count(), 2);
    }

    #[test]
    fn no_drift_for_unknown_device() {
        let inv = DeviceInventory::new();
        assert!(inv.detect_drift("unknown").is_empty());
    }
}

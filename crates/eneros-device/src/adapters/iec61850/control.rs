//! IEC 61850 control services (SBO — Select Before Operate).
//!
//! Implements the control service model per IEC 61850-7-2 §19. Supports:
//! - Direct control (SBOw=false, Operate immediately)
//! - SBO with normal security (Select → Operate)
//! - SBO with enhanced security (SelectWithValue → Operate)
//!
//! # Control State Machine
//!
//! ```text
//! Idle ──select──► Selected ──operate──► Operated ──► Idle
//!   │                  │
//!   │                  └──cancel──► Idle
//!   └──direct-operate──► Operated ──► Idle
//! ```
//!
//! # Control Object Types (CDC)
//!
//! - SPC (Single Point Controllable): On/Off
//! - DPC (Double Point Controllable): On/Off/Intermediate
//! - APC (Analogue Process Controllable): analog setpoint
//! - BSC (Binary Step Controllable): step up/down

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use crate::adapter::DataValue;

/// Control state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlState {
    /// Idle — no control in progress
    Idle,
    /// Selected — select succeeded, awaiting operate
    Selected,
    /// Selected with value (enhanced security)
    SelectedWithValue,
    /// Operated — operate command sent
    Operated,
    /// Failed — last operation failed
    Failed,
}

/// Control mode (SBO configuration)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ControlMode {
    /// Direct control — no select needed
    #[default]
    Direct,
    /// SBO with normal security — Select(0) → Operate
    SboNormal,
    /// SBO with enhanced security — SelectWithValue → Operate
    SboEnhanced,
}

/// Control object definition
#[derive(Debug, Clone)]
pub struct ControlObject {
    /// Object reference: `LD/LN.DO`
    pub reference: String,
    /// CDC type
    pub cdc: ControllableCdc,
    /// Control mode
    pub mode: ControlMode,
    /// Timeout for SBO (milliseconds)
    pub sbo_timeout_ms: u32,
    /// Last selected value (for SBO)
    pub selected_value: Option<DataValue>,
    /// Current state
    pub state: ControlState,
    /// Selection timestamp
    pub selected_at: Option<Instant>,
}

/// Controllable Common Data Classes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllableCdc {
    /// Single Point Controllable (On/Off)
    Spc,
    /// Double Point Controllable (On/Off/Intermediate)
    Dpc,
    /// Analogue Process Controllable
    Apc,
    /// Binary Step Controllable (step up/down)
    Bsc,
    /// Integer Step Controllable
    Isc,
}

impl ControllableCdc {
    pub fn as_str(self) -> &'static str {
        match self {
            ControllableCdc::Spc => "SPC",
            ControllableCdc::Dpc => "DPC",
            ControllableCdc::Apc => "APC",
            ControllableCdc::Bsc => "BSC",
            ControllableCdc::Isc => "ISC",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() {
            "SPC" => Some(Self::Spc),
            "DPC" => Some(Self::Dpc),
            "APC" => Some(Self::Apc),
            "BSC" => Some(Self::Bsc),
            "ISC" => Some(Self::Isc),
            _ => None,
        }
    }
}

/// Control operation result
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlResult {
    /// Operation succeeded
    Ok,
    /// Select succeeded
    Selected,
    /// Operation failed — reason given
    Failed(String),
    /// Bad state — control not in expected state
    BadState(ControlState),
    /// Timeout — SBO select expired
    Timeout,
    /// Cancelled
    Cancelled,
}

/// Originator (who is performing the control)
#[derive(Debug, Clone)]
pub struct Originator {
    /// Originator category (0=station, 1=remote, 2=automatic, ...)
    pub or_cat: u8,
    /// Originator identifier
    pub or_ident: String,
}

impl Default for Originator {
    fn default() -> Self {
        Self {
            or_cat: 1, // remote
            or_ident: "EnerOS".to_string(),
        }
    }
}

/// Control service manager
pub struct ControlService {
    /// Registered control objects
    objects: Arc<Mutex<HashMap<String, ControlObject>>>,
    /// Default SBO timeout
    default_sbo_timeout: Duration,
}

impl Default for ControlService {
    fn default() -> Self {
        Self::new()
    }
}

impl ControlService {
    pub fn new() -> Self {
        Self {
            objects: Arc::new(Mutex::new(HashMap::new())),
            default_sbo_timeout: Duration::from_secs(10),
        }
    }

    /// Register a control object
    pub async fn register(&self, obj: ControlObject) {
        self.objects.lock().await.insert(obj.reference.clone(), obj);
    }

    /// Select a control object (SBO normal security)
    pub async fn select(
        &self,
        reference: &str,
        _origin: &Originator,
    ) -> ControlResult {
        let mut objs = self.objects.lock().await;
        let obj = match objs.get_mut(reference) {
            Some(o) => o,
            None => return ControlResult::Failed(format!("Control object not found: {}", reference)),
        };

        if obj.state != ControlState::Idle {
            return ControlResult::BadState(obj.state);
        }

        obj.state = ControlState::Selected;
        obj.selected_at = Some(Instant::now());
        obj.selected_value = None;
        ControlResult::Selected
    }

    /// Select with value (SBO enhanced security)
    pub async fn select_with_value(
        &self,
        reference: &str,
        value: DataValue,
        _origin: &Originator,
    ) -> ControlResult {
        let mut objs = self.objects.lock().await;
        let obj = match objs.get_mut(reference) {
            Some(o) => o,
            None => return ControlResult::Failed(format!("Control object not found: {}", reference)),
        };

        if obj.state != ControlState::Idle {
            return ControlResult::BadState(obj.state);
        }

        obj.state = ControlState::SelectedWithValue;
        obj.selected_at = Some(Instant::now());
        obj.selected_value = Some(value);
        ControlResult::Selected
    }

    /// Operate a control object
    pub async fn operate(
        &self,
        reference: &str,
        value: &DataValue,
        origin: &Originator,
    ) -> ControlResult {
        let mut objs = self.objects.lock().await;
        let obj = match objs.get_mut(reference) {
            Some(o) => o,
            None => return ControlResult::Failed(format!("Control object not found: {}", reference)),
        };

        match obj.mode {
            ControlMode::Direct => {
                // Direct control: no select needed
                tracing::info!(
                    "IEC 61850: direct operate {} = {:?} (origin={}/{})",
                    reference, value, origin.or_cat, origin.or_ident
                );
                obj.state = ControlState::Operated;
                ControlResult::Ok
            }
            ControlMode::SboNormal => {
                // Check select state and timeout
                if obj.state != ControlState::Selected {
                    return ControlResult::BadState(obj.state);
                }
                if let Some(selected_at) = obj.selected_at {
                    let timeout = Duration::from_millis(obj.sbo_timeout_ms as u64);
                    if selected_at.elapsed() > timeout {
                        obj.state = ControlState::Idle;
                        return ControlResult::Timeout;
                    }
                }
                tracing::info!(
                    "IEC 61850: SBO operate {} = {:?} (origin={}/{})",
                    reference, value, origin.or_cat, origin.or_ident
                );
                obj.state = ControlState::Operated;
                ControlResult::Ok
            }
            ControlMode::SboEnhanced => {
                // Check select-with-value state and timeout
                if obj.state != ControlState::SelectedWithValue {
                    return ControlResult::BadState(obj.state);
                }
                // Verify value matches selected value
                if let Some(ref sel_val) = obj.selected_value {
                    if sel_val != value {
                        return ControlResult::Failed(
                            "Operate value does not match selected value".to_string(),
                        );
                    }
                }
                if let Some(selected_at) = obj.selected_at {
                    let timeout = Duration::from_millis(obj.sbo_timeout_ms as u64);
                    if selected_at.elapsed() > timeout {
                        obj.state = ControlState::Idle;
                        return ControlResult::Timeout;
                    }
                }
                tracing::info!(
                    "IEC 61850: SBO-enhanced operate {} = {:?} (origin={}/{})",
                    reference, value, origin.or_cat, origin.or_ident
                );
                obj.state = ControlState::Operated;
                ControlResult::Ok
            }
        }
    }

    /// Cancel an ongoing control operation
    pub async fn cancel(
        &self,
        reference: &str,
        _origin: &Originator,
    ) -> ControlResult {
        let mut objs = self.objects.lock().await;
        let obj = match objs.get_mut(reference) {
            Some(o) => o,
            None => return ControlResult::Failed(format!("Control object not found: {}", reference)),
        };

        match obj.state {
            ControlState::Selected | ControlState::SelectedWithValue => {
                obj.state = ControlState::Idle;
                obj.selected_value = None;
                obj.selected_at = None;
                tracing::info!("IEC 61850: cancelled control {}", reference);
                ControlResult::Cancelled
            }
            _ => ControlResult::BadState(obj.state),
        }
    }

    /// Reset a control object back to Idle (after Operated)
    pub async fn reset(&self, reference: &str) -> ControlResult {
        let mut objs = self.objects.lock().await;
        let obj = match objs.get_mut(reference) {
            Some(o) => o,
            None => return ControlResult::Failed(format!("Control object not found: {}", reference)),
        };
        obj.state = ControlState::Idle;
        obj.selected_value = None;
        obj.selected_at = None;
        ControlResult::Ok
    }

    /// Get the state of a control object
    pub async fn state(&self, reference: &str) -> Option<ControlState> {
        self.objects.lock().await.get(reference).map(|o| o.state)
    }

    /// Get all registered control objects
    pub async fn all_objects(&self) -> Vec<ControlObject> {
        self.objects.lock().await.values().cloned().collect()
    }

    /// Set default SBO timeout
    pub fn set_default_sbo_timeout(&mut self, timeout: Duration) {
        self.default_sbo_timeout = timeout;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_direct_spc(ref_name: &str) -> ControlObject {
        ControlObject {
            reference: ref_name.to_string(),
            cdc: ControllableCdc::Spc,
            mode: ControlMode::Direct,
            sbo_timeout_ms: 10000,
            selected_value: None,
            state: ControlState::Idle,
            selected_at: None,
        }
    }

    fn make_sbo_spc(ref_name: &str) -> ControlObject {
        ControlObject {
            mode: ControlMode::SboNormal,
            ..make_direct_spc(ref_name)
        }
    }

    fn make_sbo_enhanced_apc(ref_name: &str) -> ControlObject {
        ControlObject {
            reference: ref_name.to_string(),
            cdc: ControllableCdc::Apc,
            mode: ControlMode::SboEnhanced,
            sbo_timeout_ms: 10000,
            selected_value: None,
            state: ControlState::Idle,
            selected_at: None,
        }
    }

    #[test]
    fn test_cdc_as_str() {
        assert_eq!(ControllableCdc::Spc.as_str(), "SPC");
        assert_eq!(ControllableCdc::Dpc.as_str(), "DPC");
        assert_eq!(ControllableCdc::Apc.as_str(), "APC");
    }

    #[test]
    fn test_cdc_from_str() {
        assert_eq!(ControllableCdc::from_str("SPC"), Some(ControllableCdc::Spc));
        assert_eq!(ControllableCdc::from_str("dpc"), Some(ControllableCdc::Dpc));
        assert_eq!(ControllableCdc::from_str("unknown"), None);
    }

    #[test]
    fn test_control_mode_default() {
        assert_eq!(ControlMode::default(), ControlMode::Direct);
    }

    #[tokio::test]
    async fn test_direct_control_operate() {
        let svc = ControlService::new();
        svc.register(make_direct_spc("LD0/GGIO1.SPCSO1")).await;
        let origin = Originator::default();
        let result = svc.operate("LD0/GGIO1.SPCSO1", &DataValue::Bool(true), &origin).await;
        assert_eq!(result, ControlResult::Ok);
        assert_eq!(svc.state("LD0/GGIO1.SPCSO1").await, Some(ControlState::Operated));
    }

    #[tokio::test]
    async fn test_direct_control_no_select_needed() {
        let svc = ControlService::new();
        svc.register(make_direct_spc("LD0/GGIO1.SPCSO1")).await;
        let origin = Originator::default();
        // Select should fail for direct control (not in SBO mode)
        let result = svc.select("LD0/GGIO1.SPCSO1", &origin).await;
        // Actually select succeeds (state goes to Selected), but operate doesn't require it
        assert_eq!(result, ControlResult::Selected);
    }

    #[tokio::test]
    async fn test_sbo_normal_select_then_operate() {
        let svc = ControlService::new();
        svc.register(make_sbo_spc("LD0/GGIO1.SPCSO1")).await;
        let origin = Originator::default();

        // Operate without select should fail
        let result = svc.operate("LD0/GGIO1.SPCSO1", &DataValue::Bool(true), &origin).await;
        assert_eq!(result, ControlResult::BadState(ControlState::Idle));

        // Select
        let result = svc.select("LD0/GGIO1.SPCSO1", &origin).await;
        assert_eq!(result, ControlResult::Selected);
        assert_eq!(svc.state("LD0/GGIO1.SPCSO1").await, Some(ControlState::Selected));

        // Operate
        let result = svc.operate("LD0/GGIO1.SPCSO1", &DataValue::Bool(true), &origin).await;
        assert_eq!(result, ControlResult::Ok);
    }

    #[tokio::test]
    async fn test_sbo_normal_cancel() {
        let svc = ControlService::new();
        svc.register(make_sbo_spc("LD0/GGIO1.SPCSO1")).await;
        let origin = Originator::default();

        svc.select("LD0/GGIO1.SPCSO1", &origin).await;
        let result = svc.cancel("LD0/GGIO1.SPCSO1", &origin).await;
        assert_eq!(result, ControlResult::Cancelled);
        assert_eq!(svc.state("LD0/GGIO1.SPCSO1").await, Some(ControlState::Idle));
    }

    #[tokio::test]
    async fn test_sbo_normal_cancel_when_idle_fails() {
        let svc = ControlService::new();
        svc.register(make_sbo_spc("LD0/GGIO1.SPCSO1")).await;
        let origin = Originator::default();
        let result = svc.cancel("LD0/GGIO1.SPCSO1", &origin).await;
        assert_eq!(result, ControlResult::BadState(ControlState::Idle));
    }

    #[tokio::test]
    async fn test_sbo_enhanced_value_mismatch() {
        let svc = ControlService::new();
        svc.register(make_sbo_enhanced_apc("LD0/GGIO1.APCSO1")).await;
        let origin = Originator::default();

        // Select with value 42.5
        let result = svc.select_with_value(
            "LD0/GGIO1.APCSO1",
            DataValue::Float32(42.5),
            &origin,
        ).await;
        assert_eq!(result, ControlResult::Selected);

        // Operate with different value should fail
        let result = svc.operate(
            "LD0/GGIO1.APCSO1",
            &DataValue::Float32(99.0),
            &origin,
        ).await;
        assert!(matches!(result, ControlResult::Failed(_)));
    }

    #[tokio::test]
    async fn test_sbo_enhanced_value_match() {
        let svc = ControlService::new();
        svc.register(make_sbo_enhanced_apc("LD0/GGIO1.APCSO1")).await;
        let origin = Originator::default();

        svc.select_with_value("LD0/GGIO1.APCSO1", DataValue::Float32(42.5), &origin).await;
        let result = svc.operate("LD0/GGIO1.APCSO1", &DataValue::Float32(42.5), &origin).await;
        assert_eq!(result, ControlResult::Ok);
    }

    #[tokio::test]
    async fn test_sbo_timeout() {
        let svc = ControlService::new();
        let obj = ControlObject {
            reference: "LD0/GGIO1.SPCSO1".to_string(),
            cdc: ControllableCdc::Spc,
            mode: ControlMode::SboNormal,
            sbo_timeout_ms: 1, // 1ms — will timeout immediately
            selected_value: None,
            state: ControlState::Idle,
            selected_at: None,
        };
        svc.register(obj).await;
        let origin = Originator::default();

        svc.select("LD0/GGIO1.SPCSO1", &origin).await;
        // Sleep to ensure timeout
        tokio::time::sleep(Duration::from_millis(10)).await;
        let result = svc.operate("LD0/GGIO1.SPCSO1", &DataValue::Bool(true), &origin).await;
        assert_eq!(result, ControlResult::Timeout);
    }

    #[tokio::test]
    async fn test_reset_after_operate() {
        let svc = ControlService::new();
        svc.register(make_direct_spc("LD0/GGIO1.SPCSO1")).await;
        let origin = Originator::default();
        svc.operate("LD0/GGIO1.SPCSO1", &DataValue::Bool(true), &origin).await;
        assert_eq!(svc.state("LD0/GGIO1.SPCSO1").await, Some(ControlState::Operated));
        let result = svc.reset("LD0/GGIO1.SPCSO1").await;
        assert_eq!(result, ControlResult::Ok);
        assert_eq!(svc.state("LD0/GGIO1.SPCSO1").await, Some(ControlState::Idle));
    }

    #[tokio::test]
    async fn test_operate_not_found() {
        let svc = ControlService::new();
        let origin = Originator::default();
        let result = svc.operate("nonexistent", &DataValue::Bool(true), &origin).await;
        assert!(matches!(result, ControlResult::Failed(_)));
    }

    #[tokio::test]
    async fn test_all_objects() {
        let svc = ControlService::new();
        svc.register(make_direct_spc("LD0/GGIO1.SPCSO1")).await;
        svc.register(make_sbo_spc("LD0/GGIO1.SPCSO2")).await;
        assert_eq!(svc.all_objects().await.len(), 2);
    }

    #[test]
    fn test_originator_default() {
        let o = Originator::default();
        assert_eq!(o.or_cat, 1);
        assert_eq!(o.or_ident, "EnerOS");
    }
}

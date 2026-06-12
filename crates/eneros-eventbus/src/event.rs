use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use eneros_core::ElementId;

/// Event type classification
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventType {
    /// Topology change event
    TopologyChanged,
    /// Power flow calculation completed
    PowerFlowConverged,
    /// Power flow calculation failed
    PowerFlowFailed,
    /// Constraint violation detected
    ConstraintViolation,
    /// Constraint violation resolved
    ConstraintResolved,
    /// Equipment status changed
    EquipmentStatusChanged,
    /// Device connected
    DeviceConnected,
    /// Device disconnected
    DeviceDisconnected,
    /// Data received from device
    DataReceived,
    /// System alarm
    SystemAlarm,
    /// System error
    SystemError,
}

/// Event data payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EventPayload {
    /// Topology change
    TopologyChange {
        change_type: String,
        element_id: ElementId,
    },
    /// Power flow result
    PowerFlowResult {
        converged: bool,
        iterations: u32,
        total_losses: f64,
    },
    /// Constraint violation
    ConstraintViolation {
        constraint_id: String,
        element_id: ElementId,
        actual_value: f64,
        limit_value: f64,
        severity: String,
    },
    /// Equipment status
    EquipmentStatus {
        equipment_id: ElementId,
        status: bool,
    },
    /// Device event
    DeviceEvent {
        device_id: String,
        event_type: String,
    },
    /// Generic message
    Message(String),
}

/// Event structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// Unique event ID
    pub id: String,
    /// Event type
    pub event_type: EventType,
    /// Event timestamp
    pub timestamp: DateTime<Utc>,
    /// Event source
    pub source: String,
    /// Event payload
    pub payload: EventPayload,
}

impl Event {
    /// Create a new event
    pub fn new(
        event_type: EventType,
        source: &str,
        payload: EventPayload,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            event_type,
            timestamp: Utc::now(),
            source: source.to_string(),
            payload,
        }
    }
}

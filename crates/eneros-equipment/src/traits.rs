use std::collections::HashMap;
use eneros_core::{ElementId, EquipmentType};

/// Trait for power equipment models
pub trait EquipmentModel: Send + Sync {
    /// Get equipment ID
    fn id(&self) -> ElementId;

    /// Get equipment type
    fn equipment_type(&self) -> EquipmentType;

    /// Get equipment name
    fn name(&self) -> &str;

    /// Get all parameters as key-value pairs
    fn parameters(&self) -> HashMap<String, f64>;

    /// Get a specific parameter value
    fn get_parameter(&self, name: &str) -> Option<f64>;

    /// Validate equipment parameters
    fn validate(&self) -> Result<(), String>;

    /// Get rated capacity (MVA)
    fn rated_capacity(&self) -> Option<f64> {
        None
    }

    /// Get rated voltage (kV)
    fn rated_voltage(&self) -> Option<f64> {
        None
    }
}

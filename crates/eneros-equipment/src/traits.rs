use std::collections::HashMap;
use eneros_core::{ElementId, EquipmentType};

/// Admittance contribution from a branch element (series + shunt)
#[derive(Debug, Clone, Copy, Default)]
pub struct AdmittanceContribution {
    /// Series admittance (1 / impedance) in per-unit
    pub y_series: num_complex::Complex<f64>,
    /// Shunt admittance at each end (half-line charging) in per-unit
    pub y_shunt: num_complex::Complex<f64>,
    /// Additional shunt admittance at the "from" bus (e.g., y/tap^2 - y for transformer tap model)
    pub y_from_shunt: num_complex::Complex<f64>,
    /// Additional shunt admittance at the "to" bus (e.g., y - y/tap for transformer tap model)
    pub y_to_shunt: num_complex::Complex<f64>,
}

/// Multi-terminal admittance contribution (for three-winding transformers etc.)
#[derive(Debug, Clone)]
pub struct MultiAdmittanceContribution {
    /// Bus IDs corresponding to each admittance contribution
    pub bus_ids: Vec<eneros_core::ElementId>,
    /// Admittance contributions for each terminal pair
    pub contributions: Vec<AdmittanceContribution>,
}

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

    /// Get bus IDs this element connects to (for branch-type elements)
    fn bus_ids(&self) -> Vec<ElementId> {
        Vec::new()
    }

    /// Compute admittance contribution in per-unit given base_mva and base_kv.
    /// For series elements (lines, transformers) this returns series + shunt.
    /// For shunt elements (loads, generators, shunts) this returns the injection.
    fn to_admittance(&self, _base_mva: f64, _base_kv: f64) -> Option<AdmittanceContribution> {
        None
    }

    /// Compute multi-terminal admittance contribution for elements with more than 2 buses.
    /// Default implementation delegates to to_admittance for two-terminal elements.
    fn to_admittance_multi(&self, base_mva: f64, base_kv: f64) -> Option<MultiAdmittanceContribution> {
        self.to_admittance(base_mva, base_kv).map(|adm| MultiAdmittanceContribution {
            bus_ids: self.bus_ids(),
            contributions: vec![adm],
        })
    }
}

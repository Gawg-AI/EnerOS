use std::collections::HashMap;
use std::sync::Arc;
use parking_lot::RwLock;
use eneros_core::{ElementId, EquipmentType, Result, EnerOSError};

use crate::traits::{EquipmentModel, AdmittanceContribution};

/// Equipment model library for managing power system equipment
pub struct EquipmentLibrary {
    models: RwLock<HashMap<ElementId, Arc<dyn EquipmentModel>>>,
}

impl EquipmentLibrary {
    /// Create a new empty equipment library
    pub fn new() -> Self {
        Self {
            models: RwLock::new(HashMap::new()),
        }
    }

    /// Add an equipment model to the library
    pub fn add(&self, model: Arc<dyn EquipmentModel>) -> Result<()> {
        let id = model.id();
        model.validate().map_err(EnerOSError::Equipment)?;
        let mut models = self.models.write();
        models.insert(id, model);
        Ok(())
    }

    /// Remove an equipment model from the library
    pub fn remove(&self, id: ElementId) -> bool {
        let mut models = self.models.write();
        models.remove(&id).is_some()
    }

    /// Get an equipment model by ID
    pub fn get(&self, id: ElementId) -> Option<Arc<dyn EquipmentModel>> {
        let models = self.models.read();
        models.get(&id).cloned()
    }

    /// Get all equipment of a specific type
    pub fn get_by_type(&self, equipment_type: EquipmentType) -> Vec<ElementId> {
        let models = self.models.read();
        models
            .iter()
            .filter(|(_, m)| m.equipment_type() == equipment_type)
            .map(|(id, _)| *id)
            .collect()
    }

    /// Get equipment count
    pub fn count(&self) -> usize {
        self.models.read().len()
    }

    /// Get all equipment IDs
    pub fn ids(&self) -> Vec<ElementId> {
        let models = self.models.read();
        models.keys().copied().collect()
    }

    /// Validate all equipment in the library
    pub fn validate_all(&self) -> Result<()> {
        let models = self.models.read();
        for (id, model) in models.iter() {
            model
                .validate()
                .map_err(|e| EnerOSError::Equipment(format!("Equipment {}: {}", id, e)))?;
        }
        Ok(())
    }

    /// Collect admittance contributions from all equipment in the library.
    /// Returns (bus_id, contribution) pairs for each bus each equipment connects to.
    pub fn collect_admittances(&self, base_mva: f64, base_kv: f64) -> Vec<(ElementId, AdmittanceContribution)> {
        let models = self.models.read();
        let mut result = Vec::new();
        for model in models.values() {
            if let Some(adm) = model.to_admittance(base_mva, base_kv) {
                for bus_id in model.bus_ids() {
                    result.push((bus_id, adm));
                }
            }
        }
        result
    }

    /// Get net power injection at a given bus (MW, MVar).
    /// Generators contribute positive injection, loads contribute negative injection.
    pub fn get_injections_at_bus(&self, bus_id: ElementId) -> (f64, f64) {
        let models = self.models.read();
        let mut p_total = 0.0;
        let mut q_total = 0.0;
        for model in models.values() {
            if !model.bus_ids().contains(&bus_id) {
                continue;
            }
            let et = model.equipment_type();
            let is_gen = matches!(et,
                EquipmentType::SynchronousGenerator
                | EquipmentType::AsynchronousGenerator
                | EquipmentType::PhotovoltaicInverter
                | EquipmentType::WindTurbineConverter
            );
            let is_load = matches!(et,
                EquipmentType::ConstantPowerLoad
                | EquipmentType::ConstantImpedanceLoad
                | EquipmentType::MotorLoad
            );
            if !is_gen && !is_load {
                continue;
            }
            let p = model.get_parameter("p_mw")
                .or_else(|| model.get_parameter("rated_mw"))
                .unwrap_or(0.0);
            let q = model.get_parameter("q_mvar")
                .or_else(|| model.get_parameter("rated_mvar"))
                .unwrap_or(0.0);
            if is_gen {
                p_total += p;
                q_total += q;
            } else {
                p_total -= p;
                q_total -= q;
            }
        }
        (p_total, q_total)
    }

    /// Return all unique bus IDs from all equipment in the library.
    pub fn bus_ids(&self) -> Vec<ElementId> {
        let models = self.models.read();
        let mut seen = std::collections::HashSet::new();
        for model in models.values() {
            for bid in model.bus_ids() {
                seen.insert(bid);
            }
        }
        seen.into_iter().collect()
    }
}

impl Default for EquipmentLibrary {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::*;

    fn make_library() -> EquipmentLibrary {
        EquipmentLibrary::new()
    }

    #[test]
    fn test_collect_admittances() {
        let lib = make_library();
        let line = Arc::new(TransmissionLine {
            id: 1, name: "L1".into(), length_km: 10.0,
            r_per_km: 0.1, x_per_km: 0.4, b_per_km: 0.001,
            rated_current_ka: 0.5, rated_kv: 10.0,
            from_bus_id: 10, to_bus_id: 20,
        });
        let load = Arc::new(ConstantPowerLoad {
            id: 2, name: "Load1".into(), p_mw: 5.0, q_mvar: 2.0,
            rated_kv: 10.0, bus_id: 10,
        });
        lib.add(line).unwrap();
        lib.add(load).unwrap();

        let admittances = lib.collect_admittances(100.0, 10.0);
        // Line has admittance, load does not (returns None)
        assert_eq!(admittances.len(), 2); // line has 2 bus_ids
        let bus_ids: Vec<ElementId> = admittances.iter().map(|(b, _)| *b).collect();
        assert!(bus_ids.contains(&10));
        assert!(bus_ids.contains(&20));
        // All contributions should have non-zero y_series
        for (_, adm) in &admittances {
            assert!(adm.y_series.norm() > 0.0);
        }
    }

    #[test]
    fn test_collect_admittances_empty() {
        let lib = make_library();
        let admittances = lib.collect_admittances(100.0, 10.0);
        assert!(admittances.is_empty());
    }

    #[test]
    fn test_get_injections_at_bus_generator() {
        let lib = make_library();
        let gen = Arc::new(SynchronousGenerator {
            id: 1, name: "G1".into(), rated_mw: 100.0, rated_mvar: 50.0,
            rated_kv: 13.8, x_d: 1.2, x_q: 0.8, x_d_trans: 0.3, bus_id: 5,
        });
        lib.add(gen).unwrap();

        let (p, q) = lib.get_injections_at_bus(5);
        assert!((p - 100.0).abs() < 1e-10);
        assert!((q - 50.0).abs() < 1e-10);
    }

    #[test]
    fn test_get_injections_at_bus_load() {
        let lib = make_library();
        let load = Arc::new(ConstantPowerLoad {
            id: 2, name: "L1".into(), p_mw: 10.0, q_mvar: 5.0,
            rated_kv: 10.0, bus_id: 5,
        });
        lib.add(load).unwrap();

        let (p, q) = lib.get_injections_at_bus(5);
        // Load is negative injection
        assert!((p - (-10.0)).abs() < 1e-10);
        assert!((q - (-5.0)).abs() < 1e-10);
    }

    #[test]
    fn test_get_injections_at_bus_mixed() {
        let lib = make_library();
        let gen = Arc::new(StaticGenerator {
            id: 1, name: "PV1".into(), p_mw: 5.0, q_mvar: 1.0,
            rated_kv: 0.4, bus_id: 5, scaling: 1.0, controllable: false,
        });
        let load = Arc::new(ConstantPowerLoad {
            id: 2, name: "L1".into(), p_mw: 3.0, q_mvar: 1.5,
            rated_kv: 10.0, bus_id: 5,
        });
        lib.add(gen).unwrap();
        lib.add(load).unwrap();

        let (p, q) = lib.get_injections_at_bus(5);
        // gen: +5 MW, +1 MVar; load: -3 MW, -1.5 MVar => net: +2 MW, -0.5 MVar
        assert!((p - 2.0).abs() < 1e-10);
        assert!((q - (-0.5)).abs() < 1e-10);
    }

    #[test]
    fn test_get_injections_at_bus_no_equipment() {
        let lib = make_library();
        let (p, q) = lib.get_injections_at_bus(99);
        assert!((p - 0.0).abs() < 1e-10);
        assert!((q - 0.0).abs() < 1e-10);
    }

    #[test]
    fn test_bus_ids() {
        let lib = make_library();
        let line = Arc::new(TransmissionLine {
            id: 1, name: "L1".into(), length_km: 10.0,
            r_per_km: 0.1, x_per_km: 0.4, b_per_km: 0.001,
            rated_current_ka: 0.5, rated_kv: 10.0,
            from_bus_id: 10, to_bus_id: 20,
        });
        let gen = Arc::new(SynchronousGenerator {
            id: 2, name: "G1".into(), rated_mw: 100.0, rated_mvar: 50.0,
            rated_kv: 13.8, x_d: 1.2, x_q: 0.8, x_d_trans: 0.3, bus_id: 10,
        });
        let load = Arc::new(ConstantPowerLoad {
            id: 3, name: "L1".into(), p_mw: 5.0, q_mvar: 2.0,
            rated_kv: 10.0, bus_id: 30,
        });
        lib.add(line).unwrap();
        lib.add(gen).unwrap();
        lib.add(load).unwrap();

        let mut ids = lib.bus_ids();
        ids.sort();
        assert_eq!(ids, vec![10, 20, 30]);
    }

    #[test]
    fn test_bus_ids_empty() {
        let lib = make_library();
        assert!(lib.bus_ids().is_empty());
    }
}

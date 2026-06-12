use std::collections::HashMap;
use std::sync::Arc;
use parking_lot::RwLock;
use eneros_core::{ElementId, EquipmentType, Result, EnerOSError};

use crate::traits::EquipmentModel;

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
        model.validate().map_err(|e| EnerOSError::Equipment(e))?;
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
}

impl Default for EquipmentLibrary {
    fn default() -> Self {
        Self::new()
    }
}

//! IEC 61850 Dataset management.
//!
//! Datasets are ordered collections of data object references (FCDA —
//! Functional Constraint Data Attribute). They provide a way to read/write
//! and report on multiple data points atomically.
//!
//! # Dataset Types
//!
//! - **Static datasets**: Defined in SCL configuration, persistent
//! - **Dynamic datasets**: Created at runtime by clients, non-persistent
//!
//! # Reference Format
//!
//! Dataset reference: `LD/LLN.dataset_name`
//! FCDA reference: `LD/LN.DO.DA.FC`

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::adapter::{DataValue, DataQuality};

/// Functional Constraint (IEC 61850-7-2 §9.2.3)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FunctionalConstraint {
    /// Status information
    St,
    /// Measurement (analog values)
    Mx,
    /// Setpoint
    Sp,
    /// Substitution
    Sv,
    /// Configuration
    Cf,
    /// Description
    Dc,
    /// Setting group
    Sg,
    /// Setting (RG)
    Se,
    /// Service response
    Sr,
    /// Operate
    Or,
    /// Control
    Co,
    /// Unicast SV
    Us,
    /// GOOSE
    Go,
    /// Report
    Rp,
    /// Log
    Lg,
}

impl FunctionalConstraint {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::St => "ST",
            Self::Mx => "MX",
            Self::Sp => "SP",
            Self::Sv => "SV",
            Self::Cf => "CF",
            Self::Dc => "DC",
            Self::Sg => "SG",
            Self::Se => "SE",
            Self::Sr => "SR",
            Self::Or => "OR",
            Self::Co => "CO",
            Self::Us => "US",
            Self::Go => "GO",
            Self::Rp => "RP",
            Self::Lg => "LG",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() {
            "ST" => Some(Self::St),
            "MX" => Some(Self::Mx),
            "SP" => Some(Self::Sp),
            "SV" => Some(Self::Sv),
            "CF" => Some(Self::Cf),
            "DC" => Some(Self::Dc),
            "SG" => Some(Self::Sg),
            "SE" => Some(Self::Se),
            "SR" => Some(Self::Sr),
            "OR" => Some(Self::Or),
            "CO" => Some(Self::Co),
            "US" => Some(Self::Us),
            "GO" => Some(Self::Go),
            "RP" => Some(Self::Rp),
            "LG" => Some(Self::Lg),
            _ => None,
        }
    }
}

/// A single data attribute reference (FCDA)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FcdaRef {
    /// Logical device instance (e.g., "LD0")
    pub ld_inst: String,
    /// Logical node name (e.g., "GGIO1")
    pub ln_name: String,
    /// Data object name (e.g., "AnIn1")
    pub do_name: String,
    /// Data attribute path (e.g., "mag.f")
    pub da_path: String,
    /// Functional constraint
    pub fc: FunctionalConstraint,
}

impl FcdaRef {
    /// Parse an FCDA reference string: `LD/LN.DO.DA.FC`
    pub fn parse(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split('/').collect();
        if parts.len() != 2 {
            return None;
        }
        let ld_inst = parts[0].to_string();
        let rest = parts[1];
        let dot_parts: Vec<&str> = rest.split('.').collect();
        if dot_parts.len() < 3 {
            return None;
        }
        let ln_name = dot_parts[0].to_string();
        let do_name = dot_parts[1].to_string();
        let fc = FunctionalConstraint::from_str(dot_parts[dot_parts.len() - 1])?;
        let da_path = dot_parts[2..dot_parts.len() - 1].join(".");
        Some(Self {
            ld_inst,
            ln_name,
            do_name,
            da_path,
            fc,
        })
    }

    /// Render as a reference string
    #[allow(clippy::inherent_to_string)]
    pub fn to_string(&self) -> String {
        if self.da_path.is_empty() {
            format!("{}/{}.{}.{}",
                self.ld_inst, self.ln_name, self.do_name, self.fc.as_str())
        } else {
            format!("{}/{}.{}.{}.{}",
                self.ld_inst, self.ln_name, self.do_name, self.da_path, self.fc.as_str())
        }
    }
}

/// A dataset definition
#[derive(Debug, Clone)]
pub struct DataSet {
    /// Dataset reference: `LD/LLN.dataset_name`
    pub reference: String,
    /// Dataset members (FCDA references)
    pub members: Vec<FcdaRef>,
    /// Static (from SCL) or dynamic (created at runtime)
    pub is_static: bool,
}

/// Dataset value (one entry per member)
#[derive(Debug, Clone)]
pub struct DataSetValue {
    /// FCDA reference
    pub fcda: FcdaRef,
    /// Current value
    pub value: DataValue,
    /// Quality
    pub quality: DataQuality,
    /// Timestamp (ms since epoch)
    pub timestamp_ms: i64,
}

/// Dataset manager
pub struct DataSetManager {
    datasets: Arc<RwLock<HashMap<String, DataSet>>>,
    values: Arc<RwLock<HashMap<String, Vec<DataSetValue>>>>,
}

impl Default for DataSetManager {
    fn default() -> Self {
        Self::new()
    }
}

impl DataSetManager {
    pub fn new() -> Self {
        Self {
            datasets: Arc::new(RwLock::new(HashMap::new())),
            values: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a static dataset (from SCL)
    pub async fn register_static(&self, dataset: DataSet) {
        self.datasets.write().await.insert(dataset.reference.clone(), dataset);
    }

    /// Create a dynamic dataset at runtime
    pub async fn create_dynamic(
        &self,
        reference: &str,
        members: Vec<FcdaRef>,
    ) -> Result<(), String> {
        let mut ds = self.datasets.write().await;
        if ds.contains_key(reference) {
            return Err(format!("Dataset already exists: {}", reference));
        }
        ds.insert(reference.to_string(), DataSet {
            reference: reference.to_string(),
            members,
            is_static: false,
        });
        Ok(())
    }

    /// Delete a dynamic dataset (static datasets cannot be deleted)
    pub async fn delete_dynamic(&self, reference: &str) -> Result<(), String> {
        let mut ds = self.datasets.write().await;
        let dataset = ds.get(reference).ok_or_else(|| format!("Dataset not found: {}", reference))?;
        if dataset.is_static {
            return Err(format!("Cannot delete static dataset: {}", reference));
        }
        ds.remove(reference);
        self.values.write().await.remove(reference);
        Ok(())
    }

    /// Get a dataset definition
    pub async fn get(&self, reference: &str) -> Option<DataSet> {
        self.datasets.read().await.get(reference).cloned()
    }

    /// List all dataset references
    pub async fn list(&self) -> Vec<String> {
        self.datasets.read().await.keys().cloned().collect()
    }

    /// Update values for a dataset
    pub async fn set_values(&self, reference: &str, values: Vec<DataSetValue>) {
        self.values.write().await.insert(reference.to_string(), values);
    }

    /// Get the current values for a dataset
    pub async fn get_values(&self, reference: &str) -> Option<Vec<DataSetValue>> {
        self.values.read().await.get(reference).cloned()
    }

    /// Count total datasets
    pub async fn count(&self) -> usize {
        self.datasets.read().await.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fc_as_str() {
        assert_eq!(FunctionalConstraint::St.as_str(), "ST");
        assert_eq!(FunctionalConstraint::Mx.as_str(), "MX");
        assert_eq!(FunctionalConstraint::Sp.as_str(), "SP");
    }

    #[test]
    fn test_fc_from_str() {
        assert_eq!(FunctionalConstraint::from_str("ST"), Some(FunctionalConstraint::St));
        assert_eq!(FunctionalConstraint::from_str("mx"), Some(FunctionalConstraint::Mx));
        assert_eq!(FunctionalConstraint::from_str("XX"), None);
    }

    #[test]
    fn test_fcda_parse() {
        let fcda = FcdaRef::parse("LD0/GGIO1.AnIn1.mag.f.MX").unwrap();
        assert_eq!(fcda.ld_inst, "LD0");
        assert_eq!(fcda.ln_name, "GGIO1");
        assert_eq!(fcda.do_name, "AnIn1");
        assert_eq!(fcda.da_path, "mag.f");
        assert_eq!(fcda.fc, FunctionalConstraint::Mx);
    }

    #[test]
    fn test_fcda_parse_no_da_path() {
        let fcda = FcdaRef::parse("LD0/GGIO1.Ind1.ST").unwrap();
        assert_eq!(fcda.ld_inst, "LD0");
        assert_eq!(fcda.ln_name, "GGIO1");
        assert_eq!(fcda.do_name, "Ind1");
        assert_eq!(fcda.da_path, "");
        assert_eq!(fcda.fc, FunctionalConstraint::St);
    }

    #[test]
    fn test_fcda_parse_invalid() {
        assert!(FcdaRef::parse("invalid").is_none());
        assert!(FcdaRef::parse("LD0/GGIO1").is_none());
        assert!(FcdaRef::parse("LD0/GGIO1.AnIn1.XX").is_none()); // invalid FC
    }

    #[test]
    fn test_fcda_to_string() {
        let fcda = FcdaRef {
            ld_inst: "LD0".to_string(),
            ln_name: "GGIO1".to_string(),
            do_name: "AnIn1".to_string(),
            da_path: "mag.f".to_string(),
            fc: FunctionalConstraint::Mx,
        };
        assert_eq!(fcda.to_string(), "LD0/GGIO1.AnIn1.mag.f.MX");
    }

    #[test]
    fn test_fcda_to_string_no_da() {
        let fcda = FcdaRef {
            ld_inst: "LD0".to_string(),
            ln_name: "GGIO1".to_string(),
            do_name: "Ind1".to_string(),
            da_path: "".to_string(),
            fc: FunctionalConstraint::St,
        };
        assert_eq!(fcda.to_string(), "LD0/GGIO1.Ind1.ST");
    }

    #[tokio::test]
    async fn test_dataset_manager_register_static() {
        let mgr = DataSetManager::new();
        let ds = DataSet {
            reference: "LD0/LLN0.dsGeneric".to_string(),
            members: vec![
                FcdaRef::parse("LD0/GGIO1.AnIn1.mag.f.MX").unwrap(),
                FcdaRef::parse("LD0/GGIO1.Ind1.ST").unwrap(),
            ],
            is_static: true,
        };
        mgr.register_static(ds).await;
        assert_eq!(mgr.count().await, 1);
        let retrieved = mgr.get("LD0/LLN0.dsGeneric").await.unwrap();
        assert!(retrieved.is_static);
        assert_eq!(retrieved.members.len(), 2);
    }

    #[tokio::test]
    async fn test_dataset_manager_create_dynamic() {
        let mgr = DataSetManager::new();
        let members = vec![FcdaRef::parse("LD0/GGIO1.AnIn1.mag.f.MX").unwrap()];
        assert!(mgr.create_dynamic("LD0/LLN0.dsDynamic", members).await.is_ok());
        let ds = mgr.get("LD0/LLN0.dsDynamic").await.unwrap();
        assert!(!ds.is_static);
    }

    #[tokio::test]
    async fn test_dataset_manager_create_duplicate_fails() {
        let mgr = DataSetManager::new();
        mgr.create_dynamic("LD0/LLN0.dsDynamic", vec![]).await.unwrap();
        let result = mgr.create_dynamic("LD0/LLN0.dsDynamic", vec![]).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_dataset_manager_delete_dynamic() {
        let mgr = DataSetManager::new();
        mgr.create_dynamic("LD0/LLN0.dsDynamic", vec![]).await.unwrap();
        assert!(mgr.delete_dynamic("LD0/LLN0.dsDynamic").await.is_ok());
        assert_eq!(mgr.count().await, 0);
    }

    #[tokio::test]
    async fn test_dataset_manager_delete_static_fails() {
        let mgr = DataSetManager::new();
        mgr.register_static(DataSet {
            reference: "LD0/LLN0.dsStatic".to_string(),
            members: vec![],
            is_static: true,
        }).await;
        let result = mgr.delete_dynamic("LD0/LLN0.dsStatic").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_dataset_manager_set_get_values() {
        let mgr = DataSetManager::new();
        let fcda = FcdaRef::parse("LD0/GGIO1.AnIn1.mag.f.MX").unwrap();
        mgr.register_static(DataSet {
            reference: "LD0/LLN0.dsGeneric".to_string(),
            members: vec![fcda.clone()],
            is_static: true,
        }).await;

        let values = vec![DataSetValue {
            fcda: fcda.clone(),
            value: DataValue::Float32(220.5),
            quality: DataQuality::Good,
            timestamp_ms: 1234567890,
        }];
        mgr.set_values("LD0/LLN0.dsGeneric", values).await;

        let retrieved = mgr.get_values("LD0/LLN0.dsGeneric").await.unwrap();
        assert_eq!(retrieved.len(), 1);
        assert_eq!(retrieved[0].fcda, fcda);
    }

    #[tokio::test]
    async fn test_dataset_manager_list() {
        let mgr = DataSetManager::new();
        mgr.register_static(DataSet {
            reference: "LD0/LLN0.ds1".to_string(),
            members: vec![],
            is_static: true,
        }).await;
        mgr.register_static(DataSet {
            reference: "LD0/LLN0.ds2".to_string(),
            members: vec![],
            is_static: true,
        }).await;
        let list = mgr.list().await;
        assert_eq!(list.len(), 2);
        assert!(list.contains(&"LD0/LLN0.ds1".to_string()));
        assert!(list.contains(&"LD0/LLN0.ds2".to_string()));
    }
}

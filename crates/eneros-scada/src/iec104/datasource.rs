//! IEC 104 data source adapter for the EnerOS SCADA system.
//!
//! Implements the `DataSource` trait so that IEC 104 data can flow into
//! `ScadaCollector` → `SnapshotBuilder` → constraint detection.

use std::collections::HashMap;
use std::sync::Arc;

use eneros_core::ElementId;
use eneros_device::adapters::iec104::client::Iec104Client;
use parking_lot::RwLock;

use crate::collector::DataSource;
use super::mapping::IoaMappingTable;

/// IEC 104 data source that implements the `DataSource` trait.
///
/// Bridges IEC 104 IOA-based data into the (element_id, parameter) → f64
/// model used by `ScadaCollector`.
pub struct Iec104DataSource {
    client: Arc<Iec104Client>,
    mapping: IoaMappingTable,
    /// Cache: (element_id, parameter) → f64
    cache: RwLock<HashMap<(ElementId, String), f64>>,
}

impl Iec104DataSource {
    /// Create a new IEC 104 data source
    pub fn new(client: Arc<Iec104Client>, mapping: IoaMappingTable) -> Self {
        Self {
            client,
            mapping,
            cache: RwLock::new(HashMap::new()),
        }
    }

    /// Refresh the cache from the IEC 104 client's latest data.
    pub async fn refresh_cache(&self) {
        let values = self.client.get_all_values().await;
        let mut cache = self.cache.write();

        for (ioa, obj) in &values {
            if let Some(mapping_entry) = self.mapping.get(*ioa) {
                if let Some(raw_value) = obj.as_float() {
                    if obj.is_valid() {
                        let eng_value = raw_value * mapping_entry.scale + mapping_entry.offset;
                        cache.insert(
                            (mapping_entry.element_id, mapping_entry.parameter.clone()),
                            eng_value,
                        );
                    }
                }
            }
        }
    }

    /// Inject a value directly into the cache (for testing)
    pub fn inject(&self, element_id: ElementId, parameter: &str, value: f64) {
        self.cache.write().insert((element_id, parameter.to_string()), value);
    }

    /// Get the mapping table reference
    pub fn mapping(&self) -> &IoaMappingTable {
        &self.mapping
    }

    /// Get the client reference
    pub fn client(&self) -> &Arc<Iec104Client> {
        &self.client
    }
}

impl DataSource for Iec104DataSource {
    fn read(&self, element_id: ElementId, parameter: &str) -> Option<f64> {
        self.cache.read().get(&(element_id, parameter.to_string())).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eneros_device::adapters::iec104::asdu::{InformationObject, MeasuredQuality, DoublePointValue};
    use eneros_device::adapters::iec104::client::Iec104Config;
    use super::super::mapping::IoaMapping;

    #[tokio::test]
    async fn test_iec104_data_source_read_from_cache() {
        let config = Iec104Config::default();
        let client = Arc::new(Iec104Client::new(config));
        let mapping = IoaMappingTable::new();
        let ds = Iec104DataSource::new(client, mapping);

        ds.inject(1, "voltage_pu", 1.045);
        ds.inject(2, "active_power_mw", 50.0);

        assert!((ds.read(1, "voltage_pu").unwrap() - 1.045).abs() < 0.001);
        assert!((ds.read(2, "active_power_mw").unwrap() - 50.0).abs() < 0.001);
        assert!(ds.read(3, "voltage_pu").is_none());
    }

    #[tokio::test]
    async fn test_iec104_data_source_refresh_from_client() {
        let config = Iec104Config::default();
        let client = Arc::new(Iec104Client::new(config));

        let obj = InformationObject::MeasuredShortFloat {
            ioa: 1001,
            value: 1.060f32,
            quality: MeasuredQuality::from_u8(0),
        };
        client.data.lock().await.insert(1001, obj);

        let mut mapping = IoaMappingTable::new();
        mapping.add(IoaMapping {
            ioa: 1001,
            element_id: 1,
            parameter: "voltage_pu".to_string(),
            scale: 1.0,
            offset: 0.0,
        });

        let ds = Iec104DataSource::new(client, mapping);
        assert!(ds.read(1, "voltage_pu").is_none());

        ds.refresh_cache().await;
        assert!((ds.read(1, "voltage_pu").unwrap() - 1.060).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_iec104_data_source_scale_conversion() {
        let config = Iec104Config::default();
        let client = Arc::new(Iec104Client::new(config));

        let obj = InformationObject::MeasuredShortFloat {
            ioa: 9001,
            value: 5000.0f32,
            quality: MeasuredQuality::from_u8(0),
        };
        client.data.lock().await.insert(9001, obj);

        let mut mapping = IoaMappingTable::new();
        mapping.add(IoaMapping {
            ioa: 9001,
            element_id: 0,
            parameter: "frequency_hz".to_string(),
            scale: 0.01,
            offset: 0.0,
        });

        let ds = Iec104DataSource::new(client, mapping);
        ds.refresh_cache().await;

        assert!((ds.read(0, "frequency_hz").unwrap() - 50.0).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_iec104_data_source_invalid_quality_skipped() {
        let config = Iec104Config::default();
        let client = Arc::new(Iec104Client::new(config));

        let obj = InformationObject::MeasuredShortFloat {
            ioa: 1001,
            value: 0.5f32,
            quality: MeasuredQuality::from_u8(0x80), // IV=1
        };
        client.data.lock().await.insert(1001, obj);

        let mut mapping = IoaMappingTable::new();
        mapping.add(IoaMapping {
            ioa: 1001,
            element_id: 1,
            parameter: "voltage_pu".to_string(),
            scale: 1.0,
            offset: 0.0,
        });

        let ds = Iec104DataSource::new(client, mapping);
        ds.refresh_cache().await;

        assert!(ds.read(1, "voltage_pu").is_none());
    }

    #[tokio::test]
    async fn test_iec104_data_source_double_point_mapping() {
        let config = Iec104Config::default();
        let client = Arc::new(Iec104Client::new(config));

        let obj = InformationObject::DoublePointTimeTag {
            ioa: 5001,
            value: DoublePointValue::On,
            quality: MeasuredQuality::from_u8(0),
            timestamp_ms: 0,
        };
        client.data.lock().await.insert(5001, obj);

        let mut mapping = IoaMappingTable::new();
        mapping.add(IoaMapping {
            ioa: 5001,
            element_id: 1,
            parameter: "breaker_status".to_string(),
            scale: 1.0,
            offset: 0.0,
        });

        let ds = Iec104DataSource::new(client, mapping);
        ds.refresh_cache().await;

        assert_eq!(ds.read(1, "breaker_status").unwrap(), 1.0);
    }
}

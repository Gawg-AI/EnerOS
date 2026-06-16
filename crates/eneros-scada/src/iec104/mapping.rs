//! IOA (Information Object Address) → ScadaPoint mapping layer.
//!
//! Maps IEC 104 measurement points to EnerOS ScadaPoint (element_id + parameter),
//! with optional engineering scale conversion (raw → engineering value).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Mapping entry: IOA → (element_id, parameter, scale, offset)
///
/// Engineering value = raw_value * scale + offset
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IoaMapping {
    /// IEC 104 Information Object Address
    pub ioa: u32,
    /// EnerOS element ID (e.g., bus number, gen number)
    pub element_id: u64,
    /// SCADA parameter name (e.g., "voltage_pu", "active_power_mw")
    pub parameter: String,
    /// Scale factor: engineering = raw * scale + offset
    #[serde(default = "default_scale")]
    pub scale: f64,
    /// Offset: engineering = raw * scale + offset
    #[serde(default)]
    pub offset: f64,
}

fn default_scale() -> f64 {
    1.0
}

/// IOA mapping table
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IoaMappingTable {
    /// Map from IOA to mapping entry
    entries: HashMap<u32, IoaMapping>,
}

impl IoaMappingTable {
    /// Create an empty mapping table
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Create from a list of mapping entries
    pub fn from_entries(entries: Vec<IoaMapping>) -> Self {
        let mut table = Self::new();
        for entry in entries {
            table.add(entry);
        }
        table
    }

    /// Add a mapping entry
    pub fn add(&mut self, entry: IoaMapping) {
        self.entries.insert(entry.ioa, entry);
    }

    /// Look up a mapping by IOA
    pub fn get(&self, ioa: u32) -> Option<&IoaMapping> {
        self.entries.get(&ioa)
    }

    /// Convert a raw IEC 104 value to engineering value for the given IOA
    pub fn to_engineering(&self, ioa: u32, raw_value: f64) -> Option<f64> {
        self.entries.get(&ioa).map(|m| raw_value * m.scale + m.offset)
    }

    /// Get all mapped IOAs
    pub fn ioas(&self) -> Vec<u32> {
        self.entries.keys().copied().collect()
    }

    /// Number of mappings
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the table is empty
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for IoaMappingTable {
    fn default() -> Self {
        Self::new()
    }
}

/// Build a standard IEEE 14-bus IOA mapping table.
///
/// IOA allocation convention:
/// - 1001–1014: Bus voltage magnitude (p.u.)
/// - 2001–2014: Bus voltage angle (degrees)
/// - 3001–3020: Branch active power flow (MW)
/// - 4001–4020: Branch reactive power flow (MVar)
/// - 5001–5005: Generator active power (MW)
/// - 6001–6005: Generator reactive power (MVar)
/// - 7001–7011: Load active power (MW)
/// - 8001–8011: Load reactive power (MVar)
/// - 9001: System frequency (Hz)
pub fn build_ieee14_ioa_mapping() -> IoaMappingTable {
    let mut entries = Vec::new();

    // Bus voltages (1-14)
    for bus in 1..=14u64 {
        entries.push(IoaMapping {
            ioa: 1000 + bus as u32,
            element_id: bus,
            parameter: "voltage_pu".to_string(),
            scale: 1.0,
            offset: 0.0,
        });
    }

    // Bus angles (1-14)
    for bus in 1..=14u64 {
        entries.push(IoaMapping {
            ioa: 2000 + bus as u32,
            element_id: bus,
            parameter: "angle_deg".to_string(),
            scale: 1.0,
            offset: 0.0,
        });
    }

    // Branch P flows (1-20)
    for branch in 1..=20u64 {
        entries.push(IoaMapping {
            ioa: 3000 + branch as u32,
            element_id: branch,
            parameter: "p_mw".to_string(),
            scale: 1.0,
            offset: 0.0,
        });
    }

    // Branch Q flows (1-20)
    for branch in 1..=20u64 {
        entries.push(IoaMapping {
            ioa: 4000 + branch as u32,
            element_id: branch,
            parameter: "q_mvar".to_string(),
            scale: 1.0,
            offset: 0.0,
        });
    }

    // Generator P outputs (gens on buses 1,2,3,6,8)
    for (idx, gen_bus) in [1u64, 2, 3, 6, 8].iter().enumerate() {
        entries.push(IoaMapping {
            ioa: 5001 + idx as u32,
            element_id: *gen_bus,
            parameter: "gen_p_mw".to_string(),
            scale: 1.0,
            offset: 0.0,
        });
    }

    // Generator Q outputs
    for (idx, gen_bus) in [1u64, 2, 3, 6, 8].iter().enumerate() {
        entries.push(IoaMapping {
            ioa: 6001 + idx as u32,
            element_id: *gen_bus,
            parameter: "gen_q_mvar".to_string(),
            scale: 1.0,
            offset: 0.0,
        });
    }

    // Load P consumption (buses 2,3,4,5,6,9,10,11,12,13,14)
    for (idx, load_bus) in [2u64, 3, 4, 5, 6, 9, 10, 11, 12, 13, 14].iter().enumerate() {
        entries.push(IoaMapping {
            ioa: 7001 + idx as u32,
            element_id: *load_bus,
            parameter: "load_p_mw".to_string(),
            scale: 1.0,
            offset: 0.0,
        });
    }

    // Load Q consumption
    for (idx, load_bus) in [2u64, 3, 4, 5, 6, 9, 10, 11, 12, 13, 14].iter().enumerate() {
        entries.push(IoaMapping {
            ioa: 8001 + idx as u32,
            element_id: *load_bus,
            parameter: "load_q_mvar".to_string(),
            scale: 1.0,
            offset: 0.0,
        });
    }

    // System frequency
    entries.push(IoaMapping {
        ioa: 9001,
        element_id: 0,
        parameter: "frequency_hz".to_string(),
        scale: 1.0,
        offset: 0.0,
    });

    IoaMappingTable::from_entries(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mapping_table_basic() {
        let mut table = IoaMappingTable::new();
        table.add(IoaMapping {
            ioa: 1001,
            element_id: 1,
            parameter: "voltage_pu".to_string(),
            scale: 1.0,
            offset: 0.0,
        });
        table.add(IoaMapping {
            ioa: 1002,
            element_id: 2,
            parameter: "voltage_pu".to_string(),
            scale: 0.001, // raw in kV → p.u. (assuming 100kV base)
            offset: 0.0,
        });

        assert_eq!(table.len(), 2);
        assert!(table.get(1001).is_some());
        assert!(table.get(9999).is_none());
    }

    #[test]
    fn test_engineering_conversion() {
        let mut table = IoaMappingTable::new();
        table.add(IoaMapping {
            ioa: 5001,
            element_id: 1,
            parameter: "gen_p_mw".to_string(),
            scale: 0.1, // raw * 0.1 = MW
            offset: 0.0,
        });
        table.add(IoaMapping {
            ioa: 9001,
            element_id: 0,
            parameter: "frequency_hz".to_string(),
            scale: 0.01, // raw * 0.01 = Hz
            offset: 50.0, // offset to 50 Hz base
        });

        // 2324 raw → 232.4 MW
        let eng = table.to_engineering(5001, 2324.0).unwrap();
        assert!((eng - 232.4).abs() < 0.01);

        // 0 raw → 50.0 Hz
        let freq = table.to_engineering(9001, 0.0).unwrap();
        assert!((freq - 50.0).abs() < 0.001);

        // 5 raw → 50.05 Hz
        let freq = table.to_engineering(9001, 5.0).unwrap();
        assert!((freq - 50.05).abs() < 0.001);
    }

    #[test]
    fn test_ieee14_ioa_mapping() {
        let table = build_ieee14_ioa_mapping();

        // Bus 1 voltage
        let m = table.get(1001).unwrap();
        assert_eq!(m.element_id, 1);
        assert_eq!(m.parameter, "voltage_pu");

        // Bus 14 voltage
        let m = table.get(1014).unwrap();
        assert_eq!(m.element_id, 14);
        assert_eq!(m.parameter, "voltage_pu");

        // Generator 1 (bus 1) P
        let m = table.get(5001).unwrap();
        assert_eq!(m.element_id, 1);
        assert_eq!(m.parameter, "gen_p_mw");

        // Load bus 2 P
        let m = table.get(7001).unwrap();
        assert_eq!(m.element_id, 2);
        assert_eq!(m.parameter, "load_p_mw");

        // Frequency
        let m = table.get(9001).unwrap();
        assert_eq!(m.element_id, 0);
        assert_eq!(m.parameter, "frequency_hz");

        // Total: 14 + 14 + 20 + 20 + 5 + 5 + 11 + 11 + 1 = 101
        assert_eq!(table.len(), 101);
    }

    #[test]
    fn test_mapping_serde_roundtrip() {
        let mut table = IoaMappingTable::new();
        table.add(IoaMapping {
            ioa: 1001,
            element_id: 1,
            parameter: "voltage_pu".to_string(),
            scale: 1.0,
            offset: 0.0,
        });

        let json = serde_json::to_string(&table).unwrap();
        let deserialized: IoaMappingTable = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.len(), 1);
        assert_eq!(deserialized.get(1001).unwrap().parameter, "voltage_pu");
    }
}

use std::collections::HashMap;

use eneros_core::{
    BranchFlow, BusVoltage, ElementId, EnerOSError, GenOutput, LoadConsumption, PowerSystemState,
    Result,
};
use serde::{Deserialize, Serialize};

use crate::collector::ScadaCollector;

/// Target measurement field in the power system state
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MeasurementField {
    /// Bus voltage magnitude (p.u.) for the given bus ID
    BusVoltage(ElementId),
    /// Bus voltage angle (degrees) for the given bus ID
    BusAngle(ElementId),
    /// Branch active power flow (MW) for the given branch ID
    BranchPFlow(ElementId),
    /// Branch reactive power flow (MVar) for the given branch ID
    BranchQFlow(ElementId),
    /// Generator active power output (MW) for the given generator ID
    GenP(ElementId),
    /// Generator reactive power output (MVar) for the given generator ID
    GenQ(ElementId),
    /// Load active power consumption (MW) for the given load ID
    LoadP(ElementId),
    /// Load reactive power consumption (MVar) for the given load ID
    LoadQ(ElementId),
    /// System frequency (Hz)
    Frequency,
}

/// Mapping from a SCADA parameter to a measurement field
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeasurementMapping {
    /// SCADA parameter name
    pub scada_parameter: String,
    /// Target measurement field
    pub target_field: MeasurementField,
}

/// Builder for constructing PowerSystemState from SCADA readings
pub struct SnapshotBuilder {
    /// Mappings from SCADA parameters to measurement fields
    mappings: Vec<MeasurementMapping>,
    /// Required fields that must have data; if any is missing, build() returns an error
    required_fields: Vec<MeasurementField>,
}

impl SnapshotBuilder {
    /// Create a new SnapshotBuilder with the given mappings
    pub fn new(mappings: Vec<MeasurementMapping>) -> Self {
        Self {
            mappings,
            required_fields: Vec::new(),
        }
    }

    /// Set required fields. If any of these fields has no reading, build() returns an error.
    pub fn with_required_fields(mut self, fields: Vec<MeasurementField>) -> Self {
        self.required_fields = fields;
        self
    }

    /// Build a PowerSystemState from the latest SCADA readings
    pub fn build(&self, collector: &ScadaCollector) -> Result<PowerSystemState> {
        // Collect all readings into a map: scada_parameter -> value
        let mut values: HashMap<&str, f64> = HashMap::new();
        for mapping in &self.mappings {
            if let Some(reading) = collector.latest(0, &mapping.scada_parameter) {
                values.insert(&mapping.scada_parameter, reading.value);
            }
        }

        // Actually, we need to look up by element_id too.
        // The mapping's target_field contains the element_id, but the scada_parameter
        // may be shared across elements. Let's collect readings keyed by (element_id, parameter).
        // We need to resolve each mapping to a reading.
        let mut resolved: HashMap<&MeasurementField, f64> = HashMap::new();
        for mapping in &self.mappings {
            let element_id = match &mapping.target_field {
                MeasurementField::BusVoltage(id) => *id,
                MeasurementField::BusAngle(id) => *id,
                MeasurementField::BranchPFlow(id) => *id,
                MeasurementField::BranchQFlow(id) => *id,
                MeasurementField::GenP(id) => *id,
                MeasurementField::GenQ(id) => *id,
                MeasurementField::LoadP(id) => *id,
                MeasurementField::LoadQ(id) => *id,
                MeasurementField::Frequency => 0,
            };
            if let Some(reading) = collector.latest(element_id, &mapping.scada_parameter) {
                resolved.insert(&mapping.target_field, reading.value);
            }
        }

        // Check required fields
        for required in &self.required_fields {
            if !resolved.contains_key(required) {
                return Err(EnerOSError::Config(format!(
                    "Missing required measurement field: {:?}",
                    required
                )));
            }
        }

        // Build PowerSystemState
        let mut bus_voltages_map: HashMap<ElementId, (f64, f64)> = HashMap::new();
        let mut branch_flows_map: HashMap<ElementId, (f64, f64)> = HashMap::new();
        let mut gen_outputs_map: HashMap<ElementId, (f64, f64)> = HashMap::new();
        let mut load_consumptions_map: HashMap<ElementId, (f64, f64)> = HashMap::new();
        let mut frequency = 50.0_f64;

        for mapping in &self.mappings {
            if let Some(&value) = resolved.get(&mapping.target_field) {
                match &mapping.target_field {
                    MeasurementField::BusVoltage(id) => {
                        bus_voltages_map.entry(*id).or_insert((0.0, 0.0)).0 = value;
                    }
                    MeasurementField::BusAngle(id) => {
                        bus_voltages_map.entry(*id).or_insert((0.0, 0.0)).1 = value;
                    }
                    MeasurementField::BranchPFlow(id) => {
                        branch_flows_map.entry(*id).or_insert((0.0, 0.0)).0 = value;
                    }
                    MeasurementField::BranchQFlow(id) => {
                        branch_flows_map.entry(*id).or_insert((0.0, 0.0)).1 = value;
                    }
                    MeasurementField::GenP(id) => {
                        gen_outputs_map.entry(*id).or_insert((0.0, 0.0)).0 = value;
                    }
                    MeasurementField::GenQ(id) => {
                        gen_outputs_map.entry(*id).or_insert((0.0, 0.0)).1 = value;
                    }
                    MeasurementField::LoadP(id) => {
                        load_consumptions_map.entry(*id).or_insert((0.0, 0.0)).0 = value;
                    }
                    MeasurementField::LoadQ(id) => {
                        load_consumptions_map.entry(*id).or_insert((0.0, 0.0)).1 = value;
                    }
                    MeasurementField::Frequency => {
                        frequency = value;
                    }
                }
            }
        }

        let bus_voltages: Vec<BusVoltage> = bus_voltages_map
            .iter()
            .map(|(id, (vm, va))| BusVoltage {
                bus_id: *id,
                voltage_magnitude: *vm,
                voltage_angle: *va,
                voltage_kv: 0.0,
            })
            .collect();

        let branch_flows: Vec<BranchFlow> = branch_flows_map
            .iter()
            .map(|(id, (p, q))| BranchFlow {
                branch_id: *id,
                from_bus: 0,
                to_bus: 0,
                active_power_mw: *p,
                reactive_power_mvar: *q,
                current_ka: 0.0,
                loading_percent: 0.0,
            })
            .collect();

        let generation: Vec<GenOutput> = gen_outputs_map
            .iter()
            .map(|(id, (p, q))| GenOutput {
                gen_id: *id,
                bus_id: 0,
                active_power_mw: *p,
                reactive_power_mvar: *q,
                voltage_setpoint: 0.0,
                status: true,
            })
            .collect();

        let loads: Vec<LoadConsumption> = load_consumptions_map
            .iter()
            .map(|(id, (p, q))| LoadConsumption {
                load_id: *id,
                bus_id: 0,
                active_power_mw: *p,
                reactive_power_mvar: *q,
                status: true,
            })
            .collect();

        Ok(PowerSystemState {
            timestamp: chrono::Utc::now(),
            bus_voltages,
            branch_flows,
            generation,
            loads,
            frequency,
            total_losses: 0.0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::MockDataSource;
    use crate::config::{ScadaConfig, ScadaPoint};
    use std::sync::Arc;

    fn setup_collector() -> (Arc<MockDataSource>, Arc<ScadaCollector>) {
        let mock = Arc::new(MockDataSource::new());
        mock.insert(1, "voltage_pu", 1.02);
        mock.insert(1, "angle_deg", -3.5);
        mock.insert(10, "p_mw", 50.0);
        mock.insert(10, "q_mvar", 10.0);
        mock.insert(100, "gen_p_mw", 200.0);
        mock.insert(100, "gen_q_mvar", 30.0);
        mock.insert(200, "load_p_mw", 150.0);
        mock.insert(200, "load_q_mvar", 20.0);
        mock.insert(0, "frequency_hz", 50.0);

        let config = ScadaConfig {
            points: vec![
                ScadaPoint {
                    element_id: 1,
                    parameter: "voltage_pu".to_string(),
                    scan_rate_ms: 500,
                    deadband: 0.01,
                    min_value: None,
                    max_value: None,
                },
                ScadaPoint {
                    element_id: 1,
                    parameter: "angle_deg".to_string(),
                    scan_rate_ms: 500,
                    deadband: 0.01,
                    min_value: None,
                    max_value: None,
                },
                ScadaPoint {
                    element_id: 10,
                    parameter: "p_mw".to_string(),
                    scan_rate_ms: 500,
                    deadband: 0.1,
                    min_value: None,
                    max_value: None,
                },
                ScadaPoint {
                    element_id: 10,
                    parameter: "q_mvar".to_string(),
                    scan_rate_ms: 500,
                    deadband: 0.1,
                    min_value: None,
                    max_value: None,
                },
                ScadaPoint {
                    element_id: 100,
                    parameter: "gen_p_mw".to_string(),
                    scan_rate_ms: 500,
                    deadband: 0.1,
                    min_value: None,
                    max_value: None,
                },
                ScadaPoint {
                    element_id: 100,
                    parameter: "gen_q_mvar".to_string(),
                    scan_rate_ms: 500,
                    deadband: 0.1,
                    min_value: None,
                    max_value: None,
                },
                ScadaPoint {
                    element_id: 200,
                    parameter: "load_p_mw".to_string(),
                    scan_rate_ms: 500,
                    deadband: 0.1,
                    min_value: None,
                    max_value: None,
                },
                ScadaPoint {
                    element_id: 200,
                    parameter: "load_q_mvar".to_string(),
                    scan_rate_ms: 500,
                    deadband: 0.1,
                    min_value: None,
                    max_value: None,
                },
                ScadaPoint {
                    element_id: 0,
                    parameter: "frequency_hz".to_string(),
                    scan_rate_ms: 1000,
                    deadband: 0.0,
                    min_value: None,
                    max_value: None,
                },
            ],
            default_scan_rate_ms: 1000,
            timeout_ms: 5000,
            enable_quality_check: true,
        };

        let collector = Arc::new(ScadaCollector::new(config, mock.clone()));
        (mock, collector)
    }

    #[test]
    fn test_measurement_field_equality() {
        let f1 = MeasurementField::BusVoltage(1);
        let f2 = MeasurementField::BusVoltage(1);
        let f3 = MeasurementField::BusVoltage(2);
        let f4 = MeasurementField::Frequency;

        assert_eq!(f1, f2);
        assert_ne!(f1, f3);
        assert_ne!(f1, f4);
    }

    #[test]
    fn test_measurement_mapping_creation() {
        let mapping = MeasurementMapping {
            scada_parameter: "voltage_pu".to_string(),
            target_field: MeasurementField::BusVoltage(1),
        };
        assert_eq!(mapping.scada_parameter, "voltage_pu");
        assert_eq!(mapping.target_field, MeasurementField::BusVoltage(1));
    }

    #[test]
    fn test_snapshot_builder_complete_data() {
        let (_, collector) = setup_collector();
        collector.collect_once();

        let mappings = vec![
            MeasurementMapping {
                scada_parameter: "voltage_pu".to_string(),
                target_field: MeasurementField::BusVoltage(1),
            },
            MeasurementMapping {
                scada_parameter: "angle_deg".to_string(),
                target_field: MeasurementField::BusAngle(1),
            },
            MeasurementMapping {
                scada_parameter: "p_mw".to_string(),
                target_field: MeasurementField::BranchPFlow(10),
            },
            MeasurementMapping {
                scada_parameter: "q_mvar".to_string(),
                target_field: MeasurementField::BranchQFlow(10),
            },
            MeasurementMapping {
                scada_parameter: "gen_p_mw".to_string(),
                target_field: MeasurementField::GenP(100),
            },
            MeasurementMapping {
                scada_parameter: "gen_q_mvar".to_string(),
                target_field: MeasurementField::GenQ(100),
            },
            MeasurementMapping {
                scada_parameter: "load_p_mw".to_string(),
                target_field: MeasurementField::LoadP(200),
            },
            MeasurementMapping {
                scada_parameter: "load_q_mvar".to_string(),
                target_field: MeasurementField::LoadQ(200),
            },
            MeasurementMapping {
                scada_parameter: "frequency_hz".to_string(),
                target_field: MeasurementField::Frequency,
            },
        ];

        let builder = SnapshotBuilder::new(mappings);
        let state = builder.build(&collector).unwrap();

        assert_eq!(state.bus_voltages.len(), 1);
        assert!((state.bus_voltages[0].voltage_magnitude - 1.02).abs() < f64::EPSILON);
        assert!((state.bus_voltages[0].voltage_angle - (-3.5)).abs() < f64::EPSILON);
        assert_eq!(state.bus_voltages[0].bus_id, 1);

        assert_eq!(state.branch_flows.len(), 1);
        assert!((state.branch_flows[0].active_power_mw - 50.0).abs() < f64::EPSILON);
        assert!((state.branch_flows[0].reactive_power_mvar - 10.0).abs() < f64::EPSILON);

        assert_eq!(state.generation.len(), 1);
        assert!((state.generation[0].active_power_mw - 200.0).abs() < f64::EPSILON);
        assert!((state.generation[0].reactive_power_mvar - 30.0).abs() < f64::EPSILON);

        assert_eq!(state.loads.len(), 1);
        assert!((state.loads[0].active_power_mw - 150.0).abs() < f64::EPSILON);
        assert!((state.loads[0].reactive_power_mvar - 20.0).abs() < f64::EPSILON);

        assert!((state.frequency - 50.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_snapshot_builder_incomplete_data_with_required() {
        let (mock, collector) = setup_collector();
        // Don't call collect_once, so no data

        let mappings = vec![
            MeasurementMapping {
                scada_parameter: "voltage_pu".to_string(),
                target_field: MeasurementField::BusVoltage(1),
            },
            MeasurementMapping {
                scada_parameter: "frequency_hz".to_string(),
                target_field: MeasurementField::Frequency,
            },
        ];

        let builder = SnapshotBuilder::new(mappings).with_required_fields(vec![
            MeasurementField::BusVoltage(1),
            MeasurementField::Frequency,
        ]);

        let result = builder.build(&collector);
        assert!(result.is_err());

        // Now collect data and try again
        mock.insert(1, "voltage_pu", 1.02);
        mock.insert(0, "frequency_hz", 50.0);
        collector.collect_once();

        let result = builder.build(&collector);
        assert!(result.is_ok());
    }

    #[test]
    fn test_snapshot_builder_partial_data_no_required() {
        let (_, collector) = setup_collector();
        collector.collect_once();

        let mappings = vec![
            MeasurementMapping {
                scada_parameter: "voltage_pu".to_string(),
                target_field: MeasurementField::BusVoltage(1),
            },
            MeasurementMapping {
                scada_parameter: "nonexistent_param".to_string(),
                target_field: MeasurementField::BusVoltage(999),
            },
        ];

        let builder = SnapshotBuilder::new(mappings);
        let state = builder.build(&collector).unwrap();

        // Only bus 1 has data; bus 999 has no reading
        assert_eq!(state.bus_voltages.len(), 1);
        assert_eq!(state.bus_voltages[0].bus_id, 1);
    }

    #[test]
    fn test_snapshot_builder_empty_mappings() {
        let (_, collector) = setup_collector();
        collector.collect_once();

        let builder = SnapshotBuilder::new(vec![]);
        let state = builder.build(&collector).unwrap();

        assert!(state.bus_voltages.is_empty());
        assert!(state.branch_flows.is_empty());
        assert!(state.generation.is_empty());
        assert!(state.loads.is_empty());
    }

    #[test]
    fn test_measurement_field_serde_roundtrip() {
        let fields = vec![
            MeasurementField::BusVoltage(1),
            MeasurementField::BusAngle(2),
            MeasurementField::BranchPFlow(10),
            MeasurementField::BranchQFlow(10),
            MeasurementField::GenP(100),
            MeasurementField::GenQ(100),
            MeasurementField::LoadP(200),
            MeasurementField::LoadQ(200),
            MeasurementField::Frequency,
        ];

        for field in &fields {
            let json = serde_json::to_string(field).unwrap();
            let deserialized: MeasurementField = serde_json::from_str(&json).unwrap();
            assert_eq!(field, &deserialized);
        }
    }
}

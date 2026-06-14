use crate::config::{ScadaConfig, ScadaPoint};
use crate::snapshot::{MeasurementField, MeasurementMapping};

/// Build SCADA config for IEEE 14-bus simulated data
pub fn build_ieee14_scada_config() -> ScadaConfig {
    let mut points = Vec::new();

    // Bus voltages
    for bus_id in 1u64..=14 {
        points.push(ScadaPoint {
            element_id: bus_id,
            parameter: "voltage_pu".to_string(),
            scan_rate_ms: 1000,
            deadband: 0.005,
            min_value: Some(0.8),
            max_value: Some(1.2),
        });
    }

    // Bus angles
    for bus_id in 1u64..=14 {
        points.push(ScadaPoint {
            element_id: bus_id,
            parameter: "angle_deg".to_string(),
            scan_rate_ms: 1000,
            deadband: 0.1,
            min_value: None,
            max_value: None,
        });
    }

    // Generator outputs
    for gen_id in [1u64, 2, 3, 6, 8] {
        points.push(ScadaPoint {
            element_id: gen_id,
            parameter: "gen_p_mw".to_string(),
            scan_rate_ms: 1000,
            deadband: 1.0,
            min_value: None,
            max_value: None,
        });
        points.push(ScadaPoint {
            element_id: gen_id,
            parameter: "gen_q_mvar".to_string(),
            scan_rate_ms: 1000,
            deadband: 1.0,
            min_value: None,
            max_value: None,
        });
    }

    // Load consumption
    for load_id in [2u64, 3, 4, 5, 6, 9, 10, 11, 12, 13, 14] {
        points.push(ScadaPoint {
            element_id: load_id,
            parameter: "load_p_mw".to_string(),
            scan_rate_ms: 1000,
            deadband: 1.0,
            min_value: None,
            max_value: None,
        });
        points.push(ScadaPoint {
            element_id: load_id,
            parameter: "load_q_mvar".to_string(),
            scan_rate_ms: 1000,
            deadband: 1.0,
            min_value: None,
            max_value: None,
        });
    }

    // Frequency
    points.push(ScadaPoint {
        element_id: 0,
        parameter: "frequency_hz".to_string(),
        scan_rate_ms: 500,
        deadband: 0.01,
        min_value: Some(49.0),
        max_value: Some(51.0),
    });

    ScadaConfig {
        points,
        default_scan_rate_ms: 1000,
        timeout_ms: 5000,
        enable_quality_check: true,
    }
}

/// Build SnapshotBuilder mappings for IEEE 14-bus
pub fn build_ieee14_snapshot_mappings() -> Vec<MeasurementMapping> {
    let mut mappings = Vec::new();

    // Bus voltages and angles
    for bus_id in 1u64..=14 {
        mappings.push(MeasurementMapping {
            scada_parameter: "voltage_pu".to_string(),
            target_field: MeasurementField::BusVoltage(bus_id),
        });
        mappings.push(MeasurementMapping {
            scada_parameter: "angle_deg".to_string(),
            target_field: MeasurementField::BusAngle(bus_id),
        });
    }

    // Generator outputs
    for gen_id in [1u64, 2, 3, 6, 8] {
        mappings.push(MeasurementMapping {
            scada_parameter: "gen_p_mw".to_string(),
            target_field: MeasurementField::GenP(gen_id),
        });
        mappings.push(MeasurementMapping {
            scada_parameter: "gen_q_mvar".to_string(),
            target_field: MeasurementField::GenQ(gen_id),
        });
    }

    // Load consumption
    for load_id in [2u64, 3, 4, 5, 6, 9, 10, 11, 12, 13, 14] {
        mappings.push(MeasurementMapping {
            scada_parameter: "load_p_mw".to_string(),
            target_field: MeasurementField::LoadP(load_id),
        });
        mappings.push(MeasurementMapping {
            scada_parameter: "load_q_mvar".to_string(),
            target_field: MeasurementField::LoadQ(load_id),
        });
    }

    // Frequency
    mappings.push(MeasurementMapping {
        scada_parameter: "frequency_hz".to_string(),
        target_field: MeasurementField::Frequency,
    });

    mappings
}

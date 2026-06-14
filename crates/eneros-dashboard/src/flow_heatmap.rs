use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Configuration for power flow heatmap
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowHeatmapConfig {
    pub min_voltage: f64,
    pub max_voltage: f64,
    pub max_loading: f64,
}

impl Default for FlowHeatmapConfig {
    fn default() -> Self {
        Self {
            min_voltage: 0.9,
            max_voltage: 1.1,
            max_loading: 100.0,
        }
    }
}

/// Bus flow data for heatmap
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusFlowData {
    pub id: u64,
    pub v_pu: f64,
}

/// Branch flow data for heatmap
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchFlowData {
    pub id: u64,
    pub from_bus: u64,
    pub to_bus: u64,
    pub loading_percent: f64,
}

/// Overlay data for power flow visualization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowOverlay {
    pub bus_colors: HashMap<u64, String>,
    pub branch_widths: HashMap<u64, f64>,
    pub branch_colors: HashMap<u64, String>,
}

/// Map a voltage value (in p.u.) to a color.
///
/// - Green at 1.0 p.u.
/// - Yellow at 0.95 or 1.05 p.u.
/// - Red at 0.9 or 1.1 p.u.
fn voltage_color(v_pu: f64) -> String {
    let deviation = (v_pu - 1.0).abs();
    if deviation <= 0.05 {
        // Green to yellow
        let t = deviation / 0.05;
        let r = (255.0 * t) as u8;
        let g = 255;
        let b = 0;
        format!("#{:02x}{:02x}{:02x}", r, g, b)
    } else {
        // Yellow to red
        let t = ((deviation - 0.05) / 0.05).min(1.0);
        let r = 255;
        let g = (255.0 * (1.0 - t)) as u8;
        let b = 0;
        format!("#{:02x}{:02x}{:02x}", r, g, b)
    }
}

/// Map a branch loading percentage to a color.
///
/// - Green: < 70%
/// - Yellow: 70-90%
/// - Red: > 90%
fn loading_color(loading_percent: f64) -> String {
    if loading_percent < 70.0 {
        "#00cc00".to_string()
    } else if loading_percent < 90.0 {
        "#cccc00".to_string()
    } else {
        "#cc0000".to_string()
    }
}

/// Map a branch loading percentage to a stroke width.
///
/// - Thin at 0%
/// - Thick at 100%+
fn loading_width(loading_percent: f64, max_loading: f64) -> f64 {
    let ratio = (loading_percent / max_loading).min(1.0);
    1.0 + 5.0 * ratio
}

/// Generate a flow overlay from bus and branch flow data.
pub fn generate_flow_overlay(
    buses: &[BusFlowData],
    branches: &[BranchFlowData],
    config: &FlowHeatmapConfig,
) -> FlowOverlay {
    let bus_colors: HashMap<u64, String> = buses
        .iter()
        .map(|b| {
            let v_clamped = b.v_pu.clamp(config.min_voltage, config.max_voltage);
            (b.id, voltage_color(v_clamped))
        })
        .collect();

    let branch_colors: HashMap<u64, String> = branches
        .iter()
        .map(|b| (b.id, loading_color(b.loading_percent)))
        .collect();

    let branch_widths: HashMap<u64, f64> = branches
        .iter()
        .map(|b| (b.id, loading_width(b.loading_percent, config.max_loading)))
        .collect();

    FlowOverlay {
        bus_colors,
        branch_widths,
        branch_colors,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_voltage_color_nominal() {
        let color = voltage_color(1.0);
        assert!(color.starts_with('#'));
        // At 1.0 p.u., should be green-ish (low red)
        let color = voltage_color(1.0);
        assert_eq!(color, "#00ff00");
    }

    #[test]
    fn test_voltage_color_low() {
        let color = voltage_color(0.9);
        // At 0.9 p.u., should be red-ish
        assert!(color.starts_with('#'));
    }

    #[test]
    fn test_voltage_color_high() {
        let color = voltage_color(1.1);
        assert!(color.starts_with('#'));
    }

    #[test]
    fn test_loading_color_green() {
        assert_eq!(loading_color(50.0), "#00cc00");
    }

    #[test]
    fn test_loading_color_yellow() {
        assert_eq!(loading_color(80.0), "#cccc00");
    }

    #[test]
    fn test_loading_color_red() {
        assert_eq!(loading_color(95.0), "#cc0000");
    }

    #[test]
    fn test_loading_width() {
        let w1 = loading_width(0.0, 100.0);
        let w2 = loading_width(100.0, 100.0);
        assert!(w1 < w2);
        assert_eq!(w1, 1.0);
        assert_eq!(w2, 6.0);
    }

    #[test]
    fn test_generate_flow_overlay() {
        let buses = vec![
            BusFlowData { id: 1, v_pu: 1.0 },
            BusFlowData { id: 2, v_pu: 0.92 },
        ];
        let branches = vec![
            BranchFlowData {
                id: 1,
                from_bus: 1,
                to_bus: 2,
                loading_percent: 50.0,
            },
            BranchFlowData {
                id: 2,
                from_bus: 2,
                to_bus: 3,
                loading_percent: 95.0,
            },
        ];
        let config = FlowHeatmapConfig::default();
        let overlay = generate_flow_overlay(&buses, &branches, &config);

        assert_eq!(overlay.bus_colors.len(), 2);
        assert_eq!(overlay.branch_colors.len(), 2);
        assert_eq!(overlay.branch_widths.len(), 2);

        // Bus 1 at nominal voltage should be green
        assert_eq!(overlay.bus_colors[&1], "#00ff00");
        // Branch 1 at 50% should be green
        assert_eq!(overlay.branch_colors[&1], "#00cc00");
        // Branch 2 at 95% should be red
        assert_eq!(overlay.branch_colors[&2], "#cc0000");
    }

    #[test]
    fn test_default_config() {
        let config = FlowHeatmapConfig::default();
        assert!((config.min_voltage - 0.9).abs() < f64::EPSILON);
        assert!((config.max_voltage - 1.1).abs() < f64::EPSILON);
        assert!((config.max_loading - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_voltage_clamping() {
        let buses = vec![BusFlowData { id: 1, v_pu: 0.5 }];
        let branches = vec![];
        let config = FlowHeatmapConfig::default();
        let overlay = generate_flow_overlay(&buses, &branches, &config);
        // Should not panic; v_pu is clamped to min_voltage
        assert!(overlay.bus_colors.contains_key(&1));
    }
}

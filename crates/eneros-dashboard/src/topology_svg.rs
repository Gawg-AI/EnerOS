use serde::{Deserialize, Serialize};

/// Configuration for topology SVG generation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologySvgConfig {
    pub width: u32,
    pub height: u32,
    pub bus_radius: u32,
    pub font_size: u32,
}

impl Default for TopologySvgConfig {
    fn default() -> Self {
        Self {
            width: 800,
            height: 600,
            bus_radius: 15,
            font_size: 10,
        }
    }
}

/// Bus data for SVG rendering
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusSvgData {
    pub id: u64,
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub zone_id: u64,
    pub voltage_level: String,
}

/// Branch data for SVG rendering
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchSvgData {
    pub id: u64,
    pub from_bus: u64,
    pub to_bus: u64,
    pub status: bool,
}

/// Color palette for zones
const ZONE_COLORS: &[&str] = &[
    "#4A90D9", // blue
    "#7B68EE", // medium slate blue
    "#3CB371", // medium sea green
    "#FF8C00", // dark orange
    "#DC143C", // crimson
    "#00CED1", // dark turquoise
    "#FFD700", // gold
    "#FF69B4", // hot pink
    "#8FBC8F", // dark sea green
    "#BA55D3", // medium orchid
];

fn zone_color(zone_id: u64) -> &'static str {
    ZONE_COLORS[(zone_id as usize) % ZONE_COLORS.len()]
}

/// Generate an SVG string representing the network topology.
///
/// Uses a circular layout: buses are placed evenly around a circle,
/// and branches are drawn as lines between connected buses.
pub fn generate_topology_svg(
    buses: &[BusSvgData],
    branches: &[BranchSvgData],
    config: &TopologySvgConfig,
) -> String {
    let mut svg = String::new();

    svg.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {} {}\" width=\"{}\" height=\"{}\">\n",
        config.width, config.height, config.width, config.height
    ));

    // Background
    svg.push_str(&format!(
        "  <rect width=\"{}\" height=\"{}\" fill=\"#1a1a2e\"/>\n",
        config.width, config.height
    ));

    // Draw branches (lines)
    let bus_map: std::collections::HashMap<u64, (f64, f64)> = buses
        .iter()
        .map(|b| (b.id, (b.x, b.y)))
        .collect();

    for branch in branches {
        let from = bus_map.get(&branch.from_bus);
        let to = bus_map.get(&branch.to_bus);
        if let (Some((x1, y1)), Some((x2, y2))) = (from, to) {
            let color = if branch.status { "#cccccc" } else { "#555555" };
            let stroke_width = if branch.status { 2 } else { 1 };
            svg.push_str(&format!(
                "  <line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" stroke=\"{}\" stroke-width=\"{}\"/>\n",
                x1, y1, x2, y2, color, stroke_width
            ));
        }
    }

    // Draw buses (circles + text)
    for bus in buses {
        let fill = zone_color(bus.zone_id);
        svg.push_str(&format!(
            "  <circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"{}\" fill=\"{}\" stroke=\"#ffffff\" stroke-width=\"1\"/>\n",
            bus.x, bus.y, config.bus_radius, fill
        ));
        svg.push_str(&format!(
            "  <text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" dominant-baseline=\"central\" fill=\"#ffffff\" font-size=\"{}\">{}</text>\n",
            bus.x, bus.y, config.font_size, bus.name
        ));
    }

    // Legend for zones
    let zones: std::collections::BTreeSet<u64> = buses.iter().map(|b| b.zone_id).collect();
    if !zones.is_empty() {
        let legend_x = 20;
        let legend_y = 20;
        svg.push_str("  <g class=\"legend\">\n");
        svg.push_str(&format!(
            "    <text x=\"{}\" y=\"{}\" fill=\"#ffffff\" font-size=\"12\" font-weight=\"bold\">Zones</text>\n",
            legend_x, legend_y
        ));
        for (i, &zone) in zones.iter().enumerate() {
            let y = legend_y as f64 + 20.0 + (i as f64) * 20.0;
            svg.push_str(&format!(
                "    <rect x=\"{}\" y=\"{:.0}\" width=\"12\" height=\"12\" fill=\"{}\"/>\n",
                legend_x, y - 9.0, zone_color(zone)
            ));
            svg.push_str(&format!(
                "    <text x=\"{}\" y=\"{:.0}\" fill=\"#ffffff\" font-size=\"11\">Zone {}</text>\n",
                legend_x + 18, y, zone
            ));
        }
        svg.push_str("  </g>\n");
    }

    svg.push_str("</svg>");
    svg
}

/// Compute circular layout positions for buses.
///
/// Places buses evenly around a circle centered in the SVG canvas.
pub fn circular_layout(buses: &[BusSvgData], config: &TopologySvgConfig) -> Vec<BusSvgData> {
    if buses.is_empty() {
        return Vec::new();
    }

    let cx = config.width as f64 / 2.0;
    let cy = config.height as f64 / 2.0;
    let radius = (config.width.min(config.height) as f64) * 0.35;
    let n = buses.len() as f64;

    buses
        .iter()
        .enumerate()
        .map(|(i, bus)| {
            let angle = 2.0 * std::f64::consts::PI * (i as f64) / n - std::f64::consts::PI / 2.0;
            BusSvgData {
                id: bus.id,
                name: bus.name.clone(),
                x: cx + radius * angle.cos(),
                y: cy + radius * angle.sin(),
                zone_id: bus.zone_id,
                voltage_level: bus.voltage_level.clone(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_topology_svg_basic() {
        let buses = vec![
            BusSvgData {
                id: 1,
                name: "Bus1".to_string(),
                x: 200.0,
                y: 200.0,
                zone_id: 0,
                voltage_level: "110kV".to_string(),
            },
            BusSvgData {
                id: 2,
                name: "Bus2".to_string(),
                x: 600.0,
                y: 200.0,
                zone_id: 1,
                voltage_level: "220kV".to_string(),
            },
        ];
        let branches = vec![BranchSvgData {
            id: 1,
            from_bus: 1,
            to_bus: 2,
            status: true,
        }];
        let config = TopologySvgConfig::default();
        let svg = generate_topology_svg(&buses, &branches, &config);

        assert!(svg.contains("<svg"));
        assert!(svg.contains("</svg>"));
        assert!(svg.contains("<circle"));
        assert!(svg.contains("<line"));
        assert!(svg.contains("<text"));
        assert!(svg.contains("Bus1"));
        assert!(svg.contains("Bus2"));
    }

    #[test]
    fn test_svg_disconnected_branch() {
        let buses = vec![
            BusSvgData {
                id: 1,
                name: "Bus1".to_string(),
                x: 200.0,
                y: 200.0,
                zone_id: 0,
                voltage_level: "110kV".to_string(),
            },
            BusSvgData {
                id: 2,
                name: "Bus2".to_string(),
                x: 600.0,
                y: 200.0,
                zone_id: 0,
                voltage_level: "110kV".to_string(),
            },
        ];
        let branches = vec![BranchSvgData {
            id: 1,
            from_bus: 1,
            to_bus: 2,
            status: false,
        }];
        let config = TopologySvgConfig::default();
        let svg = generate_topology_svg(&buses, &branches, &config);

        assert!(svg.contains("#555555")); // gray for disconnected
    }

    #[test]
    fn test_circular_layout() {
        let buses = vec![
            BusSvgData {
                id: 1,
                name: "Bus1".to_string(),
                x: 0.0,
                y: 0.0,
                zone_id: 0,
                voltage_level: "110kV".to_string(),
            },
            BusSvgData {
                id: 2,
                name: "Bus2".to_string(),
                x: 0.0,
                y: 0.0,
                zone_id: 0,
                voltage_level: "110kV".to_string(),
            },
            BusSvgData {
                id: 3,
                name: "Bus3".to_string(),
                x: 0.0,
                y: 0.0,
                zone_id: 0,
                voltage_level: "110kV".to_string(),
            },
        ];
        let config = TopologySvgConfig::default();
        let layout = circular_layout(&buses, &config);

        assert_eq!(layout.len(), 3);
        // All positions should be different
        assert!(layout[0].x != layout[1].x || layout[0].y != layout[1].y);
        // Positions should be within SVG bounds
        for bus in &layout {
            assert!(bus.x >= 0.0 && bus.x <= config.width as f64);
            assert!(bus.y >= 0.0 && bus.y <= config.height as f64);
        }
    }

    #[test]
    fn test_circular_layout_empty() {
        let buses: Vec<BusSvgData> = vec![];
        let config = TopologySvgConfig::default();
        let layout = circular_layout(&buses, &config);
        assert!(layout.is_empty());
    }

    #[test]
    fn test_svg_contains_zone_legend() {
        let buses = vec![
            BusSvgData {
                id: 1,
                name: "Bus1".to_string(),
                x: 200.0,
                y: 200.0,
                zone_id: 0,
                voltage_level: "110kV".to_string(),
            },
            BusSvgData {
                id: 2,
                name: "Bus2".to_string(),
                x: 600.0,
                y: 200.0,
                zone_id: 1,
                voltage_level: "220kV".to_string(),
            },
        ];
        let branches = vec![];
        let config = TopologySvgConfig::default();
        let svg = generate_topology_svg(&buses, &branches, &config);

        assert!(svg.contains("Zone 0"));
        assert!(svg.contains("Zone 1"));
    }

    #[test]
    fn test_default_config() {
        let config = TopologySvgConfig::default();
        assert_eq!(config.width, 800);
        assert_eq!(config.height, 600);
        assert_eq!(config.bus_radius, 15);
        assert_eq!(config.font_size, 10);
    }
}

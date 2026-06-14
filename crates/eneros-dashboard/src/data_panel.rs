use serde::{Deserialize, Serialize};

/// Reading display data for the data panel
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadingDisplay {
    pub element_id: u64,
    pub parameter: String,
    pub value: f64,
    pub unit: String,
    pub quality: String,
}

/// Data panel data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataPanelData {
    pub readings: Vec<ReadingDisplay>,
    pub timestamp: String,
}

fn quality_color(quality: &str) -> &'static str {
    match quality.to_lowercase().as_str() {
        "good" => "#00cc00",
        "uncertain" => "#cccc00",
        "bad" => "#cc0000",
        _ => "#cccccc",
    }
}

/// Generate an HTML fragment for the real-time data panel.
pub fn generate_data_panel_html(data: &DataPanelData) -> String {
    let mut html = String::new();

    html.push_str("<div class=\"data-panel\">\n");
    html.push_str(&format!(
        "  <h3>Real-Time Data (as of {})</h3>\n",
        data.timestamp
    ));
    html.push_str("  <table class=\"data-table\">\n");
    html.push_str("    <thead><tr><th>Element</th><th>Parameter</th><th>Value</th><th>Unit</th><th>Quality</th></tr></thead>\n");
    html.push_str("    <tbody>\n");

    for reading in &data.readings {
        let color = quality_color(&reading.quality);
        let formatted_value = format!("{:.3}", reading.value);
        html.push_str(&format!(
            "      <tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td><span style=\"color:{}\">●</span> {}</td></tr>\n",
            reading.element_id, reading.parameter, formatted_value, reading.unit, color, reading.quality
        ));
    }

    html.push_str("    </tbody>\n");
    html.push_str("  </table>\n");
    html.push_str("</div>");

    html
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_data_panel_html() {
        let data = DataPanelData {
            readings: vec![
                ReadingDisplay {
                    element_id: 1,
                    parameter: "Voltage".to_string(),
                    value: 1.023,
                    unit: "p.u.".to_string(),
                    quality: "Good".to_string(),
                },
                ReadingDisplay {
                    element_id: 2,
                    parameter: "Loading".to_string(),
                    value: 85.5,
                    unit: "%".to_string(),
                    quality: "Uncertain".to_string(),
                },
            ],
            timestamp: "2025-01-01T12:00:00Z".to_string(),
        };
        let html = generate_data_panel_html(&data);

        assert!(html.contains("Real-Time Data"));
        assert!(html.contains("2025-01-01T12:00:00Z"));
        assert!(html.contains("<table"));
        assert!(html.contains("</table>"));
        assert!(html.contains("Voltage"));
        assert!(html.contains("Loading"));
        assert!(html.contains("1.023"));
        assert!(html.contains("85.500"));
        assert!(html.contains("Good"));
        assert!(html.contains("Uncertain"));
    }

    #[test]
    fn test_quality_color_good() {
        assert_eq!(quality_color("good"), "#00cc00");
    }

    #[test]
    fn test_quality_color_uncertain() {
        assert_eq!(quality_color("uncertain"), "#cccc00");
    }

    #[test]
    fn test_quality_color_bad() {
        assert_eq!(quality_color("bad"), "#cc0000");
    }

    #[test]
    fn test_quality_color_unknown() {
        assert_eq!(quality_color("unknown"), "#cccccc");
    }

    #[test]
    fn test_empty_readings() {
        let data = DataPanelData {
            readings: vec![],
            timestamp: "2025-01-01T12:00:00Z".to_string(),
        };
        let html = generate_data_panel_html(&data);
        assert!(html.contains("Real-Time Data"));
        assert!(html.contains("<tbody>"));
        assert!(html.contains("</tbody>"));
    }
}

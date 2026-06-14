use std::collections::HashMap;
use eneros_equipment::{TwoWindingTransformer, TransmissionLine, ConstantPowerLoad};
use crate::python_bridge::{PythonBridge, BridgeResult};
use crate::topology_types::NetworkTopologyData;
use crate::pandapower_types::PandapowerResult;

pub struct CnpowerEquipmentLoader {
    bridge: PythonBridge,
}

impl CnpowerEquipmentLoader {
    pub fn new() -> Self {
        Self {
            bridge: PythonBridge::new(),
        }
    }

    pub fn with_bridge(bridge: PythonBridge) -> Self {
        Self {
            bridge,
        }
    }

    pub fn load_all_transformers(&mut self) -> BridgeResult<Vec<TwoWindingTransformer>> {
        let raw: Vec<serde_json::Value> = self.bridge.call("list_transformers", HashMap::new())?;
        let mut result = Vec::new();

        for entry in &raw {
            if let Some(trafo) = self.parse_transformer(entry) {
                result.push(trafo);
            }
        }

        Ok(result)
    }

    pub fn load_transformer_by_model(
        &mut self,
        category: &str,
        model: &str,
    ) -> BridgeResult<TwoWindingTransformer> {
        let mut params = HashMap::new();
        params.insert("category".to_string(), serde_json::Value::String(category.to_string()));
        params.insert("model".to_string(), serde_json::Value::String(model.to_string()));

        let raw: serde_json::Value = self.bridge.call("get_transformer", params)?;
        self.parse_transformer(&raw)
            .ok_or_else(|| crate::python_bridge::BridgeError::CommandFailed(
                format!("Failed to parse transformer: {}", model)
            ))
    }

    fn parse_transformer(&self, raw: &serde_json::Value) -> Option<TwoWindingTransformer> {
        let obj = raw.as_object()?;

        let name = obj.get("_model")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let sn_kva = extract_f64(obj, "sn_kva")?;
        let rated_mva = sn_kva / 1000.0;

        let vn_hv_kv = extract_f64(obj, "vn_hv_kv")?;
        let vn_lv_kv = extract_f64(obj, "vn_lv_kv")?;

        let vk_percent = extract_f64(obj, "vk_percent").unwrap_or(4.0);
        let vkr_percent = extract_f64(obj, "vkr_percent").unwrap_or(1.5);

        let tap_position = extract_f64(obj, "tap_position")
            .map(|v| v as i32)
            .unwrap_or(0);

        Some(TwoWindingTransformer {
            id: 0,
            name,
            rated_mva,
            rated_kv_high: vn_hv_kv,
            rated_kv_low: vn_lv_kv,
            impedance_percent: vk_percent,
            resistance_percent: vkr_percent,
            tap_position,
            hv_bus_id: 0,
            lv_bus_id: 1,
        })
    }

    pub fn load_all_cables(&mut self) -> BridgeResult<Vec<TransmissionLine>> {
        let raw: Vec<serde_json::Value> = self.bridge.call("list_cables", HashMap::new())?;
        let mut result = Vec::new();

        for entry in &raw {
            if let Some(line) = self.parse_cable(entry) {
                result.push(line);
            }
        }

        Ok(result)
    }

    fn parse_cable(&self, raw: &serde_json::Value) -> Option<TransmissionLine> {
        let obj = raw.as_object()?;

        let name = obj.get("_model")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let r_per_km = extract_f64(obj, "r_per_km")?;
        let x_per_km = extract_f64(obj, "x_per_km")?;
        let c_per_km_nf = extract_f64(obj, "c_per_km_nf").unwrap_or(0.0);
        let b_per_km = c_per_km_nf * 1e-9 * 2.0 * std::f64::consts::PI * 50.0;

        let rated_current_a = extract_f64(obj, "rated_current_a").unwrap_or(200.0);
        let rated_current_ka = rated_current_a / 1000.0;

        let rated_kv = extract_f64(obj, "vn_kv")
            .or_else(|| extract_f64(obj, "rated_voltage_kv"))
            .unwrap_or(10.0);

        Some(TransmissionLine {
            id: 0,
            name,
            length_km: 1.0,
            r_per_km,
            x_per_km,
            b_per_km,
            rated_current_ka,
            rated_kv,
            from_bus_id: 0,
            to_bus_id: 1,
        })
    }

    pub fn load_all_overhead_lines(&mut self) -> BridgeResult<Vec<TransmissionLine>> {
        let raw: Vec<serde_json::Value> = self.bridge.call("list_overhead_lines", HashMap::new())?;
        let mut result = Vec::new();

        for entry in &raw {
            if let Some(line) = self.parse_overhead_line(entry) {
                result.push(line);
            }
        }

        Ok(result)
    }

    fn parse_overhead_line(&self, raw: &serde_json::Value) -> Option<TransmissionLine> {
        let obj = raw.as_object()?;

        let name = obj.get("_model")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let r_per_km = extract_f64(obj, "r_per_km")?;
        let x_per_km = extract_f64(obj, "x_per_km")?;
        let c_per_km_nf = extract_f64(obj, "c_per_km_nf").unwrap_or(0.0);
        let b_per_km = c_per_km_nf * 1e-9 * 2.0 * std::f64::consts::PI * 50.0;

        let rated_current_a = extract_f64(obj, "rated_current_a").unwrap_or(200.0);
        let rated_current_ka = rated_current_a / 1000.0;

        let rated_kv = extract_f64(obj, "vn_kv")
            .or_else(|| extract_f64(obj, "rated_voltage_kv"))
            .unwrap_or(10.0);

        Some(TransmissionLine {
            id: 0,
            name,
            length_km: 1.0,
            r_per_km,
            x_per_km,
            b_per_km,
            rated_current_ka,
            rated_kv,
            from_bus_id: 0,
            to_bus_id: 1,
        })
    }

    /// Load load definitions from cnpower.
    /// Note: cnpower equipment catalog does not provide load data directly.
    /// Load data should come from network topology via `build_full_network()`.
    /// This method returns an empty vector as a placeholder.
    pub fn load_all_loads(&mut self) -> BridgeResult<Vec<ConstantPowerLoad>> {
        // cnpower equipment catalog does not contain load definitions.
        // Load data comes from the network topology (build_full_network).
        Ok(Vec::new())
    }

    /// Load all switchgear from cnpower equipment catalog
    pub fn load_all_switchgear(&mut self) -> BridgeResult<Vec<serde_json::Value>> {
        self.bridge.call("list_switchgear", HashMap::new())
    }

    /// Load all reactive compensation equipment from cnpower equipment catalog
    pub fn load_all_reactive_compensation(&mut self) -> BridgeResult<Vec<serde_json::Value>> {
        self.bridge.call("list_reactive_compensation", HashMap::new())
    }

    /// Load all new energy equipment (PV, wind, storage, EV chargers) from cnpower equipment catalog
    pub fn load_all_new_energy(&mut self) -> BridgeResult<serde_json::Value> {
        let pv: Vec<serde_json::Value> = self.bridge.call("list_photovoltaic", HashMap::new())?;
        let wind: Vec<serde_json::Value> = self.bridge.call("list_wind_turbines", HashMap::new())?;
        let storage: Vec<serde_json::Value> = self.bridge.call("list_energy_storage", HashMap::new())?;
        let ev: Vec<serde_json::Value> = self.bridge.call("list_ev_chargers", HashMap::new())?;
        Ok(serde_json::json!({
            "photovoltaic": pv,
            "wind_turbines": wind,
            "energy_storage": storage,
            "ev_chargers": ev,
        }))
    }

    pub fn get_standards(&mut self) -> BridgeResult<serde_json::Value> {
        self.bridge.call("list_standards", HashMap::new())
    }

    pub fn get_connection_modes(&mut self) -> BridgeResult<serde_json::Value> {
        self.bridge.call("list_connection_modes", HashMap::new())
    }

    pub fn normalize_equipment(
        &mut self,
        equipment_type: &str,
        params: serde_json::Value,
    ) -> BridgeResult<serde_json::Value> {
        let mut bridge_params = HashMap::new();
        bridge_params.insert("equipment_type".to_string(), serde_json::Value::String(equipment_type.to_string()));
        bridge_params.insert("params".to_string(), params);
        self.bridge.call("normalize_equipment", bridge_params)
    }

    pub fn check_compliance(
        &mut self,
        equipment_type: &str,
        spec: serde_json::Value,
        operating: serde_json::Value,
    ) -> BridgeResult<serde_json::Value> {
        let mut bridge_params = HashMap::new();
        bridge_params.insert("equipment_type".to_string(), serde_json::Value::String(equipment_type.to_string()));
        bridge_params.insert("spec".to_string(), spec);
        bridge_params.insert("operating".to_string(), operating);
        self.bridge.call("check_compliance", bridge_params)
    }

    pub fn build_network(
        &mut self,
        assets: serde_json::Value,
        run_powerflow: bool,
    ) -> BridgeResult<serde_json::Value> {
        let mut bridge_params = HashMap::new();
        bridge_params.insert("assets".to_string(), assets);
        bridge_params.insert("run_powerflow".to_string(), serde_json::Value::Bool(run_powerflow));
        self.bridge.call("build_network", bridge_params)
    }

    /// Build a full network from cnpower assets and return complete topology
    pub fn build_full_network(&mut self, assets: serde_json::Value) -> BridgeResult<NetworkTopologyData> {
        let mut params = HashMap::new();
        params.insert("assets".to_string(), assets);
        self.bridge.call("build_full_network", params)
    }

    /// Run pandapower power flow and return detailed results
    pub fn run_powerflow(&mut self, assets: serde_json::Value) -> BridgeResult<PandapowerResult> {
        let mut params = HashMap::new();
        params.insert("assets".to_string(), assets);
        self.bridge.call("run_powerflow", params)
    }
}

impl Default for CnpowerEquipmentLoader {
    fn default() -> Self {
        Self::new()
    }
}

fn extract_f64(obj: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<f64> {
    obj.get(key).and_then(|v| {
        if let Some(n) = v.as_f64() {
            Some(n)
        } else if let Some(s) = v.as_str() {
            s.parse::<f64>().ok()
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_loader_creation() {
        let _loader = CnpowerEquipmentLoader::new();
        // Verify the loader can be created (doesn't actually call Python)
    }

    #[test]
    fn test_extract_f64_from_number() {
        let mut map = serde_json::Map::new();
        map.insert("value".to_string(), serde_json::Value::Number(serde_json::Number::from_f64(42.5).unwrap()));
        assert_eq!(extract_f64(&map, "value"), Some(42.5));
    }

    #[test]
    fn test_extract_f64_from_string() {
        let mut map = serde_json::Map::new();
        map.insert("value".to_string(), serde_json::Value::String("3.14".to_string()));
        assert!((extract_f64(&map, "value").unwrap() - 3.14).abs() < 0.001);
    }

    #[test]
    fn test_extract_f64_missing() {
        let map = serde_json::Map::new();
        assert_eq!(extract_f64(&map, "missing"), None);
    }
}

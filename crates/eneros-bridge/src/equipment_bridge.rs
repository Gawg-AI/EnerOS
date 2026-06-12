use std::collections::HashMap;
use eneros_core::ElementId;
use eneros_equipment::{TwoWindingTransformer, TransmissionLine, ConstantPowerLoad};
use crate::python_bridge::{PythonBridge, BridgeResult};

pub struct CnpowerEquipmentLoader {
    bridge: PythonBridge,
    next_id: ElementId,
}

impl CnpowerEquipmentLoader {
    pub fn new() -> Self {
        Self {
            bridge: PythonBridge::new(),
            next_id: 1,
        }
    }

    pub fn with_bridge(bridge: PythonBridge) -> Self {
        Self {
            bridge,
            next_id: 1,
        }
    }

    fn next_id(&mut self) -> ElementId {
        let id = self.next_id;
        self.next_id += 1;
        id
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
        })
    }

    pub fn load_all_loads(&mut self) -> BridgeResult<Vec<ConstantPowerLoad>> {
        let _raw: Vec<serde_json::Value> = self.bridge.call("list_validation_rules", HashMap::new())?;
        Ok(Vec::new())
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

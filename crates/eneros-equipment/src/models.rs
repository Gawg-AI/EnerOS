use std::collections::HashMap;
use eneros_core::{ElementId, EquipmentType};
use crate::traits::EquipmentModel;

/// Synchronous generator model
#[derive(Debug, Clone)]
pub struct SynchronousGenerator {
    pub id: ElementId,
    pub name: String,
    pub rated_mw: f64,
    pub rated_mvar: f64,
    pub rated_kv: f64,
    pub x_d: f64,       // d-axis synchronous reactance
    pub x_q: f64,       // q-axis synchronous reactance
    pub x_d_trans: f64, // d-axis transient reactance
}

impl EquipmentModel for SynchronousGenerator {
    fn id(&self) -> ElementId {
        self.id
    }

    fn equipment_type(&self) -> EquipmentType {
        EquipmentType::SynchronousGenerator
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn parameters(&self) -> HashMap<String, f64> {
        let mut params = HashMap::new();
        params.insert("rated_mw".to_string(), self.rated_mw);
        params.insert("rated_mvar".to_string(), self.rated_mvar);
        params.insert("rated_kv".to_string(), self.rated_kv);
        params.insert("x_d".to_string(), self.x_d);
        params.insert("x_q".to_string(), self.x_q);
        params.insert("x_d_trans".to_string(), self.x_d_trans);
        params
    }

    fn get_parameter(&self, name: &str) -> Option<f64> {
        match name {
            "rated_mw" => Some(self.rated_mw),
            "rated_mvar" => Some(self.rated_mvar),
            "rated_kv" => Some(self.rated_kv),
            "x_d" => Some(self.x_d),
            "x_q" => Some(self.x_q),
            "x_d_trans" => Some(self.x_d_trans),
            _ => None,
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.rated_mw <= 0.0 {
            return Err("rated_mw must be positive".to_string());
        }
        if self.rated_kv <= 0.0 {
            return Err("rated_kv must be positive".to_string());
        }
        Ok(())
    }

    fn rated_capacity(&self) -> Option<f64> {
        Some((self.rated_mw.powi(2) + self.rated_mvar.powi(2)).sqrt())
    }

    fn rated_voltage(&self) -> Option<f64> {
        Some(self.rated_kv)
    }
}

/// Two-winding transformer model
#[derive(Debug, Clone)]
pub struct TwoWindingTransformer {
    pub id: ElementId,
    pub name: String,
    pub rated_mva: f64,
    pub rated_kv_high: f64,
    pub rated_kv_low: f64,
    pub impedance_percent: f64,
    pub resistance_percent: f64,
    pub tap_position: i32,
}

impl EquipmentModel for TwoWindingTransformer {
    fn id(&self) -> ElementId {
        self.id
    }

    fn equipment_type(&self) -> EquipmentType {
        EquipmentType::TwoWindingTransformer
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn parameters(&self) -> HashMap<String, f64> {
        let mut params = HashMap::new();
        params.insert("rated_mva".to_string(), self.rated_mva);
        params.insert("rated_kv_high".to_string(), self.rated_kv_high);
        params.insert("rated_kv_low".to_string(), self.rated_kv_low);
        params.insert("impedance_percent".to_string(), self.impedance_percent);
        params.insert("resistance_percent".to_string(), self.resistance_percent);
        params.insert("tap_position".to_string(), self.tap_position as f64);
        params
    }

    fn get_parameter(&self, name: &str) -> Option<f64> {
        match name {
            "rated_mva" => Some(self.rated_mva),
            "rated_kv_high" => Some(self.rated_kv_high),
            "rated_kv_low" => Some(self.rated_kv_low),
            "impedance_percent" => Some(self.impedance_percent),
            "resistance_percent" => Some(self.resistance_percent),
            "tap_position" => Some(self.tap_position as f64),
            _ => None,
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.rated_mva <= 0.0 {
            return Err("rated_mva must be positive".to_string());
        }
        if self.rated_kv_high <= 0.0 || self.rated_kv_low <= 0.0 {
            return Err("rated_kv must be positive".to_string());
        }
        Ok(())
    }

    fn rated_capacity(&self) -> Option<f64> {
        Some(self.rated_mva)
    }

    fn rated_voltage(&self) -> Option<f64> {
        Some(self.rated_kv_high)
    }
}

/// Transmission line model
#[derive(Debug, Clone)]
pub struct TransmissionLine {
    pub id: ElementId,
    pub name: String,
    pub length_km: f64,
    pub r_per_km: f64,    // Resistance per km (Ohm/km)
    pub x_per_km: f64,    // Reactance per km (Ohm/km)
    pub b_per_km: f64,    // Susceptance per km (S/km)
    pub rated_current_ka: f64,
    pub rated_kv: f64,
}

impl EquipmentModel for TransmissionLine {
    fn id(&self) -> ElementId {
        self.id
    }

    fn equipment_type(&self) -> EquipmentType {
        EquipmentType::OverheadLine
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn parameters(&self) -> HashMap<String, f64> {
        let mut params = HashMap::new();
        params.insert("length_km".to_string(), self.length_km);
        params.insert("r_per_km".to_string(), self.r_per_km);
        params.insert("x_per_km".to_string(), self.x_per_km);
        params.insert("b_per_km".to_string(), self.b_per_km);
        params.insert("rated_current_ka".to_string(), self.rated_current_ka);
        params.insert("rated_kv".to_string(), self.rated_kv);
        params
    }

    fn get_parameter(&self, name: &str) -> Option<f64> {
        match name {
            "length_km" => Some(self.length_km),
            "r_per_km" => Some(self.r_per_km),
            "x_per_km" => Some(self.x_per_km),
            "b_per_km" => Some(self.b_per_km),
            "rated_current_ka" => Some(self.rated_current_ka),
            "rated_kv" => Some(self.rated_kv),
            _ => None,
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.length_km <= 0.0 {
            return Err("length_km must be positive".to_string());
        }
        Ok(())
    }

    fn rated_capacity(&self) -> Option<f64> {
        // Calculate rated MVA from voltage and current
        Some(self.rated_kv * self.rated_current_ka * 1.732)
    }

    fn rated_voltage(&self) -> Option<f64> {
        Some(self.rated_kv)
    }
}

/// Constant power load model
#[derive(Debug, Clone)]
pub struct ConstantPowerLoad {
    pub id: ElementId,
    pub name: String,
    pub p_mw: f64,
    pub q_mvar: f64,
    pub rated_kv: f64,
}

impl EquipmentModel for ConstantPowerLoad {
    fn id(&self) -> ElementId {
        self.id
    }

    fn equipment_type(&self) -> EquipmentType {
        EquipmentType::ConstantPowerLoad
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn parameters(&self) -> HashMap<String, f64> {
        let mut params = HashMap::new();
        params.insert("p_mw".to_string(), self.p_mw);
        params.insert("q_mvar".to_string(), self.q_mvar);
        params.insert("rated_kv".to_string(), self.rated_kv);
        params
    }

    fn get_parameter(&self, name: &str) -> Option<f64> {
        match name {
            "p_mw" => Some(self.p_mw),
            "q_mvar" => Some(self.q_mvar),
            "rated_kv" => Some(self.rated_kv),
            _ => None,
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.rated_kv <= 0.0 {
            return Err("rated_kv must be positive".to_string());
        }
        Ok(())
    }

    fn rated_capacity(&self) -> Option<f64> {
        Some((self.p_mw.powi(2) + self.q_mvar.powi(2)).sqrt())
    }

    fn rated_voltage(&self) -> Option<f64> {
        Some(self.rated_kv)
    }
}

use std::collections::HashMap;
use eneros_core::{ElementId, EquipmentType};
use crate::traits::{EquipmentModel, AdmittanceContribution, MultiAdmittanceContribution};

/// Synchronous generator model
#[derive(Debug, Clone)]
pub struct SynchronousGenerator {
    pub id: ElementId,
    pub name: String,
    pub rated_mw: f64,
    pub rated_mvar: f64,
    pub rated_kv: f64,
    pub x_d: f64,
    pub x_q: f64,
    pub x_d_trans: f64,
    pub bus_id: ElementId,
}

impl EquipmentModel for SynchronousGenerator {
    fn id(&self) -> ElementId { self.id }
    fn equipment_type(&self) -> EquipmentType { EquipmentType::SynchronousGenerator }
    fn name(&self) -> &str { &self.name }

    fn parameters(&self) -> HashMap<String, f64> {
        let mut p = HashMap::new();
        p.insert("rated_mw".into(), self.rated_mw);
        p.insert("rated_mvar".into(), self.rated_mvar);
        p.insert("rated_kv".into(), self.rated_kv);
        p.insert("x_d".into(), self.x_d);
        p.insert("x_q".into(), self.x_q);
        p.insert("x_d_trans".into(), self.x_d_trans);
        p.insert("bus_id".into(), self.bus_id as f64);
        p
    }

    fn get_parameter(&self, name: &str) -> Option<f64> {
        match name {
            "rated_mw" => Some(self.rated_mw),
            "rated_mvar" => Some(self.rated_mvar),
            "rated_kv" => Some(self.rated_kv),
            "x_d" => Some(self.x_d),
            "x_q" => Some(self.x_q),
            "x_d_trans" => Some(self.x_d_trans),
            "bus_id" => Some(self.bus_id as f64),
            _ => None,
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.rated_mw <= 0.0 { return Err("rated_mw must be positive".into()); }
        if self.rated_kv <= 0.0 { return Err("rated_kv must be positive".into()); }
        Ok(())
    }

    fn rated_capacity(&self) -> Option<f64> {
        Some((self.rated_mw.powi(2) + self.rated_mvar.powi(2)).sqrt())
    }

    fn rated_voltage(&self) -> Option<f64> { Some(self.rated_kv) }
    fn bus_ids(&self) -> Vec<ElementId> { vec![self.bus_id] }

    fn to_admittance(&self, _base_mva: f64, _base_kv: f64) -> Option<AdmittanceContribution> {
        // Synchronous generator modeled as voltage source behind transient reactance.
        // For Y-bus, generator contributes as a shunt admittance: y = 1 / (j * x_d_trans)
        let x_trans = num_complex::Complex::new(0.0, self.x_d_trans);
        let y = if x_trans.norm() > 1e-12 {
            num_complex::Complex::new(1.0, 0.0) / x_trans
        } else {
            num_complex::Complex::new(0.0, 0.0)
        };
        Some(AdmittanceContribution {
            y_series: num_complex::Complex::new(0.0, 0.0),
            y_shunt: y,
            y_from_shunt: num_complex::Complex::new(0.0, 0.0),
            y_to_shunt: num_complex::Complex::new(0.0, 0.0),
        })
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
    pub tap_step_percent: f64,
    pub hv_bus_id: ElementId,
    pub lv_bus_id: ElementId,
}

impl EquipmentModel for TwoWindingTransformer {
    fn id(&self) -> ElementId { self.id }
    fn equipment_type(&self) -> EquipmentType { EquipmentType::TwoWindingTransformer }
    fn name(&self) -> &str { &self.name }

    fn parameters(&self) -> HashMap<String, f64> {
        let mut p = HashMap::new();
        p.insert("rated_mva".into(), self.rated_mva);
        p.insert("rated_kv_high".into(), self.rated_kv_high);
        p.insert("rated_kv_low".into(), self.rated_kv_low);
        p.insert("impedance_percent".into(), self.impedance_percent);
        p.insert("resistance_percent".into(), self.resistance_percent);
        p.insert("tap_position".into(), self.tap_position as f64);
        p.insert("tap_step_percent".into(), self.tap_step_percent);
        p.insert("hv_bus_id".into(), self.hv_bus_id as f64);
        p.insert("lv_bus_id".into(), self.lv_bus_id as f64);
        p
    }

    fn get_parameter(&self, name: &str) -> Option<f64> {
        match name {
            "rated_mva" => Some(self.rated_mva),
            "rated_kv_high" => Some(self.rated_kv_high),
            "rated_kv_low" => Some(self.rated_kv_low),
            "impedance_percent" => Some(self.impedance_percent),
            "resistance_percent" => Some(self.resistance_percent),
            "tap_position" => Some(self.tap_position as f64),
            "tap_step_percent" => Some(self.tap_step_percent),
            "hv_bus_id" => Some(self.hv_bus_id as f64),
            "lv_bus_id" => Some(self.lv_bus_id as f64),
            _ => None,
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.rated_mva <= 0.0 { return Err("rated_mva must be positive".into()); }
        if self.rated_kv_high <= 0.0 || self.rated_kv_low <= 0.0 {
            return Err("rated_kv must be positive".into());
        }
        Ok(())
    }

    fn rated_capacity(&self) -> Option<f64> { Some(self.rated_mva) }
    fn rated_voltage(&self) -> Option<f64> { Some(self.rated_kv_high) }
    fn bus_ids(&self) -> Vec<ElementId> { vec![self.hv_bus_id, self.lv_bus_id] }

    fn to_admittance(&self, _base_mva: f64, _base_kv: f64) -> Option<AdmittanceContribution> {
        // Two-winding transformer with off-nominal tap ratio:
        // Y-bus entries: Y_ii = y/tap^2, Y_jj = y, Y_ij = Y_ji = -y/tap
        // where y = 1/Z_pu and tap = 1.0 + tap_position * tap_step
        let z_pu = num_complex::Complex::new(
            self.resistance_percent / 100.0,
            self.impedance_percent / 100.0,
        );
        let y = if z_pu.norm() > 1e-12 { num_complex::Complex::new(1.0, 0.0) / z_pu } else { num_complex::Complex::new(0.0, 0.0) };
        let tap_step = self.tap_step_percent / 100.0;
        let tap = 1.0 + self.tap_position as f64 * tap_step;
        let tap_sq = tap * tap;

        // y_series represents the off-diagonal element: Y_ij = -y/tap
        let y_series = -y / tap;
        // y_from_shunt: additional shunt at from (HV) bus = y/tap^2 - y (diagonal minus symmetric part)
        let y_from_shunt = y / tap_sq - y;
        // y_to_shunt: additional shunt at to (LV) bus = y - y (zero when no tap)
        // Actually for the full model: Y_jj = y, so y_to_shunt = 0
        // But the symmetric y_shunt already accounts for y, so:
        // y_shunt = y (the base diagonal contribution shared by both buses)
        // y_from_shunt = y/tap^2 - y (extra at HV bus due to tap)
        // y_to_shunt = 0 (no extra at LV bus)
        Some(AdmittanceContribution {
            y_series,
            y_shunt: y, // base diagonal contribution
            y_from_shunt,
            y_to_shunt: num_complex::Complex::new(0.0, 0.0),
        })
    }
}

/// Three-winding transformer model
#[derive(Debug, Clone)]
pub struct ThreeWindingTransformer {
    pub id: ElementId,
    pub name: String,
    pub rated_mva: f64,
    pub rated_kv_hv: f64,
    pub rated_kv_mv: f64,
    pub rated_kv_lv: f64,
    pub vk_hv_percent: f64,
    pub vkr_hv_percent: f64,
    pub vk_mv_percent: f64,
    pub vkr_mv_percent: f64,
    pub vk_lv_percent: f64,
    pub vkr_lv_percent: f64,
    pub hv_bus_id: ElementId,
    pub mv_bus_id: ElementId,
    pub lv_bus_id: ElementId,
}

impl ThreeWindingTransformer {
    /// Convert pair short-circuit impedances to star equivalent branch impedances.
    /// vk_hv_percent = Z_HM, vk_mv_percent = Z_HT, vk_lv_percent = Z_MT (pair values)
    /// Star: Z_H = (Z_HM + Z_HT - Z_MT)/2, Z_M = (Z_HM + Z_MT - Z_HT)/2, Z_L = (Z_HT + Z_MT - Z_HM)/2
    fn star_impedance_from_pairs(
        vk_hm_pct: f64, vkr_hm_pct: f64,
        vk_ht_pct: f64, vkr_ht_pct: f64,
        vk_mt_pct: f64, vkr_mt_pct: f64,
    ) -> (num_complex::Complex<f64>, num_complex::Complex<f64>, num_complex::Complex<f64>) {
        let z_hm = num_complex::Complex::new(vkr_hm_pct / 100.0, vk_hm_pct / 100.0);
        let z_ht = num_complex::Complex::new(vkr_ht_pct / 100.0, vk_ht_pct / 100.0);
        let z_mt = num_complex::Complex::new(vkr_mt_pct / 100.0, vk_mt_pct / 100.0);

        let z_h = (z_hm + z_ht - z_mt) / 2.0;
        let z_m = (z_hm + z_mt - z_ht) / 2.0;
        let z_l = (z_ht + z_mt - z_hm) / 2.0;

        (z_h, z_m, z_l)
    }
}

impl EquipmentModel for ThreeWindingTransformer {
    fn id(&self) -> ElementId { self.id }
    fn equipment_type(&self) -> EquipmentType { EquipmentType::ThreeWindingTransformer }
    fn name(&self) -> &str { &self.name }

    fn parameters(&self) -> HashMap<String, f64> {
        let mut p = HashMap::new();
        p.insert("rated_mva".into(), self.rated_mva);
        p.insert("rated_kv_hv".into(), self.rated_kv_hv);
        p.insert("rated_kv_mv".into(), self.rated_kv_mv);
        p.insert("rated_kv_lv".into(), self.rated_kv_lv);
        p.insert("vk_hv_percent".into(), self.vk_hv_percent);
        p.insert("vkr_hv_percent".into(), self.vkr_hv_percent);
        p.insert("vk_mv_percent".into(), self.vk_mv_percent);
        p.insert("vkr_mv_percent".into(), self.vkr_mv_percent);
        p.insert("vk_lv_percent".into(), self.vk_lv_percent);
        p.insert("vkr_lv_percent".into(), self.vkr_lv_percent);
        p.insert("hv_bus_id".into(), self.hv_bus_id as f64);
        p.insert("mv_bus_id".into(), self.mv_bus_id as f64);
        p.insert("lv_bus_id".into(), self.lv_bus_id as f64);
        p
    }

    fn get_parameter(&self, name: &str) -> Option<f64> {
        match name {
            "rated_mva" => Some(self.rated_mva),
            "rated_kv_hv" => Some(self.rated_kv_hv),
            "rated_kv_mv" => Some(self.rated_kv_mv),
            "rated_kv_lv" => Some(self.rated_kv_lv),
            "vk_hv_percent" => Some(self.vk_hv_percent),
            "vkr_hv_percent" => Some(self.vkr_hv_percent),
            "vk_mv_percent" => Some(self.vk_mv_percent),
            "vkr_mv_percent" => Some(self.vkr_mv_percent),
            "vk_lv_percent" => Some(self.vk_lv_percent),
            "vkr_lv_percent" => Some(self.vkr_lv_percent),
            "hv_bus_id" => Some(self.hv_bus_id as f64),
            "mv_bus_id" => Some(self.mv_bus_id as f64),
            "lv_bus_id" => Some(self.lv_bus_id as f64),
            _ => None,
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.rated_mva <= 0.0 { return Err("rated_mva must be positive".into()); }
        if self.rated_kv_hv <= self.rated_kv_mv || self.rated_kv_mv <= self.rated_kv_lv {
            return Err("voltage levels must be HV > MV > LV".into());
        }
        Ok(())
    }

    fn rated_capacity(&self) -> Option<f64> { Some(self.rated_mva) }
    fn rated_voltage(&self) -> Option<f64> { Some(self.rated_kv_hv) }
    fn bus_ids(&self) -> Vec<ElementId> { vec![self.hv_bus_id, self.mv_bus_id, self.lv_bus_id] }

    fn to_admittance(&self, _base_mva: f64, _base_kv: f64) -> Option<AdmittanceContribution> {
        // Three-winding transformer requires multi-terminal representation.
        // to_admittance returns the HV-MV contribution as the primary.
        let (z_h, _z_m, _z_l) = Self::star_impedance_from_pairs(
            self.vk_hv_percent, self.vkr_hv_percent,
            self.vk_mv_percent, self.vkr_mv_percent,
            self.vk_lv_percent, self.vkr_lv_percent,
        );
        let y_hv = if z_h.norm() > 1e-12 { num_complex::Complex::new(1.0, 0.0) / z_h } else { num_complex::Complex::new(0.0, 0.0) };
        Some(AdmittanceContribution {
            y_series: y_hv,
            y_shunt: num_complex::Complex::new(0.0, 0.0),
            y_from_shunt: num_complex::Complex::new(0.0, 0.0),
            y_to_shunt: num_complex::Complex::new(0.0, 0.0),
        })
    }

    fn to_admittance_multi(&self, _base_mva: f64, _base_kv: f64) -> Option<MultiAdmittanceContribution> {
        // Three-winding transformer modeled as star (T) equivalent:
        // Convert pair short-circuit impedances to star branch impedances first.
        let (z_h, z_m, z_l) = Self::star_impedance_from_pairs(
            self.vk_hv_percent, self.vkr_hv_percent,
            self.vk_mv_percent, self.vkr_mv_percent,
            self.vk_lv_percent, self.vkr_lv_percent,
        );

        let y_hv = if z_h.norm() > 1e-12 { num_complex::Complex::new(1.0, 0.0) / z_h } else { num_complex::Complex::new(0.0, 0.0) };
        let y_mv = if z_m.norm() > 1e-12 { num_complex::Complex::new(1.0, 0.0) / z_m } else { num_complex::Complex::new(0.0, 0.0) };
        let y_lv = if z_l.norm() > 1e-12 { num_complex::Complex::new(1.0, 0.0) / z_l } else { num_complex::Complex::new(0.0, 0.0) };

        Some(MultiAdmittanceContribution {
            bus_ids: vec![self.hv_bus_id, self.mv_bus_id, self.lv_bus_id],
            contributions: vec![
                AdmittanceContribution { y_series: y_hv, y_shunt: num_complex::Complex::new(0.0, 0.0), y_from_shunt: num_complex::Complex::new(0.0, 0.0), y_to_shunt: num_complex::Complex::new(0.0, 0.0) },
                AdmittanceContribution { y_series: y_mv, y_shunt: num_complex::Complex::new(0.0, 0.0), y_from_shunt: num_complex::Complex::new(0.0, 0.0), y_to_shunt: num_complex::Complex::new(0.0, 0.0) },
                AdmittanceContribution { y_series: y_lv, y_shunt: num_complex::Complex::new(0.0, 0.0), y_from_shunt: num_complex::Complex::new(0.0, 0.0), y_to_shunt: num_complex::Complex::new(0.0, 0.0) },
            ],
        })
    }
}

/// Transmission line model (overhead or cable)
#[derive(Debug, Clone)]
pub struct TransmissionLine {
    pub id: ElementId,
    pub name: String,
    pub length_km: f64,
    pub r_per_km: f64,
    pub x_per_km: f64,
    pub b_per_km: f64,
    pub rated_current_ka: f64,
    pub rated_kv: f64,
    pub from_bus_id: ElementId,
    pub to_bus_id: ElementId,
}

impl TransmissionLine {
    pub fn total_impedance(&self) -> num_complex::Complex<f64> {
        num_complex::Complex::new(self.r_per_km * self.length_km, self.x_per_km * self.length_km)
    }

    pub fn total_shunt_admittance(&self) -> num_complex::Complex<f64> {
        num_complex::Complex::new(0.0, self.b_per_km * self.length_km)
    }
}

impl EquipmentModel for TransmissionLine {
    fn id(&self) -> ElementId { self.id }
    fn equipment_type(&self) -> EquipmentType { EquipmentType::OverheadLine }
    fn name(&self) -> &str { &self.name }

    fn parameters(&self) -> HashMap<String, f64> {
        let mut p = HashMap::new();
        p.insert("length_km".into(), self.length_km);
        p.insert("r_per_km".into(), self.r_per_km);
        p.insert("x_per_km".into(), self.x_per_km);
        p.insert("b_per_km".into(), self.b_per_km);
        p.insert("rated_current_ka".into(), self.rated_current_ka);
        p.insert("rated_kv".into(), self.rated_kv);
        p.insert("from_bus_id".into(), self.from_bus_id as f64);
        p.insert("to_bus_id".into(), self.to_bus_id as f64);
        p
    }

    fn get_parameter(&self, name: &str) -> Option<f64> {
        match name {
            "length_km" => Some(self.length_km),
            "r_per_km" => Some(self.r_per_km),
            "x_per_km" => Some(self.x_per_km),
            "b_per_km" => Some(self.b_per_km),
            "rated_current_ka" => Some(self.rated_current_ka),
            "rated_kv" => Some(self.rated_kv),
            "from_bus_id" => Some(self.from_bus_id as f64),
            "to_bus_id" => Some(self.to_bus_id as f64),
            _ => None,
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.length_km <= 0.0 { return Err("length_km must be positive".into()); }
        Ok(())
    }

    fn rated_capacity(&self) -> Option<f64> {
        Some(self.rated_kv * self.rated_current_ka * 1.732)
    }

    fn rated_voltage(&self) -> Option<f64> { Some(self.rated_kv) }
    fn bus_ids(&self) -> Vec<ElementId> { vec![self.from_bus_id, self.to_bus_id] }

    fn to_admittance(&self, base_mva: f64, base_kv: f64) -> Option<AdmittanceContribution> {
        let _base_z = base_kv * base_kv / base_mva;
        let z_total = self.total_impedance();
        let y_series = if z_total.norm() > 1e-12 {
            num_complex::Complex::new(1.0, 0.0) / z_total
        } else {
            num_complex::Complex::new(0.0, 0.0)
        };
        let y_shunt_total = self.total_shunt_admittance();
        Some(AdmittanceContribution {
            y_series,
            y_shunt: y_shunt_total * 0.5,
            y_from_shunt: num_complex::Complex::new(0.0, 0.0),
            y_to_shunt: num_complex::Complex::new(0.0, 0.0),
        })
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
    pub bus_id: ElementId,
}

impl EquipmentModel for ConstantPowerLoad {
    fn id(&self) -> ElementId { self.id }
    fn equipment_type(&self) -> EquipmentType { EquipmentType::ConstantPowerLoad }
    fn name(&self) -> &str { &self.name }

    fn parameters(&self) -> HashMap<String, f64> {
        let mut p = HashMap::new();
        p.insert("p_mw".into(), self.p_mw);
        p.insert("q_mvar".into(), self.q_mvar);
        p.insert("rated_kv".into(), self.rated_kv);
        p.insert("bus_id".into(), self.bus_id as f64);
        p
    }

    fn get_parameter(&self, name: &str) -> Option<f64> {
        match name {
            "p_mw" => Some(self.p_mw),
            "q_mvar" => Some(self.q_mvar),
            "rated_kv" => Some(self.rated_kv),
            "bus_id" => Some(self.bus_id as f64),
            _ => None,
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.rated_kv <= 0.0 { return Err("rated_kv must be positive".into()); }
        Ok(())
    }

    fn rated_capacity(&self) -> Option<f64> {
        Some((self.p_mw.powi(2) + self.q_mvar.powi(2)).sqrt())
    }

    fn rated_voltage(&self) -> Option<f64> { Some(self.rated_kv) }
    fn bus_ids(&self) -> Vec<ElementId> { vec![self.bus_id] }

    fn to_admittance(&self, _base_mva: f64, _base_kv: f64) -> Option<AdmittanceContribution> {
        // Constant power loads do not contribute to the Y-bus matrix directly.
        // They are handled as power injections (P, Q) in the power flow mismatch equations.
        None
    }
}

/// Static (distributed) generator — PV, wind, etc. modeled as constant power injection
#[derive(Debug, Clone)]
pub struct StaticGenerator {
    pub id: ElementId,
    pub name: String,
    pub p_mw: f64,
    pub q_mvar: f64,
    pub rated_kv: f64,
    pub bus_id: ElementId,
    pub scaling: f64,
    pub controllable: bool,
}

impl EquipmentModel for StaticGenerator {
    fn id(&self) -> ElementId { self.id }
    fn equipment_type(&self) -> EquipmentType { EquipmentType::PhotovoltaicInverter }
    fn name(&self) -> &str { &self.name }

    fn parameters(&self) -> HashMap<String, f64> {
        let mut p = HashMap::new();
        p.insert("p_mw".into(), self.p_mw);
        p.insert("q_mvar".into(), self.q_mvar);
        p.insert("rated_kv".into(), self.rated_kv);
        p.insert("scaling".into(), self.scaling);
        p.insert("controllable".into(), if self.controllable { 1.0 } else { 0.0 });
        p.insert("bus_id".into(), self.bus_id as f64);
        p
    }

    fn get_parameter(&self, name: &str) -> Option<f64> {
        match name {
            "p_mw" => Some(self.p_mw),
            "q_mvar" => Some(self.q_mvar),
            "rated_kv" => Some(self.rated_kv),
            "scaling" => Some(self.scaling),
            "bus_id" => Some(self.bus_id as f64),
            _ => None,
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.rated_kv <= 0.0 { return Err("rated_kv must be positive".into()); }
        if self.scaling <= 0.0 { return Err("scaling must be positive".into()); }
        Ok(())
    }

    fn rated_capacity(&self) -> Option<f64> {
        Some((self.p_mw.powi(2) + self.q_mvar.powi(2)).sqrt())
    }

    fn rated_voltage(&self) -> Option<f64> { Some(self.rated_kv) }
    fn bus_ids(&self) -> Vec<ElementId> { vec![self.bus_id] }

    fn to_admittance(&self, _base_mva: f64, _base_kv: f64) -> Option<AdmittanceContribution> {
        // Static generators (PV, wind) are constant power injections,
        // not contributing to the Y-bus matrix directly.
        None
    }
}

/// Shunt compensator (capacitor bank or reactor)
#[derive(Debug, Clone)]
pub struct ShuntCompensator {
    pub id: ElementId,
    pub name: String,
    pub q_mvar: f64,
    pub rated_kv: f64,
    pub bus_id: ElementId,
}

impl EquipmentModel for ShuntCompensator {
    fn id(&self) -> ElementId { self.id }
    fn equipment_type(&self) -> EquipmentType { EquipmentType::CapacitorBank }
    fn name(&self) -> &str { &self.name }

    fn parameters(&self) -> HashMap<String, f64> {
        let mut p = HashMap::new();
        p.insert("q_mvar".into(), self.q_mvar);
        p.insert("rated_kv".into(), self.rated_kv);
        p.insert("bus_id".into(), self.bus_id as f64);
        p
    }

    fn get_parameter(&self, name: &str) -> Option<f64> {
        match name {
            "q_mvar" => Some(self.q_mvar),
            "rated_kv" => Some(self.rated_kv),
            "bus_id" => Some(self.bus_id as f64),
            _ => None,
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.rated_kv <= 0.0 { return Err("rated_kv must be positive".into()); }
        Ok(())
    }

    fn rated_capacity(&self) -> Option<f64> { Some(self.q_mvar.abs()) }
    fn rated_voltage(&self) -> Option<f64> { Some(self.rated_kv) }
    fn bus_ids(&self) -> Vec<ElementId> { vec![self.bus_id] }

    fn to_admittance(&self, base_mva: f64, _base_kv: f64) -> Option<AdmittanceContribution> {
        // Shunt compensator: B_pu = Q_mvar / base_mva (at V=1pu, B_pu = Q_pu)
        // Positive q_mvar = capacitor (injects reactive power, B > 0)
        // Negative q_mvar = reactor (absorbs reactive power, B < 0)
        let b_pu = self.q_mvar / base_mva;
        let y = num_complex::Complex::new(0.0, b_pu);
        Some(AdmittanceContribution {
            y_series: num_complex::Complex::new(0.0, 0.0),
            y_shunt: y,
            y_from_shunt: num_complex::Complex::new(0.0, 0.0),
            y_to_shunt: num_complex::Complex::new(0.0, 0.0),
        })
    }
}

/// ZIP composite load (constant impedance + constant current + constant power)
#[derive(Debug, Clone)]
pub struct ZipLoad {
    pub id: ElementId,
    pub name: String,
    pub p_mw: f64,
    pub q_mvar: f64,
    pub rated_kv: f64,
    pub bus_id: ElementId,
    pub const_z_p_pct: f64,
    pub const_i_p_pct: f64,
    pub const_p_p_pct: f64,
    pub const_z_q_pct: f64,
    pub const_i_q_pct: f64,
    pub const_p_q_pct: f64,
}

impl EquipmentModel for ZipLoad {
    fn id(&self) -> ElementId { self.id }
    fn equipment_type(&self) -> EquipmentType { EquipmentType::ConstantImpedanceLoad }
    fn name(&self) -> &str { &self.name }

    fn parameters(&self) -> HashMap<String, f64> {
        let mut p = HashMap::new();
        p.insert("p_mw".into(), self.p_mw);
        p.insert("q_mvar".into(), self.q_mvar);
        p.insert("rated_kv".into(), self.rated_kv);
        p.insert("const_z_p_pct".into(), self.const_z_p_pct);
        p.insert("const_i_p_pct".into(), self.const_i_p_pct);
        p.insert("const_p_p_pct".into(), self.const_p_p_pct);
        p.insert("const_z_q_pct".into(), self.const_z_q_pct);
        p.insert("const_i_q_pct".into(), self.const_i_q_pct);
        p.insert("const_p_q_pct".into(), self.const_p_q_pct);
        p.insert("bus_id".into(), self.bus_id as f64);
        p
    }

    fn get_parameter(&self, name: &str) -> Option<f64> {
        match name {
            "p_mw" => Some(self.p_mw),
            "q_mvar" => Some(self.q_mvar),
            "rated_kv" => Some(self.rated_kv),
            "bus_id" => Some(self.bus_id as f64),
            _ => None,
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.rated_kv <= 0.0 { return Err("rated_kv must be positive".into()); }
        let total_p = self.const_z_p_pct + self.const_i_p_pct + self.const_p_p_pct;
        if (total_p - 100.0).abs() > 0.1 {
            return Err(format!("P percentages must sum to 100, got {}", total_p));
        }
        Ok(())
    }

    fn rated_capacity(&self) -> Option<f64> {
        Some((self.p_mw.powi(2) + self.q_mvar.powi(2)).sqrt())
    }

    fn rated_voltage(&self) -> Option<f64> { Some(self.rated_kv) }
    fn bus_ids(&self) -> Vec<ElementId> { vec![self.bus_id] }

    fn to_admittance(&self, base_mva: f64, base_kv: f64) -> Option<AdmittanceContribution> {
        // ZIP load: only the constant-impedance portion contributes to Y-bus.
        // Load convention: load absorbs S = P + jQ, so admittance y = conj(S) / V^2
        // y_shunt = (Z_pct/100) * conj(S_pu) / V_pu^2 where S_pu = (P + jQ)/base_mva
        // For inductive loads (Q > 0), y_shunt.im < 0 (negative susceptance = inductive)
        if base_kv <= 0.0 {
            return Some(AdmittanceContribution {
                y_series: num_complex::Complex::new(0.0, 0.0),
                y_shunt: num_complex::Complex::new(0.0, 0.0),
                y_from_shunt: num_complex::Complex::new(0.0, 0.0),
                y_to_shunt: num_complex::Complex::new(0.0, 0.0),
            });
        }
        let s_pu = num_complex::Complex::new(self.p_mw / base_mva, self.q_mvar / base_mva);
        let v_pu_sq = 1.0; // nominal voltage = 1 pu
        let z_p_pct = self.const_z_p_pct / 100.0;
        let z_q_pct = self.const_z_q_pct / 100.0;
        // conj(S) = P - jQ, so y_shunt = (z_p_pct * P - j * z_q_pct * Q) / base_mva / v_pu_sq
        let y_shunt = num_complex::Complex::new(z_p_pct * s_pu.re, -z_q_pct * s_pu.im) / v_pu_sq;
        Some(AdmittanceContribution {
            y_series: num_complex::Complex::new(0.0, 0.0),
            y_shunt,
            y_from_shunt: num_complex::Complex::new(0.0, 0.0),
            y_to_shunt: num_complex::Complex::new(0.0, 0.0),
        })
    }
}

/// Circuit breaker with open/closed state
#[derive(Debug, Clone)]
pub struct CircuitBreaker {
    pub id: ElementId,
    pub name: String,
    pub from_bus_id: ElementId,
    pub to_bus_id: ElementId,
    pub closed: bool,
    pub rated_kv: f64,
    pub rated_current_ka: f64,
}

impl EquipmentModel for CircuitBreaker {
    fn id(&self) -> ElementId { self.id }
    fn equipment_type(&self) -> EquipmentType { EquipmentType::CircuitBreaker }
    fn name(&self) -> &str { &self.name }

    fn parameters(&self) -> HashMap<String, f64> {
        let mut p = HashMap::new();
        p.insert("closed".into(), if self.closed { 1.0 } else { 0.0 });
        p.insert("rated_kv".into(), self.rated_kv);
        p.insert("rated_current_ka".into(), self.rated_current_ka);
        p.insert("from_bus_id".into(), self.from_bus_id as f64);
        p.insert("to_bus_id".into(), self.to_bus_id as f64);
        p
    }

    fn get_parameter(&self, name: &str) -> Option<f64> {
        match name {
            "closed" => Some(if self.closed { 1.0 } else { 0.0 }),
            "rated_kv" => Some(self.rated_kv),
            "rated_current_ka" => Some(self.rated_current_ka),
            "from_bus_id" => Some(self.from_bus_id as f64),
            "to_bus_id" => Some(self.to_bus_id as f64),
            _ => None,
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.rated_kv <= 0.0 { return Err("rated_kv must be positive".into()); }
        Ok(())
    }

    fn rated_voltage(&self) -> Option<f64> { Some(self.rated_kv) }
    fn bus_ids(&self) -> Vec<ElementId> { vec![self.from_bus_id, self.to_bus_id] }

    fn to_admittance(&self, _base_mva: f64, _base_kv: f64) -> Option<AdmittanceContribution> {
        if self.closed {
            // Closed breaker: very low impedance (high admittance) ~ 1e12 pu
            Some(AdmittanceContribution {
                y_series: num_complex::Complex::new(1e12, 0.0),
                y_shunt: num_complex::Complex::new(0.0, 0.0),
                y_from_shunt: num_complex::Complex::new(0.0, 0.0),
                y_to_shunt: num_complex::Complex::new(0.0, 0.0),
            })
        } else {
            // Open breaker: no connection
            None
        }
    }
}

/// External grid connection (slack/PV bus representing the upstream grid)
#[derive(Debug, Clone)]
pub struct ExternalGrid {
    pub id: ElementId,
    pub name: String,
    pub bus_id: ElementId,
    pub vm_pu: f64,
    pub rated_kv: f64,
}

impl EquipmentModel for ExternalGrid {
    fn id(&self) -> ElementId { self.id }
    fn equipment_type(&self) -> EquipmentType { EquipmentType::SynchronousGenerator }
    fn name(&self) -> &str { &self.name }

    fn parameters(&self) -> HashMap<String, f64> {
        let mut p = HashMap::new();
        p.insert("vm_pu".into(), self.vm_pu);
        p.insert("rated_kv".into(), self.rated_kv);
        p.insert("bus_id".into(), self.bus_id as f64);
        p
    }

    fn get_parameter(&self, name: &str) -> Option<f64> {
        match name {
            "vm_pu" => Some(self.vm_pu),
            "rated_kv" => Some(self.rated_kv),
            "bus_id" => Some(self.bus_id as f64),
            _ => None,
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.vm_pu <= 0.0 || self.vm_pu > 2.0 {
            return Err("vm_pu must be between 0 and 2".into());
        }
        Ok(())
    }

    fn rated_voltage(&self) -> Option<f64> { Some(self.rated_kv) }
    fn bus_ids(&self) -> Vec<ElementId> { vec![self.bus_id] }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_synchronous_generator_validate() {
        let gen = SynchronousGenerator {
            id: 1, name: "G1".into(), rated_mw: 100.0, rated_mvar: 50.0,
            rated_kv: 13.8, x_d: 1.2, x_q: 0.8, x_d_trans: 0.3, bus_id: 0,
        };
        assert!(gen.validate().is_ok());
        assert_eq!(gen.rated_capacity(), Some((100.0_f64.powi(2) + 50.0_f64.powi(2)).sqrt()));
    }

    #[test]
    fn test_two_winding_transformer_admittance() {
        let trafo = TwoWindingTransformer {
            id: 1, name: "T1".into(), rated_mva: 100.0,
            rated_kv_high: 110.0, rated_kv_low: 10.0,
            impedance_percent: 10.0, resistance_percent: 1.0,
            tap_position: 0, tap_step_percent: 1.25,
            hv_bus_id: 0, lv_bus_id: 1,
        };
        let adm = trafo.to_admittance(100.0, 110.0).unwrap();
        // With tap=1.0: y_series = -y (off-diagonal), y_shunt = y (diagonal base)
        assert!(adm.y_series.norm() > 0.0);
        // y_series should be negative (off-diagonal element)
        assert!(adm.y_series.re < 0.0, "y_series.re should be negative for off-diagonal");
    }

    #[test]
    fn test_two_winding_transformer_with_tap() {
        let trafo = TwoWindingTransformer {
            id: 2, name: "T2".into(), rated_mva: 100.0,
            rated_kv_high: 110.0, rated_kv_low: 10.0,
            impedance_percent: 10.0, resistance_percent: 1.0,
            tap_position: 5, tap_step_percent: 1.25,
            hv_bus_id: 0, lv_bus_id: 1, // tap = 1.0625
        };
        let adm = trafo.to_admittance(100.0, 110.0).unwrap();
        // With tap != 1.0, y_from_shunt should be non-zero
        assert!(adm.y_from_shunt.norm() > 0.0, "y_from_shunt should be non-zero with tap != 1.0");
        // y_to_shunt should still be zero
        assert!(adm.y_to_shunt.norm() < 1e-15, "y_to_shunt should be zero");
    }

    #[test]
    fn test_transmission_line_admittance() {
        let line = TransmissionLine {
            id: 1, name: "L1".into(), length_km: 10.0,
            r_per_km: 0.1, x_per_km: 0.4, b_per_km: 0.001,
            rated_current_ka: 0.5, rated_kv: 10.0,
            from_bus_id: 0, to_bus_id: 1,
        };
        let adm = line.to_admittance(100.0, 10.0).unwrap();
        assert!(adm.y_series.norm() > 0.0);
        assert!(adm.y_shunt.im > 0.0);
    }

    #[test]
    fn test_shunt_compensator_admittance() {
        let shunt = ShuntCompensator {
            id: 1, name: "C1".into(), q_mvar: 10.0, rated_kv: 10.0, bus_id: 0,
        };
        let adm = shunt.to_admittance(100.0, 10.0).unwrap();
        // B_pu = Q_mvar / base_mva = 10.0 / 100.0 = 0.1
        assert!(adm.y_shunt.im > 0.0);
        assert!((adm.y_shunt.im - 0.1).abs() < 1e-10, "Shunt B_pu should be 0.1, got {}", adm.y_shunt.im);
    }

    #[test]
    fn test_zip_load_validate() {
        let load = ZipLoad {
            id: 1, name: "Z1".into(), p_mw: 1.0, q_mvar: 0.5,
            rated_kv: 10.0, bus_id: 0,
            const_z_p_pct: 30.0, const_i_p_pct: 30.0, const_p_p_pct: 40.0,
            const_z_q_pct: 30.0, const_i_q_pct: 30.0, const_p_q_pct: 40.0,
        };
        assert!(load.validate().is_ok());

        let bad = ZipLoad {
            const_p_p_pct: 30.0, ..load
        };
        assert!(bad.validate().is_err());
    }

    #[test]
    fn test_three_winding_transformer_validate() {
        let t3w = ThreeWindingTransformer {
            id: 1, name: "T3".into(), rated_mva: 100.0,
            rated_kv_hv: 220.0, rated_kv_mv: 110.0, rated_kv_lv: 10.0,
            vk_hv_percent: 10.0, vkr_hv_percent: 1.0,
            vk_mv_percent: 10.0, vkr_mv_percent: 1.0,
            vk_lv_percent: 10.0, vkr_lv_percent: 1.0,
            hv_bus_id: 0, mv_bus_id: 1, lv_bus_id: 2,
        };
        assert!(t3w.validate().is_ok());
        assert_eq!(t3w.bus_ids().len(), 3);
    }

    #[test]
    fn test_circuit_breaker() {
        let cb = CircuitBreaker {
            id: 1, name: "CB1".into(), from_bus_id: 0, to_bus_id: 1,
            closed: true, rated_kv: 10.0, rated_current_ka: 25.0,
        };
        assert!(cb.validate().is_ok());
        assert_eq!(cb.bus_ids(), vec![0, 1]);
    }

    #[test]
    fn test_synchronous_generator_admittance() {
        let gen = SynchronousGenerator {
            id: 1, name: "G1".into(), rated_mw: 100.0, rated_mvar: 50.0,
            rated_kv: 13.8, x_d: 1.2, x_q: 0.8, x_d_trans: 0.3, bus_id: 0,
        };
        let adm = gen.to_admittance(100.0, 13.8).unwrap();
        assert!(adm.y_shunt.im < 0.0); // 1/(j*x) = -j/x
        assert_eq!(adm.y_series.re, 0.0);
    }

    #[test]
    fn test_three_winding_transformer_admittance_multi() {
        let t3w = ThreeWindingTransformer {
            id: 1, name: "T3".into(), rated_mva: 100.0,
            rated_kv_hv: 220.0, rated_kv_mv: 110.0, rated_kv_lv: 10.0,
            vk_hv_percent: 10.0, vkr_hv_percent: 1.0,
            vk_mv_percent: 10.0, vkr_mv_percent: 1.0,
            vk_lv_percent: 10.0, vkr_lv_percent: 1.0,
            hv_bus_id: 0, mv_bus_id: 1, lv_bus_id: 2,
        };
        let multi = t3w.to_admittance_multi(100.0, 220.0).unwrap();
        assert_eq!(multi.bus_ids.len(), 3);
        assert_eq!(multi.contributions.len(), 3);
        for c in &multi.contributions {
            assert!(c.y_series.norm() > 0.0);
        }
    }

    #[test]
    fn test_zip_load_admittance() {
        let load = ZipLoad {
            id: 1, name: "Z1".into(), p_mw: 10.0, q_mvar: 5.0,
            rated_kv: 10.0, bus_id: 0,
            const_z_p_pct: 30.0, const_i_p_pct: 30.0, const_p_p_pct: 40.0,
            const_z_q_pct: 30.0, const_i_q_pct: 30.0, const_p_q_pct: 40.0,
        };
        let adm = load.to_admittance(100.0, 10.0).unwrap();
        assert!(adm.y_shunt.re > 0.0); // P portion (conductance)
        assert!(adm.y_shunt.im < 0.0); // Q portion (negative susceptance for inductive load)
    }

    #[test]
    fn test_zip_load_admittance_with_positive_base_kv() {
        // When base_kv > 0, v_pu_sq = 1.0, so admittance should be non-zero
        let load = ZipLoad {
            id: 2, name: "Z2".into(), p_mw: 10.0, q_mvar: 5.0,
            rated_kv: 10.0, bus_id: 0,
            const_z_p_pct: 50.0, const_i_p_pct: 25.0, const_p_p_pct: 25.0,
            const_z_q_pct: 50.0, const_i_q_pct: 25.0, const_p_q_pct: 25.0,
        };
        let adm = load.to_admittance(100.0, 10.0).unwrap();
        // y_shunt = (Z_pct/100) * conj(S_pu) / v_pu_sq
        // conj(S_pu) = P/base_mva - j*Q/base_mva = 0.1 - j0.05
        // With v_pu_sq = 1.0: y_shunt.re = 0.5 * 0.1 = 0.05
        //                      y_shunt.im = -0.5 * 0.05 = -0.025
        assert!(adm.y_shunt.re > 0.0, "P admittance should be positive with base_kv > 0");
        assert!(adm.y_shunt.im < 0.0, "Q admittance should be negative (inductive load convention)");
        assert!((adm.y_shunt.re - 0.05).abs() < 1e-10, "P admittance value mismatch");
        assert!((adm.y_shunt.im - (-0.025)).abs() < 1e-10, "Q admittance value mismatch");
    }

    #[test]
    fn test_zip_load_admittance_with_zero_base_kv() {
        // When base_kv == 0, there is no voltage, so admittance should be zero
        let load = ZipLoad {
            id: 3, name: "Z3".into(), p_mw: 10.0, q_mvar: 5.0,
            rated_kv: 10.0, bus_id: 0,
            const_z_p_pct: 50.0, const_i_p_pct: 25.0, const_p_p_pct: 25.0,
            const_z_q_pct: 50.0, const_i_q_pct: 25.0, const_p_q_pct: 25.0,
        };
        let adm = load.to_admittance(100.0, 0.0).unwrap();
        assert_eq!(adm.y_shunt.re, 0.0, "P admittance should be zero with base_kv == 0");
        assert_eq!(adm.y_shunt.im, 0.0, "Q admittance should be zero with base_kv == 0");
    }

    #[test]
    fn test_zip_load_ybus_matrix_affected() {
        // Verify that the Y-bus shunt admittance is correctly computed
        // for a ZIP load and would correctly affect the Y-bus diagonal
        let load = ZipLoad {
            id: 4, name: "Z4".into(), p_mw: 20.0, q_mvar: 10.0,
            rated_kv: 10.0, bus_id: 0,
            const_z_p_pct: 100.0, const_i_p_pct: 0.0, const_p_p_pct: 0.0,
            const_z_q_pct: 100.0, const_i_q_pct: 0.0, const_p_q_pct: 0.0,
        };
        let adm = load.to_admittance(100.0, 10.0).unwrap();
        // 100% constant impedance: y_shunt = conj(S_pu) / v_pu_sq
        // conj(S_pu) = P/base_mva - j*Q/base_mva = 0.2 - j0.1
        // y_shunt = 0.2 - j0.1
        assert!((adm.y_shunt.re - 0.2).abs() < 1e-10, "P shunt admittance mismatch");
        assert!((adm.y_shunt.im - (-0.1)).abs() < 1e-10, "Q shunt admittance mismatch");
        // y_series should always be zero for a shunt load
        assert_eq!(adm.y_series.re, 0.0);
        assert_eq!(adm.y_series.im, 0.0);
    }

    #[test]
    fn test_circuit_breaker_admittance_closed() {
        let cb = CircuitBreaker {
            id: 1, name: "CB1".into(), from_bus_id: 0, to_bus_id: 1,
            closed: true, rated_kv: 10.0, rated_current_ka: 25.0,
        };
        let adm = cb.to_admittance(100.0, 10.0).unwrap();
        assert!(adm.y_series.re > 1e10); // Very high admittance when closed
    }

    #[test]
    fn test_circuit_breaker_admittance_open() {
        let cb = CircuitBreaker {
            id: 1, name: "CB1".into(), from_bus_id: 0, to_bus_id: 1,
            closed: false, rated_kv: 10.0, rated_current_ka: 25.0,
        };
        assert!(cb.to_admittance(100.0, 10.0).is_none());
    }

    #[test]
    fn test_constant_power_load_no_admittance() {
        let load = ConstantPowerLoad {
            id: 1, name: "L1".into(), p_mw: 10.0, q_mvar: 5.0,
            rated_kv: 10.0, bus_id: 0,
        };
        assert!(load.to_admittance(100.0, 10.0).is_none());
    }

    #[test]
    fn test_static_generator_no_admittance() {
        let gen = StaticGenerator {
            id: 1, name: "PV1".into(), p_mw: 5.0, q_mvar: 0.0,
            rated_kv: 0.4, bus_id: 0, scaling: 1.0, controllable: false,
        };
        assert!(gen.to_admittance(100.0, 0.4).is_none());
    }
}

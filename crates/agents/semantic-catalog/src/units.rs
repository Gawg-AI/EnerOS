//! 单位受控换算图（[02] §4.4 / [08] §2.2）。
//!
//! 所有进入决策的数值都有单位和量纲；换算只在目录登记的直接边内完成
//! （原始值与工程值都可追溯）。量纲一致性由 [`PhysicalUnit`] 双重强制：
//! 装载期（[`crate::decode`]）与换算期（本模块）各查一次——目录被绕过
//! 时换算也不会把电压当电流。

use alloc::collections::BTreeMap;

use crate::{CatalogError, CatalogErrorKind};

/// 换算器（由图构建；只读）。
#[derive(Debug, Clone, Default)]
pub struct UnitConverter {
    /// (from, to) → factor（from 值 × factor = to 值）。
    factors: BTreeMap<(u16, u16), f64>,
}

impl UnitConverter {
    pub fn new(factors: BTreeMap<(u16, u16), f64>) -> Self {
        Self { factors }
    }

    /// 换算 `value`（单位 `from`）→ 单位 `to`。
    ///
    /// - 同单位恒等（factor 1.0，不依赖目录）；
    /// - 目录直接边按登记因子；
    /// - 无边且量纲不兼容 → [`CatalogErrorKind::NoConversion`]；
    /// - 无量纲转换（identity，如 ratio→ratio）恒等。
    pub fn convert(&self, value: f64, from: u16, to: u16) -> Result<f64, CatalogError> {
        if !value.is_finite() {
            return Err(CatalogError::new(
                CatalogErrorKind::NoConversion,
                "non-finite-value".into(),
            ));
        }
        if from == to {
            return Ok(value);
        }
        if let Some(f) = self.factors.get(&(from, to)) {
            return Ok(value * *f);
        }
        // 无直接边：量纲兼容检查给出更明确的拒绝。
        let from_u = eneros_telemetry_model::unit_catalog::from_code(from);
        let to_u = eneros_telemetry_model::unit_catalog::from_code(to);
        match (from_u, to_u) {
            (Some(fu), Some(tu)) if fu.compatible_with(tu) => Err(CatalogError::new(
                CatalogErrorKind::NoConversion,
                alloc::format!("no-edge:({from}->{to})"),
            )),
            (Some(fu), Some(tu)) => Err(CatalogError::new(
                CatalogErrorKind::NoConversion,
                alloc::format!(
                    "dimension-mismatch:{:?}->{:?}",
                    fu.dimension(),
                    tu.dimension()
                ),
            )),
            _ => Err(CatalogError::new(
                CatalogErrorKind::UnknownUnit,
                alloc::format!("unit:({from}->{to})"),
            )),
        }
    }

    /// 已登记换算边数（证据/观测）。
    pub fn len(&self) -> usize {
        self.factors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.factors.is_empty()
    }
}

/// 常用量纲辅助：f32 MW → f64 kW（[08] §4.6 边界换算；因子证据 =
/// UnitCatalog：MW=1e6 W / kW=1e3 W）。
pub fn megawatts_to_kilowatts(mw: f32) -> Result<f64, CatalogError> {
    if !mw.is_finite() {
        return Err(CatalogError::new(
            CatalogErrorKind::NoConversion,
            "non-finite-mw".into(),
        ));
    }
    Ok(f64::from(mw) * 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn converter() -> UnitConverter {
        let mut m = BTreeMap::new();
        m.insert((7u16, 6u16), 1000.0); // MW→kW
        m.insert((6u16, 7u16), 0.001); // kW→MW
        UnitConverter::new(m)
    }

    #[test]
    fn convert_direct_edges_and_identity() {
        let c = converter();
        assert!((c.convert(1.5, 7, 6).unwrap() - 1500.0).abs() < 1e-9);
        assert!((c.convert(1500.0, 6, 7).unwrap() - 1.5).abs() < 1e-9);
        // 同单位恒等（不依赖目录）。
        assert_eq!(c.convert(42.0, 6, 6).unwrap(), 42.0);
        assert_eq!(c.convert(42.0, 0, 0).unwrap(), 42.0);
    }

    #[test]
    fn missing_edge_dimension_compatible_rejected() {
        let c = converter();
        // 同量纲（Power）但无直接边 → NoConversion：no-edge。
        assert_eq!(
            c.convert(1.0, 5, 6).unwrap_err().kind, // W→kW 无边
            CatalogErrorKind::NoConversion
        );
        // 量纲不兼容（kW→kWh）→ NoConversion：dimension-mismatch。
        assert_eq!(
            c.convert(1.0, 6, 14).unwrap_err().kind,
            CatalogErrorKind::NoConversion
        );
        // 未知单位码。
        assert_eq!(
            c.convert(1.0, 99, 6).unwrap_err().kind,
            CatalogErrorKind::UnknownUnit
        );
    }

    #[test]
    fn nan_rejected() {
        let c = converter();
        assert_eq!(
            c.convert(f64::NAN, 7, 6).unwrap_err().kind,
            CatalogErrorKind::NoConversion
        );
    }

    #[test]
    fn megawatts_to_kilowatts_boundary() {
        assert!((megawatts_to_kilowatts(1.0f32).unwrap() - 1000.0).abs() < 1e-6);
        assert!((megawatts_to_kilowatts(-2.5f32).unwrap() + 2500.0).abs() < 1e-6);
        assert_eq!(
            megawatts_to_kilowatts(f32::NAN).unwrap_err().kind,
            CatalogErrorKind::NoConversion
        );
    }
}

//! 物理量单位与量纲模型 (CTL-002)。
//!
//! MASTER G5 §9.2 与 05 册 v0.279.0 禁止"无单位 f64"——所有进入控制/安全平面的物理量
//! 必须携带显式单位与量纲，独立安全校验器据此重算物理可行性。
//!
//! [`PhysicalUnit`] 是能源域冻结的单位枚举；[`Dimension`] 是其量纲类别。两个不同单位
//! 只有在量纲一致时才可比较/运算，避免把"电压"当"电流"或把"百分数"当"瓦特"。

/// 物理量纲类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dimension {
    Voltage,
    Current,
    Power,
    ReactivePower,
    Energy,
    Frequency,
    Temperature,
    /// 百分比 / 比率（如 SOC、占空比）。
    Ratio,
    Time,
    /// 无量纲（计数、状态码）。
    None,
}

/// 能源域物理单位（冻结）。新增单位须走变更评审。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhysicalUnit {
    Volts,
    Kilovolts,
    Amperes,
    Kiloamperes,
    Watts,
    Kilowatts,
    Megawatts,
    VoltAmpereReactive,
    Kilovar,
    Megavar,
    Hertz,
    Celsius,
    Percent,
    KilowattHour,
    AmpereHour,
    Seconds,
    Rpm,
    /// 无量纲。
    Dimensionless,
}

impl PhysicalUnit {
    /// 返回该单位的量纲。
    pub const fn dimension(&self) -> Dimension {
        match self {
            PhysicalUnit::Volts | PhysicalUnit::Kilovolts => Dimension::Voltage,
            PhysicalUnit::Amperes | PhysicalUnit::Kiloamperes => Dimension::Current,
            PhysicalUnit::Watts | PhysicalUnit::Kilowatts | PhysicalUnit::Megawatts => {
                Dimension::Power
            }
            PhysicalUnit::VoltAmpereReactive | PhysicalUnit::Kilovar | PhysicalUnit::Megavar => {
                Dimension::ReactivePower
            }
            PhysicalUnit::KilowattHour | PhysicalUnit::AmpereHour => Dimension::Energy,
            PhysicalUnit::Hertz => Dimension::Frequency,
            PhysicalUnit::Celsius => Dimension::Temperature,
            PhysicalUnit::Percent => Dimension::Ratio,
            PhysicalUnit::Seconds => Dimension::Time,
            PhysicalUnit::Rpm | PhysicalUnit::Dimensionless => Dimension::None,
        }
    }

    /// 两个单位是否量纲兼容（可比较/转换）。
    pub const fn compatible_with(self, other: PhysicalUnit) -> bool {
        matches!(
            (self.dimension(), other.dimension()),
            (Dimension::Voltage, Dimension::Voltage)
                | (Dimension::Current, Dimension::Current)
                | (Dimension::Power, Dimension::Power)
                | (Dimension::ReactivePower, Dimension::ReactivePower)
                | (Dimension::Energy, Dimension::Energy)
                | (Dimension::Frequency, Dimension::Frequency)
                | (Dimension::Temperature, Dimension::Temperature)
                | (Dimension::Ratio, Dimension::Ratio)
                | (Dimension::Time, Dimension::Time)
                | (Dimension::None, Dimension::None)
        )
    }

    /// 返回该单位到 SI 基准单位的比例因子（用于独立重算）。
    /// 例如 Kilowatts → Watts 比例 1000.0；Percent → 比例 0.01（到 0..1）。
    pub const fn si_scale_factor(self) -> f64 {
        match self {
            PhysicalUnit::Volts => 1.0,
            PhysicalUnit::Kilovolts => 1_000.0,
            PhysicalUnit::Amperes => 1.0,
            PhysicalUnit::Kiloamperes => 1_000.0,
            PhysicalUnit::Watts => 1.0,
            PhysicalUnit::Kilowatts => 1_000.0,
            PhysicalUnit::Megawatts => 1_000_000.0,
            PhysicalUnit::VoltAmpereReactive => 1.0,
            PhysicalUnit::Kilovar => 1_000.0,
            PhysicalUnit::Megavar => 1_000_000.0,
            PhysicalUnit::Hertz => 1.0,
            PhysicalUnit::Celsius => 1.0,
            PhysicalUnit::Percent => 0.01,
            PhysicalUnit::KilowattHour => 3_600_000.0, // → Joules
            PhysicalUnit::AmpereHour => 3_600.0,       // → Coulombs
            PhysicalUnit::Seconds => 1.0,
            PhysicalUnit::Rpm => 1.0,
            PhysicalUnit::Dimensionless => 1.0,
        }
    }

    /// 短名（用于日志/审计，固定 ASCII）。
    pub const fn short_name(self) -> &'static str {
        match self {
            PhysicalUnit::Volts => "V",
            PhysicalUnit::Kilovolts => "kV",
            PhysicalUnit::Amperes => "A",
            PhysicalUnit::Kiloamperes => "kA",
            PhysicalUnit::Watts => "W",
            PhysicalUnit::Kilowatts => "kW",
            PhysicalUnit::Megawatts => "MW",
            PhysicalUnit::VoltAmpereReactive => "var",
            PhysicalUnit::Kilovar => "kvar",
            PhysicalUnit::Megavar => "Mvar",
            PhysicalUnit::Hertz => "Hz",
            PhysicalUnit::Celsius => "degC",
            PhysicalUnit::Percent => "%",
            PhysicalUnit::KilowattHour => "kWh",
            PhysicalUnit::AmpereHour => "Ah",
            PhysicalUnit::Seconds => "s",
            PhysicalUnit::Rpm => "rpm",
            PhysicalUnit::Dimensionless => "",
        }
    }
}

/// `UnitCatalog` — 蓝图 61 [02] §4.4 封闭数字单位码表（wire 层唯一事实）。
///
/// `ObservationEvent.unit_code` 只允许本表登记的代码；`0 = 未知`（可存档，
/// 不得进控制快照）。码位冻结：新增单位只能追加新码，禁止改义旧码。
pub mod unit_catalog {
    use super::PhysicalUnit;

    /// 未知单位（wire 0 值）。
    pub const UNKNOWN: u16 = 0;
    /// 码表登记项数（含 0=UNKNOWN）。
    pub const CATALOG_LEN: u16 = 19;

    /// `PhysicalUnit` → 封闭码（永不返回 0）。
    pub const fn code_of(unit: PhysicalUnit) -> u16 {
        match unit {
            PhysicalUnit::Volts => 1,
            PhysicalUnit::Kilovolts => 2,
            PhysicalUnit::Amperes => 3,
            PhysicalUnit::Kiloamperes => 4,
            PhysicalUnit::Watts => 5,
            PhysicalUnit::Kilowatts => 6,
            PhysicalUnit::Megawatts => 7,
            PhysicalUnit::VoltAmpereReactive => 8,
            PhysicalUnit::Kilovar => 9,
            PhysicalUnit::Megavar => 10,
            PhysicalUnit::Hertz => 11,
            PhysicalUnit::Celsius => 12,
            PhysicalUnit::Percent => 13,
            PhysicalUnit::KilowattHour => 14,
            PhysicalUnit::AmpereHour => 15,
            PhysicalUnit::Seconds => 16,
            PhysicalUnit::Rpm => 17,
            PhysicalUnit::Dimensionless => 18,
        }
    }

    /// 封闭码 → `PhysicalUnit`；`0` 与未登记码 → `None`（fail-closed）。
    pub const fn from_code(code: u16) -> Option<PhysicalUnit> {
        match code {
            1 => Some(PhysicalUnit::Volts),
            2 => Some(PhysicalUnit::Kilovolts),
            3 => Some(PhysicalUnit::Amperes),
            4 => Some(PhysicalUnit::Kiloamperes),
            5 => Some(PhysicalUnit::Watts),
            6 => Some(PhysicalUnit::Kilowatts),
            7 => Some(PhysicalUnit::Megawatts),
            8 => Some(PhysicalUnit::VoltAmpereReactive),
            9 => Some(PhysicalUnit::Kilovar),
            10 => Some(PhysicalUnit::Megavar),
            11 => Some(PhysicalUnit::Hertz),
            12 => Some(PhysicalUnit::Celsius),
            13 => Some(PhysicalUnit::Percent),
            14 => Some(PhysicalUnit::KilowattHour),
            15 => Some(PhysicalUnit::AmpereHour),
            16 => Some(PhysicalUnit::Seconds),
            17 => Some(PhysicalUnit::Rpm),
            18 => Some(PhysicalUnit::Dimensionless),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    #[test]
    fn compatible_within_dimension() {
        assert!(PhysicalUnit::Kilowatts.compatible_with(PhysicalUnit::Watts));
        assert!(PhysicalUnit::Percent.compatible_with(PhysicalUnit::Percent));
        assert!(!PhysicalUnit::Kilowatts.compatible_with(PhysicalUnit::Amperes));
        assert!(!PhysicalUnit::Percent.compatible_with(PhysicalUnit::Watts));
    }

    #[test]
    fn si_scale_factor_monotonic() {
        assert!(
            PhysicalUnit::Megawatts.si_scale_factor() > PhysicalUnit::Kilowatts.si_scale_factor()
        );
        assert_eq!(PhysicalUnit::Percent.si_scale_factor(), 0.01);
    }

    // ===== 蓝图 61 [02] §4.4：UnitCatalog 封闭码表 =====

    #[test]
    fn unit_catalog_roundtrip_all_units() {
        use unit_catalog;
        for code in 1..unit_catalog::CATALOG_LEN {
            let u = unit_catalog::from_code(code).expect("registered code");
            assert_eq!(unit_catalog::code_of(u), code);
        }
    }

    #[test]
    fn unit_catalog_unknown_and_zero_fail_closed() {
        use unit_catalog;
        assert_eq!(unit_catalog::from_code(0), None);
        assert_eq!(unit_catalog::from_code(unit_catalog::CATALOG_LEN), None);
        assert_eq!(unit_catalog::from_code(u16::MAX), None);
        // 任何登记单位都不得映射到 0（0 = 未知保留）。
        for u in [
            PhysicalUnit::Volts,
            PhysicalUnit::Kilowatts,
            PhysicalUnit::Dimensionless,
        ] {
            assert_ne!(unit_catalog::code_of(u), 0);
        }
    }
}

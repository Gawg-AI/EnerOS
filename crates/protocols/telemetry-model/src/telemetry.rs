//! 遥测（Telemetry）.
//!
//! [`Telemetry`] 表示模拟量遥测数据点，提供工程量值、单位、品质、限值、
//! 死区过滤与变化上报语义。对应电力四遥中的"遥测"（Telemetry）。

use alloc::string::String;

use eneros_upa_model::{DeviceId, PointId};

use crate::quality::QualityFlag;

/// 遥测数据点（模拟量）。
///
/// 死区过滤逻辑：仅当 `|value - last_reported| > deadband` 时触发上报，
/// 避免微小波动导致频繁上报。
#[derive(Debug, Clone)]
pub struct Telemetry {
    /// 点唯一标识。
    pub point_id: PointId,
    /// 所属设备 ID。
    pub device_id: DeviceId,
    /// 点名称（人类可读）。
    pub name: String,
    /// 工程量值。
    pub value: f64,
    /// 单位（V/A/kW/℃/Hz）。
    pub unit: String,
    /// 数据品质。
    pub quality: QualityFlag,
    /// 时间戳（u64 毫秒，D1）。
    pub timestamp_ms: u64,
    /// 死区值。
    pub deadband: f64,
    /// 高限值。
    pub high_limit: Option<f64>,
    /// 低限值。
    pub low_limit: Option<f64>,
    /// 上次上报值。
    pub last_reported: Option<f64>,
}

impl Telemetry {
    /// 创建遥测点。
    ///
    /// 默认品质 `Good`，死区 `0.0`，限值 `None`，`last_reported` 为 `None`。
    pub fn new(
        point_id: PointId,
        device_id: DeviceId,
        name: &str,
        value: f64,
        unit: &str,
        now_ms: u64,
    ) -> Self {
        Self {
            point_id,
            device_id,
            name: String::from(name),
            value,
            unit: String::from(unit),
            quality: QualityFlag::Good,
            timestamp_ms: now_ms,
            deadband: 0.0,
            high_limit: None,
            low_limit: None,
            last_reported: None,
        }
    }

    /// 死区过滤：判断是否应上报当前值。
    ///
    /// - 首次上报（`last_reported` 为 `None`）：记录并返回 `true`。
    /// - 变化超过死区：记录并返回 `true`。
    /// - 否则：返回 `false`。
    pub fn should_report(&mut self) -> bool {
        match self.last_reported {
            None => {
                self.last_reported = Some(self.value);
                true
            }
            Some(last) => {
                if (self.value - last).abs() > self.deadband {
                    self.last_reported = Some(self.value);
                    true
                } else {
                    false
                }
            }
        }
    }

    /// 限值检查：超出限值时将品质置为 `Questionable`。
    ///
    /// - 高/低限均存在：`value > high_limit` 或 `value < low_limit` → Questionable。
    /// - 仅高限：`value > high_limit` → Questionable。
    /// - 仅低限：`value < low_limit` → Questionable。
    pub fn check_quality(&mut self) {
        match (self.high_limit, self.low_limit) {
            (Some(hi), Some(lo)) => {
                if self.value > hi || self.value < lo {
                    self.quality = QualityFlag::Questionable;
                }
            }
            (Some(hi), None) => {
                if self.value > hi {
                    self.quality = QualityFlag::Questionable;
                }
            }
            (None, Some(lo)) => {
                if self.value < lo {
                    self.quality = QualityFlag::Questionable;
                }
            }
            (None, None) => {}
        }
    }

    /// 更新值与时间戳。
    pub fn update(&mut self, value: f64, now_ms: u64) {
        self.value = value;
        self.timestamp_ms = now_ms;
    }

    /// 强制上报：将 `last_reported` 设为当前值（用于品质变化等强制上报场景）。
    pub fn force_report(&mut self) {
        self.last_reported = Some(self.value);
    }
}

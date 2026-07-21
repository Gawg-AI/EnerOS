//! 遥调（Teleadjust）.
//!
//! [`Teleadjust`] 表示设定值遥调数据点，提供设定值范围校验、当前值跟踪与偏差计算。
//! 对应电力四遥中的"遥调"（Teleadjust）。

use alloc::string::String;

use eneros_upa_model::{DeviceId, PointId};

use crate::quality::QualityFlag;

/// 遥调数据点（设定值）。
#[derive(Debug, Clone)]
pub struct Teleadjust {
    /// 点唯一标识。
    pub point_id: PointId,
    /// 所属设备 ID。
    pub device_id: DeviceId,
    /// 点名称（人类可读）。
    pub name: String,
    /// 设定值。
    pub setpoint: f64,
    /// 当前实际值。
    pub current_value: f64,
    /// 数据品质。
    pub quality: QualityFlag,
    /// 时间戳（u64 毫秒，D1）。
    pub timestamp_ms: u64,
    /// 最小值。
    pub min_value: f64,
    /// 最大值。
    pub max_value: f64,
    /// 变化率限制（单位/秒）。
    pub ramp_rate: Option<f64>,
}

impl Teleadjust {
    /// 创建遥调点。
    ///
    /// 默认品质 `Good`，`current_value` 为 `0.0`，`ramp_rate` 为 `None`。
    pub fn new(
        point_id: PointId,
        device_id: DeviceId,
        name: &str,
        setpoint: f64,
        min: f64,
        max: f64,
        now_ms: u64,
    ) -> Self {
        Self {
            point_id,
            device_id,
            name: String::from(name),
            setpoint,
            current_value: 0.0,
            quality: QualityFlag::Good,
            timestamp_ms: now_ms,
            min_value: min,
            max_value: max,
            ramp_rate: None,
        }
    }

    /// 校验值是否在 `[min_value, max_value]` 范围内。
    pub fn validate(&self, value: f64) -> bool {
        value >= self.min_value && value <= self.max_value
    }

    /// 设置设定值（在范围内则设置，否则返回 `Err("out of range")`）。
    pub fn set(&mut self, value: f64, now_ms: u64) -> Result<(), &'static str> {
        if !self.validate(value) {
            return Err("out of range");
        }
        self.setpoint = value;
        self.timestamp_ms = now_ms;
        Ok(())
    }

    /// 更新当前实际值与时间戳。
    pub fn update_current(&mut self, value: f64, now_ms: u64) {
        self.current_value = value;
        self.timestamp_ms = now_ms;
    }

    /// 返回当前实际值是否在 `[min_value, max_value]` 范围内。
    pub fn is_in_range(&self) -> bool {
        self.validate(self.current_value)
    }

    /// 返回偏差（`current_value - setpoint`）。
    pub fn deviation(&self) -> f64 {
        self.current_value - self.setpoint
    }
}

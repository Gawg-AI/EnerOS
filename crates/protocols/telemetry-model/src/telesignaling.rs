//! 遥信（Telesignaling）.
//!
//! [`Telesignaling`] 表示数字量遥信数据点，提供状态变化检测与上报语义。
//! 对应电力四遥中的"遥信"（Telesignaling），采用状态变化即上报（无死区）策略。

use alloc::string::String;

use eneros_upa_model::{DeviceId, PointId};

use crate::digital::DigitalState;
use crate::quality::QualityFlag;

/// 遥信数据点（数字量）。
///
/// 变化检测：无死区，仅当状态变化时触发上报。
#[derive(Debug, Clone)]
pub struct Telesignaling {
    /// 点唯一标识。
    pub point_id: PointId,
    /// 所属设备 ID。
    pub device_id: DeviceId,
    /// 点名称（人类可读）。
    pub name: String,
    /// 当前状态。
    pub value: DigitalState,
    /// 数据品质。
    pub quality: QualityFlag,
    /// 时间戳（u64 毫秒，D1）。
    pub timestamp_ms: u64,
    /// 是否双位置遥信。
    pub double_point: bool,
    /// 上次上报状态。
    pub last_reported: Option<DigitalState>,
}

impl Telesignaling {
    /// 创建遥信点。
    ///
    /// 默认品质 `Good`，`last_reported` 为 `None`。
    pub fn new(
        point_id: PointId,
        device_id: DeviceId,
        name: &str,
        value: DigitalState,
        double_point: bool,
        now_ms: u64,
    ) -> Self {
        Self {
            point_id,
            device_id,
            name: String::from(name),
            value,
            quality: QualityFlag::Good,
            timestamp_ms: now_ms,
            double_point,
            last_reported: None,
        }
    }

    /// 变化检测：判断是否应上报当前状态（无死区）。
    ///
    /// - 首次上报（`last_reported` 为 `None`）：记录并返回 `true`。
    /// - 状态变化：记录并返回 `true`。
    /// - 状态未变：返回 `false`。
    pub fn should_report(&mut self) -> bool {
        match self.last_reported {
            None => {
                self.last_reported = Some(self.value);
                true
            }
            Some(last) => {
                if last != self.value {
                    self.last_reported = Some(self.value);
                    true
                } else {
                    false
                }
            }
        }
    }

    /// 更新状态与时间戳。
    pub fn update(&mut self, value: DigitalState, now_ms: u64) {
        self.value = value;
        self.timestamp_ms = now_ms;
    }

    /// 强制上报：将 `last_reported` 设为当前状态。
    pub fn force_report(&mut self) {
        self.last_reported = Some(self.value);
    }
}

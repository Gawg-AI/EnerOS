//! 数字量状态（DigitalState）.
//!
//! [`DigitalState`] 表示遥信/双位置遥信的离散状态，对应 IEC 104 SinglePoint/DoublePoint
//! 信息体语义的归一化表示。

/// 数字量状态（4 状态）。
///
/// - `Off`/`On`：确定态，对应单/双位置遥信的正常状态。
/// - `Intermediate`：中间态（过渡），对应双位置遥信的 OFF→ON 过渡态。
/// - `Bad`：错误态，对应采集异常或无效状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DigitalState {
    /// 分（OFF）。
    Off,
    /// 合（ON）。
    On,
    /// 中间态（过渡）。
    Intermediate,
    /// 错误。
    Bad,
}

impl DigitalState {
    /// 返回是否为 `On`（合）。
    pub fn is_on(&self) -> bool {
        matches!(self, DigitalState::On)
    }

    /// 返回是否为 `Off`（分）。
    pub fn is_off(&self) -> bool {
        matches!(self, DigitalState::Off)
    }

    /// 返回是否为有效态（`Off`/`On` 为 true，`Intermediate`/`Bad` 为 false）。
    pub fn is_valid(&self) -> bool {
        matches!(self, DigitalState::Off | DigitalState::On)
    }
}

//! FMEA / FMECA 失效模式与影响分析 (CTL-001)。
//!
//! 对每个设备类别枚举失效模式、失效影响、严重度 / 发生频度 / 探测度，并计算 RPN
//! (Risk Priority Number = S × O × D)。RPN 越高越优先缓解。

pub use crate::hazard::Severity;

/// 发生频度（1=极低 … 5=极高）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Occurrence {
    Remote = 1,
    Low = 2,
    Moderate = 3,
    High = 4,
    VeryHigh = 5,
}

/// 探测度（1=几乎肯定能探测 … 5=几乎无法探测）。
/// 注意：探测度越高（越难探测），RPN 越高。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Detection {
    AlmostCertain = 1,
    High = 2,
    Moderate = 3,
    Low = 4,
    Impossible = 5,
}

/// FMEA 条目。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FmeaEntry {
    pub id: &'static str,
    /// 关联的设备类别（与 [`crate::safe_state::SafeState`] 的 `device_class` 一致）。
    pub device_class: &'static str,
    /// 失效模式（如 `"pcs_igbt_short"`、`"bms_soc_estimator_drift"`）。
    pub failure_mode: &'static str,
    /// 失效原因。
    pub cause: &'static str,
    /// 失效影响（对系统/人员）。
    pub effect: &'static str,
    pub severity: Severity,
    pub occurrence: Occurrence,
    pub detection: Detection,
    /// 现有缓解控制（设计或运维）。
    pub mitigation: &'static str,
}

impl FmeaEntry {
    /// 风险优先数 RPN = S × O × D，范围 [1, 100]。
    pub const fn rpn(&self) -> u32 {
        (self.severity as u32) * (self.occurrence as u32) * (self.detection as u32)
    }

    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        id: &'static str,
        device_class: &'static str,
        failure_mode: &'static str,
        cause: &'static str,
        effect: &'static str,
        severity: Severity,
        occurrence: Occurrence,
        detection: Detection,
        mitigation: &'static str,
    ) -> Self {
        Self {
            id,
            device_class,
            failure_mode,
            cause,
            effect,
            severity,
            occurrence,
            detection,
            mitigation,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpn_in_expected_range() {
        let e = FmeaEntry::new(
            "F-1",
            "pcs",
            "x",
            "x",
            "x",
            Severity::Critical,
            Occurrence::Moderate,
            Detection::Moderate,
            "x",
        );
        // 3 * 3 * 3 = 27
        assert_eq!(e.rpn(), 27);
    }

    #[test]
    fn rpn_bounds() {
        let min = FmeaEntry::new(
            "F-min",
            "x",
            "x",
            "x",
            "x",
            Severity::Negligible,
            Occurrence::Remote,
            Detection::AlmostCertain,
            "x",
        );
        assert_eq!(min.rpn(), 1);
        let max = FmeaEntry::new(
            "F-max",
            "x",
            "x",
            "x",
            "x",
            Severity::Catastrophic,
            Occurrence::VeryHigh,
            Detection::Impossible,
            "x",
        );
        assert_eq!(max.rpn(), 100);
    }
}

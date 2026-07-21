//! 降级模式层级.
//!
//! 定义 [`DegradeMode`] 五级模式枚举，派生 `Ord` 支持严重程度比较（D4）。
//! 层级关系：Normal < HoldOutput < StopCharge < SafeDefault < EmergencyStop。

/// 降级模式（五级，严重程度递增）.
///
/// 派生 `Ord` 以支持严重程度比较（D4）；派生 `Default` 返回 `Normal`。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum DegradeMode {
    /// 正常模式（Agent 接管，无降级）。
    #[default]
    Normal = 0,
    /// 保持输出（冻结当前设定值，不更新）。
    HoldOutput = 1,
    /// 停止充电（下发 0.0 功率）。
    StopCharge = 2,
    /// 安全默认值（下发预配置的安全值）。
    SafeDefault = 3,
    /// 紧急停机（下发 Bool(true) 到所有控制点，不可自动恢复 — D11）。
    EmergencyStop = 4,
}

impl DegradeMode {
    /// 是否处于降级状态（非 Normal 即降级）。
    pub fn is_degraded(&self) -> bool {
        !matches!(self, DegradeMode::Normal)
    }
}

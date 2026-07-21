//! DegradeState — 端到端降级 5 态状态机.
//!
//! [`DegradeState`] 描述端到端降级流程的状态：
//! Normal → Degrading → Degraded → Recovering → Normal（或任意 → Emergency）。

/// 端到端降级状态（5 态）.
///
/// 派生 `Debug / Clone / Copy / PartialEq / Eq / Default`，可作状态机枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DegradeState {
    /// 正常运行（Agent 接管，RTOS 仅转发命令）。
    #[default]
    Normal,
    /// 降级中（瞬时态：保存设定值后立即转 Degraded）。
    Degrading,
    /// 已降级（RTOS 降级引擎接管）。
    Degraded,
    /// 恢复中（线性插值过渡回 Agent 设定值）。
    Recovering,
    /// 紧急停机（看门狗硬复位，不自动恢复 — D12）。
    Emergency,
}

impl DegradeState {
    /// 是否处于降级状态（Normal 返回 false，其余返回 true）。
    pub fn is_degraded(&self) -> bool {
        !matches!(self, DegradeState::Normal)
    }
}

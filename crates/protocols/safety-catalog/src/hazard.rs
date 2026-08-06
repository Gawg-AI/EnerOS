//! 危险源模型 (CTL-001)。
//!
//! 一个 [`Hazard`] 描述：某资产在某种能量流路径下，因某危险源可能导致某事故。
//! 严重度分级采用能源行业惯用的 4 级（Catastrophic/Critical/Marginal/Negligible）。

/// 危险类别。用于聚合统计与策略路由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HazardCategory {
    /// 电气（过流/过压/短路/孤岛）。
    Electrical,
    /// 电化学（电池过充/过放/热失控）。
    Electrochemical,
    /// 机械（执行机构卡死/飞车）。
    Mechanical,
    /// 热力（过温/火灾）。
    Thermal,
    /// 网络与控制（拒服务/越权写/注入）。
    CyberControl,
}

/// 严重度等级（按 MIL-STD-882 / IEC 61508 风险矩阵简化）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// 可忽略（Negligible）—— 不会造成人员伤害或超过最低可接受损失。
    Negligible = 1,
    /// 边缘（Marginal）—— 可控，可能造成轻微伤害或财产损失。
    Marginal = 2,
    /// 严重（Critical）—— 可能造成严重伤害或重大损失。
    Critical = 3,
    /// 灾难性（Catastrophic）—— 可能造成死亡或系统全损。
    Catastrophic = 4,
}

/// 能量流路径。描述能量从源头到负载/电网的方向，用于判断失控后果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnergyFlow {
    /// 电池 → 直流母线（放电）。
    BatteryToDcBus,
    /// 直流母线 → 电池（充电）。
    DcBusToBattery,
    /// 直流母线 → 变流器 → 交流电网（并网放电）。
    DcBusToGrid,
    /// 交流电网 → 变流器 → 直流母线（并网充电）。
    GridToDcBus,
    /// 变流器 → 本地负载（离网供电）。
    InverterToLoad,
    /// 无能量流（待机/隔离）。
    None,
}

/// 危险源记录。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hazard {
    /// 稳定 ID（如 `"HZ-ELEC-001"`）。
    pub id: &'static str,
    pub category: HazardCategory,
    pub severity: Severity,
    /// 受威胁资产（如 `"battery_pack"`、`"pcc_breaker"`）。
    pub asset: &'static str,
    pub energy_flow: EnergyFlow,
    /// 危险源（如 `"overcurrent"`、`"islanding"`）。
    pub source: &'static str,
    /// 可能导致的事故（如 `"thermal_runaway"`、`"islanding_energization"`）。
    pub accident: &'static str,
}

impl Hazard {
    pub const fn new(
        id: &'static str,
        category: HazardCategory,
        severity: Severity,
        asset: &'static str,
        energy_flow: EnergyFlow,
        source: &'static str,
        accident: &'static str,
    ) -> Self {
        Self {
            id,
            category,
            severity,
            asset,
            energy_flow,
            source,
            accident,
        }
    }
}

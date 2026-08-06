//! STPA 系统理论事故模型 (CTL-001)。
//!
//! STPA 关注**控制结构**：控制器发出控制动作，某些控制动作在特定上下文中是不安全的
//! (UCA)，从而可能导致危险。每个 UCA 对应一条安全约束，约束再追溯到代码与测试。

/// 控制动作的方向性分类。STPA 的 UCA 四类：
/// - 提供：控制器提供了不该提供的控制动作。
/// - 未提供：控制器未提供应该提供的控制动作。
/// - 提供错误：提供了错误时间/顺序/值的控制动作。
/// - 持续：控制动作持续过长/过短。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UcaDirection {
    ProvidedUnsafe,
    NotProvidedUnsafe,
    WrongTiming,
    StoppedTooSoon,
}

/// 控制动作（如 `"close_pcc_breaker"`、`"charge_battery"`、`"curtail_pv"`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlAction {
    pub id: &'static str,
    pub name: &'static str,
}

/// 不安全控制动作 (UCA)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnsafeControlAction {
    pub id: &'static str,
    pub control_action: &'static str,
    pub direction: UcaDirection,
    /// 触发上下文（如 `"grid_outage"`、`"soc_full"`）。
    pub context: &'static str,
    /// 关联的危险 ID（指向 [`crate::hazard::Hazard`]）。
    pub hazard_id: &'static str,
}

/// STPA 条目 = UCA + 安全约束 + 场景。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StpaEntry {
    pub uca: UnsafeControlAction,
    /// 由该 UCA 推导的安全约束（必须由代码/策略强制）。
    pub safety_constraint: &'static str,
    /// 导致该 UCA 的因果场景（控制器误判/反馈延迟/执行器失效等）。
    pub scenario: &'static str,
}

impl StpaEntry {
    pub const fn new(
        uca: UnsafeControlAction,
        safety_constraint: &'static str,
        scenario: &'static str,
    ) -> Self {
        Self {
            uca,
            safety_constraint,
            scenario,
        }
    }
}

impl UnsafeControlAction {
    pub const fn new(
        id: &'static str,
        control_action: &'static str,
        direction: UcaDirection,
        context: &'static str,
        hazard_id: &'static str,
    ) -> Self {
        Self {
            id,
            control_action,
            direction,
            context,
            hazard_id,
        }
    }
}

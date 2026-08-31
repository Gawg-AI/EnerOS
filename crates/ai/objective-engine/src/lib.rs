//! EnerOS 蓝图 61 [09] §2 —— v0.314.36 ObjectiveModule（规则编译）。
//!
//! # 唯一不变量（[09] §2.1）
//!
//! 进入规划器的每条硬约束、软约束和目标都具有**来源、优先级、作用域、
//! 单位、有效期和冲突裁决**；自由文本和未认证业务值不能直接进入模型。
//! 规则在 host 编译（本 crate），target 不执行任何规则脚本。
//!
//! # 公开面（[16] §3.7）
//!
//! [`compiler::ObjectiveCompiler::compile`] 是唯一入口：
//! `(snapshot, registry_generation, bundle, facts) -> CompiledObjective`
//! （内含合同形状 [`eneros_decision_contract::ObjectiveSpec`] + 求解经济
//! 输入 + 编译证据）。RuleBundle/规则解释/优先级/冲突裁决/归一化全部
//! crate 私有实现面（深模块）。
//!
//! # 关键语义
//!
//! - 七个固定优先级层（[09] §2.4）；高层硬约束不可被低层权重"买掉"
//!   （类型面：硬约束与目标项是不同类型，权重只作用于 ObjectiveTerm）。
//! - 同层同类硬约束交集为空 → [`compiler::CompileError::Conflicted`]，
//!   **不产出 ObjectiveSpec**（冲突时零 solver、零控制）。
//! - 软目标先按物理量归一化再加权；权重 bundle 版本化。
//! - IntentCandidate 输入在本版本**生产禁用**（传入即拒绝）。
//! - 确定性：规则顺序无关；输出排序与 digest 稳定。
//!
//! # 偏离登记（[09] §2.8 门内如实声明）
//!
//! - [`CompiledObjective`] 除合同 ObjectiveSpec 外携带价格序列等求解
//!   经济输入：合同 ObjectiveSpec 无经济字段，价格必须随决策文档传递
//!   （`.37` planner 消费）；spec_digest 覆盖全部内容。
//! - 起停次数规则（[09] §2.5 状态机类）依赖跨 horizon 历史状态，当前
//!   无 durable 输入源——本卷只落地 min on/off 由 `.37` 包络承载；
//!   启动次数登记待 Phase 5/6 业务事实源。
//!
//! # 依赖纪律（[16] §3.7）
//!
//! 禁止依赖 LLM engine、device writer。solver 依赖仅迁移/shadow 模块
//! （energy-lp-model 的 ScheduleConfig 值类型）。

#![no_std]
#![cfg_attr(not(test), allow(dead_code))]

extern crate alloc;

pub mod bundle;
pub mod compiler;
pub mod migrate;
pub mod spec;

pub use bundle::RuleBundle;
pub use compiler::{
    CompileError, CompileInputs, CompiledObjective, ObjectiveCompiler, ObjectiveConflict,
};
pub use spec::{ConstraintSpec, FactSet, HorizonPlan, ObjectiveTerm, ScopeRef};

/// 七个固定优先级层（[09] §2.4）。高层硬约束不可被低层目标用权重"买掉"。
pub mod layer {
    /// 设备本地保护和法定安全。
    pub const DEVICE_SAFETY: u16 = 1;
    /// 电网/并网规范与站内电气边界。
    pub const GRID_CODE: u16 = 2;
    /// 工艺联锁、产品质量和人员安全。
    pub const PROCESS_SAFETY: u16 = 3;
    /// 已确认的合同/市场/调度承诺。
    pub const COMMITMENT: u16 = 4;
    /// 维护、寿命和企业运行限制。
    pub const OPERATIONAL: u16 = 5;
    /// 经济、碳、舒适度等优化目标。
    pub const ECONOMIC: u16 = 6;
    /// LLM/人工未批准建议（生产编译**拒绝**进入）。
    pub const UNAPPROVED_ADVICE: u16 = 7;

    /// 层号合法（1..=7）。
    pub const fn is_valid(p: u16) -> bool {
        p >= 1 && p <= UNAPPROVED_ADVICE
    }
}

/// 归一化方法 id（[09] §2.4：软目标先按物理量归一化再加权）。
pub mod normalization {
    /// 按站额定功率归一（per-unit）。
    pub const PER_UNIT_SITE_RATING: u32 = 1;
    /// 按价格缩放（经济量）。
    pub const PRICE_SCALED: u32 = 2;
    /// 恒等（无量纲/已归一）。
    pub const IDENTITY: u32 = 3;

    /// 已登记 id？（闭集校验）
    pub const fn is_known(id: u32) -> bool {
        matches!(id, PER_UNIT_SITE_RATING | PRICE_SCALED | IDENTITY)
    }
}

/// 目标项种类码（闭集；[09] §2.5 目标类）。
pub mod term_kind {
    /// 经济成本/收益（价格序列驱动；元）。
    pub const ECONOMIC: u16 = 1;
    /// 峰值（kW）。
    pub const PEAK: u16 = 2;
    /// 弃电/消纳（kWh）。
    pub const CURTAIL: u16 = 3;
    /// 碳排放（kg）。
    pub const CARBON: u16 = 4;
    /// SOC 偏差（比率）。
    pub const SOC_DEVIATION: u16 = 5;
    /// 功率平滑（kW）。
    pub const SMOOTHING: u16 = 6;
    /// 寿命/循环（等效循环数）。
    pub const LIFETIME: u16 = 7;

    pub const fn is_known(code: u16) -> bool {
        code >= 1 && code <= 7
    }

    /// 主目标种类码 → 合同 ObjectiveKind（合同面当前只覆盖四类；
    /// 无映射的种类只能作软项存在）。
    pub const fn to_contract(
        code: u16,
    ) -> Option<eneros_decision_contract::objective::ObjectiveKind> {
        use eneros_decision_contract::objective::ObjectiveKind;
        match code {
            ECONOMIC => Some(ObjectiveKind::MaxRevenue),
            PEAK => Some(ObjectiveKind::PeakShave),
            CURTAIL => Some(ObjectiveKind::SelfConsume),
            _ => None,
        }
    }
}

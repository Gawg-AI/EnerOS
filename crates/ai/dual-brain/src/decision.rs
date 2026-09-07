//! 蓝图 40 WP-40.3：决策语义统一——所有路径只产生 Proposal 或 Rejection。
//!
//! 旧实现 `DualBrainResult` 把"返回了计划"与"已下发"混为一谈：FastPath/
//! Fallback/Degraded 三条路径 `Ok(schedule)` 后跳过 sink，业务层把所有
//! `Ok` 当成功（P0-10）。本模块引入：
//!
//! - [`DecisionOutcome`]：`Proposal`（无副作用计划）/ `NoAction`（明确不动作）
//!   / `Rejected`（结构化拒绝）——**任何路径都不自行写设备**；
//! - [`DecisionPath`]：Fast/Slow/Fallback/Degraded 四路径（Fallback 不再
//!   伪装成 FastPath）；
//! - [`ProposedControlPlan`]：唯一下发载体——由调用方（EnergyAgent）提交给
//!   唯一 [`ControlDispatch`]（蓝图 40 WP-40.6，control-dispatch crate），
//!   由 dispatch 完成 validate→authorize→persist→deliver→ACK→readback→audit。
//!
//! 不变量（蓝图 40 §1）：
//! 1. `Proposal` 无副作用——不调用任何写设备接口；
//! 2. `NoAction` 不是成功下发（审计路径不同）；
//! 3. 拒绝/超时/不确定结果均审计；`Ok` 只表示阶段成功。

use alloc::format;
use alloc::string::String;
use alloc::vec;

use eneros_energy_lp_model::result::{ScheduleEntry, ScheduleResult};
use eneros_solver_core::result::SolveStatus;
/// 调度窗口小时数（原 coordinator 常量；49 WP-49.4 迁入 decision——
/// target 子集消费（dispatch 引擎），不依赖 host 门控模块）。
pub const SCHEDULE_WINDOW_HOURS: f64 = 0.25;

use eneros_intent_contract::contract::FeedbackContract;

use crate::latency::LatencyBreakdown;

/// 决策路径（蓝图 40 WP-40.3 `DecisionPath`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionPath {
    /// 慢路径（完整 LLM + LP + 校验链）。
    Slow,
    /// 快路径（查表 + 微调，无 LLM）。
    Fast,
    /// 回退路径（LLM 首次失败 → fast-path 保守解）。
    Fallback,
    /// 降级路径（LLM 连续失败 → 复用上次**已提交**计划或 NoAction）。
    Degraded,
}

impl DecisionPath {
    /// 审计/证据标签。
    pub fn as_str(self) -> &'static str {
        match self {
            DecisionPath::Slow => "Slow",
            DecisionPath::Fast => "Fast",
            DecisionPath::Fallback => "Fallback",
            DecisionPath::Degraded => "Degraded",
        }
    }
}

/// 提议控制计划（无副作用——只描述"如果下发将做什么"）。
///
/// 由调用方提交给唯一 `ControlDispatch`；dispatch 校验授权后才产生设备写。
#[derive(Debug, Clone)]
pub struct ProposedControlPlan {
    /// 计划 ID（correlation，`req-{now}-{counter}` 风格）。
    pub plan_id: String,
    /// 决策路径。
    pub path: DecisionPath,
    /// 调度方案（LP 解 + 安全钳位后结果）。
    pub schedule: ScheduleResult,
    /// 执行目标：首时段净功率（kW，正=放电/合，负=充电/分）。
    pub power_kw: f64,
    /// 目标设备（如 `"pcs"`；dispatch 层经 registry 映射 DeviceId/PointId）。
    pub target_device: String,
    /// 命令有效期（秒，epoch 秒基准；≥1s 防零有效期）。
    pub ttl_secs: u32,
    /// 计划生成时刻（单调毫秒）。
    pub issued_at_ms: u64,
    /// 输入遥测摘要（`TrustedTelemetry.provenance_digest`；调用方提交时
    /// 填入；None = 未绑定——dispatch 拒绝 None 之外的模糊输入）。
    pub input_digest: Option<[u8; 32]>,
    /// 模型/算法摘要（LLM/solver 模型绑定 ProductManifest；未绑定时 None
    /// 且 dispatch 按策略允许——目标 AI backend 声明在 profile）。
    pub model_digest: Option<[u8; 32]>,
    /// 延迟分解（审计/观测）。
    pub latency: LatencyBreakdown,
    /// 是否经安全钳位（降级通过）。
    pub safety_clamped: bool,
    /// 慢路径预测反馈（仅供学习参考——学习门控在 dispatch 的 Committed
    /// 结果，见蓝图 40 WP-40.12）。
    pub feedback: Option<FeedbackContract>,
}

impl ProposedControlPlan {
    /// 单时段计划构造器（蓝图 55）：heuristic/mock backend 与 wire 解码的
    /// 十二字段结构字面量单源（蓝图 69.4 删除只写不读的
    /// `safety_violations` 后实数；旧"十五字段"措辞把构造器点位数误作
    /// 字段数，已订正）。目标设备固定 `"pcs"`（dispatch 层经 registry
    /// 映射 DeviceId/PointId）；TTL 为显式参数——fast/slow 路径语义差异
    /// 不在本卷收敛（蓝图 55 D4）。
    #[allow(clippy::too_many_arguments)]
    pub fn single_entry(
        plan_id: String,
        path: DecisionPath,
        net_power_kw: f64,
        soc_pct: f64,
        solve_status: SolveStatus,
        ttl_secs: u32,
        issued_at_ms: u64,
        input_digest: Option<[u8; 32]>,
    ) -> Self {
        Self {
            plan_id,
            path,
            schedule: ScheduleResult {
                schedule: vec![ScheduleEntry {
                    period: 0,
                    charge_power_kw: if net_power_kw < 0.0 {
                        -net_power_kw
                    } else {
                        0.0
                    },
                    discharge_power_kw: if net_power_kw > 0.0 {
                        net_power_kw
                    } else {
                        0.0
                    },
                    net_power_kw,
                    soc_pct,
                    revenue_yuan: 0.0,
                }],
                total_revenue_yuan: 0.0,
                objective_value: 0.0,
                solve_status,
            },
            power_kw: net_power_kw,
            target_device: String::from("pcs"),
            ttl_secs,
            issued_at_ms,
            input_digest,
            model_digest: None,
            latency: LatencyBreakdown::default(),
            safety_clamped: false,
            feedback: None,
        }
    }
}

/// 不动作原因（`NoAction` 不是成功下发）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoActionReason {
    /// 无可信输入（评估器拒绝；防御面——协调器只应收到已信任状态）。
    NoTrustedInput,
    /// 降级模式但无上次已提交计划可回放（不制造"安全默认计划"）。
    NoSafeFallback,
    /// 计划为空（调度无控制需要——空调度不上报成功）。
    EmptyPlan,
    /// 降级策略选择不动作（保守 hold）。
    DegradedHold,
}

impl NoActionReason {
    /// 审计标签。
    pub fn as_str(&self) -> &'static str {
        match self {
            NoActionReason::NoTrustedInput => "no-trusted-input",
            NoActionReason::NoSafeFallback => "no-safe-fallback",
            NoActionReason::EmptyPlan => "empty-plan",
            NoActionReason::DegradedHold => "degraded-hold",
        }
    }
}

/// 拒绝原因（结构化，可审计；SafetyFatal 保留既有分类语义，禁止重写）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectionReason {
    /// 速率闸拒绝。
    GateRateLimited,
    /// 能量闸拒绝。
    GateEnergyExceeded,
    /// 安全 Fatal 违规（分类保留：立即上报人工，不重试）。
    SafetyFatal(String),
    /// 校验失败（解析/契约/求解）。
    Validation(String),
    /// 引擎内部失败（无回退可用）。
    Internal(String),
}

impl RejectionReason {
    /// 审计标签。
    pub fn as_str(&self) -> &'static str {
        match self {
            RejectionReason::GateRateLimited => "gate-rate",
            RejectionReason::GateEnergyExceeded => "gate-energy",
            RejectionReason::SafetyFatal(_) => "safety-fatal",
            RejectionReason::Validation(_) => "validation",
            RejectionReason::Internal(_) => "internal",
        }
    }
}

/// 结构化拒绝。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionRejection {
    /// 原因。
    pub reason: RejectionReason,
    /// 拒绝发生的路径。
    pub path: DecisionPath,
    /// 拒绝时刻（单调毫秒）。
    pub issued_at_ms: u64,
}

/// 决策结果（唯一返回形态——四路径只产这三者之一）。
//
// 44 收口登记：ProposedControlPlan 体量显著大于其余变体，clippy
// large_enum_variant 建议 Box 化；Box 化涉及 runner / energy-market-agent /
// coordinator 多处所有权与解构语义，属后续卷（47 控制链）设计变更，本卷
// 纯收口不行为改——定向 allow 延后，解冻条件见 44.4 登记。
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum DecisionOutcome {
    /// 提议（无副作用；提交给 ControlDispatch 才可能产生设备写）。
    Proposal(ProposedControlPlan),
    /// 明确不动作（审计但不下发）。
    NoAction(NoActionReason),
    /// 结构化拒绝（安全/闸/校验失败）。
    Rejected(DecisionRejection),
}

impl DecisionOutcome {
    /// 审计标签（path/outcome 双要素）。
    pub fn audit_label(&self) -> String {
        match self {
            DecisionOutcome::Proposal(p) => {
                format!(
                    "path={};outcome=proposal;plan={}",
                    p.path.as_str(),
                    p.plan_id
                )
            }
            DecisionOutcome::NoAction(r) => format!("outcome=noaction;reason={}", r.as_str()),
            DecisionOutcome::Rejected(r) => format!(
                "path={};outcome=rejected;reason={}",
                r.path.as_str(),
                r.reason.as_str()
            ),
        }
    }
}

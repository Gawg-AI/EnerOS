//! 契约数据结构（D7/D8）.
//!
//! 定义 LLM ↔ Solver 双脑架构的契约数据结构：
//! - [`DeviceStatus`]：设备状态枚举（D7：蓝图 §4.1 line 14632 引用但未定义，本地最小集合）
//! - [`SystemContext`]：系统运行上下文（正向契约携带）
//! - [`LlmMeta`]：LLM 推理元数据（正向契约携带）
//! - [`IntentContract`]：正向契约（LLM → Solver）
//! - [`FeedbackContract`]：反向契约（Solver → LLM）

use alloc::string::String;
use alloc::vec::Vec;

use eneros_energy_lp_model::result::ScheduleEntry;
use eneros_intent_parser::intent::Intent;
use eneros_safety_validator::result::Violation;
use eneros_solver_core::result::SolveStatus;
use serde::{Deserialize, Serialize};

/// 设备状态枚举（D7）.
///
/// 蓝图 §4.1 line 14632 引用但未定义，本版本定义最小满足契约需求的集合：
/// - `Normal`：正常运行
/// - `Warning`：告警
/// - `Fault`：故障
/// - `Maintenance`：维护
/// - `Offline`：离线
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeviceStatus {
    /// 正常运行.
    Normal,
    /// 告警.
    Warning,
    /// 故障.
    Fault,
    /// 维护.
    Maintenance,
    /// 离线.
    Offline,
}

/// 系统运行上下文.
///
/// 携带契约生成时刻的系统状态快照，供 Solver 决策参考。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemContext {
    /// 当前 SOC（0.0~1.0）.
    pub current_soc: f64,
    /// 当前功率（kW，正为放电，负为充电）.
    pub current_power_kw: f64,
    /// 当前电价（元/kWh）.
    pub current_price: f64,
    /// 当前时段索引.
    pub current_period: usize,
    /// 设备状态.
    pub device_status: DeviceStatus,
    /// 活跃告警列表.
    pub alarms: Vec<String>,
}

/// LLM 推理元数据.
///
/// 记录生成该契约的 LLM 推理过程信息，用于审计与性能追踪。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmMeta {
    /// 模型名称.
    pub model_name: String,
    /// 推理耗时（毫秒）.
    pub inference_ms: u64,
    /// Token 数量.
    pub token_count: usize,
    /// 置信度（0.0~1.0）.
    pub confidence: f64,
}

/// 正向契约（LLM → Solver）.
///
/// 包含版本化意图与上下文，是 LLM 向 Solver 下发决策请求的标准化载体。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentContract {
    /// 契约 schema 版本（如 "1.1.0"）.
    pub schema_version: String,
    /// 请求 ID（用于关联正向与反向契约）.
    pub request_id: String,
    /// 时间戳（毫秒）.
    pub timestamp: u64,
    /// LLM 意图（复用 v0.68.0 `Intent`，D1）.
    pub intent: Intent,
    /// 系统运行上下文.
    pub context: SystemContext,
    /// LLM 推理元数据.
    pub llm_meta: LlmMeta,
}

/// 反向契约（Solver → LLM）.
///
/// 反馈求解与校验结果，供 LLM 在下一轮决策时参考。
///
/// 注：`solve_status` / `clamp_info` / `executed_schedule` 字段引用的
/// `SolveStatus` / `Violation` / `ScheduleEntry` 来自 v0.64.0/v0.66.0/v0.67.0，
/// 这些类型未派生 `Serialize`/`Deserialize`，故使用 `#[serde(skip)]` 跳过
/// 序列化（反序列化时回退到默认值）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackContract {
    /// 请求 ID（与正向契约 `request_id` 关联）.
    pub request_id: String,
    /// 求解状态（serde skip：`SolveStatus` 未派生 serde trait）.
    #[serde(skip, default = "default_solve_status")]
    pub solve_status: SolveStatus,
    /// 安全校验是否通过.
    pub validation_passed: bool,
    /// 截断信息（无违规时为 None；serde skip：`Violation` 未派生 serde trait）.
    #[serde(skip)]
    pub clamp_info: Option<Vec<Violation>>,
    /// 实际执行的调度方案（serde skip：`ScheduleEntry` 未派生 serde trait）.
    #[serde(skip)]
    pub executed_schedule: Option<Vec<ScheduleEntry>>,
    /// 实际收益（元）.
    pub actual_revenue: f64,
    /// 求解耗时（毫秒）.
    pub solve_ms: u64,
}

/// `SolveStatus` 的 serde 默认值（`SolveStatus` 未实现 `Default`）.
fn default_solve_status() -> SolveStatus {
    SolveStatus::Optimal
}

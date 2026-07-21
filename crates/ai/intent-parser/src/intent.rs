//! 意图数据结构（D1/D8/D9）.
//!
//! LLM 输出的意图 JSON 反序列化目标。`priority`/`reason`/`confidence`
//! 使用 `#[serde(default)]` 容错 LLM 省略字段（D9）。

use alloc::string::String;

use serde::{Deserialize, Serialize};

/// 默认优先级（D9：LLM 省略 `priority` 时回退到 3）.
fn default_priority() -> u8 {
    3
}

/// 意图类型枚举（D8：派生 `Debug + Clone + PartialEq + Serialize + Deserialize`）.
///
/// 7 种意图覆盖储能系统全部调度语义：
/// - `Charge` / `Discharge` / `Hold` / `Stop` — 显式控制指令
/// - `EmergencyStop` — 紧急停机（依赖 `SystemState` 锁定 SOC）
/// - `AutonomousSchedule` — 自主调度（仅设置 `soc_final`，由 Solver 决策）
/// - `SetSetpoint` — 设置 PCS 额定功率
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum IntentType {
    Charge,
    Discharge,
    Hold,
    Stop,
    EmergencyStop,
    AutonomousSchedule,
    SetSetpoint,
}

/// 时间范围（时段索引闭区间 `[start_period, end_period]`）.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeRange {
    pub start_period: usize,
    pub end_period: usize,
}

/// 功率意图.
///
/// `power_kw` 为正表示放电，为负表示充电（符号约定与 LP 模型一致）。
/// `power_ratio` 为可选的功率比例（0.0~1.0），本版本仅作为元数据，不参与 LP 转换。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PowerIntent {
    pub power_kw: f64,
    pub power_ratio: Option<f64>,
}

/// SOC 目标.
///
/// `target_soc` 为 0.0~1.0 的归一化 SOC；`by_period` 为达成目标的截止时段。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SocIntent {
    pub target_soc: f64,
    pub by_period: usize,
}

/// LLM 输出的意图（JSON 反序列化目标）.
///
/// 仅 `intent_type` 为必填字段，其余字段均 `Option` 或 `#[serde(default)]`，
/// 容错 LLM 输出不完整的情况（D9）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Intent {
    /// 意图类型（必填）.
    pub intent_type: IntentType,
    /// 时间范围（可选，仅 `Charge`/`Discharge` 使用）.
    pub time_range: Option<TimeRange>,
    /// 功率意图（可选，`Charge`/`Discharge`/`SetSetpoint` 使用）.
    pub power: Option<PowerIntent>,
    /// SOC 目标（可选，`AutonomousSchedule` 使用）.
    pub soc_target: Option<SocIntent>,
    /// 优先级（D9：默认 3）.
    #[serde(default = "default_priority")]
    pub priority: u8,
    /// 决策理由（D9：默认空字符串）.
    #[serde(default)]
    pub reason: String,
    /// 置信度（D9：默认 0.0）.
    #[serde(default)]
    pub confidence: f64,
}

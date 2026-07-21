//! DDS 服务质量策略（v0.76.0：History::KeepLast(u32) + deadline/lifespan/priority）.

use core::time::Duration;

/// 可靠性策略.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Reliability {
    /// 尽力而为（不重传，低延迟）.
    BestEffort,
    /// 可靠传输（重传保证到达，默认）.
    #[default]
    Reliable,
}

/// 持久性策略.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Durability {
    /// 易失（仅传递给当前在线的 subscriber，默认）.
    #[default]
    Volatile,
    /// 瞬态本地（保留历史样本供晚加入的 subscriber 读取）.
    TransientLocal,
}

/// 历史策略.
///
/// v0.76.0 BREAKING：`KeepLast` 携带深度参数（u32），移除 `QosPolicy::history_depth` 独立字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum History {
    /// 保留最后 N 条样本（深度内嵌于变体）.
    KeepLast(u32),
    /// 保留全部历史样本.
    KeepAll,
}

/// DDS 服务质量策略.
///
/// v0.76.0 BREAKING：
/// - `History::KeepLast(u32)` 内嵌深度，移除 `history_depth` 独立字段
/// - 新增 `deadline` / `lifespan` / `priority` 字段（蓝图 §4.1）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QosPolicy {
    /// 可靠性.
    pub reliability: Reliability,
    /// 持久性.
    pub durability: Durability,
    /// 历史策略（KeepLast 携带深度）.
    pub history: History,
    /// 期望最大到达间隔，超时触发告警（None 表示无限制）.
    pub deadline: Option<Duration>,
    /// 样本有效期，过期丢弃（None 表示永不过期）.
    pub lifespan: Option<Duration>,
    /// TSN 优先级映射（0=最低，7=最高）.
    pub priority: i32,
}

impl QosPolicy {
    /// 状态类数据默认 QoS：BestEffort + Volatile + KeepLast(1) + deadline=None + lifespan=5s + priority=0.
    ///
    /// 适用于遥测状态等"最新值优先"的场景。
    pub fn state_default() -> Self {
        Self {
            reliability: Reliability::BestEffort,
            durability: Durability::Volatile,
            history: History::KeepLast(1),
            deadline: None,
            lifespan: Some(Duration::from_secs(5)),
            priority: 0,
        }
    }

    /// 命令类默认 QoS：Reliable + TransientLocal + KeepAll + deadline=2s + lifespan=10s + priority=6.
    ///
    /// 适用于控制命令等"可靠不丢"的场景。
    pub fn command_default() -> Self {
        Self {
            reliability: Reliability::Reliable,
            durability: Durability::TransientLocal,
            history: History::KeepAll,
            deadline: Some(Duration::from_secs(2)),
            lifespan: Some(Duration::from_secs(10)),
            priority: 6,
        }
    }

    /// 告警类默认 QoS：Reliable + TransientLocal + KeepLast(10) + deadline=None + lifespan=None + priority=7.
    ///
    /// 适用于告警/故障等"高优先级 + 保留历史"的场景。
    pub fn alert_default() -> Self {
        Self {
            reliability: Reliability::Reliable,
            durability: Durability::TransientLocal,
            history: History::KeepLast(10),
            deadline: None,
            lifespan: None,
            priority: 7,
        }
    }
}

impl Default for QosPolicy {
    /// 默认 QoS：Reliable + Volatile + KeepLast(10) + deadline=None + lifespan=None + priority=0.
    fn default() -> Self {
        Self {
            reliability: Reliability::Reliable,
            durability: Durability::Volatile,
            history: History::KeepLast(10),
            deadline: None,
            lifespan: None,
            priority: 0,
        }
    }
}

//! DDS Topic 规范与 QoS 策略（v0.76.0 语义层）.
//!
//! 在 v0.75.0 通信底座上建立语义层：Topic 命名规范、QoS 分级策略、标准 Topic 预置。
//! 能源场景中不同消息类型需要不同 QoS：状态类消息低延迟（最新值优先），命令类消息
//! 可靠不丢，告警类消息保留历史。
//!
//! # 偏差声明
//!
//! - **D6**：仅定义 `PayloadType` 枚举，不实现 CDR 编码（CDR 编码由 v0.77.0 路由器或后续版本实现）
//! - **D8**：`standard_topics()` 使用普通函数替代 `once_cell::sync::Lazy`（no_std 兼容）
//! - **D9**：`TopicError` 作为独立错误类型（不并入 `DdsError`；Topic 语义错误与 DDS 通信错误关注点不同）

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;
use core::time::Duration;

use crate::qos::QosPolicy;

/// Topic 分类（决定默认 QoS 策略）.
///
/// 每个分类对应一组能源场景消息语义：
/// - `State`：遥测状态（BEST_EFFORT + KEEP_LAST(1)，最新值优先）
/// - `Command`：控制命令（RELIABLE + KEEP_ALL，可靠不丢）
/// - `Alert`：告警故障（RELIABLE + KEEP_LAST(10)，高优先级 + 保留历史）
/// - `Twin`：数字孪生（BEST_EFFORT + KEEP_LAST(1)）
/// - `Market`：市场信号（BEST_EFFORT + KEEP_LAST(1)）
/// - `Log`：日志（RELIABLE + KEEP_ALL）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopicCategory {
    /// 遥测状态（BEST_EFFORT + KEEP_LAST(1)）.
    State,
    /// 控制命令（RELIABLE + KEEP_ALL）.
    Command,
    /// 告警故障（RELIABLE + KEEP_LAST(10)）.
    Alert,
    /// 数字孪生（BEST_EFFORT + KEEP_LAST(1)）.
    Twin,
    /// 市场信号（BEST_EFFORT + KEEP_LAST(1)）.
    Market,
    /// 日志（RELIABLE + KEEP_ALL）.
    Log,
}

/// 负载编码格式.
///
/// **D6**：仅定义枚举，不实现 CDR 编码（CDR 编码由 v0.77.0 路由器或后续版本实现）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadType {
    /// JSON 文本编码.
    Json,
    /// Bincode 二进制编码（Rust 原生）.
    Bincode,
    /// DDS 标准 CDR 编码（Common Data Representation）.
    Cdr,
}

/// Topic 规范.
///
/// 描述一个 DDS Topic 的语义信息：名称、分类、负载格式、默认 QoS、有效期。
#[derive(Debug, Clone)]
pub struct TopicSpec {
    /// Topic 名（必须以 `/` 开头，仅含 `[a-zA-Z0-9_/{}`）.
    pub name: String,
    /// Topic 分类（决定默认 QoS 策略语义）.
    pub category: TopicCategory,
    /// 负载编码格式.
    pub payload_type: PayloadType,
    /// 默认 QoS 策略.
    pub default_qos: QosPolicy,
    /// 消息有效期，`None` 表示不过期.
    pub ttl: Option<Duration>,
}

/// Topic 错误类型.
///
/// **D9**：作为独立错误类型（不并入 `DdsError`；Topic 语义错误与 DDS 通信错误关注点不同）。
#[derive(Debug)]
pub enum TopicError {
    /// Topic 名非法（不以 `/` 开头或含非法字符）.
    InvalidName(String),
    /// 重复注册同名且 QoS 不一致.
    Conflict { name: String },
    /// QoS 策略非法（如 KeepLast(0)）.
    InvalidQos(String),
}

impl fmt::Display for TopicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TopicError::InvalidName(msg) => {
                write!(f, "topic 名非法: {}", msg)
            }
            TopicError::Conflict { name } => {
                write!(f, "topic 冲突: 已存在同名且 QoS 不一致的 topic '{}'", name)
            }
            TopicError::InvalidQos(msg) => {
                write!(f, "QoS 策略非法: {}", msg)
            }
        }
    }
}

impl core::error::Error for TopicError {}

/// 校验 Topic 名合法性.
///
/// 规则：必须以 `/` 开头，仅含 `[a-zA-Z0-9_/{}` 字符。
///
/// # 示例
///
/// ```
/// # use eneros_agent_bus_dds::topic::validate_topic_name;
/// assert!(validate_topic_name("/power/state/battery/1").is_ok());
/// assert!(validate_topic_name("/power/state/battery/{id}").is_ok());
/// assert!(validate_topic_name("power/state").is_err());  // 不以 / 开头
/// ```
pub fn validate_topic_name(name: &str) -> Result<(), TopicError> {
    if !name.starts_with('/') {
        return Err(TopicError::InvalidName(String::from("必须以 / 开头")));
    }
    for c in name.chars() {
        if !c.is_ascii_alphanumeric() && c != '/' && c != '_' && c != '{' && c != '}' {
            return Err(TopicError::InvalidName(alloc::format!("非法字符: {}", c)));
        }
    }
    Ok(())
}

/// 8 个标准预置 Topic.
///
/// **D8**：使用普通函数替代 `once_cell::sync::Lazy`（no_std 兼容；`once_cell::sync` 需 `std`）。
///
/// 涵盖能源场景核心消息流：
/// - 状态类（3 个）：battery / pv / grid
/// - 市场类（2 个）：price / signal
/// - 命令类（1 个）：internal
/// - 告警类（1 个）：fault
/// - 孪生类（1 个）：update
pub fn standard_topics() -> Vec<TopicSpec> {
    vec![
        TopicSpec {
            name: String::from("/power/state/battery/{id}"),
            category: TopicCategory::State,
            payload_type: PayloadType::Json,
            default_qos: QosPolicy::state_default(),
            ttl: Some(Duration::from_secs(5)),
        },
        TopicSpec {
            name: String::from("/power/state/pv/{id}"),
            category: TopicCategory::State,
            payload_type: PayloadType::Json,
            default_qos: QosPolicy::state_default(),
            ttl: Some(Duration::from_secs(5)),
        },
        TopicSpec {
            name: String::from("/power/state/grid"),
            category: TopicCategory::State,
            payload_type: PayloadType::Json,
            default_qos: QosPolicy::state_default(),
            ttl: Some(Duration::from_secs(2)),
        },
        TopicSpec {
            name: String::from("/power/market/price"),
            category: TopicCategory::Market,
            payload_type: PayloadType::Json,
            default_qos: QosPolicy::state_default(),
            ttl: Some(Duration::from_secs(60)),
        },
        TopicSpec {
            name: String::from("/power/market/signal"),
            category: TopicCategory::Market,
            payload_type: PayloadType::Json,
            default_qos: QosPolicy::state_default(),
            ttl: Some(Duration::from_secs(30)),
        },
        TopicSpec {
            name: String::from("/power/command/internal"),
            category: TopicCategory::Command,
            payload_type: PayloadType::Bincode,
            default_qos: QosPolicy::command_default(),
            ttl: Some(Duration::from_secs(10)),
        },
        TopicSpec {
            name: String::from("/power/alert/fault"),
            category: TopicCategory::Alert,
            payload_type: PayloadType::Json,
            default_qos: QosPolicy::alert_default(),
            ttl: None,
        },
        TopicSpec {
            name: String::from("/power/twin/update"),
            category: TopicCategory::Twin,
            payload_type: PayloadType::Json,
            default_qos: QosPolicy::state_default(),
            ttl: Some(Duration::from_secs(5)),
        },
    ]
}

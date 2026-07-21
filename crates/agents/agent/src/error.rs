//! Agent 错误类型
//!
//! 涵盖 Agent 描述符校验、配额、信任等级与重复 ID 等错误场景。

use alloc::string::String;
use core::fmt;

use crate::id::AgentId;
use crate::types::AgentState;

/// Agent 操作错误.
///
/// 注：不派生 `Eq`，因为 `ConstraintViolated` 包含 `f32` 字段（`f32` 不实现 `Eq`，
/// NaN 非自反）。错误类型通常不需要 `Eq`，仅 `PartialEq` 足以满足 `assert_eq!` 比较。
#[derive(Debug, Clone, PartialEq)]
pub enum AgentError {
    /// 无效描述符
    InvalidDescriptor,
    /// 配额超限
    QuotaExceeded,
    /// 无效信任等级
    InvalidTrustLevel,
    /// 重复 ID
    DuplicateId,
    /// 注册表中未找到 Agent
    AgentNotFound,
    /// Agent ID 已在注册表中
    AlreadyRegistered,
    /// 非法状态转换
    InvalidStateTransition {
        /// 源状态
        from: AgentState,
        /// 目标状态
        to: AgentState,
    },
    /// Agent 不在存活状态
    AgentNotAlive,
    /// 代码加载失败
    CodeLoadFailed(String),
    /// 初始化失败
    InitFailed(String),
    /// 启动失败
    StartFailed(String),
    /// 心跳超时
    HeartbeatTimeout { agent_id: AgentId, missed: u32 },
    /// Agent 不健康
    AgentUnhealthy { agent_id: AgentId },
    /// 超过最大重启次数
    MaxRestartsExceeded { agent_id: AgentId, count: u32 },
    /// 检查点数据损坏
    CheckpointCorrupted { agent_id: AgentId },
    /// 重启失败
    RestartFailed { agent_id: AgentId, reason: String },
    /// 能力 Token 已过期
    TokenExpired,
    /// 能力 Token 签名无效
    TokenSignatureInvalid,
    /// 权限不足
    PermissionDenied { required: u32, actual: u32 },
    /// 约束违反
    ConstraintViolated { value: f32, limit: f32 },
    /// 能力 Token 未签名
    TokenNotSigned,
    /// 能力 Token 已冻结
    TokenFrozen,
    /// 能力 Token 已撤销
    TokenRevoked,
    /// 无匹配能力（agent 无对应 target 的有效令牌）
    NoCapability { agent: AgentId, target: String },
    /// 系统过载
    SystemOverload,
    /// OOM 风险
    OomRisk,
    /// 系统过热（含温度值，f32 类型 — 因此不派生 Eq）
    Overheat { temp: f32 },
    /// 循环依赖检测（依赖图中出现环）
    CircularDependency,
    /// 恢复失败（超过最大尝试次数仍未恢复）
    RecoveryFailed { agent: AgentId, attempts: u32 },
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentError::InvalidDescriptor => write!(f, "invalid agent descriptor"),
            AgentError::QuotaExceeded => write!(f, "agent quota exceeded"),
            AgentError::InvalidTrustLevel => write!(f, "invalid trust level"),
            AgentError::DuplicateId => write!(f, "duplicate agent id"),
            AgentError::AgentNotFound => write!(f, "agent not found"),
            AgentError::AlreadyRegistered => write!(f, "agent already registered"),
            AgentError::InvalidStateTransition { from, to } => {
                write!(f, "invalid state transition: {:?} -> {:?}", from, to)
            }
            AgentError::AgentNotAlive => write!(f, "agent not alive"),
            AgentError::CodeLoadFailed(msg) => write!(f, "code load failed: {}", msg),
            AgentError::InitFailed(msg) => write!(f, "init failed: {}", msg),
            AgentError::StartFailed(msg) => write!(f, "start failed: {}", msg),
            AgentError::HeartbeatTimeout { agent_id, missed } => {
                write!(
                    f,
                    "heartbeat timeout: agent {:?} missed {} beats",
                    agent_id, missed
                )
            }
            AgentError::AgentUnhealthy { agent_id } => {
                write!(f, "agent unhealthy: {:?}", agent_id)
            }
            AgentError::MaxRestartsExceeded { agent_id, count } => {
                write!(
                    f,
                    "max restarts exceeded: agent {:?} restarted {} times",
                    agent_id, count
                )
            }
            AgentError::CheckpointCorrupted { agent_id } => {
                write!(f, "checkpoint corrupted: agent {:?}", agent_id)
            }
            AgentError::RestartFailed { agent_id, reason } => {
                write!(
                    f,
                    "restart failed: agent {:?}, reason: {}",
                    agent_id, reason
                )
            }
            AgentError::TokenExpired => write!(f, "capability token expired"),
            AgentError::TokenSignatureInvalid => write!(f, "capability token signature invalid"),
            AgentError::PermissionDenied { required, actual } => {
                write!(
                    f,
                    "permission denied: required 0x{:08x}, actual 0x{:08x}",
                    required, actual
                )
            }
            AgentError::ConstraintViolated { value, limit } => {
                write!(
                    f,
                    "constraint violated: value {} exceeds limit {}",
                    value, limit
                )
            }
            AgentError::TokenNotSigned => write!(f, "capability token not signed"),
            AgentError::TokenFrozen => write!(f, "capability token frozen"),
            AgentError::TokenRevoked => write!(f, "capability token revoked"),
            AgentError::NoCapability { agent, target } => {
                write!(
                    f,
                    "no capability: agent {:?} has no token for target {}",
                    agent, target
                )
            }
            AgentError::SystemOverload => write!(f, "system overload"),
            AgentError::OomRisk => write!(f, "oom risk"),
            AgentError::Overheat { temp } => write!(f, "system overheat: {}°C", temp),
            AgentError::CircularDependency => write!(f, "circular dependency detected"),
            AgentError::RecoveryFailed { agent, attempts } => {
                write!(
                    f,
                    "recovery failed: agent {:?} after {} attempts",
                    agent, attempts
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        assert_eq!(
            format!("{}", AgentError::InvalidDescriptor),
            "invalid agent descriptor"
        );
        assert_eq!(
            format!("{}", AgentError::QuotaExceeded),
            "agent quota exceeded"
        );
        assert_eq!(
            format!("{}", AgentError::InvalidTrustLevel),
            "invalid trust level"
        );
        assert_eq!(format!("{}", AgentError::DuplicateId), "duplicate agent id");
    }

    #[test]
    fn test_new_error_variants_display() {
        assert_eq!(format!("{}", AgentError::AgentNotFound), "agent not found");
        assert_eq!(
            format!("{}", AgentError::AlreadyRegistered),
            "agent already registered"
        );
    }

    #[test]
    fn test_error_clone_eq() {
        let e1 = AgentError::QuotaExceeded;
        let e2 = e1.clone();
        assert_eq!(e1, e2);
        assert_ne!(e1, AgentError::DuplicateId);

        let nf1 = AgentError::AgentNotFound;
        let nf2 = nf1.clone();
        assert_eq!(nf1, nf2);
        assert_ne!(nf1, AgentError::AlreadyRegistered);

        let ar1 = AgentError::AlreadyRegistered;
        let ar2 = ar1.clone();
        assert_eq!(ar1, ar2);
        assert_ne!(ar1, AgentError::AgentNotFound);
    }

    #[test]
    fn test_lifecycle_error_variants_display() {
        assert_eq!(
            format!(
                "{}",
                AgentError::InvalidStateTransition {
                    from: AgentState::Created,
                    to: AgentState::Running
                }
            ),
            "invalid state transition: Created -> Running"
        );
        assert_eq!(format!("{}", AgentError::AgentNotAlive), "agent not alive");
    }

    #[test]
    fn test_invalid_state_transition_eq() {
        let a = AgentError::InvalidStateTransition {
            from: AgentState::Created,
            to: AgentState::Running,
        };
        let b = AgentError::InvalidStateTransition {
            from: AgentState::Created,
            to: AgentState::Running,
        };
        assert_eq!(a, b);

        let c = AgentError::InvalidStateTransition {
            from: AgentState::Created,
            to: AgentState::Dead,
        };
        assert_ne!(a, c);
    }

    #[test]
    fn test_spawn_error_variants_display() {
        assert_eq!(
            format!(
                "{}",
                AgentError::CodeLoadFailed(String::from("unknown type"))
            ),
            "code load failed: unknown type"
        );
        assert_eq!(
            format!("{}", AgentError::InitFailed(String::from("timeout"))),
            "init failed: timeout"
        );
        assert_eq!(
            format!("{}", AgentError::StartFailed(String::from("resource busy"))),
            "start failed: resource busy"
        );
    }

    #[test]
    fn test_spawn_error_variants_eq() {
        assert_eq!(
            AgentError::CodeLoadFailed(String::from("a")),
            AgentError::CodeLoadFailed(String::from("a"))
        );
        assert_ne!(
            AgentError::CodeLoadFailed(String::from("a")),
            AgentError::CodeLoadFailed(String::from("b"))
        );
        assert_ne!(
            AgentError::InitFailed(String::from("x")),
            AgentError::StartFailed(String::from("x"))
        );
    }

    #[test]
    fn test_heartbeat_error_variants_display() {
        let msg = format!(
            "{}",
            AgentError::HeartbeatTimeout {
                agent_id: AgentId(42),
                missed: 3
            }
        );
        assert!(msg.contains("heartbeat timeout"), "got: {}", msg);
        assert!(msg.contains("missed 3 beats"), "got: {}", msg);

        let msg = format!(
            "{}",
            AgentError::AgentUnhealthy {
                agent_id: AgentId(42)
            }
        );
        assert!(msg.contains("agent unhealthy"), "got: {}", msg);
    }

    #[test]
    fn test_heartbeat_error_variants_eq() {
        assert_eq!(
            AgentError::HeartbeatTimeout {
                agent_id: AgentId(1),
                missed: 2
            },
            AgentError::HeartbeatTimeout {
                agent_id: AgentId(1),
                missed: 2
            }
        );
        assert_ne!(
            AgentError::HeartbeatTimeout {
                agent_id: AgentId(1),
                missed: 2
            },
            AgentError::HeartbeatTimeout {
                agent_id: AgentId(1),
                missed: 3
            }
        );
        assert_ne!(
            AgentError::AgentUnhealthy {
                agent_id: AgentId(1)
            },
            AgentError::HeartbeatTimeout {
                agent_id: AgentId(1),
                missed: 0
            }
        );
    }

    #[test]
    fn test_recovery_error_variants_display() {
        let msg = format!(
            "{}",
            AgentError::MaxRestartsExceeded {
                agent_id: AgentId(42),
                count: 3
            }
        );
        assert!(msg.contains("max restarts exceeded"), "got: {}", msg);
        assert!(msg.contains("restarted 3 times"), "got: {}", msg);

        let msg = format!(
            "{}",
            AgentError::CheckpointCorrupted {
                agent_id: AgentId(42)
            }
        );
        assert!(msg.contains("checkpoint corrupted"), "got: {}", msg);

        let msg = format!(
            "{}",
            AgentError::RestartFailed {
                agent_id: AgentId(42),
                reason: String::from("timeout")
            }
        );
        assert!(msg.contains("restart failed"), "got: {}", msg);
        assert!(msg.contains("reason: timeout"), "got: {}", msg);
    }

    #[test]
    fn test_recovery_error_variants_eq() {
        // MaxRestartsExceeded: same params equal, different count not equal
        assert_eq!(
            AgentError::MaxRestartsExceeded {
                agent_id: AgentId(1),
                count: 3
            },
            AgentError::MaxRestartsExceeded {
                agent_id: AgentId(1),
                count: 3
            }
        );
        assert_ne!(
            AgentError::MaxRestartsExceeded {
                agent_id: AgentId(1),
                count: 3
            },
            AgentError::MaxRestartsExceeded {
                agent_id: AgentId(1),
                count: 2
            }
        );

        // CheckpointCorrupted: same params equal
        assert_eq!(
            AgentError::CheckpointCorrupted {
                agent_id: AgentId(1)
            },
            AgentError::CheckpointCorrupted {
                agent_id: AgentId(1)
            }
        );
        assert_ne!(
            AgentError::CheckpointCorrupted {
                agent_id: AgentId(1)
            },
            AgentError::CheckpointCorrupted {
                agent_id: AgentId(2)
            }
        );

        // RestartFailed: same params equal, different reason not equal
        assert_eq!(
            AgentError::RestartFailed {
                agent_id: AgentId(1),
                reason: String::from("timeout")
            },
            AgentError::RestartFailed {
                agent_id: AgentId(1),
                reason: String::from("timeout")
            }
        );
        assert_ne!(
            AgentError::RestartFailed {
                agent_id: AgentId(1),
                reason: String::from("timeout")
            },
            AgentError::RestartFailed {
                agent_id: AgentId(1),
                reason: String::from("panic")
            }
        );

        // 3 new variants are pairwise distinct
        assert_ne!(
            AgentError::MaxRestartsExceeded {
                agent_id: AgentId(1),
                count: 0
            },
            AgentError::CheckpointCorrupted {
                agent_id: AgentId(1)
            }
        );
        assert_ne!(
            AgentError::MaxRestartsExceeded {
                agent_id: AgentId(1),
                count: 0
            },
            AgentError::RestartFailed {
                agent_id: AgentId(1),
                reason: String::from("x")
            }
        );
        assert_ne!(
            AgentError::CheckpointCorrupted {
                agent_id: AgentId(1)
            },
            AgentError::RestartFailed {
                agent_id: AgentId(1),
                reason: String::from("x")
            }
        );
    }

    #[test]
    fn test_capability_error_variants_display() {
        assert_eq!(
            format!("{}", AgentError::TokenExpired),
            "capability token expired"
        );
        assert_eq!(
            format!("{}", AgentError::TokenSignatureInvalid),
            "capability token signature invalid"
        );
        let msg = format!(
            "{}",
            AgentError::PermissionDenied {
                required: 0x01,
                actual: 0x00
            }
        );
        assert!(msg.contains("permission denied"), "got: {}", msg);
        assert!(msg.contains("required 0x00000001"), "got: {}", msg);
        assert!(msg.contains("actual 0x00000000"), "got: {}", msg);

        let msg = format!(
            "{}",
            AgentError::ConstraintViolated {
                value: 100.0,
                limit: 50.0
            }
        );
        assert!(msg.contains("constraint violated"), "got: {}", msg);
        assert!(msg.contains("value 100"), "got: {}", msg);
        assert!(msg.contains("limit 50"), "got: {}", msg);

        assert_eq!(
            format!("{}", AgentError::TokenNotSigned),
            "capability token not signed"
        );
    }

    #[test]
    fn test_capability_error_variants_eq() {
        // TokenExpired / TokenSignatureInvalid / TokenNotSigned: unit variants
        assert_eq!(AgentError::TokenExpired, AgentError::TokenExpired);
        assert_eq!(
            AgentError::TokenSignatureInvalid,
            AgentError::TokenSignatureInvalid
        );
        assert_eq!(AgentError::TokenNotSigned, AgentError::TokenNotSigned);

        // PermissionDenied: same params equal, different params not equal
        assert_eq!(
            AgentError::PermissionDenied {
                required: 0x01,
                actual: 0x00
            },
            AgentError::PermissionDenied {
                required: 0x01,
                actual: 0x00
            }
        );
        assert_ne!(
            AgentError::PermissionDenied {
                required: 0x01,
                actual: 0x00
            },
            AgentError::PermissionDenied {
                required: 0x02,
                actual: 0x00
            }
        );
        assert_ne!(
            AgentError::PermissionDenied {
                required: 0x01,
                actual: 0x00
            },
            AgentError::PermissionDenied {
                required: 0x01,
                actual: 0x01
            }
        );

        // ConstraintViolated: same params equal, different params not equal
        assert_eq!(
            AgentError::ConstraintViolated {
                value: 100.0,
                limit: 50.0
            },
            AgentError::ConstraintViolated {
                value: 100.0,
                limit: 50.0
            }
        );
        assert_ne!(
            AgentError::ConstraintViolated {
                value: 100.0,
                limit: 50.0
            },
            AgentError::ConstraintViolated {
                value: 99.0,
                limit: 50.0
            }
        );

        // 5 new variants are pairwise distinct
        assert_ne!(AgentError::TokenExpired, AgentError::TokenSignatureInvalid);
        assert_ne!(AgentError::TokenExpired, AgentError::TokenNotSigned);
        assert_ne!(
            AgentError::TokenSignatureInvalid,
            AgentError::TokenNotSigned
        );
        assert_ne!(
            AgentError::PermissionDenied {
                required: 0,
                actual: 0
            },
            AgentError::TokenExpired
        );
        assert_ne!(
            AgentError::ConstraintViolated {
                value: 0.0,
                limit: 0.0
            },
            AgentError::TokenExpired
        );
    }

    #[test]
    fn test_manager_error_variants_display() {
        assert_eq!(
            format!("{}", AgentError::TokenFrozen),
            "capability token frozen"
        );
        assert_eq!(
            format!("{}", AgentError::TokenRevoked),
            "capability token revoked"
        );
        let msg = format!(
            "{}",
            AgentError::NoCapability {
                agent: AgentId(42),
                target: String::from("Device(1)")
            }
        );
        assert!(msg.contains("no capability"), "got: {}", msg);
        assert!(msg.contains("agent"), "got: {}", msg);
        assert!(msg.contains("Device(1)"), "got: {}", msg);
    }

    #[test]
    fn test_manager_error_variants_eq() {
        // TokenFrozen / TokenRevoked: unit variants
        assert_eq!(AgentError::TokenFrozen, AgentError::TokenFrozen);
        assert_eq!(AgentError::TokenRevoked, AgentError::TokenRevoked);

        // NoCapability: same params equal, different params not equal
        assert_eq!(
            AgentError::NoCapability {
                agent: AgentId(1),
                target: String::from("Device(1)")
            },
            AgentError::NoCapability {
                agent: AgentId(1),
                target: String::from("Device(1)")
            }
        );
        assert_ne!(
            AgentError::NoCapability {
                agent: AgentId(1),
                target: String::from("Device(1)")
            },
            AgentError::NoCapability {
                agent: AgentId(2),
                target: String::from("Device(1)")
            }
        );
        assert_ne!(
            AgentError::NoCapability {
                agent: AgentId(1),
                target: String::from("Device(1)")
            },
            AgentError::NoCapability {
                agent: AgentId(1),
                target: String::from("Device(2)")
            }
        );

        // 3 new variants are pairwise distinct
        assert_ne!(AgentError::TokenFrozen, AgentError::TokenRevoked);
        assert_ne!(
            AgentError::TokenFrozen,
            AgentError::NoCapability {
                agent: AgentId(0),
                target: String::from("")
            }
        );
        assert_ne!(
            AgentError::TokenRevoked,
            AgentError::NoCapability {
                agent: AgentId(0),
                target: String::from("")
            }
        );
    }

    #[test]
    fn test_system_agent_error_variants_display() {
        // SystemOverload / OomRisk: 单元变体，字符串精确匹配
        assert_eq!(format!("{}", AgentError::SystemOverload), "system overload");
        assert_eq!(format!("{}", AgentError::OomRisk), "oom risk");

        // Overheat: 含 f32 字段，格式化结果可能因平台而异，使用 contains 校验
        let msg = format!("{}", AgentError::Overheat { temp: 85.5 });
        assert!(msg.contains("system overheat"), "got: {}", msg);
        assert!(msg.contains("85.5"), "got: {}", msg);
        assert!(msg.contains("°C"), "got: {}", msg);
    }

    #[test]
    fn test_system_agent_error_variants_eq() {
        // SystemOverload / OomRisk: 单元变体自反相等
        assert_eq!(AgentError::SystemOverload, AgentError::SystemOverload);
        assert_eq!(AgentError::OomRisk, AgentError::OomRisk);

        // Overheat: 同温度相等
        assert_eq!(
            AgentError::Overheat { temp: 85.5 },
            AgentError::Overheat { temp: 85.5 }
        );

        // Overheat: 不同温度不相等
        assert_ne!(
            AgentError::Overheat { temp: 80.0 },
            AgentError::Overheat { temp: 81.0 }
        );

        // 3 个新变体两两互不相等
        assert_ne!(AgentError::SystemOverload, AgentError::OomRisk);
        assert_ne!(
            AgentError::SystemOverload,
            AgentError::Overheat { temp: 0.0 }
        );
        assert_ne!(AgentError::OomRisk, AgentError::Overheat { temp: 0.0 });
    }

    #[test]
    fn test_recovery_orchestrator_error_variants_display() {
        let msg = format!("{}", AgentError::CircularDependency);
        assert!(msg.contains("circular dependency"), "got: {}", msg);

        let msg = format!(
            "{}",
            AgentError::RecoveryFailed {
                agent: AgentId(42),
                attempts: 3
            }
        );
        assert!(msg.contains("recovery failed"), "got: {}", msg);
        assert!(msg.contains("after 3 attempts"), "got: {}", msg);
    }

    #[test]
    fn test_recovery_orchestrator_error_variants_eq() {
        // CircularDependency: equal to itself
        assert_eq!(
            AgentError::CircularDependency,
            AgentError::CircularDependency
        );
        assert_ne!(
            AgentError::CircularDependency,
            AgentError::RecoveryFailed {
                agent: AgentId(1),
                attempts: 1
            }
        );

        // RecoveryFailed: same params equal, different agent/attempts not equal
        assert_eq!(
            AgentError::RecoveryFailed {
                agent: AgentId(1),
                attempts: 3
            },
            AgentError::RecoveryFailed {
                agent: AgentId(1),
                attempts: 3
            }
        );
        assert_ne!(
            AgentError::RecoveryFailed {
                agent: AgentId(1),
                attempts: 3
            },
            AgentError::RecoveryFailed {
                agent: AgentId(2),
                attempts: 3
            }
        );
        assert_ne!(
            AgentError::RecoveryFailed {
                agent: AgentId(1),
                attempts: 3
            },
            AgentError::RecoveryFailed {
                agent: AgentId(1),
                attempts: 2
            }
        );
    }
}

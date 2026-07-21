//! Agent 类型定义 — AgentType / AgentState / TrustLevel / CapabilityRef / AgentMetadata
//!
//! 本模块定义 Agent 的核心枚举与值类型，是 Agent Runtime 类型系统的基础。

use alloc::string::String;
use alloc::vec::Vec;

/// Agent 类型（9 种 + Custom 扩展）.
///
/// 对应 EnerOS 中不同职责的 Agent，影响默认优先级、配额与信任等级。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AgentType {
    /// 系统 Agent（最高优先级与信任等级）
    System,
    /// 设备 Agent（驱动/外设管理）
    Device,
    /// 电力市场 Agent
    Market,
    /// 电网 Agent
    Grid,
    /// 能源 Agent（调度核心）
    Energy,
    /// 数字孪生 Agent
    Twin,
    /// 边缘协调 Agent
    EdgeCoord,
    /// 云端协调 Agent
    CloudCoord,
    /// 自定义扩展类型（预留 u16 命名空间）
    Custom(u16),
}

/// Agent 生命周期状态（7 种）.
///
/// 状态转换：Created → Ready → Running ⇄ Suspended，异常进入 Error/Recovering，终态 Dead。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AgentState {
    /// 已创建，尚未就绪
    Created,
    /// 就绪，等待调度
    Ready,
    /// 运行中
    Running,
    /// 挂起（暂停）
    Suspended,
    /// 错误态
    Error,
    /// 恢复中
    Recovering,
    /// 已终止（终态）
    Dead,
}

/// 信任等级（4 级，Untrusted < Verified < Trusted < System）.
///
/// 用于资源访问控制与能力授权判定。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TrustLevel {
    /// 不受信任（最低）
    Untrusted,
    /// 已验证
    Verified,
    /// 受信任
    Trusted,
    /// 系统级（最高）
    System,
}

/// 能力引用.
///
/// 引用一个已授予的能力（由 v0.39.0 能力系统管理），可选过期时间。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilityRef {
    /// 能力 ID
    pub cap_id: u64,
    /// 授予时间戳
    pub granted_at: u64,
    /// 过期时间戳（None 表示永不过期）
    pub expires_at: Option<u64>,
}

impl CapabilityRef {
    /// 检查能力是否已过期.
    ///
    /// `now >= expires_at` 视为过期；无过期时间则永不过期。
    pub fn is_expired(&self, now: u64) -> bool {
        match self.expires_at {
            Some(exp) => now >= exp,
            None => false,
        }
    }
}

/// Agent 元数据（不可变部分）.
///
/// 描述 Agent 的静态属性，通常在打包/注册阶段写入，运行期只读。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentMetadata {
    /// Agent 名称
    pub name: String,
    /// Agent 版本（语义化版本字符串）
    pub version: String,
    /// 作者
    pub author: String,
    /// 描述
    pub description: String,
    /// 入口点
    pub entry_point: String,
    /// 所需能力列表
    pub required_capabilities: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_type_variants_distinct() {
        let types = [
            AgentType::System,
            AgentType::Device,
            AgentType::Market,
            AgentType::Grid,
            AgentType::Energy,
            AgentType::Twin,
            AgentType::EdgeCoord,
            AgentType::CloudCoord,
        ];
        // All named variants pairwise distinct
        for i in 0..types.len() {
            for j in (i + 1)..types.len() {
                assert_ne!(types[i], types[j], "variants at {} and {} collide", i, j);
            }
        }
    }

    #[test]
    fn test_agent_type_custom_distinct() {
        assert_ne!(AgentType::Custom(42), AgentType::Custom(43));
        assert_eq!(AgentType::Custom(42), AgentType::Custom(42));
    }

    #[test]
    fn test_agent_state_seven_variants() {
        let states = [
            AgentState::Created,
            AgentState::Ready,
            AgentState::Running,
            AgentState::Suspended,
            AgentState::Error,
            AgentState::Recovering,
            AgentState::Dead,
        ];
        assert_eq!(states.len(), 7);
        for i in 0..states.len() {
            for j in (i + 1)..states.len() {
                assert_ne!(states[i], states[j]);
            }
        }
    }

    #[test]
    fn test_trust_level_ordering() {
        assert!(TrustLevel::System > TrustLevel::Trusted);
        assert!(TrustLevel::Trusted > TrustLevel::Verified);
        assert!(TrustLevel::Verified > TrustLevel::Untrusted);

        // Total order chain
        let levels = [
            TrustLevel::Untrusted,
            TrustLevel::Verified,
            TrustLevel::Trusted,
            TrustLevel::System,
        ];
        for i in 0..levels.len() {
            for j in (i + 1)..levels.len() {
                assert!(levels[i] < levels[j]);
            }
        }
    }

    #[test]
    fn test_capability_ref_is_expired() {
        // Expired: now >= exp
        let cap_expired = CapabilityRef {
            cap_id: 1,
            granted_at: 100,
            expires_at: Some(1000),
        };
        assert!(cap_expired.is_expired(1000));
        assert!(cap_expired.is_expired(1001));

        // Not expired: now < exp
        assert!(!cap_expired.is_expired(999));

        // No expiry: never expires
        let cap_no_expiry = CapabilityRef {
            cap_id: 2,
            granted_at: 100,
            expires_at: None,
        };
        assert!(!cap_no_expiry.is_expired(0));
        assert!(!cap_no_expiry.is_expired(u64::MAX));
    }
}

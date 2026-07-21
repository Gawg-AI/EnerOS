//! DDS 节点配置与发现策略（D5 / D6）.

use alloc::string::String;

/// 发现策略（D5：简化为三态 enum）.
///
/// 蓝图将多播地址嵌入 enum 变体，本实现改为统一三态，
/// 具体地址由 `DdsConfig` / `configs/dds.toml` 持有。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiscoveryPolicy {
    /// 多播发现（默认，局域网内自动发现）.
    #[default]
    Multicast,
    /// 单播发现（指定对端列表，适用于跨网段）.
    Unicast,
    /// 静态发现（不进行运行时发现，仅使用预配置对端）.
    Static,
}

/// DDS 节点配置（D6：移除 multicast_addr / peers 字段）.
///
/// Mock 实现不使用网络地址；真实 FFI 启用时由 `configs/dds.toml` 解析。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DdsConfig {
    /// DDS 域 ID（同域内节点可互相发现）.
    pub domain_id: u32,
    /// 发现策略.
    pub discovery: DiscoveryPolicy,
    /// 绑定网卡名（None 表示自动选择）.
    pub interface: Option<String>,
}

impl Default for DdsConfig {
    fn default() -> Self {
        Self {
            domain_id: 0,
            discovery: DiscoveryPolicy::Multicast,
            interface: None,
        }
    }
}

impl DdsConfig {
    /// 构造指定域 ID 与发现策略的配置（interface 默认 None）.
    pub fn new(domain_id: u32, discovery: DiscoveryPolicy) -> Self {
        Self {
            domain_id,
            discovery,
            interface: None,
        }
    }
}

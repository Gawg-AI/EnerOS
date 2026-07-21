//! 端口角色、端口状态与端口结构（gPTP / IEEE 802.1AS）.
//!
//! - [`PortRole`] — BMCA 选举出的端口角色（Master/Slave/Passive/Disabled）
//! - [`PortState`] — 端口协议状态机状态
//! - [`Port`] — 单个 TSN 端口的描述符

use core::fmt;

use crate::clock::MacAddr;

/// 端口角色（BMCA 选举结果）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortRole {
    Master,
    Slave,
    Passive,
    Disabled,
}

impl fmt::Display for PortRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PortRole::Master => write!(f, "Master"),
            PortRole::Slave => write!(f, "Slave"),
            PortRole::Passive => write!(f, "Passive"),
            PortRole::Disabled => write!(f, "Disabled"),
        }
    }
}

/// 端口协议状态机状态.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortState {
    Initializing,
    Listening,
    Master,
    Slave,
    Passive,
}

impl fmt::Display for PortState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PortState::Initializing => write!(f, "Initializing"),
            PortState::Listening => write!(f, "Listening"),
            PortState::Master => write!(f, "Master"),
            PortState::Slave => write!(f, "Slave"),
            PortState::Passive => write!(f, "Passive"),
        }
    }
}

/// 单个 TSN 端口描述符.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Port {
    /// 端口号（PTP portNumber）.
    pub port_id: u16,
    /// 当前端口角色.
    pub role: PortRole,
    /// 当前端口状态.
    pub state: PortState,
    /// 端口 MAC 地址.
    pub mac: MacAddr,
    /// 是否支持硬件时间戳.
    pub hw_timestamping: bool,
}

impl Port {
    /// 构造新端口：`role = Disabled`，`state = Initializing`.
    pub fn new(port_id: u16, mac: MacAddr, hw_timestamping: bool) -> Self {
        Self {
            port_id,
            role: PortRole::Disabled,
            state: PortState::Initializing,
            mac,
            hw_timestamping,
        }
    }
}

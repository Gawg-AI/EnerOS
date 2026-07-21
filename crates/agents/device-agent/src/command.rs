//! 命令源 — DeviceCommand + CommandSource trait + MockCommandSource.

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

/// 设备控制命令（D7：本地定义，匹配蓝图语义）.
#[derive(Debug, Clone)]
pub struct DeviceCommand {
    /// 目标设备名.
    pub target_device: String,
    /// 功率设定值（kW）.
    pub power_kw: f64,
    /// 命令有效期（ms）.
    pub ttl_ms: u64,
    /// 命令时间戳（ms，外部提供）.
    pub timestamp_ms: u64,
}

/// 命令源 trait（D4：本地定义，替代 ControlBusReader）.
pub trait CommandSource {
    /// 非阻塞读取命令，无命令时返回 None.
    fn try_read(&mut self) -> Option<DeviceCommand>;
}

/// Mock 命令源（VecDeque-backed）.
pub struct MockCommandSource {
    /// 命令队列.
    commands: VecDeque<DeviceCommand>,
}

impl MockCommandSource {
    /// 创建空命令源.
    pub fn new() -> Self {
        Self {
            commands: VecDeque::new(),
        }
    }

    /// 预加载命令.
    pub fn with_commands(commands: Vec<DeviceCommand>) -> Self {
        Self {
            commands: commands.into_iter().collect(),
        }
    }

    /// 追加命令.
    pub fn push(&mut self, cmd: DeviceCommand) {
        self.commands.push_back(cmd);
    }
}

impl CommandSource for MockCommandSource {
    fn try_read(&mut self) -> Option<DeviceCommand> {
        self.commands.pop_front()
    }
}

impl Default for MockCommandSource {
    fn default() -> Self {
        Self::new()
    }
}

//! 命令下发抽象（D6：本地定义，避免跨子系统内核依赖）.

use alloc::string::String;
use alloc::vec::Vec;

use crate::error::DualBrainError;

/// 下发命令（D7：字段匹配蓝图语义）.
///
/// 蓝图 `ControlCommand` 字段差异大（`cmd_id: [u8;16]` / `DeviceId` / `setpoint: f32`），
/// 本类型匹配蓝图双脑链路的语义：目标设备 / 功率 / TTL / 时间戳。
#[derive(Debug, Clone)]
pub struct DispatchCommand {
    /// 目标设备（如 `"pcs"`）.
    pub target_device: String,
    /// 功率设定（kW）.
    pub power_kw: f64,
    /// 命令有效期（ms）.
    pub ttl_ms: u32,
    /// 时间戳（ms）.
    pub timestamp: u64,
}

/// 命令下发 trait（D6）.
///
/// `ControlBusHandle` 不存在；v0.22.0 `command_send` 是全局函数需 ring 初始化。
/// 本 trait 保持 crate 自包含可测试。
pub trait CommandSink {
    /// 写入命令到下发通道.
    fn write(&mut self, cmd: DispatchCommand) -> Result<(), DualBrainError>;
}

/// Mock 命令 sink（收集命令用于测试）.
pub struct MockCommandSink {
    commands: Vec<DispatchCommand>,
}

impl MockCommandSink {
    /// 创建空 sink.
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
        }
    }

    /// 返回已收集的命令引用.
    pub fn commands(&self) -> &[DispatchCommand] {
        &self.commands
    }
}

impl Default for MockCommandSink {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandSink for MockCommandSink {
    fn write(&mut self, cmd: DispatchCommand) -> Result<(), DualBrainError> {
        self.commands.push(cmd);
        Ok(())
    }
}

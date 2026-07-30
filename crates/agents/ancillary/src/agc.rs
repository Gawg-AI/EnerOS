//! AGC (自动发电控制) 命令处理
//!
//! 接收调度命令、执行并确认。

use alloc::vec::Vec;

use crate::types::{AgcAck, AgcCommand};
use crate::AncillaryError;

/// AGC 命令处理器
#[derive(Debug, Clone)]
pub struct AgcHandler {
    /// 待执行命令队列
    pub pending_commands: Vec<AgcCommand>,
    /// 已确认列表
    pub acks: Vec<AgcAck>,
}

impl AgcHandler {
    /// 创建空的 AGC 处理器
    pub fn new() -> Self {
        Self {
            pending_commands: Vec::new(),
            acks: Vec::new(),
        }
    }

    /// 接收 AGC 调度命令，加入待执行队列
    pub fn recv_command(&mut self, cmd: AgcCommand) {
        self.pending_commands.push(cmd);
    }

    /// 执行指定命令并生成确认
    ///
    /// 从待执行队列中移除对应命令，生成 `AgcAck`。
    pub fn execute_command(
        &mut self,
        cmd_id: u64,
        actual_kw: f64,
    ) -> Result<AgcAck, AncillaryError> {
        let pos = self
            .pending_commands
            .iter()
            .position(|c| c.cmd_id == cmd_id)
            .ok_or(AncillaryError::CommandNotFound)?;

        let cmd = self.pending_commands.remove(pos);
        let ack = AgcAck {
            cmd_id: cmd.cmd_id,
            actual_kw,
            ack_at: cmd.received_at + cmd.deadline_ms as u64,
        };
        self.acks.push(ack.clone());
        Ok(ack)
    }

    /// 是否有待执行命令
    pub fn has_pending(&self) -> bool {
        !self.pending_commands.is_empty()
    }
}

impl Default for AgcHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_cmd(id: u64) -> AgcCommand {
        AgcCommand {
            cmd_id: id,
            target_power_kw: 300.0,
            deadline_ms: 4000,
            received_at: 10000,
        }
    }

    #[test]
    fn test_recv_and_has_pending() {
        let mut handler = AgcHandler::new();
        assert!(!handler.has_pending());
        handler.recv_command(sample_cmd(1));
        assert!(handler.has_pending());
    }

    #[test]
    fn test_execute_command_success() {
        let mut handler = AgcHandler::new();
        handler.recv_command(sample_cmd(1));
        handler.recv_command(sample_cmd(2));

        let ack = handler.execute_command(1, 295.0).unwrap();
        assert_eq!(ack.cmd_id, 1);
        assert!((ack.actual_kw - 295.0).abs() < f64::EPSILON);
        assert_eq!(ack.ack_at, 14000);

        // 命令 1 已移除，仅剩命令 2
        assert!(handler.has_pending());
        assert_eq!(handler.pending_commands.len(), 1);
        assert_eq!(handler.acks.len(), 1);
    }

    #[test]
    fn test_execute_command_not_found() {
        let mut handler = AgcHandler::new();
        handler.recv_command(sample_cmd(1));
        let result = handler.execute_command(99, 100.0);
        assert_eq!(result.unwrap_err(), AncillaryError::CommandNotFound);
    }

    #[test]
    fn test_execute_all_commands() {
        let mut handler = AgcHandler::new();
        handler.recv_command(sample_cmd(1));
        handler.recv_command(sample_cmd(2));

        handler.execute_command(1, 100.0).unwrap();
        handler.execute_command(2, 200.0).unwrap();

        assert!(!handler.has_pending());
        assert_eq!(handler.acks.len(), 2);
    }
}

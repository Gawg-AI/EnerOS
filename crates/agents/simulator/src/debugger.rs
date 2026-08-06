//! GDB 调试器接口 (workspace v0.192.0).
//!
//! 提供与 QEMU GDB stub 的调试交互接口：
//! 断点管理、执行控制、寄存器/内存查看。
//!
//! 实际网络连接需要 `std` feature；核心状态机 no_std 兼容。

use alloc::vec::Vec;

use crate::SimError;

/// 调试事件.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugEvent {
    /// 命中断点.
    BreakpointHit(u32),
    /// 单步完成.
    StepComplete,
    /// 程序退出.
    ProgramExit,
    /// 程序继续执行.
    Continued,
    /// 错误.
    Error,
}

/// 断点.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Breakpoint {
    /// 断点 ID.
    pub id: u32,
    /// 地址.
    pub addr: u64,
    /// 是否启用.
    pub enabled: bool,
    /// 命中次数.
    pub hit_count: u32,
}

/// 寄存器状态（AArch64 通用寄存器）.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RegisterState {
    /// 通用寄存器 x0-x30.
    pub x: [u64; 31],
    /// 栈指针 SP.
    pub sp: u64,
    /// 程序计数器 PC.
    pub pc: u64,
    /// 程序状态寄存器 PSTATE.
    pub pstate: u32,
}

/// 调试器连接状态.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebuggerState {
    /// 未连接.
    Disconnected,
    /// 已连接.
    Connected,
    /// 目标运行中.
    Running,
    /// 目标已停止（可调试）.
    Stopped,
}

/// GDB 调试器.
///
/// 管理与 QEMU GDB stub 的调试会话。
#[derive(Debug)]
pub struct Debugger {
    /// GDB 端口.
    port: u16,
    /// 连接状态.
    state: DebuggerState,
    /// 断点列表.
    breakpoints: Vec<Breakpoint>,
    /// 下一个断点 ID.
    next_bp_id: u32,
    /// 最近事件.
    last_event: Option<DebugEvent>,
    /// 当前寄存器状态（停止时有效）.
    registers: RegisterState,
}

impl Debugger {
    /// 创建调试器（连接到指定 GDB 端口）.
    pub fn new(port: u16) -> Self {
        Self {
            port,
            state: DebuggerState::Disconnected,
            breakpoints: Vec::new(),
            next_bp_id: 1,
            last_event: None,
            registers: RegisterState::default(),
        }
    }

    /// 获取 GDB 端口.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// 获取连接状态.
    pub fn state(&self) -> DebuggerState {
        self.state
    }

    /// 获取最近事件.
    pub fn last_event(&self) -> Option<DebugEvent> {
        self.last_event
    }

    /// 模拟连接（测试用，不实际建立 TCP 连接）.
    pub fn mock_connect(&mut self) -> Result<(), SimError> {
        self.state = DebuggerState::Connected;
        Ok(())
    }

    /// 连接调试器.
    ///
    /// 在 `std` feature 下实际建立 TCP 连接到 QEMU GDB stub。
    pub fn connect(&mut self) -> Result<(), SimError> {
        #[cfg(feature = "std")]
        {
            use std::net::TcpStream;
            let addr = alloc::format!("127.0.0.1:{}", self.port);
            let _stream = TcpStream::connect(&addr).map_err(|_| SimError::DebuggerError)?;
        }
        self.state = DebuggerState::Connected;
        Ok(())
    }

    /// 断开连接.
    pub fn disconnect(&mut self) {
        self.state = DebuggerState::Disconnected;
    }

    /// 添加断点.
    pub fn add_breakpoint(&mut self, addr: u64) -> Result<u32, SimError> {
        if self.state == DebuggerState::Disconnected {
            return Err(SimError::DebuggerError);
        }
        // 检查重复
        if self.breakpoints.iter().any(|bp| bp.addr == addr) {
            return Err(SimError::DebuggerError);
        }
        let id = self.next_bp_id;
        self.next_bp_id += 1;
        self.breakpoints.push(Breakpoint {
            id,
            addr,
            enabled: true,
            hit_count: 0,
        });
        Ok(id)
    }

    /// 移除断点.
    pub fn remove_breakpoint(&mut self, id: u32) -> Result<(), SimError> {
        let idx = self
            .breakpoints
            .iter()
            .position(|bp| bp.id == id)
            .ok_or(SimError::DebuggerError)?;
        self.breakpoints.remove(idx);
        Ok(())
    }

    /// 启用/禁用断点.
    pub fn set_breakpoint_enabled(&mut self, id: u32, enabled: bool) -> Result<(), SimError> {
        let bp = self
            .breakpoints
            .iter_mut()
            .find(|bp| bp.id == id)
            .ok_or(SimError::DebuggerError)?;
        bp.enabled = enabled;
        Ok(())
    }

    /// 列出所有断点.
    pub fn list_breakpoints(&self) -> &[Breakpoint] {
        &self.breakpoints
    }

    /// 继续执行.
    pub fn continue_exec(&mut self) -> Result<(), SimError> {
        if self.state != DebuggerState::Stopped && self.state != DebuggerState::Connected {
            return Err(SimError::DebuggerError);
        }
        self.state = DebuggerState::Running;
        self.last_event = Some(DebugEvent::Continued);
        Ok(())
    }

    /// 单步执行.
    pub fn step(&mut self) -> Result<(), SimError> {
        if self.state != DebuggerState::Stopped && self.state != DebuggerState::Connected {
            return Err(SimError::DebuggerError);
        }
        // 单步后立即停止
        self.state = DebuggerState::Stopped;
        self.last_event = Some(DebugEvent::StepComplete);
        Ok(())
    }

    /// 模拟断点命中（测试用）.
    pub fn mock_breakpoint_hit(&mut self, bp_id: u32) -> Result<(), SimError> {
        let bp = self
            .breakpoints
            .iter_mut()
            .find(|bp| bp.id == bp_id)
            .ok_or(SimError::DebuggerError)?;
        bp.hit_count += 1;
        self.state = DebuggerState::Stopped;
        self.last_event = Some(DebugEvent::BreakpointHit(bp_id));
        Ok(())
    }

    /// 模拟程序退出（测试用）.
    pub fn mock_program_exit(&mut self) {
        self.state = DebuggerState::Stopped;
        self.last_event = Some(DebugEvent::ProgramExit);
    }

    /// 读取寄存器状态.
    pub fn read_registers(&self) -> Result<&RegisterState, SimError> {
        if self.state != DebuggerState::Stopped {
            return Err(SimError::DebuggerError);
        }
        Ok(&self.registers)
    }

    /// 设置寄存器状态（测试/模拟用）.
    pub fn set_registers(&mut self, regs: RegisterState) {
        self.registers = regs;
    }

    /// 读取内存（模拟）.
    pub fn read_memory(&self, _addr: u64, len: usize) -> Result<Vec<u8>, SimError> {
        if self.state != DebuggerState::Stopped {
            return Err(SimError::DebuggerError);
        }
        // 模拟返回零填充
        Ok(alloc::vec![0u8; len])
    }

    /// 获取调用栈回溯（模拟）.
    pub fn backtrace(&self) -> Result<Vec<u64>, SimError> {
        if self.state != DebuggerState::Stopped {
            return Err(SimError::DebuggerError);
        }
        // 模拟返回 PC 作为唯一帧
        Ok(alloc::vec![self.registers.pc])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_debugger_creation() {
        let dbg = Debugger::new(1234);
        assert_eq!(dbg.port(), 1234);
        assert_eq!(dbg.state(), DebuggerState::Disconnected);
        assert!(dbg.last_event().is_none());
    }

    #[test]
    fn test_connect_disconnect() {
        let mut dbg = Debugger::new(1234);
        dbg.mock_connect().unwrap();
        assert_eq!(dbg.state(), DebuggerState::Connected);
        dbg.disconnect();
        assert_eq!(dbg.state(), DebuggerState::Disconnected);
    }

    #[test]
    fn test_breakpoint_management() {
        let mut dbg = Debugger::new(1234);
        dbg.mock_connect().unwrap();

        let bp1 = dbg.add_breakpoint(0x8000_0000).unwrap();
        let bp2 = dbg.add_breakpoint(0x8000_1000).unwrap();
        assert_eq!(bp1, 1);
        assert_eq!(bp2, 2);
        assert_eq!(dbg.list_breakpoints().len(), 2);

        // 重复地址
        assert!(dbg.add_breakpoint(0x8000_0000).is_err());

        // 禁用
        dbg.set_breakpoint_enabled(bp1, false).unwrap();
        assert!(!dbg.list_breakpoints()[0].enabled);

        // 移除
        dbg.remove_breakpoint(bp1).unwrap();
        assert_eq!(dbg.list_breakpoints().len(), 1);

        // 移除不存在的
        assert!(dbg.remove_breakpoint(99).is_err());
    }

    #[test]
    fn test_breakpoint_requires_connection() {
        let mut dbg = Debugger::new(1234);
        // 未连接时不能添加断点
        assert!(dbg.add_breakpoint(0x1000).is_err());
    }

    #[test]
    fn test_execution_control() {
        let mut dbg = Debugger::new(1234);
        dbg.mock_connect().unwrap();

        // 继续执行
        dbg.continue_exec().unwrap();
        assert_eq!(dbg.state(), DebuggerState::Running);
        assert_eq!(dbg.last_event(), Some(DebugEvent::Continued));

        // 运行中不能再次 continue
        assert!(dbg.continue_exec().is_err());
    }

    #[test]
    fn test_step_execution() {
        let mut dbg = Debugger::new(1234);
        dbg.mock_connect().unwrap();

        dbg.step().unwrap();
        assert_eq!(dbg.state(), DebuggerState::Stopped);
        assert_eq!(dbg.last_event(), Some(DebugEvent::StepComplete));
    }

    #[test]
    fn test_breakpoint_hit() {
        let mut dbg = Debugger::new(1234);
        dbg.mock_connect().unwrap();
        let bp_id = dbg.add_breakpoint(0x8000_0000).unwrap();

        dbg.continue_exec().unwrap();
        dbg.mock_breakpoint_hit(bp_id).unwrap();

        assert_eq!(dbg.state(), DebuggerState::Stopped);
        assert_eq!(dbg.last_event(), Some(DebugEvent::BreakpointHit(bp_id)));
        assert_eq!(dbg.list_breakpoints()[0].hit_count, 1);
    }

    #[test]
    fn test_read_registers_requires_stop() {
        let mut dbg = Debugger::new(1234);
        dbg.mock_connect().unwrap();

        // Connected 状态下不能读寄存器（需要 Stopped）
        assert!(dbg.read_registers().is_err());

        dbg.step().unwrap(); // 进入 Stopped
        let regs = dbg.read_registers().unwrap();
        assert_eq!(regs.pc, 0);
    }

    #[test]
    fn test_memory_and_backtrace() {
        let mut dbg = Debugger::new(1234);
        dbg.mock_connect().unwrap();
        dbg.step().unwrap();

        let mem = dbg.read_memory(0x4000_0000, 64).unwrap();
        assert_eq!(mem.len(), 64);

        let bt = dbg.backtrace().unwrap();
        assert_eq!(bt.len(), 1);
    }

    #[test]
    fn test_program_exit() {
        let mut dbg = Debugger::new(1234);
        dbg.mock_connect().unwrap();
        dbg.mock_program_exit();
        assert_eq!(dbg.last_event(), Some(DebugEvent::ProgramExit));
    }
}

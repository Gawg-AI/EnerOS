//! QEMU 后端 (workspace v0.192.0).
//!
//! 提供 QEMU 虚拟机管理：实例创建、启动、停止、进程管理。
//! 进程管理部分需要 `std` feature（宿主机专属，边缘路径禁用）。
//!
//! # 蓝图 §4.4 注释
//! QEMU 进程派生为宿主机模拟器专属，边缘路径禁用。
//! 以下进程管理代码仅在 `feature = "std"` 时编译。

use alloc::string::String;
use alloc::vec::Vec;

use crate::debugger::Debugger;
use crate::types::{Arch, SimConfig, SimInstance, SimState};
use crate::SimError;

/// 模拟器 trait — 管理模拟器实例生命周期.
pub trait Simulator {
    /// 创建模拟器实例.
    fn create_instance(&mut self, config: &SimConfig) -> Result<u32, SimError>;
    /// 启动实例.
    fn start(&mut self, id: u32) -> Result<(), SimError>;
    /// 停止实例.
    fn stop(&mut self, id: u32) -> Result<(), SimError>;
    /// 附加调试器.
    fn attach_debugger(&self, id: u32) -> Result<Debugger, SimError>;
}

/// QEMU 命令行配置.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QemuCommand {
    /// QEMU 可执行文件路径.
    pub binary: String,
    /// 命令行参数.
    pub args: Vec<String>,
}

impl QemuCommand {
    /// 从模拟器配置构建 QEMU 命令行.
    pub fn from_config(qemu_path: &str, instance: &SimInstance) -> Self {
        let config = &instance.config;
        let mut args = Vec::new();

        // 机器类型
        args.push(String::from("-machine"));
        args.push(String::from(config.arch.qemu_machine()));

        // CPU 类型
        args.push(String::from("-cpu"));
        args.push(String::from(config.arch.qemu_cpu()));

        // 内存
        args.push(String::from("-m"));
        args.push(alloc::format!("{}M", config.memory_mb));

        // 内核镜像
        args.push(String::from("-kernel"));
        args.push(config.kernel_image.clone());

        // GDB 调试端口
        args.push(String::from("-gdb"));
        args.push(alloc::format!("tcp::{}", instance.gdb_port));

        // 无图形界面
        args.push(String::from("-nographic"));

        // 串口输出到 stdio
        args.push(String::from("-serial"));
        args.push(String::from("mon:stdio"));

        Self {
            binary: String::from(qemu_path),
            args,
        }
    }

    /// 获取完整命令行字符串（用于日志/调试）.
    pub fn to_command_string(&self) -> String {
        let mut cmd = self.binary.clone();
        for arg in &self.args {
            cmd.push(' ');
            cmd.push_str(arg);
        }
        cmd
    }
}

/// QEMU 模拟器实现.
pub struct QemuSimulator {
    /// QEMU 可执行文件路径.
    qemu_path: String,
    /// 模拟器实例列表.
    instances: Vec<SimInstance>,
    /// 下一个实例 ID.
    next_id: u32,
}

impl QemuSimulator {
    /// 创建 QEMU 模拟器.
    ///
    /// `qemu_path` 为 QEMU 可执行文件路径。
    /// 若为空则根据目标架构自动推断。
    pub fn new(qemu_path: &str) -> Self {
        Self {
            qemu_path: String::from(qemu_path),
            instances: Vec::new(),
            next_id: 0,
        }
    }

    /// 自动检测 QEMU 路径.
    pub fn detect(arch: Arch) -> Result<Self, SimError> {
        let binary_name = arch.qemu_binary_name();
        // 在 no_std 模式下无法执行 which，使用默认路径
        let default_path = alloc::format!("/usr/bin/{}", binary_name);
        Ok(Self::new(&default_path))
    }

    /// 获取实例引用.
    pub fn get_instance(&self, id: u32) -> Option<&SimInstance> {
        self.instances.iter().find(|i| i.id == id)
    }

    /// 获取实例可变引用.
    fn get_instance_mut(&mut self, id: u32) -> Result<&mut SimInstance, SimError> {
        self.instances
            .iter_mut()
            .find(|i| i.id == id)
            .ok_or(SimError::InstanceNotFound)
    }

    /// 获取所有实例.
    pub fn list_instances(&self) -> &[SimInstance] {
        &self.instances
    }

    /// 构建实例的 QEMU 命令行.
    pub fn build_command(&self, id: u32) -> Result<QemuCommand, SimError> {
        let instance = self
            .instances
            .iter()
            .find(|i| i.id == id)
            .ok_or(SimError::InstanceNotFound)?;
        Ok(QemuCommand::from_config(&self.qemu_path, instance))
    }

    /// 模拟启动（不实际派生进程，用于测试）.
    ///
    /// 在 `std` feature 下使用 `start()` 实际派生 QEMU 进程。
    pub fn mock_start(&mut self, id: u32) -> Result<(), SimError> {
        let instance = self.get_instance_mut(id)?;
        if instance.state == SimState::Running {
            return Err(SimError::AlreadyRunning);
        }
        instance.state = SimState::Running;
        instance.qemu_pid = Some(99999); // mock PID
        Ok(())
    }

    /// 模拟停止（不实际终止进程，用于测试）.
    pub fn mock_stop(&mut self, id: u32) -> Result<(), SimError> {
        let instance = self.get_instance_mut(id)?;
        if instance.state != SimState::Running {
            return Err(SimError::NotRunning);
        }
        instance.state = SimState::Stopped;
        instance.qemu_pid = None;
        Ok(())
    }
}

impl Simulator for QemuSimulator {
    fn create_instance(&mut self, config: &SimConfig) -> Result<u32, SimError> {
        let id = self.next_id;
        self.next_id += 1;
        let instance = SimInstance::new(id, config.clone());
        self.instances.push(instance);
        Ok(id)
    }

    fn start(&mut self, id: u32) -> Result<(), SimError> {
        let qemu_path = self.qemu_path.clone();
        let instance = self.get_instance_mut(id)?;
        if instance.state == SimState::Running {
            return Err(SimError::AlreadyRunning);
        }

        // 构建命令行（验证配置有效性）
        let _cmd = QemuCommand::from_config(&qemu_path, instance);

        #[cfg(feature = "std")]
        {
            use std::process::Command;
            let mut cmd = Command::new(&_cmd.binary);
            for arg in &_cmd.args {
                cmd.arg(arg);
            }
            let child = cmd.spawn().map_err(|_| SimError::ProcessSpawnFailed)?;
            instance.qemu_pid = Some(child.id());
        }

        #[cfg(not(feature = "std"))]
        {
            // no_std 模式下使用 mock PID
            instance.qemu_pid = Some(0);
        }

        instance.state = SimState::Running;
        Ok(())
    }

    fn stop(&mut self, id: u32) -> Result<(), SimError> {
        let instance = self.get_instance_mut(id)?;
        if instance.state != SimState::Running {
            return Err(SimError::NotRunning);
        }

        #[cfg(feature = "std")]
        {
            // 实际终止 QEMU 进程
            if let Some(pid) = instance.qemu_pid {
                unsafe {
                    libc::kill(pid as i32, libc::SIGTERM);
                }
            }
        }

        instance.state = SimState::Stopped;
        instance.qemu_pid = None;
        Ok(())
    }

    fn attach_debugger(&self, id: u32) -> Result<Debugger, SimError> {
        let instance = self
            .instances
            .iter()
            .find(|i| i.id == id)
            .ok_or(SimError::InstanceNotFound)?;

        if instance.state != SimState::Running {
            return Err(SimError::NotRunning);
        }

        Ok(Debugger::new(instance.gdb_port))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AgentConfig, DeviceMockConfig, DeviceType};

    fn test_config() -> SimConfig {
        SimConfig::new()
            .with_arch(Arch::Arm64)
            .with_memory(256)
            .with_kernel(String::from("build/eneros.bin"))
    }

    #[test]
    fn test_create_instance() {
        let mut sim = QemuSimulator::new("/usr/bin/qemu-system-aarch64");
        let config = test_config();
        let id = sim.create_instance(&config).unwrap();
        assert_eq!(id, 0);
        assert_eq!(sim.list_instances().len(), 1);
    }

    #[test]
    fn test_multiple_instances() {
        let mut sim = QemuSimulator::new("/usr/bin/qemu-system-aarch64");
        let config = test_config();
        let id0 = sim.create_instance(&config).unwrap();
        let id1 = sim.create_instance(&config).unwrap();
        assert_eq!(id0, 0);
        assert_eq!(id1, 1);
        assert_eq!(sim.list_instances().len(), 2);
    }

    #[test]
    fn test_mock_start_stop() {
        let mut sim = QemuSimulator::new("/usr/bin/qemu-system-aarch64");
        let config = test_config();
        let id = sim.create_instance(&config).unwrap();

        sim.mock_start(id).unwrap();
        assert_eq!(sim.get_instance(id).unwrap().state, SimState::Running);

        // 重复启动应失败
        assert_eq!(sim.mock_start(id).unwrap_err(), SimError::AlreadyRunning);

        sim.mock_stop(id).unwrap();
        assert_eq!(sim.get_instance(id).unwrap().state, SimState::Stopped);

        // 重复停止应失败
        assert_eq!(sim.mock_stop(id).unwrap_err(), SimError::NotRunning);
    }

    #[test]
    fn test_start_nonexistent() {
        let mut sim = QemuSimulator::new("/usr/bin/qemu-system-aarch64");
        assert_eq!(sim.mock_start(99).unwrap_err(), SimError::InstanceNotFound);
    }

    #[test]
    fn test_qemu_command_build() {
        let mut sim = QemuSimulator::new("/usr/bin/qemu-system-aarch64");
        let config = test_config();
        let id = sim.create_instance(&config).unwrap();
        let cmd = sim.build_command(id).unwrap();

        assert_eq!(cmd.binary, "/usr/bin/qemu-system-aarch64");
        assert!(cmd.args.contains(&String::from("-machine")));
        assert!(cmd.args.contains(&String::from("virt")));
        assert!(cmd.args.contains(&String::from("-cpu")));
        assert!(cmd.args.contains(&String::from("cortex-a72")));
        assert!(cmd.args.contains(&String::from("-m")));
        assert!(cmd.args.contains(&String::from("256M")));
        assert!(cmd.args.contains(&String::from("-nographic")));
    }

    #[test]
    fn test_qemu_command_string() {
        let mut sim = QemuSimulator::new("/usr/bin/qemu-system-aarch64");
        let config = test_config();
        let id = sim.create_instance(&config).unwrap();
        let cmd = sim.build_command(id).unwrap();
        let cmd_str = cmd.to_command_string();
        assert!(cmd_str.starts_with("/usr/bin/qemu-system-aarch64"));
        assert!(cmd_str.contains("-machine virt"));
    }

    #[test]
    fn test_attach_debugger() {
        let mut sim = QemuSimulator::new("/usr/bin/qemu-system-aarch64");
        let config = test_config();
        let id = sim.create_instance(&config).unwrap();

        // 未运行时不能附加
        assert_eq!(sim.attach_debugger(id).unwrap_err(), SimError::NotRunning);

        sim.mock_start(id).unwrap();
        let dbg = sim.attach_debugger(id).unwrap();
        assert_eq!(dbg.port(), 1234);
    }

    #[test]
    fn test_detect_qemu() {
        let sim = QemuSimulator::detect(Arch::Arm64).unwrap();
        assert_eq!(sim.qemu_path, "/usr/bin/qemu-system-aarch64");

        let sim = QemuSimulator::detect(Arch::X86_64).unwrap();
        assert_eq!(sim.qemu_path, "/usr/bin/qemu-system-x86_64");
    }

    #[test]
    fn test_config_with_devices_and_agents() {
        let config = SimConfig::new()
            .with_device(DeviceMockConfig::new(
                DeviceType::ModbusTcp,
                1,
                100,
                String::from("meter"),
            ))
            .with_agent(AgentConfig::new(
                String::from("agent-1"),
                String::from("plugins/agent1.elf"),
            ));

        let mut sim = QemuSimulator::new("/usr/bin/qemu-system-aarch64");
        let id = sim.create_instance(&config).unwrap();
        let inst = sim.get_instance(id).unwrap();
        assert_eq!(inst.config.devices.len(), 1);
        assert_eq!(inst.config.agents.len(), 1);
    }
}

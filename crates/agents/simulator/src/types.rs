//! 模拟器核心类型定义 (v0.167.0).
//!
//! 定义模拟器配置、实例、架构、设备类型等核心数据结构。
//! 所有类型 no_std 兼容（仅依赖 alloc）。

use alloc::string::String;
use alloc::vec::Vec;

/// 目标架构.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    /// ARM64 (AArch64) — 默认目标.
    Arm64,
    /// x86_64 — 宿主机模拟.
    X86_64,
}

impl Arch {
    /// 返回 QEMU 系统模拟器可执行文件名.
    pub fn qemu_binary_name(&self) -> &'static str {
        match self {
            Arch::Arm64 => "qemu-system-aarch64",
            Arch::X86_64 => "qemu-system-x86_64",
        }
    }

    /// 返回 QEMU -machine 参数.
    pub fn qemu_machine(&self) -> &'static str {
        match self {
            Arch::Arm64 => "virt",
            Arch::X86_64 => "q35",
        }
    }

    /// 返回 QEMU -cpu 参数.
    pub fn qemu_cpu(&self) -> &'static str {
        match self {
            Arch::Arm64 => "cortex-a72",
            Arch::X86_64 => "qemu64",
        }
    }
}

/// 模拟设备类型.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceType {
    /// Modbus RTU 从站设备.
    ModbusRtu,
    /// Modbus TCP 设备.
    ModbusTcp,
    /// IEC 60870-5-104 设备.
    Iec104,
    /// CAN 总线设备.
    Can,
    /// 块存储设备.
    Storage,
    /// 网络接口设备.
    Network,
}

/// 模拟设备配置.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceMockConfig {
    /// 设备类型.
    pub device_type: DeviceType,
    /// 设备地址（Modbus 从站地址 / CAN ID 等）.
    pub address: u8,
    /// 寄存器数量（Modbus）或数据点数量.
    pub registers: u16,
    /// 设备名称（可选标识）.
    pub name: String,
}

impl DeviceMockConfig {
    /// 创建新的设备模拟配置.
    pub fn new(device_type: DeviceType, address: u8, registers: u16, name: String) -> Self {
        Self {
            device_type,
            address,
            registers,
            name,
        }
    }
}

/// Agent 配置.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentConfig {
    /// Agent 名称.
    pub name: String,
    /// Agent 插件路径.
    pub plugin_path: String,
}

impl AgentConfig {
    /// 创建新的 Agent 配置.
    pub fn new(name: String, plugin_path: String) -> Self {
        Self { name, plugin_path }
    }
}

/// 模拟器配置.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimConfig {
    /// 目标架构.
    pub arch: Arch,
    /// 内存大小（MB）.
    pub memory_mb: usize,
    /// 模拟设备列表.
    pub devices: Vec<DeviceMockConfig>,
    /// Agent 配置列表.
    pub agents: Vec<AgentConfig>,
    /// 内核镜像路径.
    pub kernel_image: String,
    /// GDB 基础端口（实例端口 = base + instance_id）.
    pub gdb_base_port: u16,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            arch: Arch::Arm64,
            memory_mb: 256,
            devices: Vec::new(),
            agents: Vec::new(),
            kernel_image: String::from("build/eneros.bin"),
            gdb_base_port: 1234,
        }
    }
}

impl SimConfig {
    /// 创建默认配置.
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置目标架构.
    pub fn with_arch(mut self, arch: Arch) -> Self {
        self.arch = arch;
        self
    }

    /// 设置内存大小.
    pub fn with_memory(mut self, mb: usize) -> Self {
        self.memory_mb = mb;
        self
    }

    /// 添加模拟设备.
    pub fn with_device(mut self, device: DeviceMockConfig) -> Self {
        self.devices.push(device);
        self
    }

    /// 添加 Agent.
    pub fn with_agent(mut self, agent: AgentConfig) -> Self {
        self.agents.push(agent);
        self
    }

    /// 设置内核镜像路径.
    pub fn with_kernel(mut self, path: String) -> Self {
        self.kernel_image = path;
        self
    }
}

/// 模拟器实例状态.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimState {
    /// 已创建（未启动）.
    Created,
    /// 运行中.
    Running,
    /// 已停止.
    Stopped,
    /// 已崩溃.
    Crashed,
}

/// 模拟器实例.
#[derive(Debug, Clone)]
pub struct SimInstance {
    /// 实例 ID.
    pub id: u32,
    /// 实例配置.
    pub config: SimConfig,
    /// 当前状态.
    pub state: SimState,
    /// QEMU 进程 PID（std feature 下可用）.
    pub qemu_pid: Option<u32>,
    /// GDB 端口.
    pub gdb_port: u16,
}

impl SimInstance {
    /// 创建新的模拟器实例.
    pub fn new(id: u32, config: SimConfig) -> Self {
        let gdb_port = config.gdb_base_port + id as u16;
        Self {
            id,
            config,
            state: SimState::Created,
            qemu_pid: None,
            gdb_port,
        }
    }

    /// 实例是否正在运行.
    pub fn is_running(&self) -> bool {
        self.state == SimState::Running
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arch_qemu_params() {
        assert_eq!(Arch::Arm64.qemu_binary_name(), "qemu-system-aarch64");
        assert_eq!(Arch::Arm64.qemu_machine(), "virt");
        assert_eq!(Arch::Arm64.qemu_cpu(), "cortex-a72");
        assert_eq!(Arch::X86_64.qemu_binary_name(), "qemu-system-x86_64");
        assert_eq!(Arch::X86_64.qemu_machine(), "q35");
    }

    #[test]
    fn test_sim_config_builder() {
        let config = SimConfig::new()
            .with_arch(Arch::Arm64)
            .with_memory(512)
            .with_device(DeviceMockConfig::new(
                DeviceType::ModbusRtu,
                1,
                100,
                String::from("meter-1"),
            ))
            .with_agent(AgentConfig::new(
                String::from("test-agent"),
                String::from("plugins/test.elf"),
            ))
            .with_kernel(String::from("build/eneros.bin"));

        assert_eq!(config.arch, Arch::Arm64);
        assert_eq!(config.memory_mb, 512);
        assert_eq!(config.devices.len(), 1);
        assert_eq!(config.agents.len(), 1);
        assert_eq!(config.devices[0].device_type, DeviceType::ModbusRtu);
    }

    #[test]
    fn test_sim_instance_creation() {
        let config = SimConfig::new().with_memory(128);
        let instance = SimInstance::new(0, config);
        assert_eq!(instance.id, 0);
        assert_eq!(instance.state, SimState::Created);
        assert_eq!(instance.gdb_port, 1234);
        assert!(!instance.is_running());
    }

    #[test]
    fn test_sim_instance_gdb_port_offset() {
        let config = SimConfig::new();
        let inst0 = SimInstance::new(0, config.clone());
        let inst1 = SimInstance::new(1, config.clone());
        let inst2 = SimInstance::new(2, config);
        assert_eq!(inst0.gdb_port, 1234);
        assert_eq!(inst1.gdb_port, 1235);
        assert_eq!(inst2.gdb_port, 1236);
    }

    #[test]
    fn test_device_type_enum() {
        let devices = [
            DeviceType::ModbusRtu,
            DeviceType::ModbusTcp,
            DeviceType::Iec104,
            DeviceType::Can,
            DeviceType::Storage,
            DeviceType::Network,
        ];
        assert_eq!(devices.len(), 6);
        assert_ne!(DeviceType::ModbusRtu, DeviceType::ModbusTcp);
    }

    #[test]
    fn test_default_config() {
        let config = SimConfig::default();
        assert_eq!(config.arch, Arch::Arm64);
        assert_eq!(config.memory_mb, 256);
        assert_eq!(config.gdb_base_port, 1234);
        assert!(config.devices.is_empty());
        assert!(config.agents.is_empty());
    }
}

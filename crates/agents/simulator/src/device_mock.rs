//! 设备模拟器 (workspace v0.192.0).
//!
//! 模拟 Modbus RTU/TCP、IEC 104、CAN、存储、网络等设备，
//! 让 Agent 测试无需真实硬件。设备行为与真实设备一致。

use alloc::string::String;
use alloc::vec::Vec;

use crate::types::{DeviceMockConfig, DeviceType};
use crate::SimError;

/// 设备模拟 trait — 所有模拟设备的统一接口.
pub trait DeviceMock {
    /// 初始化设备.
    fn init(&mut self) -> Result<(), SimError>;
    /// 读取寄存器.
    fn read_register(&self, addr: u16) -> Result<u16, SimError>;
    /// 写入寄存器.
    fn write_register(&mut self, addr: u16, value: u16) -> Result<(), SimError>;
    /// 轮询设备状态（返回是否有新数据）.
    fn poll(&mut self) -> Result<bool, SimError>;
    /// 设备类型.
    fn device_type(&self) -> DeviceType;
    /// 设备名称.
    fn name(&self) -> &str;
}

/// Modbus RTU 从站模拟.
///
/// 模拟标准 Modbus RTU 从站设备，支持保持寄存器 (0x03) 读写。
#[derive(Debug, Clone)]
pub struct ModbusRtuMock {
    /// 从站地址.
    pub slave_addr: u8,
    /// 设备名称.
    pub name: String,
    /// 保持寄存器组.
    registers: Vec<u16>,
    /// 是否已初始化.
    initialized: bool,
}

impl ModbusRtuMock {
    /// 创建 Modbus RTU 模拟设备.
    pub fn new(config: &DeviceMockConfig) -> Self {
        Self {
            slave_addr: config.address,
            name: config.name.clone(),
            registers: alloc::vec![0u16; config.registers as usize],
            initialized: false,
        }
    }

    /// 获取寄存器总数.
    pub fn register_count(&self) -> usize {
        self.registers.len()
    }
}

impl DeviceMock for ModbusRtuMock {
    fn init(&mut self) -> Result<(), SimError> {
        self.initialized = true;
        Ok(())
    }

    fn read_register(&self, addr: u16) -> Result<u16, SimError> {
        if !self.initialized {
            return Err(SimError::DeviceMockError);
        }
        self.registers
            .get(addr as usize)
            .copied()
            .ok_or(SimError::DeviceMockError)
    }

    fn write_register(&mut self, addr: u16, value: u16) -> Result<(), SimError> {
        if !self.initialized {
            return Err(SimError::DeviceMockError);
        }
        if let Some(reg) = self.registers.get_mut(addr as usize) {
            *reg = value;
            Ok(())
        } else {
            Err(SimError::DeviceMockError)
        }
    }

    fn poll(&mut self) -> Result<bool, SimError> {
        Ok(self.initialized)
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::ModbusRtu
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// Modbus TCP 设备模拟.
///
/// 与 RTU 类似但使用 TCP 传输，额外维护连接状态。
#[derive(Debug, Clone)]
pub struct ModbusTcpMock {
    /// 单元标识符.
    pub unit_id: u8,
    /// 设备名称.
    pub name: String,
    /// 保持寄存器组.
    registers: Vec<u16>,
    /// 是否已初始化.
    initialized: bool,
    /// 模拟连接是否建立.
    connected: bool,
}

impl ModbusTcpMock {
    /// 创建 Modbus TCP 模拟设备.
    pub fn new(config: &DeviceMockConfig) -> Self {
        Self {
            unit_id: config.address,
            name: config.name.clone(),
            registers: alloc::vec![0u16; config.registers as usize],
            initialized: false,
            connected: false,
        }
    }

    /// 模拟建立 TCP 连接.
    pub fn connect(&mut self) -> Result<(), SimError> {
        if !self.initialized {
            return Err(SimError::DeviceMockError);
        }
        self.connected = true;
        Ok(())
    }

    /// 模拟断开连接.
    pub fn disconnect(&mut self) {
        self.connected = false;
    }

    /// 是否已连接.
    pub fn is_connected(&self) -> bool {
        self.connected
    }
}

impl DeviceMock for ModbusTcpMock {
    fn init(&mut self) -> Result<(), SimError> {
        self.initialized = true;
        Ok(())
    }

    fn read_register(&self, addr: u16) -> Result<u16, SimError> {
        if !self.initialized || !self.connected {
            return Err(SimError::DeviceMockError);
        }
        self.registers
            .get(addr as usize)
            .copied()
            .ok_or(SimError::DeviceMockError)
    }

    fn write_register(&mut self, addr: u16, value: u16) -> Result<(), SimError> {
        if !self.initialized || !self.connected {
            return Err(SimError::DeviceMockError);
        }
        if let Some(reg) = self.registers.get_mut(addr as usize) {
            *reg = value;
            Ok(())
        } else {
            Err(SimError::DeviceMockError)
        }
    }

    fn poll(&mut self) -> Result<bool, SimError> {
        Ok(self.initialized && self.connected)
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::ModbusTcp
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// IEC 60870-5-104 设备模拟.
///
/// 模拟 IEC 104 遥测/遥信数据源。
#[derive(Debug, Clone)]
pub struct Iec104Mock {
    /// 公共地址 (ASDU 地址).
    pub common_address: u8,
    /// 设备名称.
    pub name: String,
    /// 遥测值 (模拟量).
    telemetry: Vec<i16>,
    /// 遥信值 (开关量).
    teleindication: Vec<bool>,
    /// 是否已初始化.
    initialized: bool,
}

impl Iec104Mock {
    /// 创建 IEC 104 模拟设备.
    pub fn new(config: &DeviceMockConfig) -> Self {
        let count = config.registers as usize;
        Self {
            common_address: config.address,
            name: config.name.clone(),
            telemetry: alloc::vec![0i16; count],
            teleindication: alloc::vec![false; count],
            initialized: false,
        }
    }

    /// 设置遥测值.
    pub fn set_telemetry(&mut self, index: usize, value: i16) -> Result<(), SimError> {
        if let Some(v) = self.telemetry.get_mut(index) {
            *v = value;
            Ok(())
        } else {
            Err(SimError::DeviceMockError)
        }
    }

    /// 读取遥测值.
    pub fn get_telemetry(&self, index: usize) -> Result<i16, SimError> {
        self.telemetry
            .get(index)
            .copied()
            .ok_or(SimError::DeviceMockError)
    }

    /// 设置遥信值.
    pub fn set_teleindication(&mut self, index: usize, value: bool) -> Result<(), SimError> {
        if let Some(v) = self.teleindication.get_mut(index) {
            *v = value;
            Ok(())
        } else {
            Err(SimError::DeviceMockError)
        }
    }

    /// 读取遥信值.
    pub fn get_teleindication(&self, index: usize) -> Result<bool, SimError> {
        self.teleindication
            .get(index)
            .copied()
            .ok_or(SimError::DeviceMockError)
    }
}

impl DeviceMock for Iec104Mock {
    fn init(&mut self) -> Result<(), SimError> {
        self.initialized = true;
        Ok(())
    }

    fn read_register(&self, addr: u16) -> Result<u16, SimError> {
        if !self.initialized {
            return Err(SimError::DeviceMockError);
        }
        // 将遥测值映射为 u16
        self.telemetry
            .get(addr as usize)
            .map(|&v| v as u16)
            .ok_or(SimError::DeviceMockError)
    }

    fn write_register(&mut self, addr: u16, value: u16) -> Result<(), SimError> {
        if !self.initialized {
            return Err(SimError::DeviceMockError);
        }
        if let Some(v) = self.telemetry.get_mut(addr as usize) {
            *v = value as i16;
            Ok(())
        } else {
            Err(SimError::DeviceMockError)
        }
    }

    fn poll(&mut self) -> Result<bool, SimError> {
        Ok(self.initialized)
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Iec104
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// CAN 总线设备模拟.
///
/// 模拟 CAN 帧收发。
#[derive(Debug, Clone)]
pub struct CanMock {
    /// CAN 设备 ID.
    pub can_id: u8,
    /// 设备名称.
    pub name: String,
    /// 接收帧缓冲.
    rx_buffer: Vec<CanFrame>,
    /// 发送帧缓冲.
    tx_buffer: Vec<CanFrame>,
    /// 是否已初始化.
    initialized: bool,
}

/// CAN 帧.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CanFrame {
    /// CAN ID (11-bit 标准帧).
    pub id: u32,
    /// 数据长度 (0-8).
    pub dlc: u8,
    /// 数据.
    pub data: [u8; 8],
}

impl CanMock {
    /// 创建 CAN 模拟设备.
    pub fn new(config: &DeviceMockConfig) -> Self {
        Self {
            can_id: config.address,
            name: config.name.clone(),
            rx_buffer: Vec::new(),
            tx_buffer: Vec::new(),
            initialized: false,
        }
    }

    /// 注入接收帧（模拟外部设备发送）.
    pub fn inject_rx_frame(&mut self, frame: CanFrame) -> Result<(), SimError> {
        if !self.initialized {
            return Err(SimError::DeviceMockError);
        }
        self.rx_buffer.push(frame);
        Ok(())
    }

    /// 读取接收帧.
    pub fn read_rx_frame(&mut self) -> Option<CanFrame> {
        if self.rx_buffer.is_empty() {
            None
        } else {
            Some(self.rx_buffer.remove(0))
        }
    }

    /// 发送帧.
    pub fn send_frame(&mut self, frame: CanFrame) -> Result<(), SimError> {
        if !self.initialized {
            return Err(SimError::DeviceMockError);
        }
        self.tx_buffer.push(frame);
        Ok(())
    }

    /// 获取已发送帧数.
    pub fn tx_count(&self) -> usize {
        self.tx_buffer.len()
    }
}

impl DeviceMock for CanMock {
    fn init(&mut self) -> Result<(), SimError> {
        self.initialized = true;
        Ok(())
    }

    fn read_register(&self, addr: u16) -> Result<u16, SimError> {
        if !self.initialized {
            return Err(SimError::DeviceMockError);
        }
        // CAN 设备寄存器映射：0=rx_count, 1=tx_count
        match addr {
            0 => Ok(self.rx_buffer.len() as u16),
            1 => Ok(self.tx_buffer.len() as u16),
            _ => Err(SimError::DeviceMockError),
        }
    }

    fn write_register(&mut self, _addr: u16, _value: u16) -> Result<(), SimError> {
        // CAN 设备不支持寄存器写入
        Err(SimError::DeviceMockError)
    }

    fn poll(&mut self) -> Result<bool, SimError> {
        Ok(!self.rx_buffer.is_empty())
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Can
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// 块存储设备模拟.
#[derive(Debug, Clone)]
pub struct StorageMock {
    /// 设备名称.
    pub name: String,
    /// 存储块（每块 512 字节）.
    blocks: Vec<[u8; 512]>,
    /// 是否已初始化.
    initialized: bool,
}

impl StorageMock {
    /// 创建存储模拟设备.
    ///
    /// `block_count` 为模拟块数。
    pub fn new(config: &DeviceMockConfig) -> Self {
        let block_count = (config.registers as usize).max(1);
        Self {
            name: config.name.clone(),
            blocks: alloc::vec![[0u8; 512]; block_count],
            initialized: false,
        }
    }

    /// 读取块.
    pub fn read_block(&self, block: usize) -> Result<&[u8; 512], SimError> {
        if !self.initialized {
            return Err(SimError::DeviceMockError);
        }
        self.blocks.get(block).ok_or(SimError::DeviceMockError)
    }

    /// 写入块.
    pub fn write_block(&mut self, block: usize, data: &[u8; 512]) -> Result<(), SimError> {
        if !self.initialized {
            return Err(SimError::DeviceMockError);
        }
        if let Some(b) = self.blocks.get_mut(block) {
            *b = *data;
            Ok(())
        } else {
            Err(SimError::DeviceMockError)
        }
    }

    /// 获取块数.
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }
}

impl DeviceMock for StorageMock {
    fn init(&mut self) -> Result<(), SimError> {
        self.initialized = true;
        Ok(())
    }

    fn read_register(&self, addr: u16) -> Result<u16, SimError> {
        if !self.initialized {
            return Err(SimError::DeviceMockError);
        }
        // 寄存器 0 = 块数
        if addr == 0 {
            Ok(self.blocks.len() as u16)
        } else {
            Err(SimError::DeviceMockError)
        }
    }

    fn write_register(&mut self, _addr: u16, _value: u16) -> Result<(), SimError> {
        Err(SimError::DeviceMockError)
    }

    fn poll(&mut self) -> Result<bool, SimError> {
        Ok(self.initialized)
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Storage
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// 网络接口设备模拟.
#[derive(Debug, Clone)]
pub struct NetworkMock {
    /// 设备名称.
    pub name: String,
    /// 模拟 IP 地址（四段）.
    pub ip: [u8; 4],
    /// 是否链路连通.
    link_up: bool,
    /// 接收包计数.
    rx_count: u32,
    /// 发送包计数.
    tx_count: u32,
    /// 是否已初始化.
    initialized: bool,
}

impl NetworkMock {
    /// 创建网络模拟设备.
    pub fn new(config: &DeviceMockConfig) -> Self {
        Self {
            name: config.name.clone(),
            ip: [192, 168, 1, config.address],
            link_up: false,
            rx_count: 0,
            tx_count: 0,
            initialized: false,
        }
    }

    /// 模拟链路连通.
    pub fn set_link_up(&mut self, up: bool) {
        self.link_up = up;
    }

    /// 链路是否连通.
    pub fn is_link_up(&self) -> bool {
        self.link_up
    }

    /// 模拟接收数据包.
    pub fn simulate_rx(&mut self, count: u32) {
        self.rx_count += count;
    }

    /// 模拟发送数据包.
    pub fn simulate_tx(&mut self, count: u32) {
        self.tx_count += count;
    }

    /// 获取接收包计数.
    pub fn rx_count(&self) -> u32 {
        self.rx_count
    }

    /// 获取发送包计数.
    pub fn tx_count(&self) -> u32 {
        self.tx_count
    }
}

impl DeviceMock for NetworkMock {
    fn init(&mut self) -> Result<(), SimError> {
        self.initialized = true;
        self.link_up = true;
        Ok(())
    }

    fn read_register(&self, addr: u16) -> Result<u16, SimError> {
        if !self.initialized {
            return Err(SimError::DeviceMockError);
        }
        match addr {
            0 => Ok(self.rx_count as u16),
            1 => Ok(self.tx_count as u16),
            2 => Ok(if self.link_up { 1 } else { 0 }),
            _ => Err(SimError::DeviceMockError),
        }
    }

    fn write_register(&mut self, _addr: u16, _value: u16) -> Result<(), SimError> {
        Err(SimError::DeviceMockError)
    }

    fn poll(&mut self) -> Result<bool, SimError> {
        Ok(self.initialized && self.link_up)
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Network
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// 设备模拟注册表 — 管理所有模拟设备.
pub struct DeviceMockRegistry {
    /// Modbus RTU 设备.
    pub modbus_rtu: Vec<ModbusRtuMock>,
    /// Modbus TCP 设备.
    pub modbus_tcp: Vec<ModbusTcpMock>,
    /// IEC 104 设备.
    pub iec104: Vec<Iec104Mock>,
    /// CAN 设备.
    pub can: Vec<CanMock>,
    /// 存储设备.
    pub storage: Vec<StorageMock>,
    /// 网络设备.
    pub network: Vec<NetworkMock>,
}

impl DeviceMockRegistry {
    /// 创建空注册表.
    pub fn new() -> Self {
        Self {
            modbus_rtu: Vec::new(),
            modbus_tcp: Vec::new(),
            iec104: Vec::new(),
            can: Vec::new(),
            storage: Vec::new(),
            network: Vec::new(),
        }
    }

    /// 从配置列表创建注册表并初始化所有设备.
    pub fn from_configs(configs: &[DeviceMockConfig]) -> Result<Self, SimError> {
        let mut registry = Self::new();
        for config in configs {
            registry.add_device(config)?;
        }
        registry.init_all()?;
        Ok(registry)
    }

    /// 添加设备.
    pub fn add_device(&mut self, config: &DeviceMockConfig) -> Result<(), SimError> {
        match config.device_type {
            DeviceType::ModbusRtu => self.modbus_rtu.push(ModbusRtuMock::new(config)),
            DeviceType::ModbusTcp => self.modbus_tcp.push(ModbusTcpMock::new(config)),
            DeviceType::Iec104 => self.iec104.push(Iec104Mock::new(config)),
            DeviceType::Can => self.can.push(CanMock::new(config)),
            DeviceType::Storage => self.storage.push(StorageMock::new(config)),
            DeviceType::Network => self.network.push(NetworkMock::new(config)),
        }
        Ok(())
    }

    /// 初始化所有设备.
    pub fn init_all(&mut self) -> Result<(), SimError> {
        for dev in &mut self.modbus_rtu {
            dev.init()?;
        }
        for dev in &mut self.modbus_tcp {
            dev.init()?;
        }
        for dev in &mut self.iec104 {
            dev.init()?;
        }
        for dev in &mut self.can {
            dev.init()?;
        }
        for dev in &mut self.storage {
            dev.init()?;
        }
        for dev in &mut self.network {
            dev.init()?;
        }
        Ok(())
    }

    /// 获取设备总数.
    pub fn total_count(&self) -> usize {
        self.modbus_rtu.len()
            + self.modbus_tcp.len()
            + self.iec104.len()
            + self.can.len()
            + self.storage.len()
            + self.network.len()
    }
}

impl Default for DeviceMockRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rtu_config() -> DeviceMockConfig {
        DeviceMockConfig::new(DeviceType::ModbusRtu, 1, 100, String::from("rtu-1"))
    }

    #[test]
    fn test_modbus_rtu_mock() {
        let config = rtu_config();
        let mut dev = ModbusRtuMock::new(&config);
        assert_eq!(dev.register_count(), 100);

        // 未初始化时读取应失败
        assert!(dev.read_register(0).is_err());

        dev.init().unwrap();
        assert_eq!(dev.read_register(0).unwrap(), 0);

        dev.write_register(10, 1234).unwrap();
        assert_eq!(dev.read_register(10).unwrap(), 1234);

        // 越界
        assert!(dev.read_register(200).is_err());
    }

    #[test]
    fn test_modbus_tcp_mock() {
        let config = DeviceMockConfig::new(DeviceType::ModbusTcp, 1, 50, String::from("tcp-1"));
        let mut dev = ModbusTcpMock::new(&config);
        dev.init().unwrap();

        // 未连接时读取应失败
        assert!(dev.read_register(0).is_err());

        dev.connect().unwrap();
        assert!(dev.is_connected());
        assert_eq!(dev.read_register(0).unwrap(), 0);

        dev.write_register(5, 999).unwrap();
        assert_eq!(dev.read_register(5).unwrap(), 999);

        dev.disconnect();
        assert!(!dev.is_connected());
        assert!(dev.read_register(0).is_err());
    }

    #[test]
    fn test_iec104_mock() {
        let config = DeviceMockConfig::new(DeviceType::Iec104, 1, 20, String::from("iec-1"));
        let mut dev = Iec104Mock::new(&config);
        dev.init().unwrap();

        dev.set_telemetry(0, -100).unwrap();
        assert_eq!(dev.get_telemetry(0).unwrap(), -100);

        dev.set_teleindication(0, true).unwrap();
        assert!(dev.get_teleindication(0).unwrap());

        // 越界
        assert!(dev.set_telemetry(50, 0).is_err());
    }

    #[test]
    fn test_can_mock() {
        let config = DeviceMockConfig::new(DeviceType::Can, 0x10, 0, String::from("can-1"));
        let mut dev = CanMock::new(&config);
        dev.init().unwrap();

        let frame = CanFrame {
            id: 0x123,
            dlc: 4,
            data: [0x01, 0x02, 0x03, 0x04, 0, 0, 0, 0],
        };

        dev.inject_rx_frame(frame).unwrap();
        assert_eq!(dev.read_register(0).unwrap(), 1); // rx_count

        let rx = dev.read_rx_frame().unwrap();
        assert_eq!(rx.id, 0x123);
        assert_eq!(rx.dlc, 4);

        dev.send_frame(frame).unwrap();
        assert_eq!(dev.tx_count(), 1);
    }

    #[test]
    fn test_storage_mock() {
        let config = DeviceMockConfig::new(DeviceType::Storage, 0, 16, String::from("disk-1"));
        let mut dev = StorageMock::new(&config);
        dev.init().unwrap();
        assert_eq!(dev.block_count(), 16);

        let data = [0xABu8; 512];
        dev.write_block(0, &data).unwrap();
        assert_eq!(dev.read_block(0).unwrap()[0], 0xAB);

        // 越界
        assert!(dev.read_block(20).is_err());
    }

    #[test]
    fn test_network_mock() {
        let config = DeviceMockConfig::new(DeviceType::Network, 100, 0, String::from("eth-0"));
        let mut dev = NetworkMock::new(&config);
        assert_eq!(dev.ip, [192, 168, 1, 100]);

        dev.init().unwrap();
        assert!(dev.is_link_up());

        dev.simulate_rx(10);
        dev.simulate_tx(5);
        assert_eq!(dev.rx_count(), 10);
        assert_eq!(dev.tx_count(), 5);
        assert_eq!(dev.read_register(0).unwrap(), 10);
        assert_eq!(dev.read_register(1).unwrap(), 5);
    }

    #[test]
    fn test_registry_from_configs() {
        let configs = alloc::vec![
            DeviceMockConfig::new(DeviceType::ModbusRtu, 1, 100, String::from("rtu")),
            DeviceMockConfig::new(DeviceType::ModbusTcp, 2, 50, String::from("tcp")),
            DeviceMockConfig::new(DeviceType::Iec104, 1, 20, String::from("iec")),
            DeviceMockConfig::new(DeviceType::Can, 0x10, 0, String::from("can")),
            DeviceMockConfig::new(DeviceType::Storage, 0, 8, String::from("disk")),
            DeviceMockConfig::new(DeviceType::Network, 1, 0, String::from("eth")),
        ];

        let registry = DeviceMockRegistry::from_configs(&configs).unwrap();
        assert_eq!(registry.total_count(), 6);
        assert_eq!(registry.modbus_rtu.len(), 1);
        assert_eq!(registry.modbus_tcp.len(), 1);
        assert_eq!(registry.iec104.len(), 1);
        assert_eq!(registry.can.len(), 1);
        assert_eq!(registry.storage.len(), 1);
        assert_eq!(registry.network.len(), 1);
    }

    #[test]
    fn test_registry_empty() {
        let registry = DeviceMockRegistry::new();
        assert_eq!(registry.total_count(), 0);
    }
}

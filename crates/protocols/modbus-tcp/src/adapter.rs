//! Modbus TCP 协议适配器（PTC-002）— 接入 protocol-abstract 共享契约套件。
//!
//! 实现 [`eneros_protocol_abstract::adapter::ProtocolAdapter`] 与
//! [`eneros_protocol_abstract::access::PointAccess`]，使 modbus-tcp 与
//! modbus-rtu/iec104 跑同一共享契约测试套件：
//! - 泛型传输 `T: TcpTransport`（测试用 `MockTcpTransport`）；
//! - 连接复用（PTC-002）：同设备复用 TCP 连接，换设备才重连；
//! - [`eneros_modbus_rtu::point::RegToPoint`] 作为点表。

use alloc::vec::Vec;

use eneros_protocol_abstract::access::{PointAccess, PointAccessExt};
use eneros_protocol_abstract::adapter::{AdapterState, ProtocolAdapter};
use eneros_protocol_abstract::config::{AdapterConfig, ProtocolType};
use eneros_protocol_abstract::error::ProtocolError;
use eneros_upa_model::{
    DataPoint, DataSource, DeviceId, PointId, PointQuality, PointType, PointValue,
};

use crate::device::TcpDevice;
use crate::error::ModbusTcpError;
use crate::master::ModbusTcpMaster;
use crate::transport::TcpTransport;

/// Modbus TCP 协议适配器（泛型传输层）。
pub struct ModbusTcpAdapter<T: TcpTransport> {
    /// 底层 TCP 传输（测试 mock 或真实 socket）。
    transport: T,
    /// 目标设备。
    device: TcpDevice,
    /// 失败重试次数。
    retry_count: u8,
    /// 点表映射（point_id ↔ 寄存器）。
    mappings: Vec<eneros_modbus_rtu::point::RegToPoint>,
    /// 关联设备 ID。
    device_id: DeviceId,
    /// 适配器状态。
    state: AdapterState,
}

impl<T: TcpTransport> ModbusTcpAdapter<T> {
    /// 创建适配器（初始状态 `Uninitialized`）。
    pub fn new(transport: T, device: TcpDevice, retry_count: u8) -> Self {
        Self {
            transport,
            device,
            retry_count,
            mappings: Vec::new(),
            device_id: 0,
            state: AdapterState::Uninitialized,
        }
    }

    /// 设置点表映射（链式）。
    pub fn with_mappings(mut self, mappings: Vec<eneros_modbus_rtu::point::RegToPoint>) -> Self {
        self.mappings = mappings;
        self
    }

    /// 查找点映射。
    fn find_mapping(&self, point_id: PointId) -> Option<&eneros_modbus_rtu::point::RegToPoint> {
        self.mappings.iter().find(|m| m.point_id == point_id)
    }

    fn read_point_impl(&mut self, point_id: PointId) -> Result<DataPoint, ProtocolError> {
        if self.state != AdapterState::Running && self.state != AdapterState::Initialized {
            return Err(ProtocolError::ProtocolNotStarted);
        }
        let (reg_addr, word_count, name) = {
            let mapping = self
                .find_mapping(point_id)
                .ok_or(ProtocolError::PointNotFound)?;
            if mapping.access == eneros_modbus_rtu::point::AccessMode::WriteOnly {
                return Err(ProtocolError::Unsupported);
            }
            (
                mapping.reg_addr,
                mapping.word_count(),
                mapping.point_name.clone(),
            )
        };
        let mut master = ModbusTcpMaster::new(&mut self.transport, self.retry_count);
        let regs = master
            .read_holding_registers(&self.device, reg_addr, word_count)
            .map_err(modbus_tcp_to_protocol)?;
        let eng = self
            .find_mapping(point_id)
            .ok_or(ProtocolError::PointNotFound)?
            .convert(&regs)
            .map_err(|_| ProtocolError::ReadFailed)?;
        Ok(DataPoint {
            point_id,
            device_id: self.device_id,
            name,
            description: None,
            point_type: PointType::Analog,
            value: PointValue::Float(eng),
            quality: PointQuality::good(),
            timestamp_ms: 0,
            source: DataSource::ModbusTcp,
            unit: None,
        })
    }

    fn write_point_impl(
        &mut self,
        point_id: PointId,
        value: PointValue,
    ) -> Result<(), ProtocolError> {
        if self.state != AdapterState::Running && self.state != AdapterState::Initialized {
            return Err(ProtocolError::ProtocolNotStarted);
        }
        let (reg_addr, access) = {
            let mapping = self
                .find_mapping(point_id)
                .ok_or(ProtocolError::PointNotFound)?;
            (mapping.reg_addr, mapping.access)
        };
        if access == eneros_modbus_rtu::point::AccessMode::ReadOnly {
            return Err(ProtocolError::Unsupported);
        }
        // v0.218.0（B2-01）：写值经 scale 逆变换（替代 `f as u16` 直截断）。
        let mapping = self
            .find_mapping(point_id)
            .ok_or(ProtocolError::PointNotFound)?;
        let raw: u16 = match value {
            PointValue::Float(f) => mapping
                .inverse_convert(f)
                .map_err(ModbusTcpError::Modbus)
                .map_err(modbus_tcp_to_protocol)?,
            PointValue::Int(i) => mapping
                .inverse_convert(i as f64)
                .map_err(ModbusTcpError::Modbus)
                .map_err(modbus_tcp_to_protocol)?,
            PointValue::Enum(e) => e,
            _ => return Err(ProtocolError::Unsupported),
        };
        let mut master = ModbusTcpMaster::new(&mut self.transport, self.retry_count);
        master
            .write_single_register(&self.device, reg_addr, raw)
            .map_err(modbus_tcp_to_protocol)
    }
}

impl<T: TcpTransport> PointAccess for ModbusTcpAdapter<T> {
    fn read_point(&mut self, point_id: PointId) -> Result<DataPoint, ProtocolError> {
        self.read_point_impl(point_id)
    }

    fn write_point(&mut self, point_id: PointId, value: PointValue) -> Result<(), ProtocolError> {
        self.write_point_impl(point_id, value)
    }

    fn protocol_type(&self) -> ProtocolType {
        ProtocolType::ModbusTcp
    }
}

impl<T: TcpTransport> PointAccessExt for ModbusTcpAdapter<T> {
    fn read_device_points(&mut self, device_id: DeviceId) -> Result<Vec<DataPoint>, ProtocolError> {
        if device_id != self.device_id {
            return Err(ProtocolError::PointNotFound);
        }
        let ids: Vec<PointId> = self.mappings.iter().map(|m| m.point_id).collect();
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            out.push(self.read_point_impl(id)?);
        }
        Ok(out)
    }
}

impl<T: TcpTransport> ProtocolAdapter for ModbusTcpAdapter<T> {
    fn init(&mut self, config: &AdapterConfig) -> Result<(), ProtocolError> {
        if config.protocol_type != ProtocolType::ModbusTcp {
            return Err(ProtocolError::InvalidConfig);
        }
        let device = config
            .device_configs
            .first()
            .ok_or(ProtocolError::InvalidConfig)?;
        self.device_id = device.device_id;
        self.state = AdapterState::Initialized;
        Ok(())
    }

    fn start(&mut self) -> Result<(), ProtocolError> {
        self.state = AdapterState::Running;
        Ok(())
    }

    fn stop(&mut self) -> Result<(), ProtocolError> {
        self.state = AdapterState::Stopped;
        Ok(())
    }

    fn poll(&mut self, _now_ms: u64) -> Result<(), ProtocolError> {
        // 请求/响应式主站；轮询由上层按点表驱动。
        Ok(())
    }

    fn state(&self) -> AdapterState {
        self.state
    }
}

/// ModbusTcpError → ProtocolError 映射。
fn modbus_tcp_to_protocol(e: ModbusTcpError) -> ProtocolError {
    use ModbusTcpError::*;
    match e {
        ConnectionFailed => ProtocolError::ReadFailed,
        Modbus(_) => ProtocolError::ReadFailed,
        Timeout => ProtocolError::ReadFailed,
        _ => ProtocolError::ReadFailed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockTcpTransport;
    use eneros_modbus_rtu::point::RegToPoint;
    use eneros_protocol_abstract::config::{AdapterConfig, DeviceConfig, ProtocolType};
    use eneros_protocol_abstract::contract::run_adapter_contract;

    /// 构造 MBAP + 读保持寄存器响应帧（txn 须匹配请求事务 ID）。
    fn read_resp(txn: u16, regs: &[u16]) -> Vec<u8> {
        let mut f = Vec::new();
        f.extend_from_slice(&txn.to_be_bytes());
        f.extend_from_slice(&0x0000u16.to_be_bytes()); // protocol id
        let data_len = 3 + regs.len() * 2;
        f.extend_from_slice(&(data_len as u16).to_be_bytes());
        f.push(1u8); // unit id
        f.push(3u8); // read holding registers
        f.push((regs.len() * 2) as u8); // byte count
        for r in regs {
            f.extend_from_slice(&r.to_be_bytes());
        }
        f
    }

    /// 构造写单寄存器响应帧（txn 须匹配请求事务 ID）。
    fn write_resp(txn: u16, addr: u16, value: u16) -> Vec<u8> {
        let mut f = Vec::new();
        f.extend_from_slice(&txn.to_be_bytes());
        f.extend_from_slice(&0x0000u16.to_be_bytes());
        f.extend_from_slice(&6u16.to_be_bytes()); // 2 unit + 1 func + 2 addr + 2 val
        f.push(1u8);
        f.push(6u8);
        f.extend_from_slice(&addr.to_be_bytes());
        f.extend_from_slice(&value.to_be_bytes());
        f
    }

    fn simple_adapter_config(protocol: ProtocolType, device_id: DeviceId) -> AdapterConfig {
        AdapterConfig {
            name: String::from("tcp-test"),
            protocol_type: protocol,
            device_configs: alloc::vec![DeviceConfig {
                device_id,
                name: String::from("d1"),
                address: eneros_protocol_abstract::ProtocolAddress::Modbus {
                    slave_addr: 1,
                    reg_addr: 100,
                    func_code: 3,
                },
            }],
        }
    }

    fn make_device(unit: u8) -> TcpDevice {
        TcpDevice {
            ip: [127, 0, 0, 1],
            port: 502,
            unit_id: unit,
            timeout_ms: 1000,
        }
    }

    fn contract_adapter() -> ModbusTcpAdapter<MockTcpTransport> {
        ModbusTcpAdapter::new(MockTcpTransport::new(), make_device(1), 0).with_mappings(
            alloc::vec![
                RegToPoint {
                    point_id: 1,
                    point_name: String::from("p1"),
                    slave_addr: 1,
                    reg_addr: 100,
                    data_type: eneros_modbus_rtu::point::ModbusDataType::U16,
                    scale: 1.0,
                    offset: 0.0,
                    access: eneros_modbus_rtu::point::AccessMode::ReadWrite,
                },
                RegToPoint {
                    point_id: 2,
                    point_name: String::from("p2"),
                    slave_addr: 1,
                    reg_addr: 200,
                    data_type: eneros_modbus_rtu::point::ModbusDataType::U16,
                    scale: 1.0,
                    offset: 0.0,
                    access: eneros_modbus_rtu::point::AccessMode::ReadWrite,
                },
            ],
        )
    }

    /// PTC-002：modbus-tcp 与 mock/modbus-rtu/iec104 跑同一 seam 契约测试套件。
    #[test]
    fn modbus_tcp_adapter_passes_shared_contract() {
        // 契约执行顺序（每次请求新建 master → 事务 ID 从 0 起 → txn=0 匹配）：
        // read_point(1) → read_point(2) → read_points(1,2) → write_point →
        // read_device_points(2 点)
        let mut adapter = contract_adapter();
        adapter.transport.push_response(read_resp(0, &[100u16]));
        adapter.transport.push_response(read_resp(0, &[200u16]));
        adapter.transport.push_response(read_resp(0, &[100u16]));
        adapter.transport.push_response(read_resp(0, &[200u16]));
        adapter.transport.push_response(write_resp(0, 100, 2));
        adapter.transport.push_response(read_resp(0, &[100u16]));
        adapter.transport.push_response(read_resp(0, &[200u16]));

        let cfg = simple_adapter_config(ProtocolType::ModbusTcp, 7);
        run_adapter_contract(&mut adapter, &cfg, ProtocolType::ModbusTcp, 7, &[1, 2]);
    }

    /// PTC-002：连接复用——同设备连续两次请求只 connect 一次。
    #[test]
    fn connection_reused_same_device() {
        let mut mock = MockTcpTransport::new();
        let dev = make_device(1);
        mock.push_response(read_resp(0, &[42u16]));
        mock.push_response(read_resp(1, &[43u16]));
        let mut master = ModbusTcpMaster::new(&mut mock, 0);
        master.read_holding_registers(&dev, 0, 1).unwrap();
        master.read_holding_registers(&dev, 0, 1).unwrap();
        let stats = master.stats().clone();
        assert_eq!(
            stats.reconnect_count, 1,
            "same device must reuse connection"
        );
        assert_eq!(mock.connect_calls().len(), 1);
    }
}

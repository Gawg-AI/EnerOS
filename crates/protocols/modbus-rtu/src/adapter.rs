//! Modbus RTU 协议适配器 — 第一个真实 `ProtocolAdapter` 实现（ARCH-P01）。
//!
//! 实现 [`eneros_protocol_abstract::adapter::ProtocolAdapter`]，使
//! `protocol-abstract` seam 从 hypothetical 变为 real（07 蓝图 C2）：
//! - 通过 [`RtuTransport`] + [`ModbusRtuMaster`] 执行真实 Modbus RTU 帧交换；
//! - [`crate::point::RegToPoint`] 作为点表（point_id ↔ 从站/寄存器/数据类型）；
//! - 读点带质量位（good）与时间戳（0 注入，由上层 poll 语义决定）；
//! - 与 `protocol-abstract::contract` 共享契约测试套件（mock 与真实适配器同一套件）。
//!
//! 传输层为泛型 `T: RtuTransport`（静态组合，ADR-0025）：测试用
//! `MockRtuTransport`，生产用真实 RS485 驱动。

use alloc::vec::Vec;

use eneros_protocol_abstract::access::{PointAccess, PointAccessExt};
use eneros_protocol_abstract::adapter::{AdapterState, ProtocolAdapter};
use eneros_protocol_abstract::config::{AdapterConfig, ProtocolType};
use eneros_protocol_abstract::error::ProtocolError;
use eneros_upa_model::{
    DataPoint, DataSource, DeviceId, PointId, PointQuality, PointType, PointValue,
};

use crate::error::ModbusError;
use crate::master::{ModbusRtuMaster, RtuTransport};
use crate::point::{AccessMode, PointMapping, RegToPoint};

/// Modbus RTU 协议适配器（泛型传输层）。
pub struct ModbusRtuAdapter<T: RtuTransport> {
    /// 底层 RTU 传输（真实 RS485 驱动或测试 mock）。
    transport: T,
    /// 单次接收超时（毫秒）。
    timeout_ms: u32,
    /// 失败重试次数。
    retry_count: u8,
    /// 点表映射。
    mappings: Vec<RegToPoint>,
    /// 关联设备 ID（取自 AdapterConfig::device_configs[0]）。
    device_id: DeviceId,
    /// 适配器状态。
    state: AdapterState,
}

impl<T: RtuTransport> ModbusRtuAdapter<T> {
    /// 创建适配器（初始状态 `Uninitialized`）。
    pub fn new(transport: T, timeout_ms: u32, retry_count: u8) -> Self {
        Self {
            transport,
            timeout_ms,
            retry_count,
            mappings: Vec::new(),
            device_id: 0,
            state: AdapterState::Uninitialized,
        }
    }

    /// 设置点表映射（链式）。
    pub fn with_mappings(mut self, mappings: Vec<RegToPoint>) -> Self {
        self.mappings = mappings;
        self
    }

    /// 从 `PointMapping` 设置点表（链式）。
    pub fn with_point_mapping(mut self, mapping: &PointMapping) -> Self {
        self.mappings = mapping.mappings.clone();
        self
    }

    /// 查找点映射。
    fn find_mapping(&self, point_id: PointId) -> Option<&RegToPoint> {
        self.mappings.iter().find(|m| m.point_id == point_id)
    }

    /// 转换原始寄存器值到工程值（在 master() 借用之后调用，避免借用冲突）。
    fn mapping_convert(&self, point_id: PointId, raw: &[u16]) -> Result<f64, ModbusError> {
        let mapping = self
            .find_mapping(point_id)
            .ok_or(ModbusError::UnexpectedResponse)?;
        mapping.convert(raw)
    }

    /// 创建临时主站（借用 transport，操作完成后即释放）。
    fn master(&mut self) -> ModbusRtuMaster<'_> {
        ModbusRtuMaster::new(&mut self.transport, self.timeout_ms, self.retry_count)
    }

    /// 测试专用：可变访问底层传输（预置响应/检查发送帧）。
    #[cfg(test)]
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    /// 读取单点并转换为 DataPoint。
    fn read_point_impl(&mut self, point_id: PointId) -> Result<DataPoint, ProtocolError> {
        if self.state != AdapterState::Running && self.state != AdapterState::Initialized {
            return Err(ProtocolError::ProtocolNotStarted);
        }
        let (slave_addr, reg_addr, word_count, name) = {
            let mapping = self
                .find_mapping(point_id)
                .ok_or(ProtocolError::PointNotFound)?;
            if mapping.access == AccessMode::WriteOnly {
                return Err(ProtocolError::Unsupported);
            }
            (
                mapping.slave_addr,
                mapping.reg_addr,
                mapping.word_count(),
                mapping.point_name.clone(),
            )
        };
        let raw = self
            .master()
            .read_holding_registers(slave_addr, reg_addr, word_count)
            .map_err(modbus_to_protocol)?;
        let eng = self
            .mapping_convert(point_id, &raw)
            .map_err(modbus_to_protocol)?;
        Ok(DataPoint {
            point_id,
            device_id: self.device_id,
            name,
            description: None,
            point_type: PointType::Analog,
            value: PointValue::Float(eng),
            quality: PointQuality::good(),
            timestamp_ms: 0,
            source: DataSource::ModbusRtu,
            unit: None,
        })
    }

    /// 写入单点。
    fn write_point_impl(
        &mut self,
        point_id: PointId,
        value: PointValue,
    ) -> Result<(), ProtocolError> {
        if self.state != AdapterState::Running && self.state != AdapterState::Initialized {
            return Err(ProtocolError::ProtocolNotStarted);
        }
        let (slave_addr, reg_addr, access) = {
            let mapping = self
                .find_mapping(point_id)
                .ok_or(ProtocolError::PointNotFound)?;
            (mapping.slave_addr, mapping.reg_addr, mapping.access)
        };
        if access == AccessMode::ReadOnly {
            return Err(ProtocolError::Unsupported);
        }
        // v0.218.0（B2-01）：写值经 scale 逆变换（替代 `f as u16` 直截断）。
        // 查映射获取数据类型以选择正确的逆变换。
        let mapping = self
            .find_mapping(point_id)
            .ok_or(ProtocolError::PointNotFound)?;
        let raw: u16 = match value {
            PointValue::Float(f) => mapping.inverse_convert(f).map_err(modbus_to_protocol)?,
            PointValue::Int(i) => mapping
                .inverse_convert(i as f64)
                .map_err(modbus_to_protocol)?,
            PointValue::Enum(e) => e,
            _ => return Err(ProtocolError::Unsupported),
        };
        self.master()
            .write_single_register(slave_addr, reg_addr, raw)
            .map_err(modbus_to_protocol)
    }
}

impl<T: RtuTransport> PointAccess for ModbusRtuAdapter<T> {
    fn read_point(&mut self, point_id: PointId) -> Result<DataPoint, ProtocolError> {
        self.read_point_impl(point_id)
    }

    fn write_point(&mut self, point_id: PointId, value: PointValue) -> Result<(), ProtocolError> {
        self.write_point_impl(point_id, value)
    }

    fn protocol_type(&self) -> ProtocolType {
        ProtocolType::ModbusRtu
    }
}

impl<T: RtuTransport> PointAccessExt for ModbusRtuAdapter<T> {
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

impl<T: RtuTransport> ProtocolAdapter for ModbusRtuAdapter<T> {
    fn init(&mut self, config: &AdapterConfig) -> Result<(), ProtocolError> {
        if config.protocol_type != ProtocolType::ModbusRtu {
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
        // Modbus RTU 主站为请求/响应式，无持续连接状态；轮询由上层按点表驱动。
        Ok(())
    }

    fn state(&self) -> AdapterState {
        self.state
    }
}

/// ModbusError → ProtocolError 映射。
fn modbus_to_protocol(e: ModbusError) -> ProtocolError {
    use ModbusError::*;
    match e {
        InvalidSlaveAddr | InvalidQuantity | InvalidRegisterAddr => ProtocolError::InvalidConfig,
        Exception(_)
        | MaxRetryExceeded
        | AddrMismatch
        | CrcMismatch
        | UnexpectedResponse
        | FrameTooShort
        | UnsupportedFunction
        | FuncCodeMismatch { .. }
        | Driver(_) => ProtocolError::ReadFailed,
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::String;
    use alloc::vec;

    use eneros_protocol_abstract::contract::{run_adapter_contract, simple_adapter_config};

    use super::*;
    use crate::frame::ModbusFrame;
    use crate::mock::MockRtuTransport;
    use crate::point::ModbusDataType;

    /// 构建读保持寄存器响应帧。
    fn read_resp(slave: u8, regs: &[u16]) -> Vec<u8> {
        let mut data = vec![(regs.len() * 2) as u8];
        for r in regs {
            data.extend_from_slice(&r.to_be_bytes());
        }
        ModbusFrame::new(slave, 0x03, data).encode()
    }

    /// 构建写单寄存器响应帧。
    fn write_resp(slave: u8, addr: u16, value: u16) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&addr.to_be_bytes());
        data.extend_from_slice(&value.to_be_bytes());
        ModbusFrame::new(slave, 0x06, data).encode()
    }

    /// 契约测试用适配器：2 个点（slave 1 与 slave 2）。
    fn contract_adapter() -> ModbusRtuAdapter<MockRtuTransport> {
        ModbusRtuAdapter::new(MockRtuTransport::new(), 100, 0).with_mappings(vec![
            RegToPoint {
                point_id: 1,
                point_name: String::from("voltage"),
                slave_addr: 1,
                reg_addr: 0,
                data_type: ModbusDataType::U16,
                scale: 0.1,
                offset: 0.0,
                access: AccessMode::ReadWrite,
            },
            RegToPoint {
                point_id: 2,
                point_name: String::from("current"),
                slave_addr: 2,
                reg_addr: 0,
                data_type: ModbusDataType::U16,
                scale: 0.1,
                offset: 0.0,
                access: AccessMode::ReadWrite,
            },
        ])
    }

    /// ARCH-P01：modbus-rtu 与 mock 跑同一 seam 契约测试套件。
    #[test]
    fn modbus_adapter_passes_shared_contract() {
        let mut adapter = contract_adapter();
        // 契约执行顺序的响应队列（MockRtuTransport 先进先出）：
        // 1. read_point 逐个：2 帧
        // 2. read_points：2 帧
        // 3. write_point：1 帧
        // 4. read_device_points：2 帧
        // 5. read_point(missing)：不发帧（Err 路径）
        for _ in 0..2 {
            adapter
                .transport_mut()
                .push_response(read_resp(1, &[100u16]));
            adapter
                .transport_mut()
                .push_response(read_resp(2, &[200u16]));
        }
        adapter.transport_mut().push_response(write_resp(1, 0, 1));
        adapter
            .transport_mut()
            .push_response(read_resp(1, &[100u16]));
        adapter
            .transport_mut()
            .push_response(read_resp(2, &[200u16]));

        let cfg = simple_adapter_config(ProtocolType::ModbusRtu, 7);
        run_adapter_contract(&mut adapter, &cfg, ProtocolType::ModbusRtu, 7, &[1, 2]);
    }

    /// 读点返回真实工程值（外部可观察：帧级验证）。
    #[test]
    fn read_point_returns_engineering_value() {
        let mut mock = MockRtuTransport::new();
        mock.push_response(read_resp(1, &[1000u16])); // raw=1000, scale=0.1 → 100.0
        let mut adapter = ModbusRtuAdapter::new(mock, 100, 0).with_mappings(vec![RegToPoint {
            point_id: 1,
            point_name: String::from("voltage"),
            slave_addr: 1,
            reg_addr: 0,
            data_type: ModbusDataType::U16,
            scale: 0.1,
            offset: 0.0,
            access: AccessMode::ReadOnly,
        }]);
        let cfg = simple_adapter_config(ProtocolType::ModbusRtu, 1);
        adapter.init(&cfg).unwrap();
        let dp = adapter.read_point(1).unwrap();
        assert_eq!(dp.point_id, 1);
        assert_eq!(dp.device_id, 1);
        assert_eq!(dp.value, PointValue::Float(100.0));
        assert_eq!(dp.quality, PointQuality::good());
        assert_eq!(dp.source, DataSource::ModbusRtu);
        assert_eq!(dp.name, "voltage");
    }

    /// 未 init 时读点拒绝（fail-closed）。
    #[test]
    fn read_point_before_init_rejected() {
        let mock = MockRtuTransport::new();
        let mut adapter = ModbusRtuAdapter::new(mock, 100, 0).with_mappings(vec![RegToPoint {
            point_id: 1,
            point_name: String::from("v"),
            slave_addr: 1,
            reg_addr: 0,
            data_type: ModbusDataType::U16,
            scale: 1.0,
            offset: 0.0,
            access: AccessMode::ReadOnly,
        }]);
        assert_eq!(
            adapter.read_point(1),
            Err(ProtocolError::ProtocolNotStarted)
        );
    }

    /// 未注册点返回 PointNotFound（不发帧）。
    #[test]
    fn read_unmapped_point_rejected() {
        let mock = MockRtuTransport::new();
        let mut adapter = ModbusRtuAdapter::new(mock, 100, 0).with_mappings(vec![RegToPoint {
            point_id: 1,
            point_name: String::from("v"),
            slave_addr: 1,
            reg_addr: 0,
            data_type: ModbusDataType::U16,
            scale: 1.0,
            offset: 0.0,
            access: AccessMode::ReadOnly,
        }]);
        let cfg = simple_adapter_config(ProtocolType::ModbusRtu, 1);
        adapter.init(&cfg).unwrap();
        adapter.start().unwrap();
        assert_eq!(adapter.read_point(999), Err(ProtocolError::PointNotFound));
    }
}

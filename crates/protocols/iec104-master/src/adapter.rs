//! IEC 104 协议适配器 — 第三个真实 `ProtocolAdapter` 实现（ARCH-P04）。
//!
//! 实现 [`eneros_protocol_abstract::adapter::ProtocolAdapter`]，与 modbus-rtu
//! 跑同一共享契约测试套件（`protocol-abstract::contract`），验证 seam 泛化性：
//! - 经 [`crate::master::Iec104Master`] 执行总召唤（C_IC_NA）与命令（C_SC/C_DC）；
//! - 点值来自 master 数据面缓存（[`crate::master::Iec104Master::latest_value`]），
//!   携带 IEC 品质（QualityDescriptor）与接收时间戳；
//! - `read_point` 在缓存未命中时触发一次轮询周期（lazy poll：总召唤 +
//!   消费接收帧），随后从缓存读取——符合 IEC 104 召唤-响应语义。
//!
//! 点映射：`IecPointMapping { point_id, common_addr, ioa }`，由
//! `AdapterConfig::device_configs[].address`（`ProtocolAddress::Iec104`）推导。

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_iec104_slave::IoValue;
use eneros_iec104_slave::SinglePointValue;
use eneros_protocol_abstract::access::{
    AuthorizedWriter, PointAccess, PointAccessExt, WriteError, WriteOutcome,
};
use eneros_protocol_abstract::adapter::{AdapterState, ProtocolAdapter};
use eneros_protocol_abstract::config::{AdapterConfig, ProtocolType};
use eneros_protocol_abstract::error::ProtocolError;
use eneros_protocol_abstract::AuthorizedCommand;
use eneros_telemetry_model::command::CommandValue;
use eneros_upa_model::{
    DataPoint, DataSource, DeviceId, PointId, PointQuality, PointType, PointValue,
};

use crate::config::MasterConfig;
use crate::device::RemoteDevice;
use crate::error::MasterError;
use crate::master::Iec104Master;
use crate::transport::MasterTransport;

/// IEC 104 点映射：point_id ↔ (公共地址, 信息对象地址)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IecPointMapping {
    /// UPA 点 ID。
    pub point_id: PointId,
    /// 公共地址（设备）。
    pub common_addr: u16,
    /// 信息对象地址（IOA）。
    pub ioa: u32,
}

/// IEC 104 协议适配器。
pub struct Iec104Adapter {
    /// 底层主站。
    master: Iec104Master,
    /// 点映射表。
    mappings: Vec<IecPointMapping>,
    /// 关联设备 ID。
    device_id: DeviceId,
    /// 适配器状态。
    state: AdapterState,
    /// 轮询时钟（ms，注入）。
    now_ms: u64,
    /// 46.5：总召唤调度周期（ms）——来自 MasterConfig（配置键
    /// `poll_interval_ms`），真消费：`RemoteDevice` 轮询任务用此值排队
    /// （旧实现 adapter 硬编码 30_000，配置字段未落到运行行为）。
    poll_interval_ms: u32,
}

impl Iec104Adapter {
    /// 创建适配器（初始状态 `Uninitialized`）。
    pub fn new(config: MasterConfig, transport: Box<dyn MasterTransport>) -> Self {
        let poll_interval_ms = config.poll_interval_ms;
        Self {
            master: Iec104Master::new(config, transport),
            mappings: Vec::new(),
            device_id: 0,
            state: AdapterState::Uninitialized,
            now_ms: 0,
            poll_interval_ms,
        }
    }

    /// 设置点映射（链式）。
    pub fn with_mappings(mut self, mappings: Vec<IecPointMapping>) -> Self {
        self.mappings = mappings;
        self
    }

    /// 查找点映射。
    fn find_mapping(&self, point_id: PointId) -> Option<&IecPointMapping> {
        self.mappings.iter().find(|m| m.point_id == point_id)
    }

    /// 蓝图 61 [07] §3.3（v0.314.31）：主站只读视图（ingest adapter 经
    /// plugin 转发 drain 数据面；只读——adapter 直写 Snapshot/设备在类型
    /// 上不可达，[07] §3.5 分界）。
    pub fn master(&self) -> &Iec104Master {
        &self.master
    }

    /// 对单个设备执行一轮召唤（连接 + 总召唤 + 消费帧），填充该设备缓存。
    fn poll_device(&mut self, common_addr: u16, now_ms: u64) {
        if !self.master.conn_keys().contains(&common_addr) {
            // 31 PR-14：对端地址经 MasterConfig 注入（生产路径禁字面量 IP）。
            let (peer_ip, peer_port) = self.master.peer();
            let dev = RemoteDevice::new(peer_ip, peer_port, common_addr, self.poll_interval_ms);
            let _ = self.master.connect(&dev);
        }
        // 只消费一帧：STARTDT_CON 握手确认（此时 confirm/数据帧仍在队列，
        // 必须等 interrogation 之后处理——IEC104 状态机要求）
        let _ = self.master.process_rx(common_addr);
        // 总召唤（C_IC_NA）→ Interrogating
        let _ = self.master.interrogation(common_addr, now_ms);
        // 消费激活确认 + 数据帧 → Connected + 缓存填充
        self.drain_rx();
    }

    /// 消费传输层全部就绪帧（最多 16 轮，防无限循环）。
    fn drain_rx(&mut self) {
        for _ in 0..16 {
            let conn_keys = self.master.conn_keys();
            let mut any = false;
            for key in conn_keys {
                if self.master.process_rx(key).is_ok() {
                    any = true;
                }
            }
            if !any {
                break;
            }
        }
    }

    /// 读取单点（缓存未命中先触发一轮 lazy poll）。
    fn read_point_impl(&mut self, point_id: PointId) -> Result<DataPoint, ProtocolError> {
        if self.state != AdapterState::Running && self.state != AdapterState::Initialized {
            return Err(ProtocolError::ProtocolNotStarted);
        }
        let mapping = self
            .find_mapping(point_id)
            .ok_or(ProtocolError::PointNotFound)?;
        let (common_addr, ioa) = (mapping.common_addr, mapping.ioa);

        let cached = self.master.latest_value(common_addr, ioa).cloned();
        let lv = match cached {
            Some(v) => v,
            None => {
                // lazy poll：仅召唤目标设备，消费响应后重查
                self.now_ms = eneros_time::get_monotonic_ns() / 1_000_000;
                self.poll_device(common_addr, self.now_ms);
                self.master
                    .latest_value(common_addr, ioa)
                    .cloned()
                    .ok_or(ProtocolError::PointNotFound)?
            }
        };

        let (value, point_type) = match lv.value {
            IoValue::Float(f) => (PointValue::Float(f.into()), PointType::Analog),
            IoValue::Normalized(n) => (PointValue::Float(n as f64), PointType::Analog),
            IoValue::Scaled(s) => (PointValue::Int(s as i64), PointType::Analog),
            IoValue::SinglePoint(s) => (
                PointValue::Bool(s == SinglePointValue::On),
                PointType::Digital,
            ),
            IoValue::DoublePoint(d) => (PointValue::Enum(d as u16), PointType::Digital),
            IoValue::Counter(c) => (PointValue::Int(c as i64), PointType::Counter),
            other => (PointValue::String(format!("{other:?}")), PointType::Analog),
        };

        // IEC QualityDescriptor 无 valid 位：全部坏位为 false 即有效（ADR-0027）。
        let mut quality = PointQuality::good();
        if lv.quality.invalid
            || lv.quality.not_topical
            || lv.quality.substituted
            || lv.quality.blocked
            || lv.quality.overflow
        {
            quality.invalid = true;
        }

        Ok(DataPoint {
            point_id,
            device_id: self.device_id,
            name: String::new(),
            description: None,
            point_type,
            value,
            quality,
            timestamp_ms: lv.timestamp_ms,
            source: DataSource::Iec104,
            unit: None,
        })
    }
    /// 测试专用：访问内部 master（预置 mock 传输响应）。
    #[cfg(test)]
    pub fn master_mut(&mut self) -> Option<&mut crate::mock::MockMasterTransport> {
        self.master
            .transport_mut()
            .as_any_mut()
            .downcast_mut::<crate::mock::MockMasterTransport>()
    }

    /// 46.5 观测：主站连接/调度调试摘要（进程内可见性，非协议语义）。
    pub fn debug_conn(&self) -> alloc::string::String {
        let keys = self.master.conn_keys();
        let mut s = alloc::string::String::from("conns=[");
        for (i, k) in keys.iter().enumerate() {
            if i > 0 {
                s.push(' ');
            }
            s.push_str(&alloc::format!("{:?}", self.master.device_state(*k)));
        }
        s.push_str("] sched=");
        for (addr, task) in &self.master.scheduler_tasks() {
            s.push_str(&alloc::format!(
                "{addr}:next={} int={} ",
                task.next_poll_ms,
                task.interval_ms
            ));
        }
        s
    }
}

impl AuthorizedWriter for Iec104Adapter {
    fn write_authorized(&mut self, cmd: &AuthorizedCommand) -> Result<WriteOutcome, WriteError> {
        // 蓝图/24 §6.1：命令点/设备匹配校验通过后才落写；不匹配 →
        // CommandMismatch，设备状态不变。
        if cmd.device_id() != self.device_id {
            return Err(WriteError::CommandMismatch);
        }
        let mapping = self
            .find_mapping(cmd.point_id())
            .ok_or(WriteError::CommandMismatch)?;
        if self.state != AdapterState::Running && self.state != AdapterState::Initialized {
            return Err(WriteError::Protocol(ProtocolError::ProtocolNotStarted));
        }
        // 61 v0.314.25：开关位置唯一来源 = cmd.value()；legacy 浮点载荷
        // 语义（ADR-029）退役——模拟量命令对 C_SC 遥控 = 类型矛盾拒绝。
        let position = match cmd.value() {
            CommandValue::SwitchPosition(0) => SinglePointValue::Off,
            CommandValue::SwitchPosition(_) => SinglePointValue::On,
            _ => return Err(WriteError::Unauthorized),
        };
        self.master
            .send_single_command(mapping.common_addr, mapping.ioa, position)
            .map_err(iec104_to_protocol)
            .map_err(WriteError::Protocol)?;
        // 回读确认可空：以 IEC 104 链路确认（命令传输确认）为确认。
        Ok(WriteOutcome { echoed: None })
    }
}

impl PointAccess for Iec104Adapter {
    fn read_point(&mut self, point_id: PointId) -> Result<DataPoint, ProtocolError> {
        self.read_point_impl(point_id)
    }

    fn protocol_type(&self) -> ProtocolType {
        ProtocolType::Iec104
    }
}

impl PointAccessExt for Iec104Adapter {
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

impl ProtocolAdapter for Iec104Adapter {
    fn init(&mut self, config: &AdapterConfig) -> Result<(), ProtocolError> {
        if config.protocol_type != ProtocolType::Iec104 {
            return Err(ProtocolError::InvalidConfig);
        }
        let device = config
            .device_configs
            .first()
            .ok_or(ProtocolError::InvalidConfig)?;
        self.device_id = device.device_id;
        // 从设备配置推导点映射：单设备场景下 common_addr/ioa 由 mapping 显式注入，
        // 或从 ProtocolAddress::Iec104 推导（首个设备）。
        self.state = AdapterState::Initialized;
        Ok(())
    }

    fn start(&mut self) -> Result<(), ProtocolError> {
        // 启动即连接首个映射设备（connect 内含 STARTDT_ACT 握手）
        if let Some(m) = self.mappings.first() {
            // 31 PR-14：对端地址经 MasterConfig 注入。
            let (peer_ip, peer_port) = self.master.peer();
            let dev = RemoteDevice::new(peer_ip, peer_port, m.common_addr, self.poll_interval_ms);
            let _ = self.master.connect(&dev);
        }
        self.state = AdapterState::Running;
        Ok(())
    }

    fn stop(&mut self) -> Result<(), ProtocolError> {
        self.state = AdapterState::Stopped;
        Ok(())
    }

    fn poll(&mut self, now_ms: u64) -> Result<(), ProtocolError> {
        self.now_ms = now_ms;
        self.master.poll(now_ms);
        Ok(())
    }

    fn state(&self) -> AdapterState {
        self.state
    }
}

/// MasterError → ProtocolError 映射。
fn iec104_to_protocol(e: MasterError) -> ProtocolError {
    use MasterError::*;
    match e {
        NotConnected | StateError => ProtocolError::ProtocolNotStarted,
        ConnectFailed | SendFailed | RecvFailed | Timeout | Iec104(_) => ProtocolError::ReadFailed,
        // 31 PR-01：k 窗口满/协议违规 → 读失败（调用方重试语义不变）。
        WindowFull | ProtocolViolation => ProtocolError::ReadFailed,
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use eneros_iec104_slave::{
        Apdu, Asdu, Cot, InformationObject, IoValue, QualityDescriptor, TypeId,
    };
    use eneros_protocol_abstract::contract::{run_adapter_contract_typed, simple_adapter_config};

    use super::*;
    use crate::mock::MockMasterTransport;

    /// 构造包含一个遥测点的 M_ME_NC 数据帧（`send_seq` = 对端 N(S)，
    /// 39 收尾复验 P1-3 后必须与 master 期望序号一致）。
    fn telemetry_frame(common_addr: u16, ioa: u32, value: f32, send_seq: u16) -> Vec<u8> {
        let asdu = Asdu {
            type_id: TypeId::MeasuredValueFloat,
            cause_of_tx: Cot::Spontaneous,
            common_addr,
            ioas: vec![InformationObject {
                ioa,
                value: IoValue::Float(value),
                quality: QualityDescriptor::good(),
                time_tag: None,
            }],
        };
        Apdu::i_format(send_seq, 0, asdu).encode().unwrap()
    }

    /// 构造总召唤激活确认帧（C_IC_NA，COT=ActivationConfirm）。
    fn interrogation_confirm(common_addr: u16, send_seq: u16) -> Vec<u8> {
        let asdu = Asdu {
            type_id: TypeId::InterrogationCommand,
            cause_of_tx: Cot::ActivationConfirm,
            common_addr,
            ioas: vec![InformationObject {
                ioa: 0,
                value: IoValue::Normalized(20),
                quality: QualityDescriptor::good(),
                time_tag: None,
            }],
        };
        Apdu::i_format(send_seq, 0, asdu).encode().unwrap()
    }

    /// ARCH-P04：iec104-master 与 modbus 跑同一 seam 契约测试套件。
    /// 适配器直接持内部 mock transport，预置遥测响应后跑契约。
    #[test]
    fn iec104_adapter_passes_shared_contract() {
        let mut adapter = Iec104Adapter::new(
            MasterConfig::default(),
            Box::new(MockMasterTransport::new()),
        )
        .with_mappings(vec![
            IecPointMapping {
                point_id: 1,
                common_addr: 1,
                ioa: 100,
            },
            IecPointMapping {
                point_id: 2,
                common_addr: 2,
                ioa: 200,
            },
        ]);

        // 契约执行顺序的响应队列：
        // 1. read_point(1)：lazy poll → connect(1) + STARTDT_CON + interrogation(1)
        //    + 遥测帧(ioa=100)
        // 2. read_point(2)：lazy poll → connect(2) + STARTDT_CON + interrogation(2)
        //    + 遥测帧(ioa=200)
        // 3. read_points([1,2])：缓存命中（无新帧）
        // 4. write_point：send_single_command（无响应帧）
        // 5. read_device_points：缓存命中
        // 6. read_point(missing)：不发帧
        {
            let mock = adapter
                .master_mut()
                .expect("内部 transport 必须是 MockMasterTransport");
            // 每设备：STARTDT_CON + 总召唤激活确认(N(S)=0) + 遥测帧(N(S)=1)
            //——39 收尾复验（P1-3）：注入帧序号必须协议合法（旧测试
            // 两帧均 N(S)=0，本身就是乱序/重放样本）。
            for (conn, ioa) in [(1u32, 100u32), (2u32, 200u32)] {
                mock.push_rx(
                    conn,
                    Apdu::u_format(eneros_iec104_slave::UFormatFunction::StartDtCon)
                        .encode()
                        .unwrap(),
                );
                mock.push_rx(conn, interrogation_confirm(conn as u16, 0));
                mock.push_rx(conn, telemetry_frame(conn as u16, ioa, 42.0, 1));
            }
        }

        let cfg = simple_adapter_config(ProtocolType::Iec104, 7);
        run_adapter_contract_typed(
            &mut adapter,
            &cfg,
            ProtocolType::Iec104,
            7,
            &[1, 2],
            eneros_telemetry_model::command::CommandValue::SwitchPosition(1),
            true,
        );
    }
}

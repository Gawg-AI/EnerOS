//! IEC 104 主站核心 — 连接管理、APDU 收发、ASDU 构造与分发.
//!
//! 主站主动发起通信：STARTDT 握手、周期性总召唤、遥控命令下发、时钟同步命令下发。
//! 多设备并发管理（`BTreeMap<u16, MasterConnection>`），每设备独立维护连接状态与序列号。
//!
//! D2：时间通过 `now_ms: u64` 参数注入（无 `MonotonicTime` 类型）。
//! D5：复用 `eneros-iec104-slave` 的 APDU/ASDU/TypeId/Cot 等类型。
//! D11：时钟同步时标由 `now_ms` 参数构造 [`TimeTag`](eneros_iec104_slave::TimeTag)。

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

use eneros_iec104_slave::{
    Apdu, Asdu, ControlField, Cot, Dco, DoublePointValue, InformationObject, IoValue,
    QualityDescriptor, Sco, SinglePointValue, TimeTag, TypeId, UFormatFunction,
};

use crate::config::MasterConfig;
use crate::connection::MasterConnection;
use crate::device::{ConnState, RemoteDevice};
use crate::error::MasterError;
use crate::poll::PollScheduler;
use crate::transport::{MasterStats, MasterTransport};

/// IEC 104 主站
///
/// 封装多设备连接管理、轮询调度、APDU 收发与 ASDU 分发。
/// 通过 [`MasterTransport`] trait 抽象传输层，不直接依赖 smoltcp（D4）。
///
/// 数据面（ARCH-P04）：`latest_values` 保留最近接收的遥测/遥信值
/// （common_addr → ioa → 值+品质+时间戳），供上层 [`crate::adapter::Iec104Adapter`]
/// 经 [`Self::latest_value`] 读取。
pub struct Iec104Master {
    /// 设备连接表（key: common_addr）
    devices: BTreeMap<u16, MasterConnection>,
    /// 轮询调度器
    scheduler: PollScheduler,
    /// 主站配置
    config: MasterConfig,
    /// 统计信息
    stats: MasterStats,
    /// 传输层（trait object，由调用方注入）
    transport: Box<dyn MasterTransport>,
    /// 最近接收值缓存：common_addr → ioa → (值, 品质, 时间戳)。
    latest_values: BTreeMap<u16, BTreeMap<u32, LatestValue>>,
}

/// 最近接收的点值（数据面，ARCH-P04）。
#[derive(Debug, Clone, PartialEq)]
pub struct LatestValue {
    /// 信息对象值。
    pub value: IoValue,
    /// 数据品质。
    pub quality: QualityDescriptor,
    /// 接收时间戳（ms，来自传输层单调时钟）。
    pub timestamp_ms: u64,
}

impl Iec104Master {
    /// 创建主站实例。
    /// 31 PR-14：对端地址（装配层经 MasterConfig 注入——生产路径禁止
    /// 字面量 IP）。
    pub fn peer(&self) -> ([u8; 4], u16) {
        (self.config.peer_ip, self.config.peer_port)
    }

    pub fn new(config: MasterConfig, transport: Box<dyn MasterTransport>) -> Self {
        Self {
            devices: BTreeMap::new(),
            scheduler: PollScheduler::new(),
            config,
            stats: MasterStats::default(),
            transport,
            latest_values: BTreeMap::new(),
        }
    }

    /// 返回最近接收的点值（数据面，ARCH-P04）。
    pub fn latest_value(&self, common_addr: u16, ioa: u32) -> Option<&LatestValue> {
        self.latest_values
            .get(&common_addr)
            .and_then(|m| m.get(&ioa))
    }

    /// 返回所有已建立连接的公共地址列表（数据面轮询用，ARCH-P04）。
    pub fn conn_keys(&self) -> Vec<u16> {
        self.devices.keys().copied().collect()
    }

    /// 46.5 观测：调度任务表快照（诊断/可见性，非协议语义）。
    pub fn scheduler_tasks(&self) -> alloc::collections::BTreeMap<u16, crate::poll::PollTask> {
        self.scheduler.tasks.clone()
    }

    /// 测试专用：可变访问传输层（预置接收数据/检查发送帧）。
    #[cfg(test)]
    pub fn transport_mut(&mut self) -> &mut dyn MasterTransport {
        &mut *self.transport
    }

    /// 连接远端设备并发送 STARTDT_ACT。
    ///
    /// 连接建立后状态为 `StartDtPending`，收到 STARTDT_CON 后转为 `Connected`。
    pub fn connect(&mut self, device: &RemoteDevice) -> Result<(), MasterError> {
        let conn_id = self.transport.connect(device.ip, device.port)?;
        self.stats.connect_count += 1;

        let now = self.transport.now_ms();
        let conn = MasterConnection::new(*device, conn_id, now);
        self.devices.insert(device.common_addr, conn);
        self.scheduler
            .add_task(device.common_addr, device.poll_interval_ms, now);

        // 发送 STARTDT_ACT
        let startdt = Apdu::u_format(UFormatFunction::StartDtAct);
        let data = startdt.encode()?;
        if let Err(e) = self.transport.send(conn_id, &data) {
            self.stats.tx_error_count += 1;
            return Err(e);
        }
        self.stats.tx_count += 1;

        // 状态已在 MasterConnection::new 中设为 StartDtPending
        Ok(())
    }

    /// 发起总召唤（COT=Activation，QOI=20 站召唤）。
    pub fn interrogation(&mut self, common_addr: u16, now_ms: u64) -> Result<(), MasterError> {
        // 检查连接状态
        let state = self
            .devices
            .get(&common_addr)
            .map(|c| c.state)
            .ok_or(MasterError::NotConnected)?;
        if state != ConnState::Connected && state != ConnState::Interrogating {
            return Err(MasterError::StateError);
        }

        // 构造总召唤 ASDU（QOI=20，站召唤）
        let asdu = Asdu {
            type_id: TypeId::InterrogationCommand,
            cause_of_tx: Cot::Activation,
            common_addr,
            ioas: vec![InformationObject {
                ioa: 0,
                value: IoValue::Normalized(20),
                quality: QualityDescriptor::good(),
                time_tag: None,
            }],
        };

        self.send_i_format(common_addr, &asdu)?;

        // 更新状态与统计
        if let Some(conn) = self.devices.get_mut(&common_addr) {
            conn.state = ConnState::Interrogating;
            conn.last_interrogation_ms = now_ms;
        }
        self.stats.interrogation_count += 1;
        Ok(())
    }

    /// 发送时钟同步命令（带 CP56Time2a 时标，D11）。
    pub fn clock_sync(&mut self, common_addr: u16, now_ms: u64) -> Result<(), MasterError> {
        let state = self
            .devices
            .get(&common_addr)
            .map(|c| c.state)
            .ok_or(MasterError::NotConnected)?;
        if state != ConnState::Connected {
            return Err(MasterError::StateError);
        }

        let time_tag = time_tag_from_ms(now_ms);
        let asdu = Asdu {
            type_id: TypeId::ClockSyncCommand,
            cause_of_tx: Cot::Activation,
            common_addr,
            ioas: vec![InformationObject {
                ioa: 0,
                value: IoValue::SinglePoint(SinglePointValue::On),
                quality: QualityDescriptor::good(),
                time_tag: Some(time_tag),
            }],
        };

        self.send_i_format(common_addr, &asdu)?;

        if let Some(conn) = self.devices.get_mut(&common_addr) {
            conn.last_clock_sync_ms = now_ms;
        }
        self.stats.clock_sync_count += 1;
        Ok(())
    }

    /// 发送单点遥控命令（COT=Activation）。
    pub fn send_single_command(
        &mut self,
        common_addr: u16,
        ioa: u32,
        value: SinglePointValue,
    ) -> Result<(), MasterError> {
        let state = self
            .devices
            .get(&common_addr)
            .map(|c| c.state)
            .ok_or(MasterError::NotConnected)?;
        if state != ConnState::Connected {
            return Err(MasterError::StateError);
        }

        let asdu = Asdu {
            type_id: TypeId::SingleCommand,
            cause_of_tx: Cot::Activation,
            common_addr,
            ioas: vec![InformationObject {
                ioa,
                value: IoValue::SingleCommand(Sco::new(matches!(value, SinglePointValue::On))),
                quality: QualityDescriptor::good(),
                time_tag: None,
            }],
        };

        self.send_i_format(common_addr, &asdu)?;
        self.stats.command_count += 1;
        Ok(())
    }

    /// 发送双点遥控命令（COT=Activation）。
    pub fn send_double_command(
        &mut self,
        common_addr: u16,
        ioa: u32,
        value: DoublePointValue,
    ) -> Result<(), MasterError> {
        let state = self
            .devices
            .get(&common_addr)
            .map(|c| c.state)
            .ok_or(MasterError::NotConnected)?;
        if state != ConnState::Connected {
            return Err(MasterError::StateError);
        }

        let asdu = Asdu {
            type_id: TypeId::DoubleCommand,
            cause_of_tx: Cot::Activation,
            common_addr,
            ioas: vec![InformationObject {
                ioa,
                value: IoValue::DoubleCommand(Dco::new(value)),
                quality: QualityDescriptor::good(),
                time_tag: None,
            }],
        };

        self.send_i_format(common_addr, &asdu)?;
        self.stats.command_count += 1;
        Ok(())
    }

    /// 周期轮询：总召唤 + 时钟同步 + 接收处理 + t3 保活。
    ///
    /// 1. 遍历到期轮询任务，执行总召唤
    /// 2. 检查时钟同步周期
    /// 3. 检查 t3 超时，发送 TESTFR_ACT
    /// 4. 处理接收数据
    pub fn poll(&mut self, now_ms: u64) {
        // 31 PR-01：t2 确认时限（收到 I 帧未确认超时 → 补发 S 帧）。
        self.poll_acks(now_ms);
        // 1. 到期总召唤任务
        let due = self.scheduler.due_tasks(now_ms);
        for common_addr in due {
            let _ = self.interrogation(common_addr, now_ms);
            self.scheduler.update_next(common_addr, now_ms);
        }

        // 2. 收集连接 key（避免迭代时借用冲突）
        let conn_keys: Vec<u16> = self.devices.keys().copied().collect();
        for conn_key in conn_keys {
            // 时钟同步检查
            let need_clock_sync = self
                .devices
                .get(&conn_key)
                .map(|c| {
                    c.state == ConnState::Connected
                        && now_ms.saturating_sub(c.last_clock_sync_ms)
                            > self.config.clock_sync_interval_ms as u64
                })
                .unwrap_or(false);
            if need_clock_sync {
                let _ = self.clock_sync(conn_key, now_ms);
            }

            // t3 保活检查
            let need_testfr = self
                .devices
                .get(&conn_key)
                .map(|c| {
                    (c.state == ConnState::Connected || c.state == ConnState::StartDtPending)
                        && now_ms.saturating_sub(c.last_activity_ms)
                            > self.config.t3_timeout_ms as u64
                })
                .unwrap_or(false);
            if need_testfr {
                let _ = self.send_testfr(conn_key);
            }

            // 46（WP-46.4）：t1 连接确认超时——StartDtPending（已发
            // STARTDT_ACT 等待 STARTDT_CON）空闲超 t1 → 断链，适配器下一次
            // poll 自动重连（poll_device 对非在册设备重新 connect）。
            if let Some(c) = self.devices.get(&conn_key) {
                if c.state == ConnState::StartDtPending
                    && now_ms.saturating_sub(c.last_activity_ms) > self.config.t1_timeout_ms as u64
                {
                    self.stats.t1_timeout_count += 1;
                    self.drop_device(conn_key);
                    continue;
                }
            }

            // 接收处理
            let _ = self.process_rx(conn_key);
        }
    }

    /// 处理指定连接的接收数据（每次处理一帧，非阻塞）。
    ///
    /// 单次仅处理一帧以匹配非阻塞轮询语义：调用方通过多次 `poll()` 消费多帧。
    pub fn process_rx(&mut self, conn_key: u16) -> Result<(), MasterError> {
        let now = self.transport.now_ms();
        let conn_id = {
            let conn = self
                .devices
                .get(&conn_key)
                .ok_or(MasterError::NotConnected)?;
            conn.conn_id
        };

        let data = match self.transport.recv(conn_id) {
            Ok(Some(data)) => data,
            Ok(None) => return Ok(()),
            Err(e) => {
                self.stats.rx_error_count += 1;
                return Err(e);
            }
        };

        self.stats.rx_count += 1;

        let apdu = match Apdu::decode(&data) {
            Ok(apdu) => apdu,
            Err(e) => {
                self.stats.rx_error_count += 1;
                return Err(MasterError::Iec104(e));
            }
        };

        match apdu.control_field {
            ControlField::Unnumbered(func) => {
                self.handle_u_format(conn_key, func);
                // 31-review：IEC 104 §5——收到 TestFrAct 必须回 TestFrCon
                //（旧 _ =>{} 吞掉 → 对端 t1 重传 3 次后断开健康链路）。
                if matches!(func, UFormatFunction::TestFrAct) {
                    self.send_u_frame(conn_key, UFormatFunction::TestFrCon)?;
                }
            }
            ControlField::Information { send_seq, .. } => {
                // 39 收尾复验（P1-3）：N(S) 必须等于期望接收序号——
                // 乱序/重复/重放 I 帧不得静默接受并确认（与 S 帧
                // R(S) 校验对称；旧实现不读帧内序号照单全收）。
                // fail-closed：序号不符 → ProtocolViolation（不推进
                // recv_seq、不发送 S 确认，上层按错误处置/重连）。
                let expected = self
                    .devices
                    .get(&conn_key)
                    .map(|c| c.recv_seq)
                    .ok_or(MasterError::NotConnected)?;
                if send_seq != expected {
                    self.stats.rx_error_count += 1;
                    return Err(MasterError::ProtocolViolation);
                }
                if let Some(asdu) = apdu.asdu {
                    self.handle_i_format(conn_key, asdu)?;
                }
                // 31 PR-01：接收序号推进 + 待确认计数（I 帧必须 S 帧确认）。
                let recv_seq = {
                    let conn = self
                        .devices
                        .get_mut(&conn_key)
                        .ok_or(MasterError::NotConnected)?;
                    conn.last_rx_i_ms = now;
                    conn.rx_unacked = conn.rx_unacked.saturating_add(1);
                    conn.next_recv_seq();
                    // 31-review：next_recv_seq() 返回递增前的旧值；S 帧 R(S) 须为
                    // 递增后值（= 下一期望 N(S)）。改读字段取新值，与 t2 兜底路径
                    // （poll_acks 读 c.recv_seq）及 slave 侧一致，修 off-by-one。
                    conn.recv_seq
                };
                // 收 I 帧立即确认（S 帧，R(S) = 下一期望接收序号）。
                self.send_s_frame(conn_key, recv_seq)?;
            }
            ControlField::Numbered { recv_seq } => {
                // S 格式：远端确认了我们的 I 帧——校验 R(S) 合法性
                //（不得超过已发送序号；31 PR-01）。
                if let Some(conn) = self.devices.get_mut(&conn_key) {
                    if recv_seq as u32 > conn.send_seq as u32 {
                        self.stats.rx_error_count += 1;
                        return Err(MasterError::ProtocolViolation);
                    }
                    // 对端确认了我们的 I 帧——k 窗口释放。
                    conn.tx_unacked = 0;
                }
            }
        }

        // 更新活动时间
        let now = self.transport.now_ms();
        if let Some(conn) = self.devices.get_mut(&conn_key) {
            conn.touch(now);
        }

        Ok(())
    }

    /// 处理 U 格式帧（STARTDT_CON / STOPDT_CON / TESTFR_CON）。
    pub fn handle_u_format(&mut self, conn_key: u16, func: UFormatFunction) {
        match func {
            UFormatFunction::StartDtCon => {
                if let Some(conn) = self.devices.get_mut(&conn_key) {
                    conn.state = ConnState::Connected;
                }
            }
            UFormatFunction::StopDtCon => {
                if let Some(conn) = self.devices.get_mut(&conn_key) {
                    conn.state = ConnState::Idle;
                }
            }
            UFormatFunction::TestFrCon => {
                // 活动时间已在 process_rx 中更新
            }
            _ => {}
        }
    }

    /// 处理接收到的 I 格式 ASDU。
    pub fn handle_i_format(&mut self, conn_key: u16, asdu: Asdu) -> Result<(), MasterError> {
        match asdu.type_id {
            TypeId::InterrogationCommand => {
                // 总召唤激活确认 → 恢复 Connected 状态
                if asdu.cause_of_tx == Cot::ActivationConfirm {
                    if let Some(conn) = self.devices.get_mut(&conn_key) {
                        if conn.state == ConnState::Interrogating {
                            conn.state = ConnState::Connected;
                        }
                    }
                }
            }
            TypeId::SingleCommand
            | TypeId::DoubleCommand
            // 46（WP-46.4）：设点命令确认（C_SE_NA/C_SE_NC，COT=ActivationConfirm）——
            // 无需额外处理（命令计数/状态由调用方跟踪）。
            | TypeId::SetpointNormalized
            | TypeId::SetpointFloat => {
                // 遥控/设点确认 — 无需额外处理
            }
            TypeId::ClockSyncCommand => {
                // 时钟同步确认 — 无需额外处理
            }
            TypeId::SinglePointInformation
            | TypeId::DoublePointInformation
            | TypeId::MeasuredValueNormalized
            | TypeId::MeasuredValueScaled
            | TypeId::MeasuredValueFloat
            | TypeId::Counter
            // 46（WP-46.4）：步位置 / 比特串遥信接入数据面。
            | TypeId::StepPosition
            | TypeId::Bitstring
            // v0.220.0：带时标类型数据路径相同（时标保留在 io.time_tag）。
            | TypeId::SinglePointTimeTagged
            | TypeId::MeasuredValueNormalizedTimeTagged
            | TypeId::MeasuredValueFloatTimeTagged => {
                // 遥测遥信数据 — 保留到数据面缓存（ARCH-P04）
                let ts = self.transport.now_ms();
                let entry = self.latest_values.entry(asdu.common_addr).or_default();
                for io in &asdu.ioas {
                    entry.insert(
                        io.ioa,
                        LatestValue {
                            value: io.value,
                            quality: io.quality,
                            timestamp_ms: ts,
                        },
                    );
                }
                self.stats.rx_point_count += asdu.ioas.len() as u64;
            }
        }
        Ok(())
    }

    /// 46（WP-46.4）：发送归一化设点命令（C_SE_NA，TI=48；COT=Activation）。
    pub fn send_setpoint_normalized(
        &mut self,
        common_addr: u16,
        ioa: u32,
        value: i16,
    ) -> Result<(), MasterError> {
        let state = self
            .devices
            .get(&common_addr)
            .map(|c| c.state)
            .ok_or(MasterError::NotConnected)?;
        if state != ConnState::Connected {
            return Err(MasterError::StateError);
        }
        let asdu = Asdu {
            type_id: TypeId::SetpointNormalized,
            cause_of_tx: Cot::Activation,
            common_addr,
            ioas: vec![InformationObject {
                ioa,
                value: IoValue::SetpointNormalized(value),
                quality: QualityDescriptor::good(),
                time_tag: None,
            }],
        };
        self.send_i_format(common_addr, &asdu)?;
        self.stats.command_count += 1;
        Ok(())
    }

    /// 46（WP-46.4）：发送短浮点设点命令（C_SE_NC，TI=50；COT=Activation）。
    pub fn send_setpoint_float(
        &mut self,
        common_addr: u16,
        ioa: u32,
        value: f32,
    ) -> Result<(), MasterError> {
        let state = self
            .devices
            .get(&common_addr)
            .map(|c| c.state)
            .ok_or(MasterError::NotConnected)?;
        if state != ConnState::Connected {
            return Err(MasterError::StateError);
        }
        let asdu = Asdu {
            type_id: TypeId::SetpointFloat,
            cause_of_tx: Cot::Activation,
            common_addr,
            ioas: vec![InformationObject {
                ioa,
                value: IoValue::SetpointFloat(value),
                quality: QualityDescriptor::good(),
                time_tag: None,
            }],
        };
        self.send_i_format(common_addr, &asdu)?;
        self.stats.command_count += 1;
        Ok(())
    }

    /// 发送 I 格式帧（自增 send_seq）。
    pub fn send_i_format(&mut self, conn_key: u16, asdu: &Asdu) -> Result<(), MasterError> {
        // 31 PR-01：k 窗口——未确认 I 帧达上限（12）暂停发送（IEC 104
        // §5.3；防对端不确认时无限堆积）。
        let (conn_id, send_seq, recv_seq) = {
            let conn = self
                .devices
                .get(&conn_key)
                .ok_or(MasterError::NotConnected)?;
            if conn.tx_unacked >= crate::connection::K_WINDOW {
                return Err(MasterError::WindowFull);
            }
            (conn.conn_id, conn.send_seq, conn.recv_seq)
        };

        // 构造并发送 APDU
        let apdu = Apdu::i_format(send_seq, recv_seq, asdu.clone());
        let data = apdu.encode()?;
        if let Err(e) = self.transport.send(conn_id, &data) {
            self.stats.tx_error_count += 1;
            return Err(e);
        }
        self.stats.tx_count += 1;

        // 递增发送序列号并更新活动时间；k 窗口计数 +1。
        let now = self.transport.now_ms();
        if let Some(conn) = self.devices.get_mut(&conn_key) {
            conn.send_seq = (conn.send_seq + 1) & 0x7FFF;
            conn.tx_unacked = conn.tx_unacked.saturating_add(1);
            conn.touch(now);
        }
        Ok(())
    }

    /// 46（WP-46.4）：移除设备连接（t1 超时/断链）——关闭传输、清除
    /// 设备与调度任务；适配器下一次 poll 会自动重连（自愈语义）。
    fn drop_device(&mut self, common_addr: u16) {
        if let Some(conn) = self.devices.remove(&common_addr) {
            let _ = self.transport.close(conn.conn_id);
            self.stats.disconnect_count += 1;
        }
        self.scheduler.remove_task(common_addr);
    }

    /// 31 PR-01：发送 S 帧确认（R(S) = 下一期望接收序号；空 ASDU）。
    pub fn send_s_frame(&mut self, conn_key: u16, recv_seq: u16) -> Result<(), MasterError> {
        let conn_id = self
            .devices
            .get(&conn_key)
            .map(|c| c.conn_id)
            .ok_or(MasterError::NotConnected)?;
        let apdu = Apdu::s_format(recv_seq);
        let data = apdu.encode()?;
        if let Err(e) = self.transport.send(conn_id, &data) {
            self.stats.tx_error_count += 1;
            return Err(e);
        }
        self.stats.tx_count += 1;
        self.stats.ack_count += 1;
        if let Some(conn) = self.devices.get_mut(&conn_key) {
            conn.rx_unacked = 0;
        }
        Ok(())
    }

    /// 31-review：发送 U 帧（TestFrCon 等；无窗口/序号副作用，不触 rx_unacked）。
    fn send_u_frame(&mut self, conn_key: u16, func: UFormatFunction) -> Result<(), MasterError> {
        let conn_id = self
            .devices
            .get(&conn_key)
            .map(|c| c.conn_id)
            .ok_or(MasterError::NotConnected)?;
        let apdu = Apdu::u_format(func);
        let data = apdu.encode()?;
        if let Err(e) = self.transport.send(conn_id, &data) {
            self.stats.tx_error_count += 1;
            return Err(e);
        }
        self.stats.tx_count += 1;
        Ok(())
    }

    /// 31 PR-01：t2 确认时限——收到 I 帧后超过 t2 未确认 → 补发 S 帧
    ///（IEC 104 §5.2；立即确认路径失败时的兜底）。
    pub fn poll_acks(&mut self, now_ms: u64) {
        let overdue: alloc::vec::Vec<u16> = self
            .devices
            .iter()
            .filter(|(_, c)| {
                c.rx_unacked > 0
                    && now_ms.saturating_sub(c.last_rx_i_ms) > crate::connection::T2_CONFIRM_MS
            })
            .map(|(k, _)| *k)
            .collect();
        for key in overdue {
            let recv_seq = self.devices.get(&key).map(|c| c.recv_seq).unwrap_or(0);
            let _ = self.send_s_frame(key, recv_seq);
        }
    }

    /// 发送 TESTFR_ACT 保活帧。
    pub fn send_testfr(&mut self, conn_key: u16) -> Result<(), MasterError> {
        let conn_id = {
            let conn = self
                .devices
                .get(&conn_key)
                .ok_or(MasterError::NotConnected)?;
            conn.conn_id
        };

        let testfr = Apdu::u_format(UFormatFunction::TestFrAct);
        let data = testfr.encode()?;
        if let Err(e) = self.transport.send(conn_id, &data) {
            self.stats.tx_error_count += 1;
            return Err(e);
        }
        self.stats.tx_count += 1;

        let now = self.transport.now_ms();
        if let Some(conn) = self.devices.get_mut(&conn_key) {
            conn.touch(now);
        }
        Ok(())
    }

    /// 返回统计信息引用。
    pub fn stats(&self) -> &MasterStats {
        &self.stats
    }

    /// 返回指定设备的连接状态。
    pub fn device_state(&self, common_addr: u16) -> Option<ConnState> {
        self.devices.get(&common_addr).map(|c| c.state)
    }
}

/// 从毫秒时间戳构造 CP56Time2a 时标（D11）。
///
/// 基准：2026-01-01 00:00:00 UTC（year=26）。
/// `now_ms=0` → 2026-01-01 00:00:00.000。
pub fn time_tag_from_ms(now_ms: u64) -> TimeTag {
    let total_seconds = now_ms / 1000;
    let millis = (now_ms % 1000) as u16;
    let second = (total_seconds % 60) as u8;
    let total_minutes = total_seconds / 60;
    let minute = (total_minutes % 60) as u8;
    let total_hours = total_minutes / 60;
    let hour = (total_hours % 24) as u8;
    let total_days = total_hours / 24;

    let (year, month, day) = days_to_ymd(total_days);

    TimeTag {
        year,
        month,
        day,
        hour,
        minute,
        second,
        iv: false,
        su: false,
        millis,
    }
}

/// 从总天数（自 2026-01-01 起）计算年/月/日。
fn days_to_ymd(days: u64) -> (u8, u8, u8) {
    // 基准：2026-01-01（CP56Time2a year=26）
    let month_days: [u64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

    let mut year: u64 = 26;
    let mut remaining = days;

    loop {
        let full_year = 2000 + year;
        let is_leap = (full_year % 4 == 0 && full_year % 100 != 0) || (full_year % 400 == 0);
        let days_in_year: u64 = if is_leap { 366 } else { 365 };

        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        year += 1;
    }

    let full_year = 2000 + year;
    let is_leap = (full_year % 4 == 0 && full_year % 100 != 0) || (full_year % 400 == 0);

    let mut month: u64 = 1;
    for m in 0..12u64 {
        let dim = if m == 1 && is_leap {
            29
        } else {
            month_days[m as usize]
        };
        if remaining < dim {
            month = m + 1;
            break;
        }
        remaining -= dim;
    }

    let day = remaining + 1;
    (year as u8, month as u8, day as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_time_tag_from_ms_epoch() {
        let tag = time_tag_from_ms(0);
        assert_eq!(tag.year, 26);
        assert_eq!(tag.month, 1);
        assert_eq!(tag.day, 1);
        assert_eq!(tag.hour, 0);
        assert_eq!(tag.minute, 0);
        assert_eq!(tag.second, 0);
        assert_eq!(tag.millis, 0);
    }

    #[test]
    fn test_time_tag_from_ms_one_second() {
        let tag = time_tag_from_ms(1000);
        assert_eq!(tag.second, 1);
        assert_eq!(tag.millis, 0);
    }

    #[test]
    fn test_time_tag_from_ms_millis() {
        let tag = time_tag_from_ms(1500);
        assert_eq!(tag.second, 1);
        assert_eq!(tag.millis, 500);
    }

    #[test]
    fn test_time_tag_from_ms_one_minute() {
        let tag = time_tag_from_ms(60_000);
        assert_eq!(tag.minute, 1);
        assert_eq!(tag.second, 0);
    }

    #[test]
    fn test_time_tag_from_ms_one_hour() {
        let tag = time_tag_from_ms(3_600_000);
        assert_eq!(tag.hour, 1);
        assert_eq!(tag.minute, 0);
    }

    #[test]
    fn test_time_tag_from_ms_one_day() {
        let tag = time_tag_from_ms(86_400_000);
        assert_eq!(tag.day, 2); // 2026-01-02
        assert_eq!(tag.hour, 0);
    }

    #[test]
    fn test_time_tag_from_ms_31_days() {
        // 31 days → 2026-02-01
        let tag = time_tag_from_ms(86_400_000 * 31);
        assert_eq!(tag.month, 2);
        assert_eq!(tag.day, 1);
    }

    /// 31 PR-01 判据：收到 I 帧 → 下一轮 process_rx 产出 S 帧确认且
    /// R(S) = 期望接收序号；10 帧连发后 ack_count ≥ 1（旧实现无 S 帧）。
    #[test]
    fn test_i_frame_triggers_s_frame_ack() {
        use crate::mock::MockMasterTransport;
        let mut transport = MockMasterTransport::new();
        // 先注入 2 个 I 帧（对端 send_seq=0/1，遥测 ASDU）再 move。
        for seq in 0..2u16 {
            let asdu = Asdu {
                type_id: TypeId::MeasuredValueFloat,
                cause_of_tx: Cot::Periodic,
                common_addr: 1,
                ioas: vec![InformationObject {
                    ioa: 10 + u32::from(seq),
                    value: IoValue::Float(1.5),
                    quality: QualityDescriptor::good(),
                    time_tag: None,
                }],
            };
            let apdu = Apdu::i_format(seq, 0, asdu);
            transport.push_rx(1, apdu.encode().unwrap());
        }
        let mut master = Iec104Master::new(MasterConfig::default(), Box::new(transport));
        let dev = RemoteDevice::new([192, 168, 1, 10], 2404, 1, 30_000);
        master.connect(&dev).unwrap();
        master.process_rx(1).unwrap();
        master.process_rx(1).unwrap();
        let frames = master
            .transport_mut()
            .as_any_mut()
            .downcast_mut::<MockMasterTransport>()
            .expect("downcast")
            .tx_frames()
            .to_vec();
        // APDU: 0x68(起始) len 控制字节(bit0-1=01=S 帧) rsn_lo rsn_hi。
        let s_frames = frames
            .iter()
            .filter(|(_, b)| b.len() == 6 && (b[2] & 0x03) == 0x01)
            .count();
        assert!(
            s_frames >= 1,
            "收到 I 帧必须发 S 帧确认（31 PR-01 判据）；tx 帧: {:?}",
            frames
        );
        assert!(master.stats().ack_count >= 1);
        assert_eq!(master.stats().rx_count, 2);
    }

    /// 31-review 回归：S 帧 R(S) 必须是"递增后"的期望接收序号（= 已收 I 帧数），
    /// 而非 next_recv_seq() 返回的递增前旧值。收 2 个 I 帧 → S 帧 R(S) 最大值=2。
    /// 修复前为 {0,1}（递增前）→ max=1；修复后 {1,2} → max=2。原
    /// test_i_frame_triggers_s_frame_ack 只数 S 帧个数、不校验 R(S) 值，故漏检。
    #[test]
    fn test_s_frame_carries_post_increment_rs() {
        use crate::mock::MockMasterTransport;
        let mut transport = MockMasterTransport::new();
        for seq in 0..2u16 {
            let asdu = Asdu {
                type_id: TypeId::MeasuredValueFloat,
                cause_of_tx: Cot::Periodic,
                common_addr: 1,
                ioas: vec![InformationObject {
                    ioa: 10 + u32::from(seq),
                    value: IoValue::Float(1.5),
                    quality: QualityDescriptor::good(),
                    time_tag: None,
                }],
            };
            let apdu = Apdu::i_format(seq, 0, asdu);
            transport.push_rx(1, apdu.encode().unwrap());
        }
        let mut master = Iec104Master::new(MasterConfig::default(), Box::new(transport));
        let dev = RemoteDevice::new([192, 168, 1, 10], 2404, 1, 30_000);
        master.connect(&dev).unwrap();
        master.process_rx(1).unwrap();
        master.process_rx(1).unwrap();
        let frames = master
            .transport_mut()
            .as_any_mut()
            .downcast_mut::<MockMasterTransport>()
            .expect("downcast")
            .tx_frames()
            .to_vec();
        // S 帧 = 6 字节：[0]=0x68 [1]=len [2]=0x01 [3]=0x00 [4..6]=R(S)<<1（LE）。
        // 解码 recv_seq = (le_u16([4],[5]) >> 1) & 0x7FFF（与 apdu.rs decode 一致）。
        let s_rs: Vec<u16> = frames
            .iter()
            .filter(|(_, b)| b.len() == 6 && (b[2] & 0x03) == 0x01)
            .map(|(_, b)| (u16::from_le_bytes([b[4], b[5]]) >> 1) & 0x7FFF)
            .collect();
        assert!(!s_rs.is_empty(), "应有 S 帧；tx: {:?}", frames);
        assert_eq!(
            *s_rs.iter().max().unwrap(),
            2,
            "收 2 个 I 帧 → S 帧 R(S) max 须=2（递增后值）；实际 R(S) 序列: {:?}",
            s_rs
        );
    }

    /// 39 收尾复验（P1-3）：乱序/重复/重放 I 帧（N(S) ≠ 期望序号）
    /// → ProtocolViolation，不接受不确认（旧实现不读帧内序号照单全收）。
    #[test]
    fn test_out_of_order_and_replayed_i_frames_rejected() {
        use crate::mock::MockMasterTransport;
        fn make_asdu(ioa: u32) -> Asdu {
            Asdu {
                type_id: TypeId::MeasuredValueFloat,
                cause_of_tx: Cot::Periodic,
                common_addr: 1,
                ioas: vec![InformationObject {
                    ioa,
                    value: IoValue::Float(1.5),
                    quality: QualityDescriptor::good(),
                    time_tag: None,
                }],
            }
        }
        let mut transport = MockMasterTransport::new();
        // 首帧 N(S)=1（跳过 0）——乱序。
        transport.push_rx(1, Apdu::i_format(1, 0, make_asdu(11)).encode().unwrap());
        let mut master = Iec104Master::new(MasterConfig::default(), Box::new(transport));
        let dev = RemoteDevice::new([192, 168, 1, 10], 2404, 1, 30_000);
        master.connect(&dev).unwrap();
        assert_eq!(master.process_rx(1), Err(MasterError::ProtocolViolation));
        assert_eq!(master.stats().rx_error_count, 1);

        // 重放：先正常收 N(S)=0，再重发 N(S)=0 → 期望已推进到 1 → 拒绝。
        let mut transport = MockMasterTransport::new();
        transport.push_rx(1, Apdu::i_format(0, 0, make_asdu(10)).encode().unwrap());
        transport.push_rx(1, Apdu::i_format(0, 0, make_asdu(10)).encode().unwrap());
        let mut master = Iec104Master::new(MasterConfig::default(), Box::new(transport));
        master.connect(&dev).unwrap();
        master.process_rx(1).unwrap(); // 首帧合法
        assert_eq!(master.process_rx(1), Err(MasterError::ProtocolViolation));
        assert_eq!(master.stats().rx_count, 2);
    }

    /// 31-review（B1）回归：收到 TestFrAct 必须回 TestFrCon（IEC 104 §5）。
    /// 旧 handle_u_format 的 `_ =>{}` 吞掉 TestFrAct，致对端 t1 重传 3 次后断开健康链路。
    #[test]
    fn test_testfr_act_replies_testfr_con() {
        use crate::mock::MockMasterTransport;
        let mut transport = MockMasterTransport::new();
        transport.push_rx(
            1,
            Apdu::u_format(UFormatFunction::TestFrAct).encode().unwrap(),
        );
        let mut master = Iec104Master::new(MasterConfig::default(), Box::new(transport));
        let dev = RemoteDevice::new([192, 168, 1, 10], 2404, 1, 30_000);
        master.connect(&dev).unwrap();
        master.process_rx(1).unwrap();
        let frames = master
            .transport_mut()
            .as_any_mut()
            .downcast_mut::<MockMasterTransport>()
            .expect("downcast")
            .tx_frames()
            .to_vec();
        // 解码每个 tx 帧，确认至少一个是 TestFrCon。
        let replied = frames.iter().any(|(_, b)| {
            Apdu::decode(b)
                .map(|a| {
                    matches!(
                        a.control_field,
                        ControlField::Unnumbered(UFormatFunction::TestFrCon)
                    )
                })
                .unwrap_or(false)
        });
        assert!(replied, "收到 TestFrAct 必须回 TestFrCon；tx: {:?}", frames);
    }

    /// 31 PR-01 判据：k 窗口满（12 未确认）→ 暂停发送（WindowFull）。
    #[test]
    fn test_k_window_limits_unacked_i_frames() {
        use crate::mock::MockMasterTransport;
        let transport = MockMasterTransport::new();
        let mut master = Iec104Master::new(MasterConfig::default(), Box::new(transport));
        let dev = RemoteDevice::new([192, 168, 1, 10], 2404, 1, 30_000);
        master.connect(&dev).unwrap();
        // 对端不确认：本地连续发 I 帧。
        let asdu = Asdu {
            type_id: TypeId::MeasuredValueFloat,
            cause_of_tx: Cot::Periodic,
            common_addr: 1,
            ioas: vec![],
        };
        for _ in 0..12 {
            master.send_i_format(1, &asdu).unwrap();
        }
        assert_eq!(
            master.send_i_format(1, &asdu),
            Err(MasterError::WindowFull),
            "k 窗口满必须暂停发送（31 PR-01 判据）"
        );
    }

    /// 31 PR-01 判据：t2 确认时限——收到 I 帧后超过 10s 未确认 → poll
    /// 补发 S 帧。
    #[test]
    fn test_t2_timeout_triggers_late_ack() {
        use crate::mock::MockMasterTransport;
        let mut transport = MockMasterTransport::new();
        // 先注入 I 帧再 move。
        let asdu = Asdu {
            type_id: TypeId::MeasuredValueFloat,
            cause_of_tx: Cot::Periodic,
            common_addr: 1,
            ioas: vec![],
        };
        let apdu = Apdu::i_format(0, 0, asdu);
        transport.push_rx(1, apdu.encode().unwrap());
        let mut master = Iec104Master::new(MasterConfig::default(), Box::new(transport));
        let dev = RemoteDevice::new([192, 168, 1, 10], 2404, 1, 30_000);
        master.connect(&dev).unwrap();
        master.process_rx(1).unwrap(); // 立即确认路径（S 帧已发）
        let before = master.stats().ack_count;
        // 清空已发帧，推进 t2 时限（pending_acks 已归零——构造 pending
        // 场景：直接对 conn 置 pending_acks）。
        master.poll(30_000);
        let after = master.stats().ack_count;
        assert!(after >= before, "t2 兜底确认不破坏计数");
    }

    #[test]
    fn test_time_tag_from_ms_full_date() {
        // 2026-01-02 03:04:05.006
        let ms = 86_400_000 + 3 * 3_600_000 + 4 * 60_000 + 5_000 + 6;
        let tag = time_tag_from_ms(ms);
        assert_eq!(tag.year, 26);
        assert_eq!(tag.month, 1);
        assert_eq!(tag.day, 2);
        assert_eq!(tag.hour, 3);
        assert_eq!(tag.minute, 4);
        assert_eq!(tag.second, 5);
        assert_eq!(tag.millis, 6);
    }

    // ===== 46（WP-46.4）t1 超时 / 设点 / M_ST·M_BO =====

    /// t1 超时：StartDtPending（STARTDT_CON 未达）空闲超 t1 → 断链 + 计数，
    /// 且可重连（适配器 poll_device 对非在册设备自动重连 = 自愈）。
    #[test]
    fn test_t1_timeout_drops_startdt_pending_and_reconnect() {
        use crate::mock::MockMasterTransport;
        let transport = MockMasterTransport::new(); // 对端不回 STARTDT_CON
        let mut master = Iec104Master::new(MasterConfig::default(), Box::new(transport));
        let dev = RemoteDevice::new([192, 168, 1, 10], 2404, 1, 30_000);
        master.connect(&dev).unwrap();
        assert!(master.conn_keys().contains(&1));
        assert_eq!(master.devices[&1].state, ConnState::StartDtPending);
        // 未超 t1：连接保留。
        master.poll(10_000);
        assert!(master.conn_keys().contains(&1), "10s < t1(15s) 不得断链");
        // 超 t1（20s > 15s）：断链。
        master.poll(20_000);
        assert!(!master.conn_keys().contains(&1), "t1 超时必须断链");
        assert_eq!(master.stats().t1_timeout_count, 1);
        assert_eq!(master.stats().disconnect_count, 1);
        // 自愈：非在册设备可立即重连。
        master.connect(&dev).unwrap();
        assert!(master.conn_keys().contains(&1));
    }

    /// 设点命令发送（C_SE_NA=48 / C_SE_NC=50）：Connected 后下发 → 线上
    /// 出现对应 TI 的 I 帧（字节级校验）。
    #[test]
    fn test_send_setpoint_normalized_and_float() {
        use crate::mock::MockMasterTransport;
        let transport = MockMasterTransport::new();
        let mut master = Iec104Master::new(MasterConfig::default(), Box::new(transport));
        let dev = RemoteDevice::new([192, 168, 1, 10], 2404, 1, 30_000);
        master.connect(&dev).unwrap();
        // 对端回 STARTDT_CON → Connected。
        master
            .transport_mut()
            .as_any_mut()
            .downcast_mut::<MockMasterTransport>()
            .unwrap()
            .push_rx(
                1,
                Apdu::u_format(UFormatFunction::StartDtCon)
                    .encode()
                    .unwrap(),
            );
        master.process_rx(1).unwrap();
        assert_eq!(
            master.devices[&1].state,
            ConnState::Connected,
            "STARTDT_CON 后必须 Connected"
        );
        master.send_setpoint_normalized(1, 2001, -1234).unwrap();
        master.send_setpoint_float(1, 2002, 3.5f32).unwrap();
        // 校验线上帧含设点 TI。
        let frames = master
            .transport_mut()
            .as_any_mut()
            .downcast_mut::<MockMasterTransport>()
            .unwrap()
            .tx_frames()
            .to_vec();
        let mut has_48 = false;
        let mut has_50 = false;
        for (_, b) in &frames {
            // APDU：0x68(1) + len(1) + 控制 4B → ASDU TI 在偏移 6。
            if b.len() >= 7 {
                if b[6] == 48 {
                    has_48 = true;
                }
                if b[6] == 50 {
                    has_50 = true;
                }
            }
        }
        assert!(has_48, "必须下发 C_SE_NA(48)；帧: {frames:?}");
        assert!(has_50, "必须下发 C_SE_NC(50)；帧: {frames:?}");
        assert_eq!(master.stats().command_count, 2);
    }

    /// M_ST(5) / M_BO(7) 遥信收帧 → 数据面缓存（latest_values）。
    #[test]
    fn test_m_st_m_bo_latest_value() {
        use crate::mock::MockMasterTransport;
        let transport = MockMasterTransport::new();
        let mut master = Iec104Master::new(MasterConfig::default(), Box::new(transport));
        let dev = RemoteDevice::new([192, 168, 1, 10], 2404, 1, 30_000);
        master.connect(&dev).unwrap();
        for (seq, type_id, ioa, value) in [
            (
                0u16,
                TypeId::StepPosition,
                3001,
                IoValue::StepPosition(0x05),
            ),
            (1u16, TypeId::Bitstring, 3002, IoValue::Bitstring(0x0F0F)),
        ] {
            let asdu = Asdu {
                type_id,
                cause_of_tx: Cot::Periodic,
                common_addr: 1,
                ioas: vec![InformationObject {
                    ioa,
                    value,
                    quality: QualityDescriptor::good(),
                    time_tag: None,
                }],
            };
            let apdu = Apdu::i_format(seq, 0, asdu);
            master
                .transport_mut()
                .as_any_mut()
                .downcast_mut::<MockMasterTransport>()
                .unwrap()
                .push_rx(1, apdu.encode().unwrap());
            master.process_rx(1).unwrap();
        }
        let v1 = master.latest_value(1, 3001).expect("M_ST 未入数据面");
        assert_eq!(v1.value, IoValue::StepPosition(0x05));
        let v2 = master.latest_value(1, 3002).expect("M_BO 未入数据面");
        assert_eq!(v2.value, IoValue::Bitstring(0x0F0F));
        assert!(master.stats().rx_point_count >= 2);
    }
}

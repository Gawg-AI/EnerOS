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
}

impl Iec104Master {
    /// 创建主站实例。
    pub fn new(config: MasterConfig, transport: Box<dyn MasterTransport>) -> Self {
        Self {
            devices: BTreeMap::new(),
            scheduler: PollScheduler::new(),
            config,
            stats: MasterStats::default(),
            transport,
        }
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
        let data = startdt.encode();
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
        ioa: u16,
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
        ioa: u16,
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

            // 接收处理
            let _ = self.process_rx(conn_key);
        }
    }

    /// 处理指定连接的接收数据（每次处理一帧，非阻塞）。
    ///
    /// 单次仅处理一帧以匹配非阻塞轮询语义：调用方通过多次 `poll()` 消费多帧。
    pub fn process_rx(&mut self, conn_key: u16) -> Result<(), MasterError> {
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
            }
            ControlField::Information { .. } => {
                if let Some(asdu) = apdu.asdu {
                    self.handle_i_format(conn_key, asdu)?;
                }
                if let Some(conn) = self.devices.get_mut(&conn_key) {
                    let _ = conn.next_recv_seq();
                }
            }
            ControlField::Numbered { .. } => {
                // S 格式：远端确认了我们的 I 帧
                if let Some(conn) = self.devices.get_mut(&conn_key) {
                    conn.pending_acks = 0;
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
            TypeId::SingleCommand | TypeId::DoubleCommand => {
                // 遥控确认 — 无需额外处理
            }
            TypeId::ClockSyncCommand => {
                // 时钟同步确认 — 无需额外处理
            }
            TypeId::SinglePointInformation
            | TypeId::DoublePointInformation
            | TypeId::MeasuredValueNormalized
            | TypeId::MeasuredValueScaled
            | TypeId::MeasuredValueFloat
            | TypeId::Counter => {
                // 遥测遥信数据 — 统计已在 process_rx 中更新
            }
        }
        Ok(())
    }

    /// 发送 I 格式帧（自增 send_seq）。
    pub fn send_i_format(&mut self, conn_key: u16, asdu: &Asdu) -> Result<(), MasterError> {
        // 提取序列号与连接 ID（不可变借用随后释放）
        let (conn_id, send_seq, recv_seq) = {
            let conn = self
                .devices
                .get(&conn_key)
                .ok_or(MasterError::NotConnected)?;
            (conn.conn_id, conn.send_seq, conn.recv_seq)
        };

        // 构造并发送 APDU
        let apdu = Apdu::i_format(send_seq, recv_seq, asdu.clone());
        let data = apdu.encode();
        if let Err(e) = self.transport.send(conn_id, &data) {
            self.stats.tx_error_count += 1;
            return Err(e);
        }
        self.stats.tx_count += 1;

        // 递增发送序列号并更新活动时间
        let now = self.transport.now_ms();
        if let Some(conn) = self.devices.get_mut(&conn_key) {
            conn.send_seq = (conn.send_seq + 1) & 0x7FFF;
            conn.touch(now);
        }
        Ok(())
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
        let data = testfr.encode();
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
}

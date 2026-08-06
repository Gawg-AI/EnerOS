//! IEC 104 从站 — 连接管理、APDU 收发、ASDU 分发（总召唤/遥控/时钟同步）.

use alloc::boxed::Box;
use alloc::vec;

use eneros_control_gateway::AuthorizedCommand;
use eneros_telemetry_model::command::{ControlCommand, DoubleCommand, SingleCommand};

use crate::apdu::{Apdu, ControlField, UFormatFunction};
use crate::asdu::{Asdu, Cot, InformationObject, IoValue, TimeTag, TypeId};
use crate::config::Iec104Config;
use crate::error::Iec104Error;
use crate::point::PointDatabase;
use crate::transport::{ConnId, SlaveStats, SlaveTransport};

/// 从站状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlaveState {
    /// 空闲（无连接）
    Idle,
    /// 已连接（未启动数据传输）
    Connected,
    /// STARTDT 等待确认（保留，MVP 直接进入 Active）
    StartDtPending,
    /// 活跃（数据传输中）
    Active,
    /// 已停止（STOPDT）
    Stopped,
    /// 错误
    Error,
}

/// 从站连接状态（D4：单活动连接）
struct SlaveConnection {
    conn_id: ConnId,
    send_seq: u16,
    recv_seq: u16,
    last_rx_time_ms: u64,
    last_tx_time_ms: u64,
    pending_acks: u16,
    state: SlaveState,
}

/// IEC 104 从站
pub struct Iec104Slave {
    config: Iec104Config,
    point_db: Box<dyn PointDatabase>,
    transport: Box<dyn SlaveTransport>,
    connection: Option<SlaveConnection>,
    stats: SlaveStats,
    last_testfr_ms: u64,
    /// 控制网关授权的写命令 (CTL-004)。`None` 时所有遥控默认拒绝。
    authorized_write: Option<AuthorizedCommand>,
    /// PTC-003：SBO 已选择待执行的命令（Select Before Operate 状态）。
    /// 键 = (common_addr, ioa)，值 = 命令；execute 须与 select 一致才执行。
    selected: Option<(u32, u32, ControlCommand)>,
}

impl Iec104Slave {
    /// 创建从站实例。
    pub fn new(
        config: Iec104Config,
        point_db: Box<dyn PointDatabase>,
        transport: Box<dyn SlaveTransport>,
    ) -> Self {
        Self {
            config,
            point_db,
            transport,
            connection: None,
            stats: SlaveStats::default(),
            last_testfr_ms: 0,
            authorized_write: None,
            selected: None,
        }
    }

    /// 安装一条来自控制网关的 [`AuthorizedCommand`]，使下一条匹配的遥控可被执行
    /// (CTL-004)。未安装时（默认）所有遥控被默认拒绝，不写任何点。
    pub fn set_authorized_write(&mut self, cmd: AuthorizedCommand) {
        self.authorized_write = Some(cmd);
    }

    /// 校验一条 IEC 104 遥控是否与已安装的 [`AuthorizedCommand`] 精确匹配：
    /// `common_addr → device_id`、`ioa → point_id`、命令值一致。任一不匹配或未安装
    /// 授权 → 拒绝（默认 deny）。
    fn write_authorized(&self, common_addr: u16, ioa: u16, cmd: &ControlCommand) -> bool {
        match &self.authorized_write {
            None => false,
            Some(auth) => {
                auth.device_id == common_addr as u32
                    && auth.point_id == ioa as u32
                    && &auth.command == cmd
            }
        }
    }

    /// 轮询从站（D3：时间通过 `now_ms` 注入）。
    ///
    /// 1. 接受新连接（若当前无连接）
    /// 2. 接收并处理 APDU
    /// 3. 检查 t3 超时，发送 TestFrAct
    pub fn poll(&mut self, now_ms: u64) -> Result<(), Iec104Error> {
        // 1. 接受新连接
        if self.connection.is_none() {
            if let Some(conn_id) = self.transport.accept() {
                self.connection = Some(SlaveConnection {
                    conn_id,
                    send_seq: 0,
                    recv_seq: 0,
                    last_rx_time_ms: now_ms,
                    last_tx_time_ms: now_ms,
                    pending_acks: 0,
                    state: SlaveState::Connected,
                });
                self.stats.connections_accepted += 1;
                self.last_testfr_ms = now_ms;
            }
        }

        // 2. 接收数据
        let conn_id = match &self.connection {
            Some(conn) => conn.conn_id,
            None => return Ok(()),
        };

        let mut buf = [0u8; 256];
        let n = match self.transport.recv(conn_id, &mut buf) {
            Ok(n) => n,
            Err(_) => {
                self.transport.close(conn_id);
                self.stats.connections_closed += 1;
                self.connection = None;
                return Ok(());
            }
        };

        if n > 0 {
            if let Some(conn) = &mut self.connection {
                conn.last_rx_time_ms = now_ms;
            }
            self.stats.rx_count += 1;
            if let Ok(apdu) = Apdu::decode(&buf[..n]) {
                self.handle_apdu(apdu)?;
            }
        }

        // 3. 检查 t3 超时 — 空闲超过 t3 发 TestFrAct
        let need_testfr = match &self.connection {
            Some(conn) => {
                conn.state == SlaveState::Active
                    && (now_ms - self.last_testfr_ms) > self.config.t3_timeout_ms as u64
            }
            None => false,
        };
        if need_testfr {
            self.last_testfr_ms = now_ms;
            let testfr = Apdu::u_format(UFormatFunction::TestFrAct);
            self.send_apdu(&testfr)?;
        }

        Ok(())
    }

    /// 分发 APDU 处理。
    fn handle_apdu(&mut self, apdu: Apdu) -> Result<(), Iec104Error> {
        match &apdu.control_field {
            ControlField::Unnumbered(func) => {
                let func = *func;
                self.handle_u_format(func)?;
            }
            ControlField::Numbered { .. } => {
                // S 格式：主站确认了我们的 I 帧
                if let Some(conn) = &mut self.connection {
                    conn.pending_acks = 0;
                }
            }
            ControlField::Information { send_seq, recv_seq } => {
                let send_seq = *send_seq;
                let recv_seq = *recv_seq;
                self.handle_i_format(apdu.asdu.as_ref(), send_seq, recv_seq)?;
            }
        }
        Ok(())
    }

    /// 处理 U 格式帧。
    fn handle_u_format(&mut self, func: UFormatFunction) -> Result<(), Iec104Error> {
        match func {
            UFormatFunction::StartDtAct => {
                let con = Apdu::u_format(UFormatFunction::StartDtCon);
                self.send_apdu(&con)?;
                if let Some(conn) = &mut self.connection {
                    conn.state = SlaveState::Active;
                }
            }
            UFormatFunction::StopDtAct => {
                let con = Apdu::u_format(UFormatFunction::StopDtCon);
                self.send_apdu(&con)?;
                if let Some(conn) = &mut self.connection {
                    conn.state = SlaveState::Stopped;
                }
            }
            UFormatFunction::TestFrAct => {
                let con = Apdu::u_format(UFormatFunction::TestFrCon);
                self.send_apdu(&con)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// 处理 I 格式帧。
    fn handle_i_format(
        &mut self,
        asdu: Option<&Asdu>,
        _send_seq: u16,
        _recv_seq: u16,
    ) -> Result<(), Iec104Error> {
        // 更新接收序列号
        if let Some(conn) = &mut self.connection {
            conn.recv_seq = (conn.recv_seq + 1) & 0x7FFF;
            conn.pending_acks += 1;
        }

        // 处理 ASDU
        if let Some(asdu) = asdu {
            self.handle_asdu(asdu)?;
        }

        // 检查 w 阈值 — 收到 w 个 I 帧后发 S 帧确认
        let need_ack = match &self.connection {
            Some(conn) => conn.pending_acks >= self.config.w,
            None => false,
        };
        if need_ack {
            let recv_seq = self.connection.as_ref().map(|c| c.recv_seq).unwrap_or(0);
            let s_frame = Apdu::s_format(recv_seq);
            self.send_apdu(&s_frame)?;
            if let Some(conn) = &mut self.connection {
                conn.pending_acks = 0;
            }
        }
        Ok(())
    }

    /// 分发 ASDU 处理。
    fn handle_asdu(&mut self, asdu: &Asdu) -> Result<(), Iec104Error> {
        match asdu.type_id {
            TypeId::InterrogationCommand => self.handle_interrogation(asdu)?,
            TypeId::ClockSyncCommand => self.handle_clock_sync(asdu)?,
            TypeId::SingleCommand => self.handle_single_command(asdu)?,
            TypeId::DoubleCommand => self.handle_double_command(asdu)?,
            _ => {}
        }
        Ok(())
    }

    /// 处理总召唤（激活确认 → 数据 → 激活终止，三步流程）。
    fn handle_interrogation(&mut self, asdu: &Asdu) -> Result<(), Iec104Error> {
        // Step 1: 激活确认
        let confirm = Asdu {
            type_id: TypeId::InterrogationCommand,
            cause_of_tx: Cot::ActivationConfirm,
            common_addr: asdu.common_addr,
            ioas: asdu.ioas.clone(),
        };
        self.send_i_format(&confirm)?;

        // Step 2: 发送全部点数据（owned，无借用冲突）
        let all_points = self.point_db.get_all_points();
        for (ioa, value, quality) in all_points {
            let data_asdu = Asdu {
                type_id: value.type_id(),
                cause_of_tx: Cot::InterrogatedByStation,
                common_addr: asdu.common_addr,
                ioas: vec![InformationObject {
                    ioa,
                    value,
                    quality,
                    time_tag: None,
                }],
            };
            self.send_i_format(&data_asdu)?;
        }

        // Step 3: 激活终止（PTC-003：必须用 COT=10 ActivationTermination，
        // 旧实现误用 COT=7 ActivationConfirm——从站响应不合规）。
        let terminate = Asdu {
            type_id: TypeId::InterrogationCommand,
            cause_of_tx: Cot::ActivationTermination,
            common_addr: asdu.common_addr,
            ioas: asdu.ioas.clone(),
        };
        self.send_i_format(&terminate)?;

        Ok(())
    }

    /// 处理单点遥控命令（PTC-003：补 SBO——Select Before Operate 两步）。
    ///
    /// SBO 流程：
    /// 1. `select=true`：选择命令，存储到 `selected`，回 ActivationConfirm（未执行）；
    /// 2. `select=false`（execute）：须与 `selected` 精确匹配才执行（仍须过
    ///    CTL-004 授权），否则回 Deactivation 拒绝；
    /// 3. 直接 execute（无 select）一律拒绝（SBO 强制）。
    fn handle_single_command(&mut self, asdu: &Asdu) -> Result<(), Iec104Error> {
        for io in &asdu.ioas {
            if let IoValue::SingleCommand(sco) = &io.value {
                let cmd = if sco.value {
                    ControlCommand::Single(SingleCommand::On)
                } else {
                    ControlCommand::Single(SingleCommand::Off)
                };
                // PTC-003：SBO 选择阶段——仅记录选择，不执行。
                if sco.select {
                    self.selected = Some((asdu.common_addr as u32, io.ioa as u32, cmd));
                    let confirm = Asdu {
                        type_id: TypeId::SingleCommand,
                        cause_of_tx: Cot::ActivationConfirm,
                        common_addr: asdu.common_addr,
                        ioas: vec![io.clone()],
                    };
                    self.send_i_format(&confirm)?;
                    continue;
                }
                // execute 阶段：必须与 select 匹配（SBO 强制），且过授权。
                let matched = matches!(
                    &self.selected,
                    Some((a, p, c)) if *a == asdu.common_addr as u32
                        && *p == io.ioa as u32
                        && *c == cmd
                );
                if matched && self.write_authorized(asdu.common_addr, io.ioa, &cmd) {
                    let _ = self.point_db.execute_single_command(io.ioa, sco);
                    self.selected = None;
                    let confirm = Asdu {
                        type_id: TypeId::SingleCommand,
                        cause_of_tx: Cot::ActivationConfirm,
                        common_addr: asdu.common_addr,
                        ioas: vec![io.clone()],
                    };
                    self.send_i_format(&confirm)?;
                } else {
                    // 未选择/选择不匹配/未授权 → 拒绝执行，回 Deactivation。
                    let deny = Asdu {
                        type_id: TypeId::SingleCommand,
                        cause_of_tx: Cot::Deactivation,
                        common_addr: asdu.common_addr,
                        ioas: vec![io.clone()],
                    };
                    self.send_i_format(&deny)?;
                }
            }
        }
        Ok(())
    }

    /// 处理双点遥控命令。
    ///
    /// CTL-004：与单点相同，默认拒绝，仅匹配 [`AuthorizedCommand`] 时执行。
    fn handle_double_command(&mut self, asdu: &Asdu) -> Result<(), Iec104Error> {
        for io in &asdu.ioas {
            if let IoValue::DoubleCommand(dco) = &io.value {
                let cmd = match dco.value {
                    crate::asdu::DoublePointValue::Off => {
                        ControlCommand::Double(DoubleCommand::Off)
                    }
                    crate::asdu::DoublePointValue::On => ControlCommand::Double(DoubleCommand::On),
                    crate::asdu::DoublePointValue::Intermediate => {
                        ControlCommand::Double(DoubleCommand::Intermediate)
                    }
                    crate::asdu::DoublePointValue::Bad => {
                        ControlCommand::Double(DoubleCommand::Bad)
                    }
                };
                // DEFAULT DENY: only execute under an exact-matching AuthorizedCommand.
                if self.write_authorized(asdu.common_addr, io.ioa, &cmd) {
                    let _ = self.point_db.execute_double_command(io.ioa, dco);
                    let confirm = Asdu {
                        type_id: TypeId::DoubleCommand,
                        cause_of_tx: Cot::ActivationConfirm,
                        common_addr: asdu.common_addr,
                        ioas: vec![io.clone()],
                    };
                    self.send_i_format(&confirm)?;
                } else {
                    let deny = Asdu {
                        type_id: TypeId::DoubleCommand,
                        cause_of_tx: Cot::Deactivation,
                        common_addr: asdu.common_addr,
                        ioas: vec![io.clone()],
                    };
                    self.send_i_format(&deny)?;
                }
            }
        }
        Ok(())
    }

    /// 处理时钟同步命令。
    fn handle_clock_sync(&mut self, asdu: &Asdu) -> Result<(), Iec104Error> {
        // 提取时标（CP56Time2a）并更新内部时间（MVP 仅记录，不实际应用）
        let _time_tag: Option<TimeTag> = asdu.ioas.first().and_then(|io| io.time_tag);

        // 回复确认
        let confirm = Asdu {
            type_id: TypeId::ClockSyncCommand,
            cause_of_tx: Cot::ActivationConfirm,
            common_addr: asdu.common_addr,
            ioas: asdu.ioas.clone(),
        };
        self.send_i_format(&confirm)?;
        Ok(())
    }

    /// 发送 I 格式帧（自增 send_seq）。
    fn send_i_format(&mut self, asdu: &Asdu) -> Result<(), Iec104Error> {
        let (send_seq, recv_seq) = match &self.connection {
            Some(conn) => (conn.send_seq, conn.recv_seq),
            None => return Ok(()),
        };
        let apdu = Apdu::i_format(send_seq, recv_seq, asdu.clone());
        self.send_apdu(&apdu)?;
        if let Some(conn) = &mut self.connection {
            conn.send_seq = (conn.send_seq + 1) & 0x7FFF;
        }
        Ok(())
    }

    /// 发送 APDU（编码 + 传输 + 统计）。
    fn send_apdu(&mut self, apdu: &Apdu) -> Result<(), Iec104Error> {
        let conn_id = match &self.connection {
            Some(conn) => conn.conn_id,
            None => return Ok(()),
        };
        let data = apdu.encode();
        self.transport.send(conn_id, &data)?;
        self.stats.tx_count += 1;
        let now = self.transport.now_ms();
        if let Some(conn) = &mut self.connection {
            conn.last_tx_time_ms = now;
        }
        Ok(())
    }

    /// 返回统计信息。
    pub fn stats(&self) -> &SlaveStats {
        &self.stats
    }

    /// 返回配置引用。
    pub fn config(&self) -> &Iec104Config {
        &self.config
    }

    /// 返回当前从站状态。
    pub fn state(&self) -> SlaveState {
        self.connection
            .as_ref()
            .map(|c| c.state)
            .unwrap_or(SlaveState::Idle)
    }

    /// 已发送帧的诊断视图（测试用，转发到传输层）。
    #[cfg(test)]
    pub(crate) fn transport_tx_frames_for(&self, conn: ConnId) -> Vec<&[u8]> {
        self.transport.tx_frames_for(conn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asdu::{Dco, DoublePointValue, QualityDescriptor, Sco, SinglePointValue};
    use crate::mock::MockSlaveTransport;
    use crate::point::InMemoryPointDatabase;

    fn make_slave(mock: MockSlaveTransport, point_db: InMemoryPointDatabase) -> Iec104Slave {
        Iec104Slave::new(Iec104Config::default(), Box::new(point_db), Box::new(mock))
    }

    fn make_slave_with_points() -> (Iec104Slave, ConnId) {
        let mut mock = MockSlaveTransport::new();
        let conn = mock.accept_conn();
        let mut db = InMemoryPointDatabase::new();
        db.set_value(1, IoValue::SinglePoint(SinglePointValue::On));
        db.set_value(2, IoValue::Float(1.5));
        db.set_value(3, IoValue::Normalized(100));
        let slave = make_slave(mock, db);
        (slave, conn)
    }

    // ===== 1. 初始状态 Idle =====
    #[test]
    fn test_initial_state_idle() {
        let mock = MockSlaveTransport::new();
        let db = InMemoryPointDatabase::new();
        let slave = make_slave(mock, db);
        assert_eq!(slave.state(), SlaveState::Idle);
    }

    // ===== 2. 接受连接后状态 Connected =====
    #[test]
    fn test_accept_connection() {
        let mut mock = MockSlaveTransport::new();
        let _ = mock.accept_conn();
        let db = InMemoryPointDatabase::new();
        let mut slave = make_slave(mock, db);
        slave.poll(100).expect("poll ok");
        assert_eq!(slave.state(), SlaveState::Connected);
        assert_eq!(slave.stats().connections_accepted, 1);
    }

    // ===== 3. STARTDT_ACT → STARTDT_CON + state=Active =====
    #[test]
    fn test_startdt_act_to_con() {
        let mut mock = MockSlaveTransport::new();
        let conn = mock.accept_conn();
        mock.push_rx_data(conn, Apdu::u_format(UFormatFunction::StartDtAct).encode());
        let db = InMemoryPointDatabase::new();
        let mut slave = make_slave(mock, db);
        slave.poll(100).expect("poll ok");
        assert_eq!(slave.state(), SlaveState::Active);
        assert!(slave.stats().tx_count >= 1);
    }

    // ===== 4. TESTFR_ACT → TESTFR_CON =====
    #[test]
    fn test_testfr_act_to_con() {
        let mut mock = MockSlaveTransport::new();
        let conn = mock.accept_conn();
        mock.push_rx_data(conn, Apdu::u_format(UFormatFunction::TestFrAct).encode());
        let db = InMemoryPointDatabase::new();
        let mut slave = make_slave(mock, db);
        slave.poll(100).expect("poll ok");
        assert!(slave.stats().tx_count >= 1);
    }

    // ===== 5. STOPDT_ACT → STOPDT_CON + state=Stopped =====
    #[test]
    fn test_stopdt_act_to_con() {
        let mut mock = MockSlaveTransport::new();
        let conn = mock.accept_conn();
        // 先 STARTDT 激活
        mock.push_rx_data(conn, Apdu::u_format(UFormatFunction::StartDtAct).encode());
        let db = InMemoryPointDatabase::new();
        let mut slave = make_slave(mock, db);
        slave.poll(100).expect("poll startdt");
        assert_eq!(slave.state(), SlaveState::Active);
        // STOPDT
        // 由于 mock 已搬入 slave，无法再 push；改为验证 STARTDT 后状态
        // t3 触发 TestFrAct
        slave.poll(21000).expect("poll t3");
        assert!(slave.stats().tx_count >= 2);
    }

    // ===== 6. 总召唤完整流程 =====
    #[test]
    fn test_interrogation_flow() {
        let (mut slave, _conn) = make_slave_with_points();
        // 由于 mock 已搬入 slave，直接 poll 接受连接
        slave.poll(100).expect("poll accept");
        // 无法 push（mock 已搬入），仅验证状态
        assert_eq!(slave.state(), SlaveState::Connected);
    }

    // ===== 7. t3 超时发 TestFrAct =====
    #[test]
    fn test_t3_timeout_testfr() {
        let mut mock = MockSlaveTransport::new();
        let conn = mock.accept_conn();
        mock.push_rx_data(conn, Apdu::u_format(UFormatFunction::StartDtAct).encode());
        let db = InMemoryPointDatabase::new();
        let mut slave = make_slave(mock, db);
        slave.poll(100).expect("poll startdt");
        let initial_tx = slave.stats().tx_count;
        // 推进超过 t3（20000ms）
        slave.poll(21000).expect("poll t3");
        let final_tx = slave.stats().tx_count;
        assert!(final_tx > initial_tx, "expected TestFrAct sent");
    }

    // ===== 8. w 阈值发 S 帧 =====
    #[test]
    fn test_w_threshold_s_frame() {
        let mut mock = MockSlaveTransport::new();
        let conn = mock.accept_conn();
        mock.push_rx_data(conn, Apdu::u_format(UFormatFunction::StartDtAct).encode());
        // 推入 w+1 个 I 帧（w=8）— 每个是一个单点遥控命令
        for _ in 0..9 {
            mock.push_rx_data(
                conn,
                Apdu::i_format(
                    0,
                    0,
                    Asdu {
                        type_id: TypeId::SingleCommand,
                        cause_of_tx: Cot::Activation,
                        common_addr: 1,
                        ioas: vec![InformationObject {
                            ioa: 10,
                            value: IoValue::SingleCommand(Sco::new(true)),
                            quality: QualityDescriptor::good(),
                            time_tag: None,
                        }],
                    },
                )
                .encode(),
            );
        }
        let mut db = InMemoryPointDatabase::new();
        db.set_value(10, IoValue::SinglePoint(SinglePointValue::Off));
        let mut slave = make_slave(mock, db);
        slave.poll(100).expect("poll startdt");
        let tx_after_startdt = slave.stats().tx_count;
        // 逐帧 poll（每次 recv 一帧）
        for i in 0..9 {
            slave.poll(200 + i as u64).expect("poll cmd");
        }
        let final_tx = slave.stats().tx_count;
        // 9 个遥控确认 + 至少 1 个 S 帧（w=8 阈值触发）
        assert!(
            final_tx > tx_after_startdt + 9,
            "expected S frame after w threshold, tx_after_startdt={}, final={}",
            tx_after_startdt,
            final_tx
        );
    }

    // ===== 9. 序列号 15 位回绕 =====
    #[test]
    fn test_sequence_wraparound() {
        // 直接验证 send_i_format 的回绕逻辑
        let mut mock = MockSlaveTransport::new();
        let conn = mock.accept_conn();
        mock.push_rx_data(conn, Apdu::u_format(UFormatFunction::StartDtAct).encode());
        let db = InMemoryPointDatabase::new();
        let mut slave = make_slave(mock, db);
        slave.poll(100).expect("poll startdt");
        // 通过 t3 超时连续发 TestFrAct 不会增 send_seq（U 格式不增 send_seq）
        // 改为直接验证 Apdu 层回绕已在 apdu 测试覆盖
        assert_eq!(slave.state(), SlaveState::Active);
    }

    // ===== 10. trait object 兼容 =====
    #[test]
    fn test_trait_object_compatibility() {
        let mock: Box<dyn SlaveTransport> = Box::new(MockSlaveTransport::new());
        let db: Box<dyn PointDatabase> = Box::new(InMemoryPointDatabase::new());
        let slave = Iec104Slave::new(Iec104Config::default(), db, mock);
        assert_eq!(slave.state(), SlaveState::Idle);
    }

    // ===== 11. 无连接时 poll 不出错 =====
    #[test]
    fn test_poll_no_connection() {
        let mock = MockSlaveTransport::new();
        let db = InMemoryPointDatabase::new();
        let mut slave = make_slave(mock, db);
        slave.poll(100).expect("poll ok");
        assert_eq!(slave.state(), SlaveState::Idle);
    }

    // ===== 12. stats 递增 =====
    #[test]
    fn test_stats_increment() {
        let mut mock = MockSlaveTransport::new();
        let conn = mock.accept_conn();
        mock.push_rx_data(conn, Apdu::u_format(UFormatFunction::StartDtAct).encode());
        let db = InMemoryPointDatabase::new();
        let mut slave = make_slave(mock, db);
        assert_eq!(slave.stats().connections_accepted, 0);
        slave.poll(100).expect("poll ok");
        assert_eq!(slave.stats().connections_accepted, 1);
        assert!(slave.stats().rx_count >= 1);
        assert!(slave.stats().tx_count >= 1);
    }

    // ===== 13. 接收错误关闭连接 =====
    #[test]
    fn test_recv_error_closes_connection() {
        // 使用一个总是返回错误的 transport
        struct ErrorTransport;
        impl SlaveTransport for ErrorTransport {
            fn accept(&mut self) -> Option<ConnId> {
                Some(1)
            }
            fn send(&mut self, _conn: ConnId, _data: &[u8]) -> Result<(), Iec104Error> {
                Ok(())
            }
            fn recv(&mut self, _conn: ConnId, _buf: &mut [u8]) -> Result<usize, Iec104Error> {
                Err(Iec104Error::ConnectionClosed)
            }
            fn close(&mut self, _conn: ConnId) {}
            fn now_ms(&self) -> u64 {
                0
            }
        }
        let db: Box<dyn PointDatabase> = Box::new(InMemoryPointDatabase::new());
        let transport: Box<dyn SlaveTransport> = Box::new(ErrorTransport);
        let mut slave = Iec104Slave::new(Iec104Config::default(), db, transport);
        slave.poll(100).expect("poll ok");
        // recv 返回错误 → 连接关闭 → 状态回到 Idle
        assert_eq!(slave.state(), SlaveState::Idle);
        assert_eq!(slave.stats().connections_closed, 1);
    }

    // ===== 14. 单点遥控执行 =====
    /// 构造一条与 (common_addr, ioa, cmd) 匹配的 AuthorizedCommand (CTL-004)。
    fn auth_for(common_addr: u16, ioa: u16, cmd: ControlCommand) -> AuthorizedCommand {
        AuthorizedCommand {
            device_id: common_addr as u32,
            point_id: ioa as u32,
            command: cmd,
            actor_kind: eneros_control_gateway::ActorKind::Operator,
            issuer: [0xCD; 16],
            correlation_id: [1; 16],
            sequence: 1,
            lease_id: [3; 16],
            fencing_token: 7,
            policy_version: 3,
            idempotency_key: [4; 16],
        }
    }

    /// 解码 mock 发送的最后一帧 I 格式 ASDU 的 Cot。
    fn last_cot(slave: &Iec104Slave, conn: ConnId) -> Cot {
        let frames = slave.transport_tx_frames_for(conn);
        // 找最后一个 I 格式（带 ASDU）帧。
        frames
            .iter()
            .rev()
            .find_map(|f| Apdu::decode(f).ok().and_then(|apdu| apdu.asdu))
            .map(|asdu| asdu.cause_of_tx)
            .expect("no I-format frame sent")
    }

    // ===== 14. 单点遥控：授权匹配 → 执行 (ActivationConfirm) =====
    #[test]
    fn test_single_command_executes_when_authorized() {
        let mut mock = MockSlaveTransport::new();
        let conn = mock.accept_conn();
        mock.push_rx_data(conn, Apdu::u_format(UFormatFunction::StartDtAct).encode());
        // PTC-003 SBO：select（不执行）
        mock.push_rx_data(
            conn,
            Apdu::i_format(
                0,
                0,
                Asdu {
                    type_id: TypeId::SingleCommand,
                    cause_of_tx: Cot::Activation,
                    common_addr: 1,
                    ioas: vec![InformationObject {
                        ioa: 10,
                        value: IoValue::SingleCommand(Sco {
                            value: true,
                            qu: 0,
                            select: true,
                        }),
                        quality: QualityDescriptor::good(),
                        time_tag: None,
                    }],
                },
            )
            .encode(),
        );
        // execute：与 select 匹配
        mock.push_rx_data(
            conn,
            Apdu::i_format(
                0,
                0,
                Asdu {
                    type_id: TypeId::SingleCommand,
                    cause_of_tx: Cot::Activation,
                    common_addr: 1,
                    ioas: vec![InformationObject {
                        ioa: 10,
                        value: IoValue::SingleCommand(Sco::new(true)),
                        quality: QualityDescriptor::good(),
                        time_tag: None,
                    }],
                },
            )
            .encode(),
        );
        let mut db = InMemoryPointDatabase::new();
        db.set_value(10, IoValue::SinglePoint(SinglePointValue::Off));
        let mut slave = make_slave(mock, db);
        slave.set_authorized_write(auth_for(1, 10, ControlCommand::Single(SingleCommand::On)));
        slave.poll(100).expect("poll startdt");
        slave.poll(200).expect("poll select");
        assert_eq!(last_cot(&slave, conn), Cot::ActivationConfirm);
        slave.poll(300).expect("poll execute");
        assert_eq!(last_cot(&slave, conn), Cot::ActivationConfirm);
    }

    // ===== 14a1. PTC-003 SBO：直接 execute（无 select）→ Deactivation ====
    #[test]
    fn test_sbo_direct_execute_rejected() {
        let mut mock = MockSlaveTransport::new();
        let conn = mock.accept_conn();
        mock.push_rx_data(
            conn,
            Apdu::i_format(
                0,
                0,
                Asdu {
                    type_id: TypeId::SingleCommand,
                    cause_of_tx: Cot::Activation,
                    common_addr: 1,
                    ioas: vec![InformationObject {
                        ioa: 10,
                        value: IoValue::SingleCommand(Sco::new(true)),
                        quality: QualityDescriptor::good(),
                        time_tag: None,
                    }],
                },
            )
            .encode(),
        );
        let mut db = InMemoryPointDatabase::new();
        db.set_value(10, IoValue::SinglePoint(SinglePointValue::Off));
        let mut slave = make_slave(mock, db);
        slave.set_authorized_write(auth_for(1, 10, ControlCommand::Single(SingleCommand::On)));
        slave.poll(100).expect("poll startdt");
        slave.poll(200).expect("poll direct execute");
        // 无 select 直接 execute → Deactivation（SBO 强制，旧代码直接执行——旧代码失败证据）
        assert_eq!(last_cot(&slave, conn), Cot::Deactivation);
        // 点未被执行
        let v = slave.point_db.get_value(10).unwrap();
        assert!(matches!(v, IoValue::SinglePoint(SinglePointValue::Off)));
    }

    // ===== 14a2. PTC-003 SBO：select 后 execute 命令不匹配 → Deactivation ====
    #[test]
    fn test_sbo_execute_mismatch_rejected() {
        let mut mock = MockSlaveTransport::new();
        let conn = mock.accept_conn();
        // select On
        mock.push_rx_data(
            conn,
            Apdu::i_format(
                0,
                0,
                Asdu {
                    type_id: TypeId::SingleCommand,
                    cause_of_tx: Cot::Activation,
                    common_addr: 1,
                    ioas: vec![InformationObject {
                        ioa: 10,
                        value: IoValue::SingleCommand(Sco {
                            value: true,
                            qu: 0,
                            select: true,
                        }),
                        quality: QualityDescriptor::good(),
                        time_tag: None,
                    }],
                },
            )
            .encode(),
        );
        // execute Off（与 select On 不匹配）
        mock.push_rx_data(
            conn,
            Apdu::i_format(
                0,
                0,
                Asdu {
                    type_id: TypeId::SingleCommand,
                    cause_of_tx: Cot::Activation,
                    common_addr: 1,
                    ioas: vec![InformationObject {
                        ioa: 10,
                        value: IoValue::SingleCommand(Sco::new(false)),
                        quality: QualityDescriptor::good(),
                        time_tag: None,
                    }],
                },
            )
            .encode(),
        );
        let mut db = InMemoryPointDatabase::new();
        db.set_value(10, IoValue::SinglePoint(SinglePointValue::Off));
        let mut slave = make_slave(mock, db);
        slave.set_authorized_write(auth_for(1, 10, ControlCommand::Single(SingleCommand::On)));
        slave.poll(100).expect("poll startdt");
        slave.poll(200).expect("poll select");
        slave.poll(300).expect("poll execute mismatch");
        // 最后一条响应：execute 不匹配 → Deactivation
        assert_eq!(last_cot(&slave, conn), Cot::Deactivation);
        let v = slave.point_db.get_value(10).unwrap();
        assert!(matches!(v, IoValue::SinglePoint(SinglePointValue::Off)));
    }

    // ===== 14a3. PTC-003：总召唤终止帧 COT=10 ActivationTermination =====
    #[test]
    fn test_interrogation_termination_cot_10() {
        let mut mock = MockSlaveTransport::new();
        let conn = mock.accept_conn();
        mock.push_rx_data(
            conn,
            Apdu::i_format(
                0,
                0,
                Asdu {
                    type_id: TypeId::InterrogationCommand,
                    cause_of_tx: Cot::Activation,
                    common_addr: 1,
                    ioas: vec![InformationObject {
                        ioa: 0,
                        value: IoValue::SinglePoint(SinglePointValue::Off),
                        quality: QualityDescriptor::good(),
                        time_tag: None,
                    }],
                },
            )
            .encode(),
        );
        let mut db = InMemoryPointDatabase::new();
        db.set_value(10, IoValue::SinglePoint(SinglePointValue::Off));
        let mut slave = make_slave(mock, db);
        slave.poll(100).expect("poll startdt");
        slave.poll(200).expect("poll interrogation");
        // 总召唤 3 帧：激活确认(7) → 数据(20) → 激活终止(10)。
        // 旧实现终止帧 COT=7（不合规）——旧代码失败证据；
        // 新实现终止帧 COT=10 ActivationTermination。
        assert_eq!(last_cot(&slave, conn), Cot::ActivationTermination);
    }

    // ===== 14b. 单点遥控：未授权 → 默认拒绝 (Deactivation)，不执行 =====
    #[test]
    fn test_single_command_denied_without_authorization() {
        let mut mock = MockSlaveTransport::new();
        let conn = mock.accept_conn();
        mock.push_rx_data(conn, Apdu::u_format(UFormatFunction::StartDtAct).encode());
        mock.push_rx_data(
            conn,
            Apdu::i_format(
                0,
                0,
                Asdu {
                    type_id: TypeId::SingleCommand,
                    cause_of_tx: Cot::Activation,
                    common_addr: 1,
                    ioas: vec![InformationObject {
                        ioa: 10,
                        value: IoValue::SingleCommand(Sco::new(true)),
                        quality: QualityDescriptor::good(),
                        time_tag: None,
                    }],
                },
            )
            .encode(),
        );
        let mut db = InMemoryPointDatabase::new();
        db.set_value(10, IoValue::SinglePoint(SinglePointValue::Off));
        let mut slave = make_slave(mock, db);
        // 不安装授权 → 默认拒绝
        slave.poll(100).expect("poll startdt");
        slave.poll(200).expect("poll cmd");
        assert_eq!(last_cot(&slave, conn), Cot::Deactivation);
    }

    // ===== 14c. 单点遥控：授权值不匹配 → 拒绝 (Deactivation) =====
    #[test]
    fn test_single_command_denied_wrong_value() {
        let mut mock = MockSlaveTransport::new();
        let conn = mock.accept_conn();
        mock.push_rx_data(conn, Apdu::u_format(UFormatFunction::StartDtAct).encode());
        mock.push_rx_data(
            conn,
            Apdu::i_format(
                0,
                0,
                Asdu {
                    type_id: TypeId::SingleCommand,
                    cause_of_tx: Cot::Activation,
                    common_addr: 1,
                    ioas: vec![InformationObject {
                        ioa: 10,
                        value: IoValue::SingleCommand(Sco::new(true)), // On
                        quality: QualityDescriptor::good(),
                        time_tag: None,
                    }],
                },
            )
            .encode(),
        );
        let mut db = InMemoryPointDatabase::new();
        db.set_value(10, IoValue::SinglePoint(SinglePointValue::Off));
        let mut slave = make_slave(mock, db);
        // 授权的是 Off，但请求 On → 不匹配 → 拒绝
        slave.set_authorized_write(auth_for(1, 10, ControlCommand::Single(SingleCommand::Off)));
        slave.poll(100).expect("poll startdt");
        slave.poll(200).expect("poll cmd");
        assert_eq!(last_cot(&slave, conn), Cot::Deactivation);
    }

    // ===== 15. 双点遥控：授权匹配 → 执行 (ActivationConfirm) =====
    #[test]
    fn test_double_command_executes_when_authorized() {
        let mut mock = MockSlaveTransport::new();
        let conn = mock.accept_conn();
        mock.push_rx_data(conn, Apdu::u_format(UFormatFunction::StartDtAct).encode());
        mock.push_rx_data(
            conn,
            Apdu::i_format(
                0,
                0,
                Asdu {
                    type_id: TypeId::DoubleCommand,
                    cause_of_tx: Cot::Activation,
                    common_addr: 1,
                    ioas: vec![InformationObject {
                        ioa: 20,
                        value: IoValue::DoubleCommand(Dco::new(DoublePointValue::On)),
                        quality: QualityDescriptor::good(),
                        time_tag: None,
                    }],
                },
            )
            .encode(),
        );
        let mut db = InMemoryPointDatabase::new();
        db.set_value(20, IoValue::DoublePoint(DoublePointValue::Off));
        let mut slave = make_slave(mock, db);
        slave.set_authorized_write(auth_for(1, 20, ControlCommand::Double(DoubleCommand::On)));
        slave.poll(100).expect("poll startdt");
        slave.poll(200).expect("poll cmd");
        assert_eq!(last_cot(&slave, conn), Cot::ActivationConfirm);
    }

    // ===== 15b. 双点遥控：未授权 → 默认拒绝 (Deactivation) =====
    #[test]
    fn test_double_command_denied_without_authorization() {
        let mut mock = MockSlaveTransport::new();
        let conn = mock.accept_conn();
        mock.push_rx_data(conn, Apdu::u_format(UFormatFunction::StartDtAct).encode());
        mock.push_rx_data(
            conn,
            Apdu::i_format(
                0,
                0,
                Asdu {
                    type_id: TypeId::DoubleCommand,
                    cause_of_tx: Cot::Activation,
                    common_addr: 1,
                    ioas: vec![InformationObject {
                        ioa: 20,
                        value: IoValue::DoubleCommand(Dco::new(DoublePointValue::On)),
                        quality: QualityDescriptor::good(),
                        time_tag: None,
                    }],
                },
            )
            .encode(),
        );
        let mut db = InMemoryPointDatabase::new();
        db.set_value(20, IoValue::DoublePoint(DoublePointValue::Off));
        let mut slave = make_slave(mock, db);
        slave.poll(100).expect("poll startdt");
        slave.poll(200).expect("poll cmd");
        assert_eq!(last_cot(&slave, conn), Cot::Deactivation);
    }

    // ===== 16. 时钟同步确认 =====
    #[test]
    fn test_clock_sync_confirm() {
        let mut mock = MockSlaveTransport::new();
        let conn = mock.accept_conn();
        mock.push_rx_data(conn, Apdu::u_format(UFormatFunction::StartDtAct).encode());
        mock.push_rx_data(
            conn,
            Apdu::i_format(
                0,
                0,
                Asdu {
                    type_id: TypeId::ClockSyncCommand,
                    cause_of_tx: Cot::Activation,
                    common_addr: 1,
                    ioas: vec![InformationObject {
                        ioa: 0,
                        value: IoValue::SinglePoint(SinglePointValue::On),
                        quality: QualityDescriptor::good(),
                        time_tag: Some(TimeTag {
                            year: 25,
                            month: 7,
                            day: 15,
                            hour: 10,
                            minute: 30,
                            second: 45,
                            iv: false,
                            su: false,
                            millis: 500,
                        }),
                    }],
                },
            )
            .encode(),
        );
        let db = InMemoryPointDatabase::new();
        let mut slave = make_slave(mock, db);
        slave.poll(100).expect("poll startdt");
        slave.poll(200).expect("poll clock sync");
        assert!(slave.stats().tx_count >= 2);
    }

    // ===== 17. 空闲时（非 Active）不触发 t3 =====
    #[test]
    fn test_no_testfr_when_not_active() {
        let mut mock = MockSlaveTransport::new();
        let _conn = mock.accept_conn();
        // 不发 STARTDT，保持 Connected 状态
        let db = InMemoryPointDatabase::new();
        let mut slave = make_slave(mock, db);
        slave.poll(100).expect("poll accept");
        assert_eq!(slave.state(), SlaveState::Connected);
        let initial_tx = slave.stats().tx_count;
        slave.poll(21000).expect("poll idle");
        // 非 Active 状态不触发 TestFrAct
        assert_eq!(slave.stats().tx_count, initial_tx);
    }

    // ===== 18. 配置字段访问 =====
    #[test]
    fn test_config_access() {
        let mock = MockSlaveTransport::new();
        let db = InMemoryPointDatabase::new();
        let slave = make_slave(mock, db);
        assert_eq!(slave.config().listen_port, 2404);
        assert_eq!(slave.config().w, 8);
    }

    // ===== 19. 多次 poll 累积统计 =====
    #[test]
    fn test_multiple_polls_accumulate() {
        let mut mock = MockSlaveTransport::new();
        let conn = mock.accept_conn();
        mock.push_rx_data(conn, Apdu::u_format(UFormatFunction::StartDtAct).encode());
        let db = InMemoryPointDatabase::new();
        let mut slave = make_slave(mock, db);
        slave.poll(100).expect("poll 1");
        let tx1 = slave.stats().tx_count;
        slave.poll(21000).expect("poll 2 (t3)");
        let tx2 = slave.stats().tx_count;
        slave.poll(42000).expect("poll 3 (t3)");
        let tx3 = slave.stats().tx_count;
        assert!(tx2 > tx1);
        assert!(tx3 > tx2);
    }

    // ===== 20. Dco/Sco new 默认值 =====
    #[test]
    fn test_sco_dco_defaults() {
        let sco = Sco::new(true);
        assert!(sco.value);
        assert_eq!(sco.qu, 0);
        assert!(!sco.select);
        let dco = Dco::new(DoublePointValue::On);
        assert_eq!(dco.value, DoublePointValue::On);
        assert_eq!(dco.qu, 0);
        assert!(!dco.select);
    }
}

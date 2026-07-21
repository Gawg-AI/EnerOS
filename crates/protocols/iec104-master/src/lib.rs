//! EnerOS IEC 60870-5-104 主站协议栈（v0.49.0）.
//!
//! 实现电力行业标准 IEC 60870-5-104 主站（控制中心侧），支持：
//! - 多设备并发连接（`BTreeMap<u16, MasterConnection>`）
//! - 周期性总召唤（InterrogationCommand, QOI=20）
//! - 遥控命令下发（SingleCommand / DoubleCommand）
//! - 时钟同步命令下发（ClockSyncCommand, CP56Time2a 时标）
//! - t3 保活（TESTFR_ACT）
//! - STARTDT 握手流程
//!
//! # 核心类型
//! - [`master::Iec104Master`] — IEC 104 主站，封装连接管理、APDU 收发、ASDU 分发
//! - [`transport::MasterTransport`] — 传输层抽象 trait（D1）
//! - [`transport::MasterStats`] — 主站统计
//! - [`device::RemoteDevice`] — 远端设备描述
//! - [`device::ConnState`] — 连接状态
//! - [`config::MasterConfig`] — 主站配置
//! - [`connection::MasterConnection`] — 连接管理（序列号/状态/时间戳）
//! - [`poll::PollScheduler`] — 轮询调度器（D10）
//! - [`error::MasterError`] — 主站错误
//!
//! # 与 v0.48.0 的关系
//!
//! 复用 `eneros-iec104-slave` 的 APDU/ASDU/TypeId/Cot/InformationObject/IoValue/
//! QualityDescriptor/Sco/Dco/TimeTag/SinglePointValue/DoublePointValue/Iec104Error 类型
//!（D5，path 依赖，类比 v0.46.0 modbus-tcp 复用 v0.45.0 modbus-rtu）。
//!
//! # 偏差声明（D1~D11）
//!
//! | 偏差 | 说明 |
//! |------|------|
//! | **D1** | 定义本地 [`transport::MasterTransport`] trait（connect/send/recv/close/now_ms），解耦 smoltcp，类比 v0.48.0 `SlaveTransport` |
//! | **D2** | 时间通过 `now_ms: u64` 参数注入（无 `MonotonicTime` 类型，与 v0.48.0 D3 一致） |
//! | **D3** | 超时/间隔使用 `u32` 毫秒（无 `Duration` 类型，与 v0.48.0 D5 一致） |
//! | **D4** | 不依赖 `eneros-net`/smoltcp（传输层由 trait 抽象，与 v0.48.0 D8 一致） |
//! | **D5** | 复用 `eneros-iec104-slave` 的 APDU/ASDU/TypeId/Cot 等类型（path 依赖，类比 v0.46.0 modbus-tcp 复用 v0.45.0 modbus-rtu） |
//! | **D6** | crate 放入 `crates/protocols/iec104-master/`（与 iec104-slave/modbus-rtu/modbus-tcp 同级） |
//! | **D7** | 不实现 `DeviceDriver` trait（协议栈非设备驱动，与 v0.48.0 D9 一致） |
//! | **D8** | IP 地址用 `[u8; 4]` 表示 IPv4（无 `std::net::IpAddr`，与 v0.46.0 `TcpDevice` 一致） |
//! | **D9** | `SocketHandle` 抽象为 `ConnId = u32`（传输层 trait 返回连接 ID，主站按 ID 操作） |
//! | **D10** | `PollScheduler` 简化为基于 `now_ms` 的时间戳比较（无定时器对象，Simplicity First） |
//! | **D11** | 时钟同步时标由调用方通过 `now_ms` 参数注入并构造 [`TimeTag`](eneros_iec104_slave::TimeTag) |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，外部依赖仅 `eneros-iec104-slave`。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod config;
pub mod connection;
pub mod device;
pub mod error;
pub mod master;
pub mod poll;
pub mod transport;

#[cfg(test)]
pub mod mock;

// 重导出公共 API
pub use config::MasterConfig;
pub use connection::MasterConnection;
pub use device::{ConnState, RemoteDevice};
// 重导出 v0.48.0 slave 类型，便于上游统一从本 crate 引用（D5）
pub use eneros_iec104_slave::{
    Apdu, Asdu, ControlField, Cot, Dco, DoublePointValue, InformationObject, IoValue,
    QualityDescriptor, Sco, SinglePointValue, TimeTag, TypeId, UFormatFunction,
};
pub use error::{Iec104Error, MasterError};
pub use master::{time_tag_from_ms, Iec104Master};
pub use poll::{PollScheduler, PollTask};
pub use transport::{ConnId, MasterStats, MasterTransport};

#[cfg(test)]
mod tests {
    //! 跨模块集成测试 — 端到端验证主站协议流程（master + mock + 依赖类型全链路）.

    use alloc::boxed::Box;
    use alloc::vec;
    use alloc::vec::Vec;

    use eneros_iec104_slave::{
        Apdu, Asdu, ControlField, Cot, DoublePointValue, InformationObject, IoValue,
        QualityDescriptor, Sco, SinglePointValue, TypeId, UFormatFunction,
    };

    use crate::config::MasterConfig;
    use crate::device::{ConnState, RemoteDevice};
    use crate::master::time_tag_from_ms;
    use crate::mock::MockMasterTransport;
    use crate::transport::MasterTransport;
    use crate::Iec104Master;

    // ===== 测试辅助函数 =====

    /// 构造远端设备（common_addr=1, port=2404, poll=30000ms）.
    fn make_device(common_addr: u16) -> RemoteDevice {
        RemoteDevice::new([192, 168, 1, 10], 2404, common_addr, 30_000)
    }

    /// 构造 STARTDT_CON 的字节流.
    fn startdt_con_bytes() -> Vec<u8> {
        Apdu::u_format(UFormatFunction::StartDtCon).encode()
    }

    /// 构造 TESTFR_CON 的字节流.
    fn testfr_con_bytes() -> Vec<u8> {
        Apdu::u_format(UFormatFunction::TestFrCon).encode()
    }

    /// 构造总召唤激活确认 ASDU（从站 → 主站）.
    fn interrogation_confirm_asdu(common_addr: u16) -> Asdu {
        Asdu {
            type_id: TypeId::InterrogationCommand,
            cause_of_tx: Cot::ActivationConfirm,
            common_addr,
            ioas: vec![InformationObject {
                ioa: 0,
                value: IoValue::Normalized(20),
                quality: QualityDescriptor::good(),
                time_tag: None,
            }],
        }
    }

    /// 构造总召唤激活确认 I 帧（从站 → 主站）.
    fn interrogation_confirm_bytes(common_addr: u16) -> Vec<u8> {
        Apdu::i_format(0, 0, interrogation_confirm_asdu(common_addr)).encode()
    }

    /// 构造遥测数据 I 帧（从站 → 主站，单点遥信）.
    fn single_point_data_bytes(common_addr: u16, ioa: u16, value: SinglePointValue) -> Vec<u8> {
        Apdu::i_format(
            0,
            0,
            Asdu {
                type_id: TypeId::SinglePointInformation,
                cause_of_tx: Cot::InterrogatedByStation,
                common_addr,
                ioas: vec![InformationObject {
                    ioa,
                    value: IoValue::SinglePoint(value),
                    quality: QualityDescriptor::good(),
                    time_tag: None,
                }],
            },
        )
        .encode()
    }

    /// 创建主站并连接设备，预填 STARTDT_CON，完成握手后状态为 Connected.
    fn connect_and_handshake(device: RemoteDevice) -> Iec104Master {
        let mut mock = MockMasterTransport::new();
        // 预填 STARTDT_CON（connect() 将返回 conn_id=1）
        mock.push_rx(1, startdt_con_bytes());

        let mut master = Iec104Master::new(MasterConfig::default(), Box::new(mock));
        master.connect(&device).expect("connect ok");
        assert_eq!(
            master.device_state(device.common_addr),
            Some(ConnState::StartDtPending)
        );

        // poll 处理 STARTDT_CON → Connected
        master.poll(100);
        assert_eq!(
            master.device_state(device.common_addr),
            Some(ConnState::Connected)
        );
        master
    }

    // ===== T1: 主站连接从站 + STARTDT 握手 =====
    #[test]
    fn test_t1_connect_and_startdt_handshake() {
        let device = make_device(1);
        let master = connect_and_handshake(device);

        // 验证统计
        let stats = master.stats();
        assert_eq!(stats.connect_count, 1);
        // STARTDT_ACT 发送 = 1 + 可能的 TESTFR（poll 中无 t3 超时）
        assert!(
            stats.tx_count >= 1,
            "expected >= 1 tx, got {}",
            stats.tx_count
        );
        assert_eq!(stats.rx_count, 1); // STARTDT_CON
        assert_eq!(master.device_state(1), Some(ConnState::Connected));
    }

    // ===== T2: 总召唤命令发送 + 接收响应数据 =====
    #[test]
    fn test_t2_interrogation_send_and_receive() {
        let device = make_device(1);
        let mut mock = MockMasterTransport::new();
        // 预填 STARTDT_CON + 总召唤激活确认 + 遥测数据
        mock.push_rx(1, startdt_con_bytes());
        mock.push_rx(1, interrogation_confirm_bytes(1));
        mock.push_rx(1, single_point_data_bytes(1, 1, SinglePointValue::On));

        let mut master = Iec104Master::new(MasterConfig::default(), Box::new(mock));
        master.connect(&device).expect("connect");
        master.poll(100); // STARTDT_CON → Connected

        // 发起总召唤
        master.interrogation(1, 200).expect("interrogation");
        assert_eq!(master.device_state(1), Some(ConnState::Interrogating));

        // poll 处理总召唤确认 + 遥测数据
        master.poll(300);
        // 收到激活确认 → 恢复 Connected
        assert_eq!(master.device_state(1), Some(ConnState::Connected));

        let stats = master.stats();
        assert_eq!(stats.interrogation_count, 1);
        assert!(
            stats.rx_count >= 2,
            "expected >= 2 rx, got {}",
            stats.rx_count
        );
    }

    // ===== T3: 单点遥控命令发送 =====
    #[test]
    fn test_t3_single_command() {
        let device = make_device(1);
        let mut master = connect_and_handshake(device);
        let initial_tx = master.stats().tx_count;

        // 发送单点遥控（IOA=10, On）
        master
            .send_single_command(1, 10, SinglePointValue::On)
            .expect("single command");

        let stats = master.stats();
        assert_eq!(stats.command_count, 1);
        assert!(stats.tx_count > initial_tx, "tx should increase");
    }

    // ===== T4: 双点遥控命令发送 =====
    #[test]
    fn test_t4_double_command() {
        let device = make_device(1);
        let mut master = connect_and_handshake(device);
        let initial_tx = master.stats().tx_count;

        // 发送双点遥控（IOA=20, On）
        master
            .send_double_command(1, 20, DoublePointValue::On)
            .expect("double command");

        let stats = master.stats();
        assert_eq!(stats.command_count, 1);
        assert!(stats.tx_count > initial_tx, "tx should increase");
    }

    // ===== T5: 时钟同步命令 + TimeTag 构造 =====
    #[test]
    fn test_t5_clock_sync_and_timetag() {
        // 直接验证 time_tag_from_ms 构造正确
        let tag = time_tag_from_ms(0);
        assert_eq!(tag.year, 26);
        assert_eq!(tag.month, 1);
        assert_eq!(tag.day, 1);
        assert_eq!(tag.hour, 0);
        assert_eq!(tag.minute, 0);
        assert_eq!(tag.second, 0);
        assert_eq!(tag.millis, 0);

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

        // 验证时钟同步命令发送
        let device = make_device(1);
        let mut master = connect_and_handshake(device);
        let initial_tx = master.stats().tx_count;

        master.clock_sync(1, 60_000).expect("clock sync");

        let stats = master.stats();
        assert_eq!(stats.clock_sync_count, 1);
        assert!(stats.tx_count > initial_tx, "tx should increase");
    }

    // ===== T6: 多设备并发连接 + 轮询 =====
    #[test]
    fn test_t6_multi_device_polling() {
        let mut mock = MockMasterTransport::new();
        // 预填两个设备的 STARTDT_CON
        // connect() 返回递增 conn_id: 设备1 → conn_id=1, 设备2 → conn_id=2
        mock.push_rx(1, startdt_con_bytes());
        mock.push_rx(2, startdt_con_bytes());

        let dev1 = RemoteDevice::new([192, 168, 1, 1], 2404, 1, 30_000);
        let dev2 = RemoteDevice::new([192, 168, 1, 2], 2404, 2, 30_000);

        let mut master = Iec104Master::new(MasterConfig::default(), Box::new(mock));

        // 连接设备1
        master.connect(&dev1).expect("connect 1");
        assert_eq!(master.device_state(1), Some(ConnState::StartDtPending));

        // 连接设备2
        master.connect(&dev2).expect("connect 2");
        assert_eq!(master.device_state(2), Some(ConnState::StartDtPending));

        // poll → 两个设备都收到 STARTDT_CON → Connected
        master.poll(100);
        assert_eq!(master.device_state(1), Some(ConnState::Connected));
        assert_eq!(master.device_state(2), Some(ConnState::Connected));

        let stats = master.stats();
        assert_eq!(stats.connect_count, 2);
    }

    // ===== T7: t3 超时保活（TESTFR 发送）=====
    #[test]
    fn test_t7_t3_timeout_testfr() {
        let device = make_device(1);
        let mut mock = MockMasterTransport::new();
        mock.push_rx(1, startdt_con_bytes());

        let mut master = Iec104Master::new(MasterConfig::default(), Box::new(mock));
        master.connect(&device).expect("connect");
        master.poll(100); // STARTDT_CON → Connected

        let initial_tx = master.stats().tx_count;

        // 推进时间超过 t3（默认 20000ms）
        master.poll(21_000);

        let stats = master.stats();
        assert!(
            stats.tx_count > initial_tx,
            "expected TESTFR sent, tx was {} now {}",
            initial_tx,
            stats.tx_count
        );
    }

    // ===== T8: 序列号递增与 15 位回绕 =====
    #[test]
    fn test_t8_sequence_wraparound() {
        use crate::connection::MasterConnection;

        let remote = make_device(1);
        let mut conn = MasterConnection::new(remote, 1, 0);

        // 递增
        assert_eq!(conn.next_send_seq(), 0);
        assert_eq!(conn.next_send_seq(), 1);
        assert_eq!(conn.next_send_seq(), 2);

        // 回绕：设为 0x7FFF
        conn.send_seq = 0x7FFF;
        assert_eq!(conn.next_send_seq(), 0x7FFF);
        // 下一次应回绕到 0
        assert_eq!(conn.next_send_seq(), 0);
        assert_eq!(conn.send_seq, 1);

        // recv_seq 同理
        conn.recv_seq = 0x7FFF;
        assert_eq!(conn.next_recv_seq(), 0x7FFF);
        assert_eq!(conn.next_recv_seq(), 0);
        assert_eq!(conn.recv_seq, 1);

        // 通过 Apdu 编解码验证 15 位回绕
        let asdu = Asdu {
            type_id: TypeId::SingleCommand,
            cause_of_tx: Cot::Activation,
            common_addr: 1,
            ioas: vec![InformationObject {
                ioa: 1,
                value: IoValue::SingleCommand(Sco::new(true)),
                quality: QualityDescriptor::good(),
                time_tag: None,
            }],
        };
        let apdu = Apdu::i_format(32767, 0, asdu);
        let bytes = apdu.encode();
        let decoded = Apdu::decode(&bytes).expect("decode ok");
        assert!(matches!(
            decoded.control_field,
            ControlField::Information { send_seq, recv_seq } if send_seq == 32767 && recv_seq == 0
        ));
    }

    // ===== T9: poll 周期触发总召唤 =====
    #[test]
    fn test_t9_poll_triggers_interrogation() {
        let mut mock = MockMasterTransport::new();
        mock.push_rx(1, startdt_con_bytes());

        // poll_interval = 10000ms
        let device = RemoteDevice::new([192, 168, 1, 1], 2404, 1, 10_000);
        let mut master = Iec104Master::new(MasterConfig::default(), Box::new(mock));

        master.connect(&device).expect("connect");
        // connect 时 scheduler 设置 next_poll = 0 + 10000 = 10000
        master.poll(100); // STARTDT_CON → Connected

        assert_eq!(master.stats().interrogation_count, 0);

        // 未到周期
        master.poll(9_000);
        assert_eq!(master.stats().interrogation_count, 0);

        // 到期（10000ms）
        master.poll(10_001);
        assert_eq!(master.stats().interrogation_count, 1);
    }

    // ===== T10: 连接状态机转换 =====
    #[test]
    fn test_t10_state_machine_transitions() {
        let mut mock = MockMasterTransport::new();
        // 预填 STARTDT_CON + 总召唤激活确认
        mock.push_rx(1, startdt_con_bytes());
        mock.push_rx(1, interrogation_confirm_bytes(1));

        let device = make_device(1);
        let mut master = Iec104Master::new(MasterConfig::default(), Box::new(mock));

        // 初始：无连接 → None
        assert_eq!(master.device_state(1), None);

        // connect → StartDtPending
        master.connect(&device).expect("connect");
        assert_eq!(master.device_state(1), Some(ConnState::StartDtPending));

        // poll 处理 STARTDT_CON → Connected
        master.poll(100);
        assert_eq!(master.device_state(1), Some(ConnState::Connected));

        // 发起总召唤 → Interrogating
        master.interrogation(1, 200).expect("interrogation");
        assert_eq!(master.device_state(1), Some(ConnState::Interrogating));

        // poll 处理总召唤确认 → Connected
        master.poll(300);
        assert_eq!(master.device_state(1), Some(ConnState::Connected));

        // 验证完整状态转换：StartDtPending → Connected → Interrogating → Connected
        let stats = master.stats();
        assert_eq!(stats.interrogation_count, 1);
    }

    // ===== 附加测试：TESTFR_CON 处理 =====
    #[test]
    fn test_testfr_con_handling() {
        let mut mock = MockMasterTransport::new();
        mock.push_rx(1, startdt_con_bytes());
        mock.push_rx(1, testfr_con_bytes());

        let device = make_device(1);
        let mut master = Iec104Master::new(MasterConfig::default(), Box::new(mock));
        master.connect(&device).expect("connect");
        master.poll(100); // STARTDT_CON → Connected
        master.poll(200); // TESTFR_CON → 应保持 Connected

        assert_eq!(master.device_state(1), Some(ConnState::Connected));
        assert!(master.stats().rx_count >= 2);
    }

    // ===== 附加测试：未连接设备操作返回错误 =====
    #[test]
    fn test_not_connected_error() {
        let mock = MockMasterTransport::new();
        let mut master = Iec104Master::new(MasterConfig::default(), Box::new(mock));

        let result = master.interrogation(99, 0);
        assert_eq!(result, Err(crate::error::MasterError::NotConnected));

        let result = master.clock_sync(99, 0);
        assert_eq!(result, Err(crate::error::MasterError::NotConnected));

        let result = master.send_single_command(99, 1, SinglePointValue::On);
        assert_eq!(result, Err(crate::error::MasterError::NotConnected));
    }

    // ===== 附加测试：状态错误（未 Connected 时发命令）=====
    #[test]
    fn test_state_error_when_not_connected() {
        let mock = MockMasterTransport::new();
        let device = make_device(1);
        // 不预填 STARTDT_CON，连接后仍为 StartDtPending

        let mut master = Iec104Master::new(MasterConfig::default(), Box::new(mock));
        master.connect(&device).expect("connect");
        // 状态为 StartDtPending，非 Connected

        let result = master.send_single_command(1, 10, SinglePointValue::On);
        assert_eq!(result, Err(crate::error::MasterError::StateError));
    }

    // ===== 附加测试：trait object 兼容 =====
    #[test]
    fn test_trait_object_compatibility() {
        let mock: Box<dyn MasterTransport> = Box::new(MockMasterTransport::new());
        let master = Iec104Master::new(MasterConfig::default(), mock);
        assert_eq!(master.stats().tx_count, 0);
    }
}

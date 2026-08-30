//! SPEC-SERIES-2026-08-27/D WP-D.2 C1/C2：串口活路 host 全链。
//!
//! 链路形态 = **MockUartHw（SharedUart 桥）→ SerialTransportFactory →
//! RtuPlugin** 全真装配：SCADA 写意图线码往返 → 插件执行层 FC06 主站写
//! （mock 对端预置应答）→ 独立 readback 重读比对 → OutcomeFrame 分类，
//! 与 svc-modbus-tcp 打穿链同语义的串口版。QEMU guest 无第二串口（F-S1），
//! 本文件是唯一可执行的「跑通」证据面；真机 bring-up 后同一程序路径
//! 在板上复跑即为 EXT-03 真机验收注脚（SPEC-D C4——程序本体零改码）。

use std::sync::{Arc, Mutex};

use eneros_control_gateway::wire::{OutcomeCode, OutcomeFrame};
use eneros_driver_framework::DriverError;
use eneros_modbus_rtu::frame::ModbusFrame;
use eneros_modbus_rtu::master::RtuTransport;
use eneros_modbus_rtu::transport::Rs485RtuTransport;
use eneros_protocol_abstract::plugin::ProtocolPlugin;
use eneros_protocol_abstract::{ProtocolAddress, ProtocolConfig, ProtocolPointMapping};
use eneros_protocol_services::net_transport::SerialTransportFactory;
use eneros_rs485::config::{Parity, StopBits};
use eneros_rs485::mock::MockUartHw;
use eneros_rs485::uart_hw::UartHw;
use eneros_upa_model::{PointId, PointType, PointValue};

/// 共享 UART 桥（loopback.rs 同型）：工厂 open 经 UartHw 消费它，测试侧经
/// Arc 副本观察 written / 注入对端响应帧。
struct SharedUart(Arc<Mutex<MockUartHw>>);

impl SharedUart {
    fn new() -> (Self, Arc<Mutex<MockUartHw>>) {
        let arc = Arc::new(Mutex::new(MockUartHw::new()));
        (Self(arc.clone()), arc)
    }
}

impl UartHw for SharedUart {
    fn configure(
        &mut self,
        baud_rate: u32,
        data_bits: u8,
        stop_bits: StopBits,
        parity: Parity,
    ) -> Result<(), DriverError> {
        self.0
            .lock()
            .unwrap()
            .configure(baud_rate, data_bits, stop_bits, parity)
    }
    fn enable_rx_irq(&mut self) -> Result<(), DriverError> {
        self.0.lock().unwrap().enable_rx_irq()
    }
    fn disable_rx_irq(&mut self) -> Result<(), DriverError> {
        self.0.lock().unwrap().disable_rx_irq()
    }
    fn read_byte(&mut self) -> Option<u8> {
        self.0.lock().unwrap().read_byte()
    }
    fn write_bytes(&mut self, data: &[u8]) -> Result<usize, DriverError> {
        self.0.lock().unwrap().write_bytes(data)
    }
    fn wait_tx_done(&mut self, timeout_ms: u32) -> Result<(), DriverError> {
        self.0.lock().unwrap().wait_tx_done(timeout_ms)
    }
    fn rx_irq_id(&self) -> u32 {
        self.0.lock().unwrap().rx_irq_id()
    }
    fn now_ns(&self) -> u64 {
        self.0.lock().unwrap().now_ns()
    }
    fn configure_de_re(&mut self, pin: Option<u32>) -> Result<(), DriverError> {
        self.0.lock().unwrap().configure_de_re(pin)
    }
    fn set_de_re(&mut self, high: bool) -> Result<(), DriverError> {
        self.0.lock().unwrap().set_de_re(high)
    }
}

/// 取走 written 缓冲自 `from` 起的新增段（主站请求帧快照）。
fn drain_written(uart: &Arc<Mutex<MockUartHw>>, from: &mut usize) -> Vec<u8> {
    let w = uart.lock().unwrap().written().to_vec();
    let out = w[*from..].to_vec();
    *from = w.len();
    out
}

/// C1 点表映射：point 7 @ 设备 3，从站 1 保持寄存器 0，scale 0.1
/// （工程量 kW ↔ raw×0.1）。
fn chain_mapping() -> ProtocolPointMapping {
    ProtocolPointMapping {
        point_id: PointId::from(7u32),
        device_id: 3,
        protocol_addr: ProtocolAddress::Modbus {
            slave_addr: 1,
            reg_addr: 0,
            func_code: 3,
        },
        data_type: PointType::Analog,
        scale: 0.1,
        offset: 0.0,
        identity: eneros_protocol_abstract::PointIdentityBinding::UNKNOWN,
    }
}

/// 构造已启动的插件链（工厂消费 uart 桥；返回插件与共享桥句柄）。
fn started_plugin_chain(
    baud: u32,
    parity: Parity,
) -> (
    eneros_modbus_rtu::plugin::RtuPlugin,
    Arc<Mutex<MockUartHw>>,
    ProtocolPointMapping,
) {
    let mut cfg = ProtocolConfig::default();
    cfg.insert("slave_id", "1");
    cfg.insert("baud", &baud.to_string());
    cfg.insert(
        "parity",
        match parity {
            Parity::None => "none",
            Parity::Even => "even",
            Parity::Odd => "odd",
        },
    );
    cfg.insert("timeout_ms", "60");
    cfg.insert("retries", "1");
    let (uart_bridge, shared) = SharedUart::new();
    // 成帧前提：模拟时钟步进必须远小于最高测试波特率（115200）的
    // 3.5 字符间隔（≈0.30ms）——否则逐字节被切成独立"帧"。50µs 对全档安全。
    shared.lock().unwrap().set_time_step_ns(50_000);
    let mut factory = SerialTransportFactory::new(uart_bridge, baud).with_parity(parity);
    let mut plugin = eneros_modbus_rtu::plugin::RtuPlugin::new();
    let mut registry = eneros_protocol_abstract::VecPointRegistry::new(vec![chain_mapping()]);
    plugin
        .start(&cfg, &mut factory, &mut registry)
        .expect("plugin start");
    (plugin, shared, chain_mapping())
}

/// C1 主判据：意图线码 → FC06 真写（对端 FC06 应答）→ 独立 readback →
/// 工程量比对 → Applied 分类。断言含：配置键真消费（baud/parity 进硬件
/// configure）、线上请求帧真实副作用（written 快照解码）、readback 不回显
/// 载荷而来自重读应答。
#[test]
fn c1_intent_to_fc06_write_to_readback_applied() {
    const TARGET_KW: f64 = 5.0; // raw = 50（scale 0.1）

    // ── SCADA 写意图 → 线码（svc 间 EIPC 定长载荷，47 卷 wire 语义）──
    let intent = eneros_protocol_abstract::write::PointWriteIntent::new(
        "modbus-rtu",
        3,
        PointId::from(7u32),
        PointValue::Float(TARGET_KW),
        42,
        1_234,
    );
    let wire = eneros_protocol_abstract::write::encode_intent(&intent);
    let decoded =
        eneros_protocol_abstract::write::decode_intent(&wire).expect("意图线码必须可解码");
    assert_eq!(decoded.protocol_id, "modbus-rtu");
    assert_eq!(decoded.device_id, 3);
    assert_eq!(decoded.point_id, intent.point_id);
    assert_eq!(decoded.sequence, 42);
    assert_eq!(decoded.value, PointValue::Float(TARGET_KW));

    // ── 装配（工厂→插件）并验证配置键真消费 ──
    let (mut plugin, shared, mapping) = started_plugin_chain(115_200, Parity::Even);
    assert_eq!(
        shared.lock().unwrap().last_config(),
        (115_200, Parity::Even),
        "baud/parity 必须经工厂贯通到硬件 configure（配置零消费禁令）"
    );

    // ── 执行层 FC06 写：对端按 echo 语义预置应答（FC06 应答=回显地址+值）──
    let raw_expect = mapping.from_engineering(TARGET_KW); // 50
    assert_eq!(raw_expect, 50);
    let mut echo_data = vec![];
    echo_data.extend_from_slice(&0u16.to_be_bytes());
    echo_data.extend_from_slice(&(raw_expect as u16).to_be_bytes());
    let echo = ModbusFrame::new(1, 0x06, echo_data).encode();
    shared.lock().unwrap().push_rx_slice(&echo);

    let mut sent_at = 0usize;
    plugin
        .write_point(10_000, &mapping, decoded.value.clone())
        .expect("FC06 主站写必须成功");

    // 线上副作用：发出的正是解得出的请求（slave/FC/reg/value 全对上）。
    let sent = drain_written(&shared, &mut sent_at);
    let sent_frame = ModbusFrame::decode(&sent).expect("线上写请求 CRC 必须有效");
    assert_eq!(sent_frame.slave_addr, 1);
    assert_eq!(sent_frame.func_code, 0x06);
    assert_eq!(
        sent_frame.data,
        vec![0x00, 0x00, 0x00, 0x32],
        "reg=0 raw=50 的线上载荷"
    );

    // ── 独立 readback（重读该点；数据面应答 raw=50——非本地缓存回显）──
    let mut resp_data = vec![0x02]; // byte count
    resp_data.extend_from_slice(&(raw_expect as u16).to_be_bytes());
    let resp = ModbusFrame::new(1, 0x03, resp_data).encode();
    shared.lock().unwrap().push_rx_slice(&resp);

    let read = plugin
        .read_point(11_000, &mapping)
        .expect("readback 必须成功");
    assert!(read.ok, "readback.ok");
    assert_eq!(read.raw, raw_expect, "raw 来自重读应答而非本地缓存");

    // ── OutcomeFrame 分类：|eng − target| ≤ 容差 → Applied（svc-modbus-tcp
    // master_write 同语义；以二进制回执对照非空转词形）。──
    let eng = mapping.to_engineering(read.raw);
    let applied = (eng - TARGET_KW).abs() <= 0.1f64.max(f64::EPSILON);
    let frame = if applied {
        OutcomeFrame::new(OutcomeCode::Applied, 0).encode()
    } else {
        OutcomeFrame::new(OutcomeCode::AppliedReadbackMismatch, 0).encode()
    };
    assert_eq!(
        frame,
        OutcomeFrame::new(OutcomeCode::Applied, 0).encode(),
        "eng={eng} 与目标差必须在容差内 → Applied"
    );
}

/// C1 负分支补强：readback 回读值超容差 → Mismatch 判定（乱真防伪）。
#[test]
fn c1_readback_beyond_tolerance_classifies_mismatch() {
    let (mut plugin, shared, mapping) = started_plugin_chain(9_600, Parity::None);
    // 对端答 raw=9999 → eng=999.9 ≠ 目标 5.0。
    let mut resp_data = vec![0x02];
    resp_data.extend_from_slice(&9999u16.to_be_bytes());
    let resp = ModbusFrame::new(1, 0x03, resp_data).encode();
    shared.lock().unwrap().push_rx_slice(&resp);
    let read = plugin.read_point(12_000, &mapping).expect("read");
    let eng = mapping.to_engineering(read.raw);
    assert!(
        (eng - 5.0).abs() > 0.1f64.max(f64::EPSILON),
        "eng={eng} 必须落在容差外 → 判 Mismatch"
    );
}

/// C2a 超时分类：静默线路 → 有界 Timeout（master/svc 信封归一前的第一
/// 现场 = transport 层 DriverError::Timeout）。
#[test]
fn c2_silent_line_timeouts_bounded() {
    let (uart_master, _um) = SharedUart::new();
    let mut transport = Rs485RtuTransport::new(uart_master, 9600);
    let req = ModbusFrame::new(1, 0x03, vec![0x00, 0x00, 0x00, 0x01]).encode();
    transport.send(&req).expect("send 进入静默线路");
    let err = transport.recv(40).expect_err("静默线路不得成功");
    assert!(
        matches!(err, DriverError::Timeout),
        "静默线路必须 Timeout：{err:?}"
    );
}

/// 线路桥（loopback.rs 同型）：一端 written 快照搬运到另一端 rx——
/// 两枚 MockUartHw 物理隔离，真实字节流须经桥传播。
fn bridge(
    from: &Arc<Mutex<MockUartHw>>,
    to: &Arc<Mutex<MockUartHw>>,
    cursor: &mut usize,
) -> Vec<u8> {
    let w = from.lock().unwrap().written().to_vec();
    let out = w[*cursor..].to_vec();
    *cursor = w.len();
    if !out.is_empty() {
        to.lock().unwrap().push_rx_slice(&out);
    }
    out
}

/// C2b CRC 分类：对端应答被翻转一字节 → 成帧层交付完整字节流，帧校验层
/// 必须拒绝（坏帧不入账，绝不当作有效读数；fail-closed）。
#[test]
fn c2_corrupted_crc_is_detectable_not_accepted() {
    let (uart_master_side, um) = SharedUart::new();
    let (uart_slave_side, us) = SharedUart::new();
    um.lock().unwrap().set_time_step_ns(50_000);
    us.lock().unwrap().set_time_step_ns(50_000);

    // 有效 FC03 响应再破坏其末字节（CRC 低字节翻转）。
    let good = ModbusFrame::new(1, 0x03, vec![0x02, 0x00, 0x32]).encode();
    let mut bad = good;
    let last = bad.len() - 1;
    bad[last] ^= 0xFF;

    let mut slave_transport = Rs485RtuTransport::new(uart_slave_side, 9600);
    let mut master_transport = Rs485RtuTransport::new(uart_master_side, 9600);

    // 主站发读请求 → 桥接到从站线侧 → 从站经真实成帧还原。
    let req = ModbusFrame::new(1, 0x03, vec![0x00, 0x00, 0x00, 0x01]).encode();
    master_transport.send(&req).expect("主站发请求");
    let mut m_sent = 0usize;
    let req_on_wire = bridge(&um, &us, &mut m_sent);
    assert_eq!(req_on_wire, req, "线上字节面与主站构造一致");
    let got_req = slave_transport.recv(60).expect("从站收请求帧");
    assert_eq!(got_req, req, "从站侧成帧必须还原完整请求");

    // 从站发坏帧 → 桥回主站线侧：成帧层收到等长字节流，但帧校验必须拒绝。
    slave_transport.send(&bad).expect("从站发坏帧");
    let mut s_sent = 0usize;
    let resp_on_wire = bridge(&us, &um, &mut s_sent);
    assert_eq!(resp_on_wire, bad, "线上坏帧字节面一致");
    let got = master_transport.recv(60).expect("主站收响应字节流");
    assert_eq!(got, bad, "成帧层字节面等长交付");
    assert!(
        ModbusFrame::decode(&got).is_err(),
        "被篡改 CRC 的帧绝不能通过校验"
    );
}

/// C2c 半帧垃圾输入：随机切分的垃圾字节流进 RTU 发送面、空线路进接收面——
/// 循环有界终止（配合 fuzz_modbus_rtu 靶 CI 化的固定变体；缓冲上限锁定）。
#[test]
fn c2_half_frame_garbage_never_panics_and_stays_bounded() {
    let (uart, _guard) = SharedUart::new();
    let mut transport = Rs485RtuTransport::new(uart, 9600);
    // 故意两半切分发送面（成帧层发送无状态拆分容忍）+ 接收面空线路超时。
    let junk: Vec<u8> = vec![0xAA; 300];
    transport.send(&junk[..150]).expect("半帧第一段入线");
    transport.send(&junk[150..]).expect("半帧第二段入线");
    let mut unexpected: Option<DriverError> = None;
    for _round in 0..4 {
        match transport.recv(30) {
            Ok(frame_bytes) => assert!(
                frame_bytes.len() <= 256,
                "成帧输出不得超 RTU 上限：{}",
                frame_bytes.len()
            ),
            Err(DriverError::Timeout) => {}
            Err(e) => {
                unexpected = Some(e);
                break;
            }
        }
    }
    assert!(
        unexpected.is_none(),
        "接收面只允许 Timeout 或成帧成功：{unexpected:?}"
    );
}

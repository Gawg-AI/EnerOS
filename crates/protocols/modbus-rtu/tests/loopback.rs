//! 21-B B1 集成测试：Rs485RtuTransport 请求-响应闭环（真实帧层 + CRC）。
//!
//! 主站与从站各持 `Rs485RtuTransport`（生产实现），UART 物理层用
//! `SharedUart`（`Rc<RefCell<MockUartHw>>` 包装——测试桥可访问 written/
//! push_rx，验证 send 的真实副作用）。真 PL011 MMIO 路径由
//! `pl011_loopback.rs` 覆盖（RamMmio 仿真逐字节对通）。
//!
//! 证明"旧行为（trait 悬空）无法完成、新行为可以"：请求帧（读保持
//! 寄存器）经真实 CRC 编码发送，对端经真实帧层解码校验，完成一次
//! 完整请求-响应。

use std::sync::{Arc, Mutex};

use eneros_driver_framework::DriverError;
use eneros_modbus_rtu::frame::ModbusFrame;
use eneros_modbus_rtu::master::RtuTransport;
use eneros_modbus_rtu::transport::Rs485RtuTransport;
use eneros_rs485::config::{Parity, StopBits};
use eneros_rs485::mock::MockUartHw;
use eneros_rs485::uart_hw::UartHw;

/// 共享 UART 包装：测试桥可访问 `written` / `push_rx`（仅测试）。
/// `UartHw: Send + Sync`——用 `Arc<Mutex>`（Rc/RefCell 不满足）。
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

#[test]
fn request_response_roundtrip_with_crc() {
    // 主站（transport 拥有 uart；rc 供测试桥访问）。
    let (master_uart, master_rc) = SharedUart::new();
    let mut master = Rs485RtuTransport::new(master_uart, 9600);

    // 从站.
    let (slave_uart, slave_rc) = SharedUart::new();
    let mut slave = Rs485RtuTransport::new(slave_uart, 9600);

    // 主站：请求读保持寄存器（addr=0, qty=2）——真实 CRC 编码。
    let request = ModbusFrame::new(1, 0x03, vec![0x00, 0x00, 0x00, 0x02]).encode();
    master.send(&request).expect("master send");

    // 线路：主站 → 从站（written 是 send 的真实副作用）。
    let sent = master_rc.lock().unwrap().written().to_vec();
    assert_eq!(sent, request, "master send 必须真实写入 UART（written）");
    slave_rc.lock().unwrap().push_rx_slice(&sent);

    // 从站：接收完整请求帧（真实帧层 + CRC 校验）。
    let recv = slave.recv(100).expect("slave recv request");
    let frame = ModbusFrame::decode(&recv).expect("request CRC valid");
    assert_eq!(frame.slave_addr, 1);
    assert_eq!(frame.func_code, 0x03);
    assert_eq!(frame.data, vec![0x00, 0x00, 0x00, 0x02]);

    // 从站：响应（2 个寄存器 0x1234/0x5678）。
    let response = ModbusFrame::new(1, 0x03, vec![0x04, 0x12, 0x34, 0x56, 0x78]).encode();
    slave.send(&response).expect("slave send");

    // 线路：从站 → 主站.
    let sent = slave_rc.lock().unwrap().written().to_vec();
    assert_eq!(sent, response, "slave send 必须真实写入 UART（written）");
    master_rc.lock().unwrap().push_rx_slice(&sent);

    // 主站：接收完整响应帧。
    let recv = master.recv(100).expect("master recv response");
    let frame = ModbusFrame::decode(&recv).expect("response CRC valid");
    assert_eq!(frame.slave_addr, 1);
    assert_eq!(frame.func_code, 0x03);
    assert_eq!(frame.data, vec![0x04, 0x12, 0x34, 0x56, 0x78]);
}

/// 超时语义：对端无响应时 recv 返回 Timeout（有界，不挂死）。
#[test]
fn recv_timeout_when_no_peer_response() {
    let (master_uart, _rc) = SharedUart::new();
    let mut master = Rs485RtuTransport::new(master_uart, 9600);

    // 空线路：recv 必须在超时后返回 Err(Timeout)（MockUartHw 时间
    // 每 now_ns 调用推进 1ms；50ms 超时 = 有界 50 次轮询）。
    let t0 = master.now_ms_probe();
    let err = master.recv(50).expect_err("no peer response must timeout");
    assert!(matches!(err, DriverError::Timeout));
    assert!(
        master.now_ms_probe().saturating_sub(t0) <= 500,
        "超时必须有界"
    );
}

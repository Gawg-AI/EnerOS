//! 21-B B1 集成测试：Pl011UartHw 真 MMIO 路径 + 真实 RTU 帧层对通。
//!
//! 双端各持 `Pl011UartHw`（真 PL011 寄存器路径，`read_volatile`/
//! `write_volatile`），寄存器区为 RamMmio 仿真（`#[repr(C)]` 结构体 +
//! FIFO 解释，与 rs485 crate 测试同模式）。物理层逐字节驱动
//! （`write_bytes`/`read_byte` 真实调用），帧层用真实 `RtuFramer` +
//! `ModbusFrame`（CRC）完成请求-响应。
//!
//! `Rs485RtuTransport` 的 send/recv 编排逻辑（超时/循环）由
//! loopback.rs（MockUartHw）覆盖——本测试聚焦"真 PL011 MMIO 读写路径
//! 能承载真实 RTU 帧"。

use core::sync::atomic::{AtomicU64, Ordering};

use eneros_driver_framework::DriverError;
use eneros_modbus_rtu::frame::ModbusFrame;
use eneros_modbus_rtu::framer::RtuFramer;
use eneros_rs485::config::{Parity, StopBits};
use eneros_rs485::pl011_uart_hw::Pl011UartHw;
use eneros_rs485::uart_hw::UartHw;

/// 测试时钟（静态原子，满足 ClockFn 的 Send+Sync）。
static TEST_CLOCK: AtomicU64 = AtomicU64::new(0);

/// PL011 寄存器文件仿真（RamMmio）：`#[repr(C)]` 偏移与 PL011 一致。
#[repr(C)]
#[allow(dead_code)]
struct Pl011Regs {
    dr: u32,     // 0x00
    _rsvd1: u32, // 0x04
    _rsvd2: u32, // 0x08
    _rsvd3: u32, // 0x0C
    _rsvd4: u32, // 0x10
    _rsvd5: u32, // 0x14
    fr: u32,     // 0x18
    _rsvd6: u32, // 0x1C
    _rsvd7: u32, // 0x20
    ibrd: u32,   // 0x24
    fbrd: u32,   // 0x28
    lcrh: u32,   // 0x2C
    cr: u32,     // 0x30
    _rsvd8: u32, // 0x34
    imsc: u32,   // 0x38
}

impl Pl011Regs {
    const fn zero() -> Self {
        Self {
            dr: 0,
            _rsvd1: 0,
            _rsvd2: 0,
            _rsvd3: 0,
            _rsvd4: 0,
            _rsvd5: 0,
            fr: 0,
            _rsvd6: 0,
            _rsvd7: 0,
            ibrd: 0,
            fbrd: 0,
            lcrh: 0,
            cr: 0,
            _rsvd8: 0,
            imsc: 0,
        }
    }
}

/// 每个测试独立的 MMIO 区（Box 分配，生命周期=测试）+ FIFO 解释。
struct Sim {
    regs: *mut Pl011Regs,
    uart: Pl011UartHw,
    rx_fifo: Vec<u8>,
    tx_fifo: Vec<u8>,
}

impl Sim {
    fn new() -> Self {
        TEST_CLOCK.store(1_000_000_000, Ordering::Relaxed);
        let regs = Box::into_raw(Box::new(Pl011Regs::zero()));
        // 时钟步进：每次 now_ns +1µs——baud=9600 的 3.5 字符时间 ≈ 4ms，
        // 帧内字节间隔（1µs）远小于间隔阈值，帧不碎裂。
        let clock_fn: Box<dyn Fn() -> u64 + Send + Sync> =
            Box::new(|| TEST_CLOCK.fetch_add(1_000, Ordering::Relaxed));
        // SAFETY: 测试专用——`regs` 指向 Box 分配的仿真 PL011 寄存器。
        let uart = unsafe { Pl011UartHw::new(regs as u64, 24_000_000, clock_fn) };
        Sim {
            regs,
            uart,
            rx_fifo: Vec::new(),
            tx_fifo: Vec::new(),
        }
    }

    /// 把 FIFO 状态同步到 FR 寄存器（仿真设备行为）。
    ///
    /// BUSY 恒 0（"发送即完成"模型：TX FIFO 记录保留给测试断言，
    /// 硬件发送视为即时完成）——否则 `wait_tx_done` 永远忙等超时。
    /// `rx_has_byte`：本次读周期是否有字节可读（RXFE 必须反映"当前
    /// DR 待读"，而非 remove 后的队列状态——否则末字节因 RXFE=1
    /// 被 `read_byte` 拒读而丢失）。
    fn sync_fr(&self, rx_has_byte: bool) {
        // SAFETY: 测试专用——`regs` 指向 Box 分配的仿真寄存器。
        unsafe {
            let fr = &mut (*self.regs).fr;
            let mut v = 0u32;
            if self.tx_fifo.len() >= 16 {
                v |= 1 << 5; // FR_TXFF
            }
            if !rx_has_byte {
                v |= 1 << 4; // FR_RXFE
            }
            *fr = v;
        }
    }

    /// 仿真设备角色：消费 DR 写（进 TX FIFO）。
    fn device_after_write(&mut self) {
        // SAFETY: 测试专用——`regs` 指向 Box 分配的仿真寄存器。
        unsafe {
            let dr = &mut (*self.regs).dr;
            self.tx_fifo.push(*dr as u8);
            *dr = 0;
        }
    }

    /// 对端向 RX FIFO 压入字节。
    fn push_rx(&mut self, bytes: &[u8]) {
        self.rx_fifo.extend_from_slice(bytes);
        self.sync_fr(!self.rx_fifo.is_empty());
    }

    /// 适配器读 DR 前，从 RX FIFO 取一字节放入 DR（仿真设备行为）。
    fn device_before_read(&mut self) {
        // SAFETY: 测试专用——`regs` 指向 Box 分配的仿真寄存器。
        unsafe {
            let dr = &mut (*self.regs).dr;
            let had_byte = !self.rx_fifo.is_empty();
            if had_byte {
                *dr = self.rx_fifo.remove(0) as u32;
            }
            self.sync_fr(had_byte);
        }
    }
}

impl Drop for Sim {
    fn drop(&mut self) {
        // SAFETY: 测试专用——`regs` 指向 Box 分配的仿真寄存器。
        unsafe {
            drop(Box::from_raw(self.regs));
        }
    }
}

/// 物理层发送：逐字节经真 PL011 MMIO 写路径（等价 Rs485RtuTransport::send
/// 的底层操作序列），仿真捕获到 TX FIFO。
fn mmio_send(sim: &mut Sim, data: &[u8]) -> Result<(), DriverError> {
    for &b in data {
        sim.uart.write_bytes(&[b])?;
        sim.device_after_write();
    }
    sim.uart.wait_tx_done(100)?;
    Ok(())
}

/// 物理层接收：逐字节经真 PL011 MMIO 读路径，喂入真实帧层。
fn mmio_recv(
    sim: &mut Sim,
    framer: &mut RtuFramer,
    timeout_ms: u32,
) -> Result<Vec<u8>, DriverError> {
    let deadline = sim.uart.now_ns() + timeout_ms as u64 * 1_000_000;
    loop {
        sim.device_before_read();
        if let Some(b) = sim.uart.read_byte() {
            if let Ok(Some(frame)) = framer.feed_byte(b, sim.uart.now_ns()) {
                return Ok(frame.to_vec());
            }
            continue;
        }
        if sim.uart.now_ns() >= deadline {
            if let Some(frame) = framer.flush() {
                return Ok(frame.to_vec());
            }
            return Err(DriverError::Timeout);
        }
    }
}

#[test]
fn pl011_mmio_request_response_roundtrip() {
    let mut master = Sim::new();
    let mut slave = Sim::new();
    master
        .uart
        .configure(9600, 8, StopBits::One, Parity::None)
        .expect("master configure");
    slave
        .uart
        .configure(9600, 8, StopBits::One, Parity::None)
        .expect("slave configure");

    // 主站：请求读保持寄存器（addr=0, qty=2）——真实 CRC 编码。
    let request = ModbusFrame::new(1, 0x03, vec![0x00, 0x00, 0x00, 0x02]).encode();
    mmio_send(&mut master, &request).expect("master mmio send");

    // 线路：主站 TX → 从站 RX.
    let on_wire = master.tx_fifo.clone();
    assert_eq!(
        on_wire, request,
        "主站 TX FIFO 必须包含完整请求帧（真 MMIO 写路径）"
    );
    slave.push_rx(&on_wire);

    // 从站：真实帧层接收 + CRC 校验。
    let mut slave_framer = RtuFramer::new(9600);
    let recv = mmio_recv(&mut slave, &mut slave_framer, 100).expect("slave recv");
    let frame = ModbusFrame::decode(&recv).expect("request CRC valid");
    assert_eq!(frame.slave_addr, 1);
    assert_eq!(frame.func_code, 0x03);
    assert_eq!(frame.data, vec![0x00, 0x00, 0x00, 0x02]);

    // 从站：响应（2 个寄存器 0x1234/0x5678）。
    let response = ModbusFrame::new(1, 0x03, vec![0x04, 0x12, 0x34, 0x56, 0x78]).encode();
    mmio_send(&mut slave, &response).expect("slave mmio send");

    // 线路：从站 TX → 主站 RX.
    let on_wire = slave.tx_fifo.clone();
    assert_eq!(
        on_wire, response,
        "从站 TX FIFO 必须包含完整响应帧（真 MMIO 写路径）"
    );
    master.push_rx(&on_wire);

    // 主站：真实帧层接收 + CRC 校验。
    let mut master_framer = RtuFramer::new(9600);
    let recv = mmio_recv(&mut master, &mut master_framer, 100).expect("master recv");
    let frame = ModbusFrame::decode(&recv).expect("response CRC valid");
    assert_eq!(frame.slave_addr, 1);
    assert_eq!(frame.func_code, 0x03);
    assert_eq!(frame.data, vec![0x04, 0x12, 0x34, 0x56, 0x78]);
}

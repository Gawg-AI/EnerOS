//! 21-B B1 集成测试：SmoltcpTcpTransport 经 loopback 对通（QEMU loopback 语义）。
//!
//! 拓扑：两个 `LoopbackNetDevice` 互联（A.tx → B.rx 队列）——
//! 客户端（SmoltcpTcpTransport + ModbusTcpMaster）与迷你 Modbus 从站
//! （NetworkInterface + TcpSocket::listen）完成一次完整请求-响应。
//!
//! 证明"旧行为（trait 悬空）无法完成、新行为可以"：本测试使用生产
//! transport 走完整 MBAP 帧交换，无任何 mock transport。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use eneros_hal::{HalClock, HalError};
use eneros_modbus_tcp::device::TcpDevice;
use eneros_modbus_tcp::master::ModbusTcpMaster;
use eneros_modbus_tcp::mbap::MbapHeader;
use eneros_modbus_tcp::transport::SmoltcpTcpTransport;
use eneros_modbus_tcp::FunctionCode;
use eneros_net::tcpip::{ipv4_addr, ipv4_cidr, InterfaceConfig, NetworkInterface, TcpSocket};
use eneros_net::NetDevice;

// ============================================================================
// Loopback 网络设备（互联队列）
// ============================================================================

/// 互联介质：A 的 tx 帧进入 B 的 rx 队列（反之亦然）。
#[derive(Default)]
struct LinkCore {
    rx: VecDeque<Vec<u8>>,
}

/// 接收队列有界上限：防止互联拓扑错误导致帧无人消费、队列无界增长
/// （21-B B1 实测缺陷：错误接线使 client 帧堆积，内存无限占用）。
const MAX_PENDING_FRAMES: usize = 65_536;

/// 测试用 NetDevice：send 帧入对端 rx 队列，recv 从自身 rx 队列取。
///
/// **接线契约**：`rx`（自身接收队列）与 `peer`（对端接收队列）必须由
/// 构造方共享绑定——`A.rx == B.peer` 且 `B.rx == A.peer`，否则帧无人
/// 消费、队列无界增长（内存无限占用，见 21-B B1）。
#[derive(Clone)]
struct LoopbackNetDevice {
    mac: [u8; 6],
    rx: Arc<Mutex<LinkCore>>,
    peer: Arc<Mutex<LinkCore>>,
}

impl LoopbackNetDevice {
    fn new(mac: [u8; 6], rx: Arc<Mutex<LinkCore>>, peer: Arc<Mutex<LinkCore>>) -> Self {
        Self { mac, rx, peer }
    }
}

impl NetDevice for LoopbackNetDevice {
    fn send(&mut self, frame: &[u8]) -> Result<(), eneros_net::error::NetError> {
        let mut peer = self.peer.lock().unwrap();
        if peer.rx.len() >= MAX_PENDING_FRAMES {
            // fail-closed：互联错误必须立即暴露，禁止静默堆积（内存红线）。
            return Err(eneros_net::error::NetError::NoBuffer);
        }
        // 帧进入对端接收队列。
        peer.rx.push_back(frame.to_vec());
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, eneros_net::error::NetError> {
        let mut link = self.rx.lock().unwrap();
        match link.rx.pop_front() {
            Some(frame) => {
                // 对齐真驱动语义（MacController::recv mac.rs:345）：
                // 拷贝 min(len, buf.len())，帧超长截断而非丢弃。
                let copy_len = frame.len().min(buf.len());
                buf[..copy_len].copy_from_slice(&frame[..copy_len]);
                Ok(copy_len)
            }
            // NetDevice recv 契约（21-B B1 实测确立）：无帧必须返回 Err
            // （NoBuffer），不能返回 Ok(0)——`SmolcpDevice::drain_rx` 的
            // `while recv_frame()` 依赖 Err 终止；Ok(0) 会使 drain_rx
            // 无限循环入队空帧，内存无界增长。
            None => Err(eneros_net::error::NetError::NoBuffer),
        }
    }

    fn mac_address(&self) -> [u8; 6] {
        self.mac
    }

    fn mtu(&self) -> usize {
        1500
    }

    fn link_up(&self) -> bool {
        true
    }

    fn set_promiscuous(&mut self, _on: bool) {}

    fn stats(&self) -> eneros_net::NetStats {
        eneros_net::NetStats::default()
    }
}

// ============================================================================
// 迷你 Modbus 从站（测试专用真实 server：listen + 应答读保持寄存器）
// ============================================================================

/// 构造互联的两端设备：
/// 返回 (client_device, server_device)。
///
/// 接线（关键）：`client.rx == server.peer`、`server.rx == client.peer`——
/// client.send 的帧进入 server.rx（server 的 `recv` 可消费），反之亦然。
/// 两侧队列共享绑定，任何一端 poll 都会消费对端发来的帧。
fn make_pair() -> (
    LoopbackNetDevice,
    LoopbackNetDevice,
    Arc<Mutex<LinkCore>>,
    Arc<Mutex<LinkCore>>,
) {
    let client_rx = Arc::new(Mutex::new(LinkCore::default()));
    let server_rx = Arc::new(Mutex::new(LinkCore::default()));
    let client_dev = LoopbackNetDevice::new(
        [0x02, 0x00, 0x00, 0x00, 0x00, 0x01],
        client_rx.clone(),
        server_rx.clone(),
    );
    let server_dev = LoopbackNetDevice::new(
        [0x02, 0x00, 0x00, 0x00, 0x00, 0x02],
        server_rx.clone(),
        client_rx.clone(),
    );
    (client_dev, server_dev, client_rx, server_rx)
}

/// 从站轮询循环：处理一次 poll，若收到完整请求则应答读保持寄存器。
/// 返回 true 表示已应答（调用方可退出轮询）。
fn server_poll(
    iface: &mut NetworkInterface<LoopbackNetDevice>,
    tcp: &mut TcpSocket,
    now_ms: u64,
) -> bool {
    let _ = iface.poll(now_ms);
    let mut buf = [0u8; 512];
    match tcp.recv(iface, &mut buf) {
        Ok(n) if n >= 12 => {
            // 请求帧：MBAP(7) + PDU(func + addr(2) + qty(2))。
            let header = MbapHeader::decode(&buf[..7]).expect("valid mbap");
            let func = buf[7];
            let txn = header.transaction_id;
            let unit = header.unit_id;
            if func == FunctionCode::ReadHoldingRegisters as u8 {
                // 响应：MBAP(7) + func + byte_count + 2 个寄存器（0x1234, 0x5678）。
                let mut resp = Vec::new();
                resp.extend_from_slice(&MbapHeader::new(txn, unit, 5).encode());
                resp.push(func);
                resp.push(4); // 2 寄存器 × 2 字节
                resp.extend_from_slice(&[0x12, 0x34, 0x56, 0x78]);
                tcp.send(iface, &resp).expect("server send");
                // 推进 TX：smoltcp 的 send_slice 只入 TX 缓冲，实际发送在
                // 后续 poll 中完成——立即返回会导致响应帧滞留缓冲、client
                // 永远收不到（21-B B1 实测：MaxRetryExceeded）。
                for _ in 0..100 {
                    let now_ms = eneros_time::get_monotonic_ns() / 1_000_000;
                    let _ = iface.poll(now_ms);
                }
                return true;
            }
        }
        _ => {}
    }
    false
}

// ============================================================================
// 集成测试：完整请求-响应（读保持寄存器）
// ============================================================================

/// Host 测试时钟：基于 `std::time::SystemTime` 的真实单调时钟。
///
/// eneros-time 在 boot 前 `get_monotonic_ns()` 恒返回 0——smoltcp 的
/// ARP/TCP 重传定时依赖时间推进，时钟不前进则连接永不建立（21-B B1
/// 实测挂起）。集成测试必须在开头 `time_init` 注入真实时钟。
struct HostClock;

impl HalClock for HostClock {
    fn now_ns(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    }

    fn frequency_hz(&self) -> u64 {
        1_000_000_000
    }

    fn set_deadline(&self, _ns: u64) -> Result<(), HalError> {
        Ok(())
    }
}

static HOST_CLOCK: HostClock = HostClock;

#[test]
fn loopback_read_holding_registers_roundtrip() {
    // 注入真实单调时钟（幂等：多次调用仅重新赋值全局时钟）。
    eneros_time::time_init(&HOST_CLOCK, 0);

    // 构建互联对。
    let (client_dev, server_dev, client_rx, server_rx) = make_pair();

    // 从站线程：listen 502 + 持续 poll 应答。
    // 有界循环（约 1s 真实时间）——测试完成即退出，禁止无界运行。
    let server_ready = Arc::new(AtomicBool::new(false));
    let server_ready_flag = server_ready.clone();
    let server_handle = thread::spawn(move || {
        let mut iface = NetworkInterface::new(
            server_dev,
            InterfaceConfig {
                mac_addr: [0x02, 0x00, 0x00, 0x00, 0x00, 0x02],
                ipv4_addr: Some(ipv4_cidr(ipv4_addr(10, 0, 0, 2), 24)),
                gateway: None,
                dhcp: false,
            },
        );
        let handle = iface.sockets.add_tcp(4096, 4096);
        let mut tcp = TcpSocket::new(handle);
        tcp.listen(&mut iface, 502).expect("listen 502");
        // 就绪标志：client 等待 listen 生效后再 connect（消除启动竞态）。
        server_ready_flag.store(true, Ordering::SeqCst);
        let mut server_answered = false;
        for tick in 0..50_000u64 {
            let now_ms = eneros_time::get_monotonic_ns() / 1_000_000;
            server_answered |= server_poll(&mut iface, &mut tcp, now_ms);
            if tick % 500 == 499 {
                // 心跳：避免死循环未被发现（测试正常应在 tick << 50_000 完成）。
                eprintln!("[loopback] server still polling tick={tick}");
            }
            thread::yield_now();
            // 收到请求并应答后退出（server_poll 已消费请求帧）——
            // 避免 client 完成后线程继续空转（有界运行，内存/CPU 红线）。
            if server_answered {
                break;
            }
        }
    });

    // 客户端：SmoltcpTcpTransport（生产实现）+ ModbusTcpMaster。
    // 借用而非 Box::leak——master 生命周期限于本测试（无内存泄漏）。
    let client_iface = InterfaceConfig {
        mac_addr: [0x02, 0x00, 0x00, 0x00, 0x00, 0x01],
        ipv4_addr: Some(ipv4_cidr(ipv4_addr(10, 0, 0, 1), 24)),
        gateway: None,
        dhcp: false,
    };
    let mut transport = SmoltcpTcpTransport::new(client_dev, client_iface);
    let mut master = ModbusTcpMaster::new(&mut transport, 1);

    // 等待从站就绪（有界忙等：≤5s；就绪后立即继续）。
    let mut waited = 0u32;
    while !server_ready.load(Ordering::SeqCst) {
        std::thread::yield_now();
        waited += 1;
        assert!(waited < 5_000_000, "server thread never became ready");
    }

    let device = TcpDevice::new([10, 0, 0, 2], 502, 1);
    let resp = master.read_holding_registers(&device, 0, 2).unwrap_or_else(|e| {
        // 诊断：队列长度（测试失败时可定位断点）；随后断言失败暴露。
        let client_rx_len = client_rx.lock().unwrap().rx.len();
        let server_rx_len = server_rx.lock().unwrap().rx.len();
        eprintln!(
            "roundtrip failed: {e:?} client_rx_pending={client_rx_len} server_rx_pending={server_rx_len}"
        );
        Vec::new()
    });

    assert_eq!(resp.len(), 2, "两个保持寄存器");
    assert_eq!(resp[0], 0x1234, "寄存器 0 值");
    assert_eq!(resp[1], 0x5678, "寄存器 1 值");

    server_handle.join().expect("server thread");
}

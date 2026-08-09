//! 21-B B1 集成测试：SmoltcpSlaveTransport 经 loopback 对通（QEMU loopback 语义）。
//!
//! 拓扑：两个 `LoopbackNetDevice` 互联（A.tx → B.rx 队列）——客户端
//! （NetworkInterface + TcpSocket::connect）与从站（SmoltcpSlaveTransport
//! listen 2404）完成一次完整 IEC 104 请求-响应（STARTDT act → con）。
//!
//! 证明"旧行为（trait 悬空）无法完成、新行为可以"：生产 transport 走
//! 完整 APDU 帧交换，无任何 mock transport。
//!
//! 内存纪律（21-B B1 实测确立）：`NetDevice::recv` 无帧必须返回
//! `Err(NoBuffer)`（`SmolcpDevice::drain_rx` 依赖该契约终止）；互联
//! 队列有界（`MAX_PENDING_FRAMES` fail-closed）。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use eneros_hal::{HalClock, HalError};
use eneros_iec104_slave::transport::{ConnId, SlaveTransport, SmoltcpSlaveTransport};
use eneros_net::tcpip::{ipv4_addr, ipv4_cidr, InterfaceConfig, NetworkInterface, TcpSocket};
use eneros_net::NetDevice;

/// 接收队列有界上限：互联错误导致帧无人消费时 fail-closed（内存红线）。
const MAX_PENDING_FRAMES: usize = 65_536;

/// 互联介质：A 的 tx 帧进入 B 的 rx 队列（反之亦然）。
#[derive(Default)]
struct LinkCore {
    rx: VecDeque<Vec<u8>>,
}

/// 测试用 NetDevice：send 帧入对端 rx 队列，recv 从自身 rx 队列取。
///
/// 接线契约：`A.rx == B.peer` 且 `B.rx == A.peer`（帧必须有人消费）。
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
        peer.rx.push_back(frame.to_vec());
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, eneros_net::error::NetError> {
        let mut link = self.rx.lock().unwrap();
        match link.rx.pop_front() {
            Some(frame) => {
                // 对齐真驱动语义：拷贝 min(len, buf.len())。
                let copy_len = frame.len().min(buf.len());
                buf[..copy_len].copy_from_slice(&frame[..copy_len]);
                Ok(copy_len)
            }
            // NetDevice recv 契约：无帧必须返回 Err(NoBuffer)，不能 Ok(0)。
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

/// 构造互联的两端设备（接线：`A.rx == B.peer`、`B.rx == A.peer`）。
fn make_pair() -> (LoopbackNetDevice, LoopbackNetDevice) {
    let client_rx = Arc::new(Mutex::new(LinkCore::default()));
    let server_rx = Arc::new(Mutex::new(LinkCore::default()));
    let client_dev = LoopbackNetDevice::new(
        [0x02, 0x00, 0x00, 0x00, 0x00, 0x01],
        client_rx.clone(),
        server_rx.clone(),
    );
    let server_dev =
        LoopbackNetDevice::new([0x02, 0x00, 0x00, 0x00, 0x00, 0x02], server_rx, client_rx);
    (client_dev, server_dev)
}

/// Host 测试时钟：基于 `std::time::SystemTime` 的真实单调时钟。
///
/// eneros-time 在 boot 前 `get_monotonic_ns()` 恒返回 0——smoltcp 的
/// ARP/TCP 重传定时依赖时间推进。集成测试必须在开头 `time_init` 注入。
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

/// IEC 104 STARTDT 激活帧（启动数据传输）。
const STARTDT_ACT: [u8; 6] = [0x68, 0x04, 0x07, 0x00, 0x00, 0x00];
/// IEC 104 STARTDT 确认帧。
const STARTDT_CON: [u8; 6] = [0x68, 0x04, 0x0b, 0x00, 0x00, 0x00];

#[test]
fn loopback_startdt_roundtrip() {
    // 注入真实单调时钟（幂等）。
    eneros_time::time_init(&HOST_CLOCK, 0);

    let (client_dev, server_dev) = make_pair();

    // 从站线程：SmoltcpSlaveTransport listen 2404 + 轮询
    // （accept → recv → 应答 STARTDT con → 有界退出）。
    let server_ready = Arc::new(AtomicBool::new(false));
    let server_ready_flag = server_ready.clone();
    let server_handle = thread::spawn(move || {
        let iface_cfg = InterfaceConfig {
            mac_addr: [0x02, 0x00, 0x00, 0x00, 0x00, 0x02],
            ipv4_addr: Some(ipv4_cidr(ipv4_addr(10, 0, 0, 2), 24)),
            gateway: None,
            dhcp: false,
        };
        let mut transport =
            SmoltcpSlaveTransport::new(server_dev, iface_cfg, 2404).expect("slave listen 2404");
        server_ready_flag.store(true, Ordering::SeqCst);

        let mut conn: Option<ConnId> = None;
        // 有界轮询（约 2s 真实时间）：收到请求并应答后退出。
        for _tick in 0..200_000u64 {
            if conn.is_none() {
                conn = transport.accept();
            }
            let Some(conn_id) = conn else {
                thread::yield_now();
                continue;
            };
            let mut buf = [0u8; 256];
            match transport.recv(conn_id, &mut buf) {
                Ok(n) if n > 0 => {
                    assert_eq!(&buf[..n], &STARTDT_ACT, "收到 STARTDT 激活帧");
                    transport
                        .send(conn_id, &STARTDT_CON)
                        .expect("slave send STARTDT con");
                    // 推进 TX 发送（send 内部已推进；退出前再 poll 保证发出）。
                    transport.close(conn_id);
                    break;
                }
                Ok(_) => {}
                Err(_) => {}
            }
            thread::yield_now();
        }
    });

    // 等待从站就绪（有界忙等 ≤5s）。
    let mut waited = 0u32;
    while !server_ready.load(Ordering::SeqCst) {
        std::thread::yield_now();
        waited += 1;
        assert!(waited < 5_000_000, "server thread never became ready");
    }

    // 客户端：connect + 发送 STARTDT 激活 + 接收确认。
    let client_iface = InterfaceConfig {
        mac_addr: [0x02, 0x00, 0x00, 0x00, 0x00, 0x01],
        ipv4_addr: Some(ipv4_cidr(ipv4_addr(10, 0, 0, 1), 24)),
        gateway: None,
        dhcp: false,
    };
    let mut iface = NetworkInterface::new(client_dev, client_iface);
    let handle = iface.sockets.add_tcp(4096, 4096);
    let mut tcp = TcpSocket::new(handle);
    let remote = eneros_net::tcpip::ip_endpoint(ipv4_addr(10, 0, 0, 2), 2404);
    tcp.connect(&mut iface, remote, 40500).expect("connect");

    // 轮询推进握手（有界 2000 次）。
    let mut connected = false;
    for _ in 0..2000 {
        let now_ms = eneros_time::get_monotonic_ns() / 1_000_000;
        let _ = iface.poll(now_ms);
        if tcp.state(&iface).is_connected() {
            connected = true;
            break;
        }
    }
    assert!(connected, "client TCP connect must succeed");

    // 发送 STARTDT 激活帧。
    let mut sent = 0usize;
    for _ in 0..2000 {
        let now_ms = eneros_time::get_monotonic_ns() / 1_000_000;
        let _ = iface.poll(now_ms);
        match tcp.send(&mut iface, &STARTDT_ACT[sent..]) {
            Ok(0) => continue,
            Ok(n) => sent += n,
            Err(_) => {
                // 发送失败：清零 sent 使后续断言失败并暴露。
                eprintln!("client send failed");
                sent = 0;
                break;
            }
        }
        if sent >= STARTDT_ACT.len() {
            break;
        }
    }
    assert_eq!(sent, STARTDT_ACT.len(), "client must send full STARTDT act");

    // 接收 STARTDT 确认帧。
    let mut resp = [0u8; 256];
    let mut got = 0usize;
    for _ in 0..10_000 {
        let now_ms = eneros_time::get_monotonic_ns() / 1_000_000;
        let _ = iface.poll(now_ms);
        match tcp.recv(&mut iface, &mut resp) {
            Ok(0) => continue,
            Ok(n) => {
                got = n;
                break;
            }
            Err(_) => {
                // 接收失败：got 保持 0，断言失败并暴露。
                eprintln!("client recv failed");
                break;
            }
        }
    }
    assert_eq!(
        &resp[..got],
        &STARTDT_CON,
        "client must receive STARTDT con"
    );

    server_handle.join().expect("server thread");
}

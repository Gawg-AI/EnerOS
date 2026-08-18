//! svc-modbus-tcp — Modbus TCP 协议 EL0 服务（蓝图 39 WP-39.12；
//! 43 L2 协议互操作接线）。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![allow(clippy::empty_loop)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;

#[cfg(target_os = "none")]
extern crate eneros_protocol_services;

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use alloc::boxed::Box;

#[cfg(target_os = "none")]
use eneros_protocol_abstract::plugin::ProtocolPlugin;
#[cfg(target_os = "none")]
use eneros_protocol_abstract::TransportFactory;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOC: eneros_protocol_services::HeapAllocator =
    eneros_protocol_services::HeapAllocator::new();

/// 43（L2 协议互操作面）：modbus-tcp 真实 I/O 服务。
///
/// `_start` 收到 root 移交的 virtio-net 设备（X3=MMIO vaddr,
/// X4=DMA vaddr, X5=DMA phys）→ 经 [`net_transport`] 构造 smoltcp TCP
/// 传输并连接外部 Modbus-TCP 从站 peer；`EXECUTE` 触发真实读保持寄存器
/// 事务（外部 peer 侧可见并应答）。未移交（三参数为零）→ 维持能力壳
/// （EXECUTE → ST_UNAVAILABLE，fail-closed 不伪造协议 I/O）。
#[cfg(target_os = "none")]
struct ModbusTcpIoService {
    handle: alloc::boxed::Box<dyn eneros_protocol_abstract::TransportHandle>,
    device: eneros_modbus_tcp::device::TcpDevice,
    retries: u8,
    transactions: u32,
}

#[cfg(target_os = "none")]
impl ModbusTcpIoService {
    fn try_new(mmio_vaddr: u64, dma_vaddr: u64, dma_phys: u64) -> Result<Self, &'static str> {
        let transport = eneros_protocol_services::net_transport::build_net_transport(
            mmio_vaddr, dma_vaddr, dma_phys,
        )?;
        let mut factory =
            eneros_protocol_services::net_transport::NetTransportFactory::new(transport);
        // L2 互操作测试 peer：QEMU user-mode（slirp）10.0.2.2 = host
        // loopback 别名；5502 = 独立 raw Modbus-TCP 从站（非 EnerOS 代码）。
        // 生产 peer 来源：config-bundle（41/43 接线，未接通前以此测试面）。
        const PEER: &str = "10.0.2.2:5502";
        let (ip, port) = eneros_protocol_abstract::parse_ip_port(PEER).ok_or("invalid-peer")?;
        let mut spec = eneros_protocol_abstract::TransportSpec {
            kind: eneros_protocol_abstract::TransportKind::Tcp,
            peer: [0; 128],
            peer_len: PEER.len() as u8,
            mtu: 260,
            connect_timeout_ms: 3000,
            recv_timeout_ms: 1500,
        };
        spec.peer[..PEER.len()].copy_from_slice(PEER.as_bytes());
        let handle = factory.open(&spec).map_err(|_| "transport-open-failed")?;
        Ok(Self {
            handle,
            device: eneros_modbus_tcp::device::TcpDevice {
                ip,
                port,
                unit_id: 1,
                timeout_ms: 1500,
            },
            retries: 2,
            transactions: 0,
        })
    }

    /// 一轮真实读保持寄存器事务（三个地址；外部小从站应答）。
    fn read_cycle(&mut self) -> Result<alloc::vec::Vec<u16>, &'static str> {
        let mut out = alloc::vec::Vec::new();
        {
            let mut master =
                eneros_modbus_tcp::master::ModbusTcpMaster::new(&mut self.handle, self.retries);
            for addr in [0x1111u16, 0x2222u16, 0x3333u16] {
                let regs = master
                    .read_holding_registers(&self.device, addr, 1)
                    .map_err(|_| "modbus-read-failed")?;
                out.extend_from_slice(&regs);
            }
        }
        Ok(out)
    }
}

#[cfg(target_os = "none")]
impl eneros_drv_svc::runtime::DriverService for ModbusTcpIoService {
    fn capability(&self) -> Option<&'static str> {
        Some("net-modbus-tcp")
    }

    fn handle(
        &mut self,
        req: &eneros_drv_svc::runtime::DriverRequest,
        resp: &mut eneros_drv_svc::runtime::DriverResponse,
    ) {
        use eneros_drv_svc::runtime::*;
        if req.opcode == OP_EXECUTE {
            match self.read_cycle() {
                Ok(vals) => {
                    self.transactions += 1;
                    resp.status = ST_OK;
                    eneros_runtime::println!(
                        "[svc-modbus-tcp] MODBUS-L2-TX txn={} values=[{:?}]",
                        self.transactions,
                        vals
                    );
                }
                Err(e) => {
                    resp.status = ST_UNAVAILABLE;
                    eneros_runtime::println!("[svc-modbus-tcp] MODBUS-L2-ERR {:?}", e);
                }
            }
        } else {
            resp.status = ST_INVALID;
        }
    }
}

#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn _start(
    ep: u64,
    ipc_buffer: u64,
    ready_ntfn: u64,
    mmio_vaddr: u64,
    dma_vaddr: u64,
    dma_phys: u64,
) -> ! {
    eneros_sel4_sys::safe::set_ipc_buffer(ipc_buffer);
    // SAFETY: `_start` 单线程首调（首次 alloc 前）。
    unsafe { ALLOC.init() };
    // smoltcp 传输用 eneros_time 单调时钟推进；EL0 服务进程内初始化
    // （arm64 通用定时器，EL0 可读；rtc_base 0 = 无板载 RTC 事实）。
    eneros_time::time_init(eneros_hal::arm64::timer::clock(), 0);
    // 39：协议插件在服务侧构造（descriptor 随 readiness 报告）。
    let plugin = eneros_modbus_tcp::plugin::TcpPlugin::new();
    eneros_runtime::println!(
        "[protocol-service] modbus-tcp plugin={} role={:?} interop={:?}",
        plugin.descriptor().id,
        plugin.descriptor().role,
        plugin.descriptor().interop_level
    );
    // 43：设备移交（virtio-net 槽 8）→ 真实 I/O 服务；未移交 → 能力壳。
    let mut service: Box<dyn eneros_drv_svc::runtime::DriverService> =
        match ModbusTcpIoService::try_new(mmio_vaddr, dma_vaddr, dma_phys) {
            Ok(mut io) => {
                eneros_runtime::println!(
                    "[svc-modbus-tcp] NET-TRANSPORT READY capability=net-modbus-tcp"
                );
                // 启动期自检：连接 + 一轮真实读（外部 peer 记录事务；串口
                // 打印读回值 = L2 互操作证据行）。
                match io.read_cycle() {
                    Ok(vals) => {
                        eneros_runtime::println!(
                            "[svc-modbus-tcp] MODBUS-L2-SELFCHECK ok values=[{:?}]",
                            vals
                        );
                    }
                    Err(e) => {
                        eneros_runtime::println!(
                            "[svc-modbus-tcp] MODBUS-L2-SELFCHECK failed {:?}",
                            e
                        );
                    }
                }
                Box::new(io)
            }
            Err(e) => {
                eneros_runtime::println!(
                    "[svc-modbus-tcp] handover missing ({:?}) —— capability 壳（EXECUTE ST_UNAVAILABLE）",
                    e
                );
                Box::new(eneros_protocol_services::service::UnassignedProtocolService)
            }
        };
    eneros_drv_svc::runtime::driver_service_main(
        ep,
        "modbus-tcp",
        ready_ntfn,
        eneros_protocol_services::now_ms,
        eneros_protocol_services::instance_epoch(),
        &mut *service,
    )
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// host 构建 no-op 入口（与 svc-drv-* 同模式）。
#[cfg(all(not(test), not(target_os = "none")))]
fn main() {}

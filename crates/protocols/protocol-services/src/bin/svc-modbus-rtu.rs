//! svc-modbus-rtu — Modbus RTU 协议 EL0 服务。
//!
//! 蓝图 39 WP-39.12 建桩（未分配壳）；SPEC-SERIES-2026-08-27/D 升级为
//! **条件活路**（两态、无中间谎言态）：
//!
//! - **句柄在场**（root 经 `UART_HANDOVER_TABLE` 登记并移交非-console
//!   PL011 MMIO 页，`_start` X7 收 vaddr）⇒ 按 protocol-modbus-rtu 配置
//!   （双键表：baud/parity/clock_hz 真消费面）装配 `SerialTransportFactory`
//!   → `RtuPlugin` 真传输运行，readiness 词形 `RTU-ACTIVE`；
//! - **句柄缺席**（今日 QEMU 唯一事实 F-S1：无第二串口 ⇒ 允许清单空表）
//!   ⇒ 维持诚实能力壳（EXECUTE `ST_UNAVAILABLE`），readiness 词形
//!   `RTU-UNAVAILABLE reason=no-handle`。
//!
//! 真机验收程序注脚（SPEC-D WP-D.2 C4）：owner 在
//! `crates/runtime/root-task/src/spawn.rs::UART_HANDOVER_TABLE` 登记板级
//! 非-console pl011 reg 基址后，host 全链判据链（modbus-rtu/tests/
//! serial_plugin_chain.rs 同一插件/工厂路径）在板上复跑即为真机验收——
//! 本 bin 无需再改码。

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
use eneros_protocol_abstract::VecPointRegistry;
#[cfg(target_os = "none")]
use eneros_rs485::config::Parity;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOC: eneros_protocol_services::HeapAllocator =
    eneros_protocol_services::HeapAllocator::new();

/// 解析校验位（键集 KNOWN_KEYS 双源之一侧的值域：none|even|odd）。
#[cfg(target_os = "none")]
fn parse_parity(v: Option<&str>) -> Result<Parity, ()> {
    match v.unwrap_or("none") {
        "none" => Ok(Parity::None),
        "even" => Ok(Parity::Even),
        "odd" => Ok(Parity::Odd),
        _ => Err(()),
    }
}

/// SPEC-D WP-D.2 两态装配：句柄在场 → 全链组装为 `PluginIoService`；
/// 任一前置不满足 → 显式原因（词表 = `RTU-UNAVAILABLE reason=<word>`，
/// 与 host 测试/证据 grep 词形一致）。fail-closed：任何分支不得猜默认
/// 参数冒充活路（47 卷「配置零消费」禁令的 RTU 落地）。
#[cfg(target_os = "none")]
fn assemble(
    uart_mmio_vaddr: u64,
) -> Result<
    eneros_protocol_services::service::PluginIoService<eneros_modbus_rtu::plugin::RtuPlugin>,
    &'static str,
> {
    if uart_mmio_vaddr == 0 {
        return Err("no-handle");
    }
    // 46.3：配置经 svc-config 授予边拉取（S1 起按名 eipc_resolve——本服务
    // 首条边 = svc-config；管道单源 = service_cfg/config_gateway）；不可得
    // → 能力壳。wire 失败呈现（too short / length prefix）在管道内，
    // 词形单源 = protocol_ladder::line_*。
    // 64.2 WP-64.2.3：建端三行闭包收编 ChunkedEndpoint::bind（经
    // client_mut 喂 config 管道——config 面裸客户端，chunked 壳零使用）。
    let mut config_ep =
        match eneros_agent_sdk::endpoint::ChunkedEndpoint::bind("svc-modbus-rtu", "svc-config") {
            Some(ep) => ep,
            None => return Err("config-missing"),
        };
    let Ok(cfg) = eneros_protocol_services::service_cfg::fetch_protocol_config(
        config_ep.client_mut(),
        "modbus-rtu-config",
        "protocol-modbus-rtu",
    ) else {
        return Err("config-missing");
    };
    // baud/parity/clock_hz —— 插件 KNOWN_KEYS 白名单内的**真消费面**：
    // baud 必填非零（缺/坏 → fail-closed 不以 9600 冒充）；parity 缺省
    // 8N1；clock_hz 缺省 0 = Pl011UartHw 板级默认 24 MHz。
    let baud = match cfg.get("baud").map(str::parse::<u32>) {
        Some(Ok(b)) if b > 0 => b,
        _ => return Err("bad-baud"),
    };
    let parity = parse_parity(cfg.get("parity")).map_err(|_| "bad-parity")?;
    let clock_hz = cfg
        .get("clock_hz")
        .map(str::parse::<u32>)
        .transpose()
        .map_err(|_| "bad-clock-hz")?
        .unwrap_or(0);
    let mut plugin = eneros_modbus_rtu::plugin::RtuPlugin::new();
    let mut factory = eneros_protocol_services::net_transport::SerialTransportFactory::new(
        // SAFETY: `uart_mmio_vaddr` 为 root `try_uart_handover` 映射到本服务
        // VSpace 的 PL011 寄存器页基址（设备帧 DEVICE_RW_NX；生命周期=
        // 服务进程）。
        unsafe {
            eneros_rs485::Pl011UartHw::new(
                uart_mmio_vaddr,
                clock_hz,
                Box::new(eneros_time::get_monotonic_ns),
            )
        },
        baud,
    )
    .with_parity(parity);
    // 点表注入（45.4 路由；服务层按协议过滤 ModbusRtu 点）。
    let mut registry = fetch_points_registry();
    plugin
        .start(&cfg, &mut factory, &mut registry)
        .map_err(|_| "plugin-start")?;
    Ok(eneros_protocol_services::service::PluginIoService::new(
        plugin,
        "net-modbus-rtu",
    ))
}

/// 点表注入（45.4 路由）：经 lib service_cfg::fetch_points_registry 取
/// 全量注册表 + 协议过滤（仅 ModbusRtu）——SPEC-B WP-B.1 收编私有副本；
/// S1 起槽位按名解析、管道单源 = agent-sdk config_gateway。
#[cfg(target_os = "none")]
fn fetch_points_registry() -> VecPointRegistry {
    use eneros_config_bundle::points::PointProtocol;
    // 64.2 WP-64.2.3：建端闭包收编 ChunkedEndpoint::bind（config 面裸
    // 客户端经 client_mut；chunked 壳零使用）。
    match eneros_agent_sdk::endpoint::ChunkedEndpoint::bind("svc-modbus-rtu", "svc-config") {
        Some(mut ep) => eneros_protocol_services::service_cfg::fetch_points_registry(
            ep.client_mut(),
            "modbus-rtu-config",
            |p| p.protocol == PointProtocol::ModbusRtu,
        ),
        // S1 evidence M-3w：本 wrapper 的解析/建端失败为静默空表（原为
        // NOT-granted 行；结构上不可达——边在表内由 C6 冻结测试锁定）。
        None => VecPointRegistry::new(alloc::vec::Vec::new()),
    }
}

/// 寄存器布局由 root `initial_registers` 单一持有（W2.1/SPEC-B；SPEC-D 起
/// X7=UART vaddr）。形参位置 ↔ 寄存器：ep=X0 / buffer=X1 / ready=X2 /
/// X3=EIPC 掩码（本 bin 经 agent_sdk 槽常量寻径，掩码不入逻辑）/
/// X4=心跳槽 / X5=保留 / X6=闸门墙钟 epoch 秒 / X7=UART MMIO vaddr。
#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn _start(
    ep: u64,
    ipc_buffer: u64,
    ready_ntfn: u64,
    _x3_eipc_mask: u64,
    _x4_heartbeat_slot: u64,
    _x5_reserved: u64,
    wall_epoch: u64,
    uart_mmio_vaddr: u64,
) -> ! {
    eneros_sel4_sys::safe::set_ipc_buffer(ipc_buffer);
    // SAFETY: `_start` 单线程首调（首次 alloc 前）。
    unsafe { ALLOC.init() };
    // RS485 成帧用 eneros_time 单调时钟推进（EL0 arm64 通用定时器）。
    eneros_time::time_init(eneros_hal::arm64::timer::clock(), 0);
    // SPEC-B WP-B.1：X6 墙钟 epoch 秒（0=缺失显式降级）。
    eneros_protocol_services::apply_wall_epoch("modbus-rtu", wall_epoch);
    // 39：协议插件在服务侧构造（descriptor 随 readiness 报告）。
    let plugin = eneros_modbus_rtu::plugin::RtuPlugin::new();
    eneros_runtime::println!(
        "[protocol-service] modbus-rtu plugin={} role={:?} interop={:?}",
        plugin.descriptor().id,
        plugin.descriptor().role,
        plugin.descriptor().interop_level
    );
    drop(plugin);
    // SPEC-D 两态装配（词表见 assemble 文档）。
    let mut service: Box<dyn eneros_drv_svc::runtime::DriverService> = match assemble(
        uart_mmio_vaddr,
    ) {
        Ok(io) => {
            eneros_runtime::println!(
                "[svc-modbus-rtu] RTU-ACTIVE uart=0x{uart_mmio_vaddr:x} capability=net-modbus-rtu"
            );
            Box::new(io)
        }
        Err(reason) => {
            eneros_runtime::println!(
                "[svc-modbus-rtu] RTU-UNAVAILABLE reason={reason} —— capability 壳（EXECUTE ST_UNAVAILABLE）"
            );
            // 能力壳构造单点（63.2 WP-63.2.4 P1 收编——直呼 Unassigned 违纪修除）。
            eneros_protocol_services::protocol_ladder::capability_shell()
        }
    };
    eneros_drv_svc::runtime::driver_service_main(
        ep,
        "modbus-rtu",
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

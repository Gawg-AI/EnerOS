//! svc-mqtt — 协议 EL0 服务（S1：装配阶梯单源 `protocol_ladder`，bin 纯接线；
//! 46.3 配置驱动装配 + 47 WP-47.1 协议入站写：下行写命令 → 意图导出 →
//! 转发 svc-control-dispatch）。
//!
//! 蓝图 64 卷 64.2（WP-64.2.4）：`MqttWriteIoService` 中继面（三转发环 +
//! 双面分片重组 + 计数）迁 lib `mqtt_io`（host 可测）；本 bin 只保留装配
//! （SECURITY-CHECK → 阶梯 → 三端点绑定 → 服务构造 → 自检轮）。
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
use eneros_protocol_services::protocol_ladder;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOC: eneros_protocol_services::HeapAllocator =
    eneros_protocol_services::HeapAllocator::new();

/// 46.2/46.3 + 47 + S1：手柄移交 → 配置装配插件 → 意图转发服务。
#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn _start(
    ep: u64,
    ipc_buffer: u64,
    ready_ntfn: u64,
    mmio_vaddr: u64,
    dma_vaddr: u64,
    dma_phys: u64,
    wall_epoch: u64,
) -> ! {
    use eneros_protocol_services::mqtt_io::MqttWriteIoService;

    eneros_sel4_sys::safe::set_ipc_buffer(ipc_buffer);
    // SAFETY: `_start` 单线程首调（首次 alloc 前）。
    unsafe { ALLOC.init() };
    let parts = protocol_ladder::ProtocolParts {
        service: "svc-mqtt",
        config_client: "mqtt-config",
        config_id: "protocol-mqtt",
        capability: "eneros_mqtt::plugin::MqttPlugin",
        points: protocol_ladder::PointsPolicy::None,
        config_dump: false,
        // 46.5：启动期自检轮询（post-connect 交换尽早完成）。
        self_check_rounds: 150,
        degraded: protocol_ladder::DegradedStyle::Standard,
    };
    let mut io = protocol_ladder::Sel4ServiceIo::new(
        parts.service,
        parts.config_client,
        mmio_vaddr,
        dma_vaddr,
        dma_phys,
    );
    // 39：协议插件在服务侧构造（descriptor 随 readiness 报告）。
    let mut plugin = eneros_mqtt::plugin::MqttPlugin::new();
    protocol_ladder::print_descriptor(parts.service, &plugin);
    // 47（WP-47.5）：policy/实现一致机器校验——注册表。
    // 70.5（WP-70.5.6）双臂装配：
    // - qemu-test（feature qemu-test-synth）：AssemblyMode::QemuTest 显式
    //   明文（InsecureStream 永不冒充；closure gate 强制 release 闭包
    //   无此 feature）；
    // - release（mqtt-tls）：AssemblyMode::ReleaseTls——构建期嵌入材料
    //   在位 → 注入插件 TLS 材料 + 注册工厂（has=true → 装配允许，真实
    //   TLS 1.3 握手）；材料缺失 → has=false → 能力壳 fail-closed
    //   （不降级明文）。
    {
        #[cfg(feature = "qemu-test-synth")]
        {
            let registry = eneros_protocol_abstract::channel::SecureChannelRegistry::new();
            let mode = eneros_protocol_abstract::channel::AssemblyMode::QemuTest;
            let profile = plugin.descriptor().security_profile;
            let has = registry.has(plugin.descriptor().id);
            let policy = eneros_protocol_abstract::channel::ChannelPolicy::default_dev();
            let ok =
                eneros_protocol_abstract::channel::assembly_allows(mode, profile, has, &policy);
            eneros_runtime::println!(
                "[svc-mqtt] SECURITY-CHECK mode=QemuTest profile={profile:?} factory={has} assembly={}",
                if ok {
                    "allowed（显式明文）"
                } else {
                    "REJECTED"
                }
            );
            if !ok {
                eneros_runtime::println!(
                    "[svc-mqtt] policy/实现不一致 —— capability 壳（fail-closed）"
                );
                let mut shell = protocol_ladder::capability_shell();
                eneros_drv_svc::runtime::driver_service_main(
                    ep,
                    "mqtt",
                    ready_ntfn,
                    eneros_protocol_services::now_ms,
                    eneros_protocol_services::instance_epoch(),
                    shell.as_mut(),
                );
            }
        }
        #[cfg(all(not(feature = "qemu-test-synth"), feature = "mqtt-tls"))]
        {
            // 时间 = X6 墙钟链派生（SPEC-B；单调 now_ms 会让证书域检
            // 落在 1970 年——窗A 实弹 InvalidCertificate 教训）。
            let material = eneros_protocol_services::tls_material::embedded_material(wall_epoch);
            match material {
                Some(mat) => {
                    plugin.set_tls_material(mat);
                    if wall_epoch == 0 {
                        // X6 墙钟缺失：证书有效期以 0 为基准 → 全部
                        // CertNotYetValid，握手必败（fail-closed 方向正确）——
                        // 诚实打印，不让下一行 "allowed" 误导观测（70.5 复查）。
                        eneros_runtime::println!(
                            "[svc-mqtt] SECURITY-CHECK mode=ReleaseTls: X6 wall epoch missing (0) —— TLS 握手将全部失败（证书 not-yet-valid）直至时间可得"
                        );
                    }
                    eneros_runtime::println!(
                        "[svc-mqtt] SECURITY-CHECK mode=ReleaseTls profile=Tls13 factory=true assembly=allowed（材料嵌入在位，握手时验证）"
                    );
                }
                None => {
                    eneros_runtime::println!(
                        "[svc-mqtt] SECURITY-CHECK mode=ReleaseTls: 材料缺失 —— capability 壳（fail-closed，不降级明文）"
                    );
                    let mut shell = protocol_ladder::capability_shell();
                    eneros_drv_svc::runtime::driver_service_main(
                        ep,
                        "mqtt",
                        ready_ntfn,
                        eneros_protocol_services::now_ms,
                        eneros_protocol_services::instance_epoch(),
                        shell.as_mut(),
                    );
                }
            }
        }
        #[cfg(all(not(feature = "qemu-test-synth"), not(feature = "mqtt-tls")))]
        {
            // 无 mqtt-tls 的 release 构建 = 装配面缺 TLS 支持——fail-closed
            // （能力壳；70.5 起明文 broker 不允许出现在 release 闭包）。
            eneros_runtime::println!(
                "[svc-mqtt] SECURITY-CHECK mode=ReleaseTls: TLS 支持未编译 —— capability 壳（fail-closed）"
            );
            let mut shell = protocol_ladder::capability_shell();
            eneros_drv_svc::runtime::driver_service_main(
                ep,
                "mqtt",
                ready_ntfn,
                eneros_protocol_services::now_ms,
                eneros_protocol_services::instance_epoch(),
                shell.as_mut(),
            );
        }
    }
    let mut service: Box<dyn eneros_drv_svc::runtime::DriverService> =
        match protocol_ladder::run(wall_epoch, &parts, plugin, &mut io) {
            protocol_ladder::Ladder::Ready { plugin, .. } => {
                // 47：dispatch 端点（WP-54.4 按名解析；未授予
                // → 写意图路径 fail-closed）。
                let dispatch = eneros_agent_sdk::endpoint::ChunkedEndpoint::bind(
                    "svc-mqtt",
                    "svc-control-dispatch",
                );
                if dispatch.is_none() {
                    eneros_runtime::println!(
                        "[svc-mqtt] dispatch EIPC NOT granted —— 写意图路径 fail-closed"
                    );
                }
                // 49（WP-49.2）：ai-worker 端点（表序第 3 条授予）；AI 请求
                // 主题取自配置（插件已消费）。
                let ai_worker =
                    eneros_agent_sdk::endpoint::ChunkedEndpoint::bind("svc-mqtt", "svc-ai-worker");
                let ai_request_topic = plugin.ai_request_topic().map(alloc::string::String::from);
                match (&ai_worker, &ai_request_topic) {
                    (None, _) => {
                        eneros_runtime::println!(
                            "[svc-mqtt] ai-worker EIPC NOT granted —— 应答转发 fail-closed"
                        );
                    }
                    (_, None) => {
                        eneros_runtime::println!(
                            "[svc-mqtt] ai_request_topic 未配置 —— AI 链路关闭（诚实）"
                        );
                    }
                    _ => {
                        eneros_runtime::println!(
                            "[svc-mqtt] AI-RELAY armed topic={}",
                            ai_request_topic.as_deref().unwrap_or_default()
                        );
                    }
                }
                // 50（WP-50.2）：svc-mgmt 端点（按名解析；未授予 → 管理请求
                // 丢弃）。
                let mgmt =
                    eneros_agent_sdk::endpoint::ChunkedEndpoint::bind("svc-mqtt", "svc-mgmt");
                // 蓝图 66 WP-66.3.2：ai_request_topic 不再注入 io（主题解析
                // 内化插件）；上方装配期打印保留 = 诚实观测，三臂 match 非门
                // （门在插件 enqueue_ai_publish fail-closed 分类）。
                let mut svc = MqttWriteIoService::new(
                    eneros_protocol_services::service::PluginIoService::new(
                        plugin,
                        parts.capability,
                    ),
                    dispatch,
                    ai_worker,
                    mgmt,
                );
                // 46.5：启动期自检轮询（post-connect 交换尽早完成）。
                svc.io.self_check_poll(parts.self_check_rounds);
                svc.forward_intents();
                Box::new(svc)
            }
            protocol_ladder::Ladder::Degraded(_) => protocol_ladder::capability_shell(),
        };
    eneros_drv_svc::runtime::driver_service_main(
        ep,
        "mqtt",
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

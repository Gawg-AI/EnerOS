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

/// C9：材料在位性探测（mqtt-tls 构建 = embedded_material 判定；非
/// mqtt-tls 构建恒 false——门第三臂）。
#[cfg(all(target_os = "none", feature = "mqtt-tls"))]
fn material_available(wall_epoch: u64) -> bool {
    eneros_protocol_services::tls_material::embedded_material(wall_epoch).is_some()
}
#[cfg(all(target_os = "none", not(feature = "mqtt-tls")))]
fn material_available(_wall_epoch: u64) -> bool {
    false
}

/// C9：材料注入（Proceed + inject 臂；`embedded_material` 为种子派生
/// 纯函数——探测与注入两次调用逐字节等值）。
#[cfg(all(target_os = "none", feature = "mqtt-tls"))]
fn inject_tls_material(wall_epoch: u64, plugin: &mut eneros_mqtt::plugin::MqttPlugin) {
    if let Some(mat) = eneros_protocol_services::tls_material::embedded_material(wall_epoch) {
        plugin.set_tls_material(mat);
    }
}
#[cfg(all(target_os = "none", not(feature = "mqtt-tls")))]
fn inject_tls_material(_wall_epoch: u64, _plugin: &mut eneros_mqtt::plugin::MqttPlugin) {}

/// C9：gate 调用（default_dev 策略 = HEAD bin 语义；策略单源在
/// protocol-abstract）。
#[cfg(target_os = "none")]
fn protocol_services_mqtt_gate(
    wall_epoch: u64,
    profile: eneros_protocol_abstract::plugin::SecurityProfile,
    plugin_id: &'static str,
    material_present: bool,
) -> protocol_ladder_gate::MqttGateVerdict {
    eneros_protocol_services::mqtt_assembly::security_gate(
        cfg!(feature = "qemu-test-synth"),
        cfg!(feature = "mqtt-tls"),
        wall_epoch,
        profile,
        plugin_id,
        &eneros_protocol_abstract::channel::ChannelPolicy::default_dev(),
        material_present,
    )
}

/// gate verdict 类型别名（可读性）。
#[cfg(target_os = "none")]
use eneros_protocol_services::mqtt_assembly as protocol_ladder_gate;

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
    // 47（WP-47.5）+ 70.5（WP-70.5.6）：三臂 SECURITY-CHECK 判定 + 观测
    // 词形单源 = `mqtt_assembly::security_gate`（C9 收口，host 逐臂可测）；
    // 本处只剩：材料探测（feature 门控）、逐行打印、注入/能力壳分派。
    // 臂互斥优先序（qemu-test 优先）与词形由 gate 冻结，与原 cfg 臂逐
    // 字节一致。
    let material_present = material_available(wall_epoch);
    match protocol_services_mqtt_gate(
        wall_epoch,
        plugin.descriptor().security_profile,
        plugin.descriptor().id,
        material_present,
    ) {
        protocol_ladder_gate::MqttGateVerdict::Proceed {
            lines,
            // B75-1 附带发现（预存）：本绑定与同模块自由函数
            // `inject_tls_material` 同名遮蔽——target-only cfg 下 E0618
            // （host 空 bin 桩不可见，C9 收口在途层遗留）。改名 `inject`
            // 保留调用词形与行为。
            inject_tls_material: inject,
        } => {
            for line in &lines {
                eneros_runtime::println!("{line}");
            }
            if inject {
                inject_tls_material(wall_epoch, &mut plugin);
            }
        }
        protocol_ladder_gate::MqttGateVerdict::Shell { lines } => {
            for line in &lines {
                eneros_runtime::println!("{line}");
            }
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

eneros_runtime::svc_panic!("svc-mqtt");

/// host 构建 no-op 入口（与 svc-drv-* 同模式）。
#[cfg(all(not(test), not(target_os = "none")))]
fn main() {}

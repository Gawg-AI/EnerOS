//! svc-mqtt — 协议 EL0 服务（46 WP-46.2 句柄移交 + 46.3 配置驱动装配 +
//! 47 WP-47.1 协议入站写：下行写命令 → 意图导出 → 转发 svc-control-dispatch）。
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
use eneros_protocol_abstract::EmptyPointRegistry;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOC: eneros_protocol_services::HeapAllocator =
    eneros_protocol_services::HeapAllocator::new();

/// 47（WP-47.1）：写意图转发服务——插件 poll（含下行写解析）+ 意图
/// EIPC 上报 svc-control-dispatch（`DISP_SUBMIT_INTENT` + 意图线码 +
/// actor）。dispatch 端点未授予 → 意图不转发（fail-closed，串口登记）。
#[cfg(target_os = "none")]
struct MqttWriteIoService {
    io: eneros_protocol_services::service::PluginIoService<eneros_mqtt::plugin::MqttPlugin>,
    /// svc-control-dispatch EIPC 客户端（授予槽 `EIPC_EP_BASE_SLOT + 1`；
    /// 槽 0 = svc-config 协议配置）。
    dispatch: Option<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
    /// 分块传输 epoch（单调）。
    dispatch_epoch: u64,
    /// 转发成功计数（回执 Committed/Failed/Indeterminate 均算已受理）。
    pub forwarded_total: u64,
    /// 转发被拒计数（服务侧拒绝/回执 DISP_ERR）。
    pub rejected_total: u64,
    /// 49（WP-49.2）：AI 请求分片组装器。
    ai_assembler: eneros_agent_sdk::relay_face::RelayAssembler,
    /// 49：svc-ai-worker EIPC 客户端（授予槽 `EIPC_EP_BASE_SLOT + 2`；
    /// 槽序 = 表序 config(0)/dispatch(1)/ai-worker(2)）。未授予 → 应答
    /// 丢弃计数（fail-closed）。
    ai_worker: Option<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
    /// 49：分块传输 epoch（单调）。
    ai_epoch: u64,
    /// 49：AI 请求发布主题（protocol-mqtt 配置；未配置 = AI 链路关闭）。
    ai_request_topic: Option<alloc::string::String>,
    // 49 观测计数。
    pub ai_req_published: u64,
    pub ai_resp_forwarded: u64,
    pub ai_errors: u64,
    /// 50（WP-50.1）：遥测帧分片组装器（与 AI 请求组装同语义；不解释内容）。
    tel_assembler: eneros_agent_sdk::relay_face::RelayAssembler,
    /// 50：遥测帧发布/拒绝计数（观测；拒绝 = 未配置主题/断连/坏帧）。
    pub tel_published: u64,
    pub tel_rejected: u64,
    /// 50（WP-50.2）：svc-mgmt EIPC 客户端（授予槽 `EIPC_EP_BASE_SLOT + 3`；
    /// 槽序 = 表序 config(0)/dispatch(1)/ai-worker(2)/mgmt(3)）。未授予 →
    /// 管理请求丢弃计数（fail-closed）。
    mgmt: Option<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
    /// 50：分块传输 epoch（单调）。
    mgmt_epoch: u64,
    /// 50：管理请求转发/失败计数（观测）。
    pub mgmt_forwarded: u64,
    pub mgmt_errors: u64,
}

#[cfg(target_os = "none")]
impl MqttWriteIoService {
    fn forward_intents(&mut self) {
        let now = eneros_protocol_services::now_ms();
        let mut intents: alloc::vec::Vec<eneros_protocol_abstract::write::PointWriteIntent> =
            alloc::vec::Vec::new();
        self.io.drain_write_intents(&mut intents);
        if intents.is_empty() {
            return;
        }
        for intent in intents {
            // 请求布局：[DISP_SUBMIT_INTENT=2][intent wire 49B][actor 2=Ai]。
            let mut req: alloc::vec::Vec<u8> = alloc::vec::Vec::with_capacity(1 + 49 + 1);
            req.push(2);
            req.extend_from_slice(&eneros_protocol_abstract::write::encode_intent(&intent));
            req.push(2);
            let Some(client) = &mut self.dispatch else {
                eneros_runtime::println!(
                    "[svc-mqtt] INTENT-FWD dispatch EP NOT granted —— 意图未转发（fail-closed）"
                );
                self.rejected_total = self.rejected_total.saturating_add(1);
                continue;
            };
            let mut transfer =
                eneros_agent_sdk::ChunkedClient::new(client, self.dispatch_epoch, 5_000);
            self.dispatch_epoch = self.dispatch_epoch.saturating_add(1);
            match transfer.transfer("svc-control-dispatch", &req) {
                Ok(resp) if resp.len() >= 2 && resp[0] == 0 => {
                    self.forwarded_total = self.forwarded_total.saturating_add(1);
                    eneros_runtime::println!(
                        "[svc-mqtt] INTENT-FWD protocol=mqtt device={} point={} power={:?} seq={} state={} total={}",
                        intent.device_id,
                        u32::from(intent.point_id),
                        intent.value,
                        intent.sequence,
                        resp[1],
                        self.forwarded_total
                    );
                }
                Ok(resp) => {
                    self.rejected_total = self.rejected_total.saturating_add(1);
                    eneros_runtime::println!(
                        "[svc-mqtt] INTENT-FWD rejected full-receipt {:02x?}",
                        &resp[..resp.len().min(82)]
                    );
                }
                Err(e) => {
                    self.rejected_total = self.rejected_total.saturating_add(1);
                    eneros_runtime::println!("[svc-mqtt] INTENT-FWD eipc={e:?}");
                }
            }
        }
        let _ = now;
    }

    /// 49（WP-49.2）：把插件收到的 peer 应答原始字节推送 svc-ai-worker
    ///（ChunkedClient，载荷 = [AI_PEER_MSG][json]）。未授予/失败 → 计数
    /// 丢弃（fail-closed；ai-worker 侧超时降级接管）。
    fn forward_ai_responses(&mut self) {
        let mut msgs: alloc::vec::Vec<alloc::vec::Vec<u8>> = alloc::vec::Vec::new();
        self.io.plugin_mut().drain_peer_messages(&mut msgs);
        for payload in msgs {
            let mut req: alloc::vec::Vec<u8> = alloc::vec::Vec::with_capacity(1 + payload.len());
            req.push(eneros_agent_sdk::ai_relay::AI_PEER_MSG);
            req.extend_from_slice(&payload);
            let Some(client) = &mut self.ai_worker else {
                self.ai_errors = self.ai_errors.saturating_add(1);
                eneros_runtime::println!(
                    "[svc-mqtt] AI-RESP-DROP ai-worker EP NOT granted（fail-closed）"
                );
                continue;
            };
            let mut transfer = eneros_agent_sdk::ChunkedClient::new(client, self.ai_epoch, 5_000);
            self.ai_epoch = self.ai_epoch.saturating_add(1);
            match transfer.transfer("svc-ai-worker", &req) {
                Ok(_) => {
                    self.ai_resp_forwarded = self.ai_resp_forwarded.saturating_add(1);
                    eneros_runtime::println!(
                        "[svc-mqtt] AI-RESP-FWD len={} total={}",
                        payload.len(),
                        self.ai_resp_forwarded
                    );
                }
                Err(e) => {
                    self.ai_errors = self.ai_errors.saturating_add(1);
                    eneros_runtime::println!("[svc-mqtt] AI-RESP-DROP eipc={e:?}");
                }
            }
        }
    }

    /// 50（WP-50.2）：把插件收到的管理请求（mgmt_request_topic 原始
    /// MGT1 帧）经 EIPC 推送 svc-mgmt；应答原样发布回 mgmt_response_topic。
    /// 未授予/失败 → 计数丢弃（fail-closed——管理通道降级，不伪造成功）。
    fn forward_mgmt_requests(&mut self) {
        let mut reqs: alloc::vec::Vec<alloc::vec::Vec<u8>> = alloc::vec::Vec::new();
        self.io.plugin_mut().drain_mgmt_requests(&mut reqs);
        for payload in reqs {
            let Some(client) = &mut self.mgmt else {
                self.mgmt_errors = self.mgmt_errors.saturating_add(1);
                eneros_runtime::println!(
                    "[svc-mqtt] MGMT-FWD-DROP svc-mgmt EP NOT granted（fail-closed）"
                );
                continue;
            };
            let mut transfer = eneros_agent_sdk::ChunkedClient::new(client, self.mgmt_epoch, 5_000);
            self.mgmt_epoch = self.mgmt_epoch.saturating_add(1);
            match transfer.transfer("svc-mgmt", &payload) {
                Ok(resp) => {
                    self.mgmt_forwarded = self.mgmt_forwarded.saturating_add(1);
                    // 应答原样发布（MGT1 响应帧：status 1B + detail）。
                    let published = self.io.plugin_mut().enqueue_mgmt_response(&resp);
                    eneros_runtime::println!(
                        "[svc-mqtt] MGMT-FWD len={} total={} resp-publish={}",
                        payload.len(),
                        self.mgmt_forwarded,
                        if published.is_ok() {
                            "ok"
                        } else {
                            "drop（未配置应答主题/outbox 满）"
                        }
                    );
                }
                Err(e) => {
                    self.mgmt_errors = self.mgmt_errors.saturating_add(1);
                    eneros_runtime::println!("[svc-mqtt] MGMT-FWD eipc={e:?}");
                }
            }
        }
    }

    /// 蓝图 59：EXECUTE 载荷分片子码统一处理（AI 请求面 + 遥测帧面）。
    /// BEGIN/CONT 两面同构走 relay_face 单源；END 按面定稿（域语义不同：
    /// AI 发布应答主题 vs 遥测帧头三重校验）。返回 Some(1B 状态) = 已消费；
    /// None = 非本两面子码（调用方继续既有 EXECUTE 处理）。
    fn handle_relay_fragment(&mut self, payload: &[u8]) -> Option<u8> {
        use eneros_agent_sdk::ai_relay::FACE as AI_FACE;
        use eneros_agent_sdk::relay_face::FragmentStep;
        use eneros_agent_sdk::telemetry_relay::FACE as TEL_FACE;

        for (face, is_ai) in [(AI_FACE, true), (TEL_FACE, false)] {
            match face.classify(payload) {
                Some(FragmentStep::Begin { total, data }) => {
                    let asm = self.assembler_mut(is_ai);
                    return match asm.begin(total, data, face.max_total) {
                        Ok(()) => Some(0),
                        Err(e) => {
                            self.relay_reject(is_ai, "begin", &e);
                            Some(1)
                        }
                    };
                }
                Some(FragmentStep::Cont { data }) => {
                    let asm = self.assembler_mut(is_ai);
                    return match asm.cont(data) {
                        Ok(()) => Some(0),
                        Err(e) => {
                            self.relay_reject(is_ai, "cont", &e);
                            Some(1)
                        }
                    };
                }
                Some(FragmentStep::End) => {
                    return Some(if is_ai {
                        self.ai_finalize()
                    } else {
                        self.tel_finalize()
                    });
                }
                None => continue,
            }
        }
        None
    }

    /// 面选择器（AI/遥测组装器）。
    fn assembler_mut(&mut self, is_ai: bool) -> &mut eneros_agent_sdk::relay_face::RelayAssembler {
        if is_ai {
            &mut self.ai_assembler
        } else {
            &mut self.tel_assembler
        }
    }

    /// 分片错误公共出口：计数 + 前缀化打印（文本与原双 handler 逐字一致）。
    fn relay_reject(
        &mut self,
        is_ai: bool,
        step: &str,
        e: &eneros_agent_sdk::relay_face::AssembleError,
    ) {
        if is_ai {
            self.ai_errors = self.ai_errors.saturating_add(1);
            eneros_runtime::println!("[svc-mqtt] AI-FRAG-REJECT {step} {e:?}");
        } else {
            self.tel_rejected = self.tel_rejected.saturating_add(1);
            eneros_runtime::println!("[svc-mqtt] TEL-FRAG-REJECT {step} {e:?}");
        }
    }

    /// AI 面 END 定稿（原 handle_ai_fragment END 分支原样迁移）。
    fn ai_finalize(&mut self) -> u8 {
        match self.ai_assembler.end() {
            Ok(json) => {
                let topic = self.ai_request_topic.clone();
                let enq = match topic {
                    Some(t) => self.io.plugin_mut().enqueue_app_publish(&t, &json),
                    None => {
                        eneros_runtime::println!(
                            "[svc-mqtt] AI-REQ-DROP ai_request_topic 未配置（fail-closed）"
                        );
                        Err(eneros_protocol_abstract::ProtocolError::InvalidConfig)
                    }
                };
                match enq {
                    Ok(()) => {
                        self.ai_req_published = self.ai_req_published.saturating_add(1);
                        eneros_runtime::println!(
                            "[svc-mqtt] AI-REQ-ENQUEUE len={} total={}",
                            json.len(),
                            self.ai_req_published
                        );
                        0
                    }
                    Err(_) => {
                        self.ai_errors = self.ai_errors.saturating_add(1);
                        1
                    }
                }
            }
            Err(e) => {
                self.ai_errors = self.ai_errors.saturating_add(1);
                eneros_runtime::println!("[svc-mqtt] AI-FRAG-REJECT end {e:?}");
                1
            }
        }
    }

    /// 遥测面 END 定稿（原 handle_telemetry_fragment END 分支原样迁移）：
    /// 1. 帧头（magic `OBSF` + schema 版本）不匹配 → 拒绝解析 + 计数；
    /// 2. telemetry_topic 未配置 → 拒绝（遥测出口关闭）；
    /// 3. 插件未连接（broker 断）→ 拒绝 → 调用方 durable outbox 兜底补传。
    fn tel_finalize(&mut self) -> u8 {
        match self.tel_assembler.end() {
            Ok(frame) => {
                // 1. 帧头校验（schema 版本不匹配 → 拒绝解析）。
                if let Err(e) = eneros_agent_sdk::telemetry_relay::check_frame_header(&frame) {
                    self.tel_rejected = self.tel_rejected.saturating_add(1);
                    eneros_runtime::println!(
                        "[svc-mqtt] TEL-FRAME-REJECT header={e:?} len={}（fail-closed）",
                        frame.len()
                    );
                    return 1;
                }
                // 2. 插件已连接（broker 在线——断连时拒绝 → durable 兜底）。
                if !self.io.plugin_mut().is_connected() {
                    self.tel_rejected = self.tel_rejected.saturating_add(1);
                    eneros_runtime::println!(
                        "[svc-mqtt] TEL-FRAME-REJECT broker 断连（fail-closed → 补传）"
                    );
                    return 1;
                }
                // 3. 入插件 app outbox（QoS1 发布；未配置主题插件拒绝）。
                match self.io.plugin_mut().enqueue_telemetry_publish(&frame) {
                    Ok(()) => {
                        self.tel_published = self.tel_published.saturating_add(1);
                        eneros_runtime::println!(
                            "[svc-mqtt] TEL-FRAME-ACCEPT len={} total={}",
                            frame.len(),
                            self.tel_published
                        );
                        0
                    }
                    Err(_) => {
                        self.tel_rejected = self.tel_rejected.saturating_add(1);
                        eneros_runtime::println!(
                            "[svc-mqtt] TEL-FRAME-REJECT 未配置主题/outbox 满（fail-closed）"
                        );
                        1
                    }
                }
            }
            Err(e) => {
                self.tel_rejected = self.tel_rejected.saturating_add(1);
                eneros_runtime::println!("[svc-mqtt] TEL-FRAG-REJECT end {e:?}");
                1
            }
        }
    }
}

#[cfg(target_os = "none")]
impl eneros_drv_svc::runtime::DriverService for MqttWriteIoService {
    fn capability(&self) -> Option<&'static str> {
        self.io.capability()
    }

    fn handle(
        &mut self,
        req: &eneros_drv_svc::runtime::DriverRequest,
        resp: &mut eneros_drv_svc::runtime::DriverResponse,
    ) {
        use eneros_drv_svc::runtime::*;
        if req.opcode == OP_EXECUTE {
            let p = &req.payload[..req.payload_len as usize];
            // 先 io.handle（本轮 poll 驱动插件 connect 状态刷新），再处理
            // 分片——telemetry 的 broker 断连拒绝依赖新鲜 connected。
            self.io.handle(req, resp);
            // 蓝图 59：两面分片统一处理（原双 handler 收敛）。
            if let Some(status) = self.handle_relay_fragment(p) {
                resp.status = ST_OK;
                resp.payload[0] = status;
                resp.payload_len = 1;
                return;
            }
            // 50 二分诊断：未识别 EXECUTE 载荷首字节打印（定位后移除）。
            if req.payload_len > 0 {
                eneros_runtime::println!(
                    "[svc-mqtt] DBG-EXEC p0={:#04x} len={}",
                    p[0],
                    req.payload_len
                );
            }
            // 47（WP-47.1）：本轮 poll 解析到的下行写意图 → dispatch 授权链。
            self.forward_intents();
            // 49（WP-49.2）：本轮收到的 peer 应答 → ai-worker 推送。
            self.forward_ai_responses();
            // 50（WP-50.2）：本轮收到的管理请求 → svc-mgmt（应答回发布）。
            self.forward_mgmt_requests();
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
    // smoltcp 传输用 eneros_time 单调时钟推进；EL0 服务进程内初始化。
    eneros_time::time_init(eneros_hal::arm64::timer::clock(), 0);
    // 39：协议插件在服务侧构造（descriptor 随 readiness 报告）。
    let plugin = eneros_mqtt::plugin::MqttPlugin::new();
    eneros_runtime::println!(
        "[protocol-service] mqtt plugin={} role={:?} interop={:?}",
        plugin.descriptor().id,
        plugin.descriptor().role,
        plugin.descriptor().interop_level
    );
    // 47（WP-47.5）：policy/实现一致机器校验——注册表（本装配无目标侧
    // TLS 工厂；嵌定 first 实现 host 已验证）。qemu-test 装配允许显式明文
    //（InsecureStream，永不冒充）；ReleaseTls 下声明 SecureProfile 而无
    // 工厂 → assembly_allows=false → 能力壳（fail-closed）。
    {
        let registry = eneros_protocol_abstract::channel::SecureChannelRegistry::new();
        let mode = eneros_protocol_abstract::channel::AssemblyMode::QemuTest;
        let profile = plugin.descriptor().security_profile;
        let has = registry.has(plugin.descriptor().id);
        let policy = eneros_protocol_abstract::channel::ChannelPolicy::default_dev();
        let ok = eneros_protocol_abstract::channel::assembly_allows(mode, profile, has, &policy);
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
            eneros_drv_svc::runtime::driver_service_main(
                ep,
                "mqtt",
                ready_ntfn,
                eneros_protocol_services::now_ms,
                eneros_protocol_services::instance_epoch(),
                &mut eneros_protocol_services::service::UnassignedProtocolService,
            );
        }
    }
    // 46.2/46.3 + 47：手柄移交 → 配置装配插件 → 意图转发服务。
    let mut service: Box<dyn eneros_drv_svc::runtime::DriverService> =
        match eneros_protocol_services::net_transport::build_net_transport(
            mmio_vaddr, dma_vaddr, dma_phys,
        ) {
            Ok(stack) => {
                let mut factory =
                    eneros_protocol_services::net_transport::NetTransportFactory::new(stack);
                let mut plugin = eneros_mqtt::plugin::MqttPlugin::new();
                // 46.3：配置即 peer/app_id 唯一来源（protocol-mqtt）；不可得
                // → 能力壳（fail-closed，不猜默认）。
                match eneros_agent_sdk::eipc_resolve("svc-mqtt", "svc-config").and_then(
                    |(_, cfg_slot)| {
                        eneros_protocol_services::service_cfg::fetch_protocol_config(
                            "mqtt-config",
                            cfg_slot,
                            "protocol-mqtt",
                        )
                    },
                ) {
                    Some(cfg) => match plugin.start(&cfg, &mut factory, &mut EmptyPointRegistry) {
                        Ok(()) => {
                            eneros_runtime::println!(
                                "[svc-mqtt] NET-TRANSPORT READY capability=eneros_mqtt::plugin::MqttPlugin"
                            );
                            // 47：dispatch 端点（WP-54.4 按名解析；未授予
                            // → 写意图路径 fail-closed）。
                            let dispatch =
                                eneros_agent_sdk::eipc_resolve("svc-mqtt", "svc-control-dispatch")
                                    .and_then(|(_, slot)| {
                                        eneros_agent_sdk::ipc_sel4::Sel4IpcClient::new(
                                            "mqtt-dispatch",
                                            slot,
                                            eneros_drv_svc::now_ns,
                                        )
                                        .ok()
                                    });
                            if dispatch.is_none() {
                                eneros_runtime::println!(
                                    "[svc-mqtt] dispatch EIPC NOT granted —— 写意图路径 fail-closed"
                                );
                            }
                            // 49（WP-49.2）：ai-worker 端点在槽 base+2（表序
                            // 第 3 条授予）；AI 请求主题取自配置（插件已消费）。
                            let ai_worker =
                                eneros_agent_sdk::eipc_resolve("svc-mqtt", "svc-ai-worker")
                                    .and_then(|(_, slot)| {
                                        eneros_agent_sdk::ipc_sel4::Sel4IpcClient::new(
                                            "mqtt-ai-worker",
                                            slot,
                                            eneros_drv_svc::now_ns,
                                        )
                                        .ok()
                                    });
                            let ai_request_topic =
                                plugin.ai_request_topic().map(alloc::string::String::from);
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
                            let mut io = MqttWriteIoService {
                                io: eneros_protocol_services::service::PluginIoService::new(
                                    plugin,
                                    "eneros_mqtt::plugin::MqttPlugin",
                                ),
                                dispatch,
                                dispatch_epoch: 0,
                                forwarded_total: 0,
                                rejected_total: 0,
                                ai_assembler: eneros_agent_sdk::relay_face::RelayAssembler::new(),
                                ai_worker,
                                ai_epoch: 0,
                                ai_request_topic,
                                ai_req_published: 0,
                                ai_resp_forwarded: 0,
                                ai_errors: 0,
                                tel_assembler: eneros_agent_sdk::relay_face::RelayAssembler::new(),
                                tel_published: 0,
                                tel_rejected: 0,
                                // 50（WP-50.2）：svc-mgmt 端点（按名解析；
                                // 未授予 → 管理请求丢弃）。
                                mgmt: eneros_agent_sdk::eipc_resolve("svc-mqtt", "svc-mgmt")
                                    .and_then(|(_, slot)| {
                                        eneros_agent_sdk::ipc_sel4::Sel4IpcClient::new(
                                            "mqtt-mgmt",
                                            slot,
                                            eneros_drv_svc::now_ns,
                                        )
                                        .ok()
                                    }),
                                mgmt_epoch: 0,
                                mgmt_forwarded: 0,
                                mgmt_errors: 0,
                            };
                            // 46.5：启动期自检轮询（post-connect 交换尽早完成）。
                            io.io.self_check_poll(150);
                            io.forward_intents();
                            Box::new(io)
                        }
                        Err(e) => {
                            eneros_runtime::println!(
                                "[svc-mqtt] plugin start FAILED {:?} —— capability 壳（EXECUTE ST_UNAVAILABLE）",
                                e
                            );
                            Box::new(eneros_protocol_services::service::UnassignedProtocolService)
                        }
                    },
                    None => {
                        eneros_runtime::println!(
                            "[svc-mqtt] protocol config unavailable —— capability 壳（EXECUTE ST_UNAVAILABLE）"
                        );
                        Box::new(eneros_protocol_services::service::UnassignedProtocolService)
                    }
                }
            }
            Err(e) => {
                eneros_runtime::println!(
                    "[svc-mqtt] handover missing ({:?}) —— capability 壳（EXECUTE ST_UNAVAILABLE）",
                    e
                );
                Box::new(eneros_protocol_services::service::UnassignedProtocolService)
            }
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

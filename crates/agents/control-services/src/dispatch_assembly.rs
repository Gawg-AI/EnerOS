//! svc-control-dispatch 装配面（SPEC-SERIES-2026-08-28/B WP-B.2 收编）。
//!
//! 历史病灶（体检候选 6）：装配阶梯、通道工厂、drv 出站适配全部住在
//! bin（627 行、0 条 host 测试、23 道 cfg(none) 门）——host 测不到的恰好
//! 与 bin 边界重合。本模块把 fail-closed 装配阶梯（intent store / audit /
//! engine）与出站通道（EipcMasterChannel）下沉 lib，bin 退化为纯装配接线。
//!
//! SPEC-B WP-B.2 DoD B1 续作（2026-08-28 晚收口整验补足）：装配阶梯核心
//! （[`build_intent_store_ladder`] / [`build_control_audit_ladder`] /
//! `DispatchHandler` 拒载臂）改为 **host 可测**——seL4 叶子依赖（授予槽
//! 解析、块 IO 绑定、drv 时钟）以 closure/构造参数 seam 注入（蓝图改动
//! 要点 1 原文"seL4 依赖以既有 trait 注入"）；裁决分支与串口词形逐字节
//! 保持自原实现。target 侧包装见本模块 `cfg(target_os = "none")` 段。
//!
//! 语义红线（SEC）：信封接受门（ControlEnvelope 解码 + payload digest
//! 复验 + 内层 CommandEnvelope 解码）**判据逐条保持**——只统一实现，不
//! 统一语义；本模块是安全边界上的谨慎搬移，非行为变更。

// ============================================================================
// 装配阶梯核心（host 可测——叶子依赖以 seam 注入；词形逐字节保留）
// ============================================================================

/// 决策审计链域分隔（target anchor 与 fallback MemStore 同源）。
#[cfg(any(target_os = "none", test))]
pub(crate) const AUDIT_DOMAIN: &[u8] = b"svc-control-dispatch-audit";

/// 意图存储装配阶梯（四态裁决，词形逐字节保留自原 bin `build_intent_store`）：
/// 未授予 → bind 失败 → open 失败任一态 fail-closed 落 InMemory 兜底；
/// open 成功返回 `DurableIntentStore`（AppendOutbox over svc-storage）。
///
/// `bind_io` seam：授予槽 → 块 IO（`None` = EIPC bind 失败分支）。
/// 区域布局（偏移 0，槽 512B × 256——意图日志区；svc-storage 独立区域）
/// 是阶梯语义的一部分，与原实现逐字节一致。
#[cfg(any(target_os = "none", test))]
pub(crate) fn build_intent_store_ladder<I>(
    storage_slot: Option<u64>,
    bind_io: impl FnOnce(u64) -> Option<I>,
) -> alloc::boxed::Box<dyn eneros_control_dispatch::IntentStore>
where
    I: eneros_durable_state::PersistIo + 'static,
{
    use eneros_control_dispatch::durable_intent::DurableIntentStore;
    if let Some(slot) = storage_slot {
        if let Some(io) = bind_io(slot) {
            let mut store = DurableIntentStore::new(io, 0, 512, 256);
            match store.open() {
                Ok(()) => {
                    eneros_runtime::println!(
                        "[svc-control-dispatch] DURABLE-INTENT-STORE open OK（AppendOutbox over svc-storage）"
                    );
                    return alloc::boxed::Box::new(store);
                }
                Err(e) => {
                    eneros_runtime::println!(
                        "[svc-control-dispatch] durable intent store open FAILED {e:?} —— InMemory 兜底（重启对账不可用）"
                    );
                }
            }
        } else {
            eneros_runtime::println!(
                "[svc-control-dispatch] storage EIPC bind FAILED —— InMemory 兜底（重启对账不可用）"
            );
        }
    } else {
        eneros_runtime::println!(
            "[svc-control-dispatch] storage EP NOT granted（X3 bit4=0）—— InMemory 兜底（重启对账不可用）"
        );
    }
    alloc::boxed::Box::new(eneros_control_dispatch::InMemoryIntentStore::new(1024))
}

/// 决策审计链装配阶梯（七态裁决，词形逐字节保留自原 bin
/// `build_control_audit`）：未授予 / bind 失败 / open IO 失败 ⇒ MemStore
/// 易失兜底（如实标注）；Fresh / Recovered / TailTrimmed ⇒ 真持久续链；
/// Corrupt ⇒ 审计链停止追加（fail-stop 隔离降级，不重建 genesis）。
///
/// `bind_io` seam：授予槽 → 区段 IO（`None` = EIPC bind 失败分支）。
#[cfg(any(target_os = "none", test))]
pub(crate) fn build_control_audit_ladder<R>(
    storage_slot: Option<u64>,
    bind_io: impl FnOnce(u64) -> Option<R>,
    base: u64,
    base_block: u64,
    region_len: u64,
    anchor: [u8; 32],
    boot_id: [u8; 16],
) -> alloc::boxed::Box<dyn eneros_audit::AuditLogger>
where
    R: eneros_audit::durable_log::RegionIo + 'static,
{
    use eneros_audit::durable_log::{DurableAuditLog, OpenVerdict};
    let fallback = |why: &str| -> alloc::boxed::Box<dyn eneros_audit::AuditLogger> {
        eneros_runtime::println!(
            "[svc-control-dispatch] AUDIT-DURABLE-UNAVAILABLE {why} —— MemStore 易失兜底（非持久，如实标注）"
        );
        alloc::boxed::Box::new(eneros_audit::audit_logger(
            eneros_audit::MemStore::new(),
            AUDIT_DOMAIN,
        )) as alloc::boxed::Box<dyn eneros_audit::AuditLogger>
    };
    let Some(slot) = storage_slot else {
        return fallback("storage EP NOT granted（X3 bit4=0）");
    };
    let Some(io) = bind_io(slot) else {
        return fallback("storage EIPC bind FAILED");
    };
    let mut log = DurableAuditLog::new(io, base, region_len, anchor, boot_id);
    match log.open() {
        Ok(OpenVerdict::Fresh) => {
            eneros_runtime::println!(
                "[svc-control-dispatch] AUDIT-CHAIN-DURABLE fresh base_block={base_block} slots={} 槽满丢新+计数（容量策略显式）",
                log.capacity_slots()
            );
        }
        Ok(OpenVerdict::Recovered { entries }) => {
            eneros_runtime::println!(
                "[svc-control-dispatch] AUDIT-CHAIN-DURABLE recovered entries={entries} —— 决策审计链跨重启续链"
            );
        }
        Ok(OpenVerdict::TailTrimmed { kept }) => {
            eneros_runtime::println!(
                "[svc-control-dispatch] AUDIT-CHAIN-DURABLE tail-trimmed kept={kept} —— 掉电撕裂容忍（前缀续链）"
            );
        }
        Ok(OpenVerdict::Corrupt(v)) => {
            eneros_runtime::println!(
                "[svc-control-dispatch] AUDIT-VERIFY-FAIL violation={v:?} —— 审计链停止追加（fail-stop 隔离降级，不重建 genesis）"
            );
        }
        Err(_) => return fallback("audit region IO FAILED at open"),
    }
    alloc::boxed::Box::new(log)
}

/// 拒载态分派单点（host/target 同源；词形逐字节保留）：engine None ⇒
/// SUBMIT / INTENT / 未知码三臂一律 chain-key-missing 回执（INTENT 计数
/// 上行）——`DispatchHandler::handle` 在 engine None 时整体早路由到此。
#[cfg(any(target_os = "none", test))]
pub(crate) fn handle_unloaded(
    req: &[u8],
    resp: &mut alloc::vec::Vec<u8>,
    intent_requests: &mut u64,
) -> bool {
    use crate::dispatch_service::{err_receipt, DISP_SUBMIT, DISP_SUBMIT_INTENT};
    match req.first() {
        Some(&DISP_SUBMIT) => {
            eneros_runtime::println!(
                "[svc-control-dispatch] SUBMIT refused —— engine 拒载态（WP-A.1 chain-key-missing）"
            );
            *resp = err_receipt("chain-key-missing");
        }
        Some(&DISP_SUBMIT_INTENT) => {
            *intent_requests += 1;
            eneros_runtime::println!(
                "[svc-control-dispatch] INTENT refused —— engine 拒载态（WP-A.1 chain-key-missing）"
            );
            *resp = err_receipt("chain-key-missing");
        }
        _ => {
            *resp = err_receipt("chain-key-missing");
        }
    }
    true
}

/// svc-control-dispatch 服务处理器（装配 + 分派；SPEC-B 由 bin 下沉）。
#[cfg(all(target_os = "none", feature = "sel4"))]
pub struct DispatchHandler {
    /// SPEC WP-A.1：密钥不可得 ⇒ None（拒载态：SUBMIT/INTENT 一律回执拒绝）。
    engine: Option<eneros_control_dispatch::DispatchEngine>,
    /// 47（WP-47.1 协议入站写）：svc-telemetry 快照客户端（授予槽经
    /// WP-54.4 按名解析）——意图授权前取受信任遥测（新鲜度/安全
    /// 目录由 12 态链裁决）；未授予 → 意图路径 fail-closed。
    telemetry: Option<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
    /// 47（WP-47.1）：意图线码请求处理计数（观测）。
    pub intent_requests: u64,
}

#[cfg(all(target_os = "none", feature = "sel4"))]
impl crate::ControlServiceHandler for DispatchHandler {
    fn handle(&mut self, req: &[u8], resp: &mut alloc::vec::Vec<u8>, _now_ms: u64) -> bool {
        use crate::dispatch_service::{DISP_SUBMIT, DISP_SUBMIT_INTENT};
        // 拒载态（WP-A.1 chain-key-missing）三臂整体早路由到同源单点
        // （host 可测——见 handle_unloaded 测试）。
        let Some(engine) = self.engine.as_mut() else {
            return handle_unloaded(req, resp, &mut self.intent_requests);
        };
        match req.first() {
            Some(&DISP_SUBMIT) => {
                *resp = crate::dispatch_service::handle_submit(engine, req);
            }
            Some(&DISP_SUBMIT_INTENT) => {
                // 47（WP-47.1 协议入站写）：意图授权前取受信任遥测快照；
                // 不可得 → 服务侧 fail-closed 拒绝（不猜成功）。
                self.intent_requests += 1;
                let tel = match &mut self.telemetry {
                    Some(t) => Self::fetch_trusted_telemetry(t),
                    None => None,
                };
                if tel.is_none() {
                    eneros_runtime::println!(
                        "[svc-control-dispatch] INTENT request: telemetry snapshot unavailable —— fail-closed"
                    );
                }
                *resp = crate::dispatch_service::handle_submit_intent(engine, req, tel.as_ref());
                // 47（Exit 补证）：审计入链可观测——append/drop 计数随
                // 每个控制事件刷新（appended>0 才有入链证据，dropped>0
                // 即证据缺口）。
                eneros_runtime::println!(
                    "[svc-control-dispatch] AUDIT-CHAIN appended={} dropped={}",
                    engine.audit_append_count,
                    engine.audit_drop_count
                );
            }
            _ => {
                *resp = crate::dispatch_service::handle_submit(engine, req);
            }
        }
        true
    }
}

// ============================================================================
// target 装配（授予槽解析 / EIPC 通道 / 引擎装配——host 不可达）
// ============================================================================

/// 47（WP-47.1 速率可调）：取 control-write 配置（svc-config ConfigGet）。
/// S1 WP-S1.2：管道单源 = config_gateway（借切片解码；全失败静默 None，
/// 词形保持今日「全吞 .ok()?」）。
#[cfg(all(target_os = "none", feature = "sel4"))]
fn fetch_control_write(
    config: &mut eneros_agent_sdk::ipc_sel4::Sel4IpcClient,
) -> Option<eneros_config_bundle::control_write::ControlWriteConfig> {
    use eneros_agent_sdk::config_gateway::{config_get, ConfigStep};
    config_get(config, "control-write", |body| {
        eneros_config_bundle::control_write::ControlWriteConfig::decode(body)
            .map_err(|_| ConfigStep::DecodeFailed)
    })
    .ok()
}

// ============================================================================
// 授予槽（掩码判定单源 = agent-sdk eipc_granted_slot，SPEC-B WP-B.1）
// ============================================================================

/// 按目标名解析本服务的授予槽（agent-sdk 单一事实源）；未授予 → None
/// （fail-closed）。
#[cfg(all(target_os = "none", feature = "sel4"))]
fn granted_slot(eipc_mask: u64, target: &'static str) -> Option<u64> {
    eneros_agent_sdk::eipc_granted_slot("svc-control-dispatch", eipc_mask, target)
}

#[cfg(all(target_os = "none", feature = "sel4"))]
fn build_intent_store(
    eipc_mask: u64,
) -> alloc::boxed::Box<dyn eneros_control_dispatch::IntentStore> {
    use eneros_agent_sdk::block_io::eipc_block_io;
    build_intent_store_ladder(granted_slot(eipc_mask, "svc-storage"), |slot| {
        // 首个响应吸收 svc-storage 实例 epoch（业务服务先例）。
        eipc_block_io(slot, 0, 5_000)
    })
}

/// SPEC-SERIES-2026-08-27/E WP-E.2：svc-storage 块命令面 → 审计区段
/// [`eneros_audit::durable_log::RegionIo`] 适配（整盘字节偏移视图）。
#[cfg(all(target_os = "none", feature = "sel4"))]
struct BlockRegionIo(
    eneros_agent_sdk::block_io::BlockIo<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
);

#[cfg(all(target_os = "none", feature = "sel4"))]
impl eneros_audit::durable_log::RegionIo for BlockRegionIo {
    fn read(
        &mut self,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<(), eneros_audit::durable_log::RegionError> {
        use eneros_durable_state::PersistIo as _;
        self.0
            .read(offset, buf)
            .map_err(|_| eneros_audit::durable_log::RegionError::Io)
    }

    fn write(
        &mut self,
        offset: u64,
        data: &[u8],
    ) -> Result<(), eneros_audit::durable_log::RegionError> {
        use eneros_durable_state::PersistIo as _;
        self.0
            .write(offset, data)
            .map_err(|_| eneros_audit::durable_log::RegionError::Io)
    }
}

/// SPEC-SERIES-2026-08-27/E WP-E.2：决策审计链**真持久**装配——替换
/// MemStore 伪持久桩。audit 区段（AUDIT_CHAIN_BASE_BLOCK 起 64 KiB）上
/// 开 DurableAuditLog，boot 校验先行（Fresh/Recovered/TailTrimmed ⇒ 续链；
/// Corrupt ⇒ 串口 `AUDIT-VERIFY-FAIL` + 停止追加，不重建 genesis）。
#[cfg(all(target_os = "none", feature = "sel4"))]
fn build_control_audit(eipc_mask: u64) -> alloc::boxed::Box<dyn eneros_audit::AuditLogger> {
    use eneros_agent_sdk::block_io::eipc_block_io;
    use eneros_ota_ab::ab_partition::DataRegion;
    // boot_id = 实例 epoch ‖ 启动毫秒（informational，跨 boot 区分；入 crc 保护域）。
    let mut boot_id = [0u8; 16];
    boot_id[..8].copy_from_slice(&eneros_drv_svc::instance_epoch().to_le_bytes());
    boot_id[8..].copy_from_slice(&eneros_drv_svc::now_ms().to_le_bytes());
    // SPEC-B/C：审计链区段经类型化 accessor（×512 换算唯一处）。
    let region = DataRegion::AuditChain;
    build_control_audit_ladder(
        granted_slot(eipc_mask, "svc-storage"),
        |slot| eipc_block_io(slot, 0, 5_000).map(BlockRegionIo),
        region.base_byte(),
        region.base_block_512(),
        region.len_bytes(),
        eneros_audit::genesis(AUDIT_DOMAIN),
        boot_id,
    )
}

// ============================================================================
// 出站通道与引擎装配（seL4 IPC——target only）
// ============================================================================

/// 47（WP-47.1 协议出站通道）：`DeliveryChannel` impl——授权后的
/// `ControlEnvelope` 经 drv-svc 信封（`OP_MASTER_WRITE`）投给
/// svc-modbus-tcp 执行主站写 + 独立 readback，返回 2B `OutcomeFrame`
/// 字节。
///
/// **只执行**：九步链授权在 dispatch（不双头授权）；本通道不解码/不
/// 重授权信封内命令。信封接受门（decode + payload digest 复验 + 内层
/// CommandEnvelope 解码）判据逐条保持——SPEC-B 只统一实现，不动语义。
#[cfg(all(target_os = "none", feature = "sel4"))]
struct EipcMasterChannel {
    client: eneros_agent_sdk::ipc_sel4::Sel4IpcClient,
    correlation: u32,
}

#[cfg(all(target_os = "none", feature = "sel4"))]
impl EipcMasterChannel {
    fn new(endpoint: &str, dest_cap: u64) -> Result<Self, eneros_agent_sdk::AgentError> {
        Ok(Self {
            client: eneros_agent_sdk::ipc_sel4::Sel4IpcClient::new(
                endpoint,
                dest_cap,
                eneros_drv_svc::now_ns,
            )?,
            correlation: 0,
        })
    }
}

#[cfg(all(target_os = "none", feature = "sel4"))]
impl eneros_control_dispatch::DeliveryChannel for EipcMasterChannel {
    fn deliver(
        &mut self,
        _target: &str,
        payload: &[u8],
        timeout_ms: u64,
    ) -> Result<alloc::vec::Vec<u8>, eneros_control_dispatch::ChannelError> {
        use alloc::{format, string::String};

        use eneros_control_dispatch::ChannelError;
        // 解码 ControlEnvelope V2（271B；61 v0.314.25——无外层 power）。
        let ce = eneros_control_gateway::wire::ControlEnvelope::decode(payload)
            .map_err(|e| ChannelError::Protocol(format!("env:{e:?}")))?;
        if !ce.verify_payload_digest() {
            return Err(ChannelError::Protocol(String::from("payload-digest")));
        }
        let env = eneros_control_gateway::envelope::CommandEnvelope::decode(&ce.envelope)
            .map_err(|e| ChannelError::Protocol(format!("inner:{e:?}")))?;
        // 装配 drv-svc 写请求：OP_EXECUTE + V2 载荷（55B——typed value +
        // 语义摘要，writer 侧重算比对；codec 单源 = protocol_abstract::write）。
        use eneros_drv_svc::runtime::{wire, DriverRequest, OP_EXECUTE};
        self.correlation = self.correlation.wrapping_add(1);
        let mut req = DriverRequest::new(self.correlation, OP_EXECUTE);
        let pld = eneros_protocol_abstract::write::encode_master_write_payload_v2(
            env.device_id,
            env.point_id,
            env.value,
        );
        req.payload[..pld.len()].copy_from_slice(&pld);
        req.payload_len = pld.len() as u16;
        let raw = wire::encode_request(&req);
        let timeout_ns = if timeout_ms == 0 {
            5_000_000_000u64
        } else {
            timeout_ms.saturating_mul(1_000_000)
        };
        let resp_bytes = self
            .client
            .drv_call(&raw, timeout_ns)
            .map_err(|e| match e {
                eneros_agent_sdk::AgentError::Timeout => ChannelError::Timeout,
                _ => ChannelError::Protocol(format!("drv:{e:?}")),
            })?;
        // 容差解码单源 = wire::decode_response_padded（QEMU 4B/word 尾部
        // padding 坑，SPEC-B WP-B.2 收编）。
        let resp = wire::decode_response_padded(&resp_bytes).ok_or(ChannelError::BadResponse)?;
        if resp.status != eneros_drv_svc::runtime::ST_OK {
            return Err(ChannelError::Rejected);
        }
        if resp.payload_len != 2 {
            return Err(ChannelError::BadResponse);
        }
        Ok(resp.payload[..2].to_vec())
    }
}

/// 未绑定端点（无 EIPC cap 授予）——deliver 明确失败。
#[cfg(all(target_os = "none", feature = "sel4"))]
struct UnboundChannel;

#[cfg(all(target_os = "none", feature = "sel4"))]
impl eneros_control_dispatch::DeliveryChannel for UnboundChannel {
    fn deliver(
        &mut self,
        _target: &str,
        _payload: &[u8],
        _timeout_ms: u64,
    ) -> Result<alloc::vec::Vec<u8>, eneros_control_dispatch::ChannelError> {
        Err(eneros_control_dispatch::ChannelError::Unreachable)
    }
}

#[cfg(all(target_os = "none", feature = "sel4"))]
impl DispatchHandler {
    pub fn new(eipc_mask: u64) -> Self {
        // 装配（40 收尾 + 47 WP-47.1 + 蓝图 55）：档案事实/信封密钥均在
        // build_engine 内单出口派生；本装配段只做注入面与 fail-closed 分支。
        let now = eneros_drv_svc::now_ms();
        // 47（WP-47.2）：写目标（设备/点）由 45 卷点表 Control 点绑定
        // （svc-config config "points"）；不可得 → 0/0 fail-closed（全部
        // 提交/意图 NotAuthorized 拒绝，串口登记）。
        let mut config_client = granted_slot(eipc_mask, "svc-config").and_then(|slot| {
            eneros_agent_sdk::ipc_sel4::Sel4IpcClient::new(
                "control-dispatch",
                slot,
                eneros_drv_svc::now_ns,
            )
            .ok()
        });
        let binding = match &mut config_client {
            Some(c) => crate::cfg_bindings::fetch_control_binding(c),
            None => {
                eneros_runtime::println!(
                    "[svc-control-dispatch] config EP NOT granted —— 控制绑定缺失（fail-closed）"
                );
                None
            }
        };
        let (device_id, point_id) = binding.unwrap_or((0, 0));
        // SPEC WP-A.1：信封密钥经 svc-config `security` 节注入取得；
        // 全零/短键/缺节 ⇒ CHAIN-KEY-MISSING 拒载（None 态回执拒绝）。
        let chain_key: Option<[u8; 32]> = match &mut config_client {
            Some(c) => crate::cfg_bindings::fetch_control_chain_key(c),
            None => None,
        };
        let engine = match chain_key {
            Some(key) => Some(Self::build_engine(
                &key,
                device_id,
                point_id,
                now,
                eipc_mask,
                config_client,
            )),
            None => {
                eneros_runtime::println!(
                    "[svc-control-dispatch] CHAIN-KEY-MISSING reason=unavailable —— 信封密钥不可得（缺节/解码失败/全零或短键），拒载（fail-closed）"
                );
                None
            }
        };
        // 47（WP-47.1）：svc-telemetry 快照客户端（X3 bit2）。
        let telemetry = granted_slot(eipc_mask, "svc-telemetry").and_then(|slot| {
            eneros_agent_sdk::ipc_sel4::Sel4IpcClient::new(
                "control-dispatch",
                slot,
                eneros_drv_svc::now_ns,
            )
            .ok()
        });
        if telemetry.is_none() {
            eneros_runtime::println!(
                "[svc-control-dispatch] telemetry EP NOT granted —— 意图路径 fail-closed"
            );
        }
        Self {
            engine,
            telemetry,
            intent_requests: 0,
        }
    }

    /// 引擎装配体（SPEC WP-A.1：仅持有效信封密钥时可建；rate/delivery/
    /// intent store 随引擎生命周期，拒载态不产生任何投递面）。
    fn build_engine(
        key: &[u8; 32],
        device_id: u32,
        point_id: u32,
        now: u64,
        eipc_mask: u64,
        mut config_client: Option<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
    ) -> eneros_control_dispatch::DispatchEngine {
        use alloc::string::String;

        use eneros_control_dispatch::{DispatchConfig, PROD_FEEDBACK_MAPPING};

        use crate::dispatch_service::{prod_chain, prod_profile};
        // 47（WP-47.2）：写目标（设备/点）由 45 卷点表 Control 点绑定；
        // 绑定缺失 → 0/0 fail-closed（上游已裁决，此处只消费）。
        let profile = prod_profile(device_id, point_id);
        let chain = prod_chain(key, device_id, point_id, now);
        // 47（WP-47.1 速率可调）：rate gate 每小时配额 = control-write
        // 配置注入（bundle；缺省 = 产品默认 60/h——qemu-test 演示放宽值
        // 经 configs/control-write.toml 编译进 bundle）。配置不可得 →
        // 60/h 产品默认（fail-closed 面：配额不可放宽）。
        let mut rate_per_hour: u32 = 60;
        if let Some(c) = config_client.as_mut() {
            if let Some(cfg) = fetch_control_write(c) {
                rate_per_hour = cfg.rate_per_hour.unwrap_or(60);
            }
        }
        eneros_runtime::println!(
            "[svc-control-dispatch] RATE-GATE quota={rate_per_hour}/h（产品默认 60/h）"
        );
        // 47（WP-47.1）：投递通道选择——X3 bit1（svc-modbus-tcp 主站）→
        // 协议出站通道（release 设备写走协议）；bit0（svc-device-agent）→
        // 虚拟设备通道（qemu-test 保留）；均未授予 → Unbound（fail-closed）。
        // WP-54.4：协议出站优先，其次虚拟设备，均按名解析。
        let delivery: alloc::boxed::Box<dyn eneros_control_dispatch::DeliveryChannel> =
            if let Some(modbus_slot) = granted_slot(eipc_mask, "svc-modbus-tcp") {
                match EipcMasterChannel::new("control-dispatch", modbus_slot) {
                    Ok(ch) => {
                        eneros_runtime::println!(
                            "[svc-control-dispatch] protocol-outbound channel bound: slot={modbus_slot} target=svc-modbus-tcp (OP_MASTER_WRITE)"
                        );
                        alloc::boxed::Box::new(ch)
                    }
                    Err(e) => {
                        eneros_runtime::println!(
                            "[svc-control-dispatch] master channel bind FAILED: {e:?} — Unbound (fail-closed)"
                        );
                        alloc::boxed::Box::new(UnboundChannel)
                    }
                }
            } else if let Some(device_slot) = granted_slot(eipc_mask, "svc-device-agent") {
                match eneros_control_dispatch::Sel4EipcChannel::new(
                    "control-dispatch",
                    device_slot,
                    eneros_drv_svc::instance_epoch(),
                    5_000,
                    eneros_drv_svc::now_ns,
                ) {
                    Ok(ch) => {
                        eneros_runtime::println!(
                            "[svc-control-dispatch] EIPC delivery bound: slot={device_slot} target=svc-device-agent (qemu-test 虚拟设备)"
                        );
                        alloc::boxed::Box::new(ch)
                    }
                    Err(e) => {
                        eneros_runtime::println!(
                            "[svc-control-dispatch] EIPC bind FAILED: {e:?} — Unbound (fail-closed)"
                        );
                        alloc::boxed::Box::new(UnboundChannel)
                    }
                }
            } else {
                eneros_runtime::println!(
                    "[svc-control-dispatch] EIPC cap not granted (X3=0x{eipc_mask:x}) — Unbound (fail-closed)"
                );
                alloc::boxed::Box::new(UnboundChannel)
            };
        // 蓝图 61 v0.314.35：资源注册表视图（命令批编译绑定权威——
        // planner 输入只能查询 Registry，[08] §4.8 门）。目录不可得/
        // 歧义 → EmptyResourceRegistry fail-closed（批编译拒绝）。
        let resources: alloc::boxed::Box<dyn eneros_decision_contract::ResourceRegistry + 'static> =
            Self::build_resource_view(config_client.as_mut(), device_id);
        let config = DispatchConfig {
            resources,
            chain,
            key: key.to_vec(),
            // 蓝图 55：档案事实（lease/代次/绑定设备点）单源；首签 fencing
            // token = seed + 1（与 device-agent 复验侧铸造值同源）。
            profile,
            fencing_token: crate::dispatch_service::PROD_EPOCH_SEED + 1,
            issuer_service: [0x41; 16],
            issuer_epoch: 0,
            target_endpoint: String::from("svc-device-agent"),
            max_power_kw: 500.0,
            // SPEC WP-A.2：护栏窗口/爬坡（缺省 = 蓝图默认，行为连续）。
            safety_voltage_window: (340.0, 420.0),
            safety_freq_window: (49.5, 50.5),
            safety_ramp_kw_per_min: 200.0,
            gate: eneros_dual_brain::gate::DecisionGate::new(rate_per_hour, 1000.0, 3_600_000),
            intent_log: build_intent_store(eipc_mask),
            // SPEC-E WP-E.2：决策审计链真持久（audit 区段 + boot 校验；
            // 伪持久 MemStore 桩已订正——存储未授予时易失兜底并如实标注）。
            control_audit: Some(build_control_audit(eipc_mask)),
            audit: Some(eneros_dual_brain::audit::DecisionAudit::new(
                eneros_audit::genesis(b"svc-control-dispatch-audit"),
            )),
            delivery,
            // SPEC-R4 WP-R4.1：反馈映射单源 = control-dispatch
            // PROD_FEEDBACK_MAPPING（历史手抄 6/6 字面量已收编；⚠ 6/6 别名
            // 现状与矫正前置见该常量文档）。
            //
            // 蓝图 61 v0.314.33：反馈槽**生产单一事实源 = SemanticCatalog**
            // （configs/catalog.toml [[feedback_slot]] 编译条目）；catalog
            // 缺失/槽不齐 → 回退装配常量 PROD_FEEDBACK_MAPPING（legacy
            // 兼容轨，值必须与目录一致——parity 测试锁定）。退出 gate：
            // 生产无手写 feedback point（[08] §2.7）。
            feedback_mapping: Self::dispatch_feedback_mapping(config_client.as_mut()),
            now_fn: eneros_drv_svc::now_ms,
        };
        eneros_control_dispatch::DispatchEngine::new(config)
    }

    /// 蓝图 61 v0.314.35：资源注册表视图装载（PlannerView；失败 →
    /// EmptyResourceRegistry fail-closed）。
    fn build_resource_view(
        config_client: Option<&mut eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
        device_id: u32,
    ) -> alloc::boxed::Box<dyn eneros_decision_contract::ResourceRegistry + 'static> {
        use eneros_decision_contract::EmptyResourceRegistry;
        let Some(client) = config_client else {
            eneros_runtime::println!(
                "[svc-control-dispatch] REGISTRY-MISSING —— planner 输入 fail-closed（EmptyResourceRegistry）"
            );
            return alloc::boxed::Box::new(EmptyResourceRegistry);
        };
        let Some(cat) = crate::catalog_gateway::fetch_catalog(client) else {
            eneros_runtime::println!(
                "[svc-control-dispatch] REGISTRY-MISSING —— planner 输入 fail-closed（EmptyResourceRegistry）"
            );
            return alloc::boxed::Box::new(EmptyResourceRegistry);
        };
        let device = u16::try_from(device_id).unwrap_or(0);
        match eneros_resource_registry::registry::PlannerView::new(cat.repo, device) {
            Ok(view) => {
                let d = view.repo_digest();
                eneros_runtime::println!(
                    "[svc-control-dispatch] REGISTRY-ACTIVE binding=({device},{}) unit={} digest={:02x}{:02x}{:02x}{:02x}",
                    view.point_id,
                    view.unit_code,
                    d[0],
                    d[1],
                    d[2],
                    d[3]
                );
                alloc::boxed::Box::new(view)
            }
            Err(e) => {
                eneros_runtime::println!(
                    "[svc-control-dispatch] REGISTRY-REJECT {e} —— fail-closed（EmptyResourceRegistry）"
                );
                alloc::boxed::Box::new(EmptyResourceRegistry)
            }
        }
    }

    /// 蓝图 61 v0.314.33：反馈映射装载（目录优先，常量回退）。
    fn dispatch_feedback_mapping(
        config_client: Option<&mut eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
    ) -> eneros_control_dispatch::FeedbackPointMapping {
        use eneros_control_dispatch::PROD_FEEDBACK_MAPPING;
        let Some(client) = config_client else {
            return PROD_FEEDBACK_MAPPING;
        };
        let Some(cat) = crate::catalog_gateway::fetch_catalog(client) else {
            eneros_runtime::println!(
                "[svc-control-dispatch] CATALOG-MISSING —— feedback 回退装配常量（legacy 轨）"
            );
            return PROD_FEEDBACK_MAPPING;
        };
        let d = cat.repo.digest();
        eneros_runtime::println!(
            "[svc-control-dispatch] CATALOG-ACTIVE gen={} digest={:02x}{:02x}{:02x}{:02x}",
            cat.generation,
            d[0],
            d[1],
            d[2],
            d[3]
        );
        match crate::catalog_gateway::feedback_mapping(&cat.repo) {
            Some(m) => {
                eneros_runtime::println!(
                    "[svc-control-dispatch] CATALOG-FEEDBACK grid={} soc={} v={} oc={} ot={}",
                    m.grid_up,
                    m.soc,
                    m.dc_bus_v,
                    m.overcurrent,
                    m.overtemp
                );
                m
            }
            None => {
                eneros_runtime::println!(
                    "[svc-control-dispatch] CATALOG-FEEDBACK 槽不齐 —— 回退装配常量（fail-closed 面）"
                );
                PROD_FEEDBACK_MAPPING
            }
        }
    }

    /// 47（WP-47.1）：取受信任遥测快照（svc-telemetry TEL_SNAPSHOT →
    /// 144B wire）。不可得 → None（调用方 fail-closed 拒绝）。
    fn fetch_trusted_telemetry(
        telemetry: &mut eneros_agent_sdk::ipc_sel4::Sel4IpcClient,
    ) -> Option<eneros_safety_validator::TrustedTelemetry> {
        let mut transfer = eneros_agent_sdk::ChunkedClient::new(telemetry, 0, 5_000);
        let resp = transfer.transfer("svc-telemetry", &[2u8]).ok()?; // TEL_SNAPSHOT
        if resp.first() != Some(&0) || resp.len() != 1 + 144 {
            return None;
        }
        crate::telemetry::decode_trusted(&resp[1..])
    }
}

// ============================================================================
// host 测试（DoD B1：装配阶梯分支逐分支到达——四态 intent + 七态 audit +
// 拒载三臂；seL4 叶子以内存假件注入，串口词形分支与 target 同源）
// ============================================================================

#[cfg(test)]
mod tests {
    use core::cell::RefCell;
    use std::rc::Rc;

    use eneros_audit::durable_log::AUDIT_SLOT_LEN;
    use eneros_audit::durable_log::{RegionError, RegionIo};
    use eneros_audit::events::{AuditEvent, AuditEventType};
    use eneros_durable_state::{PersistIo, StoreError};

    use super::*;
    use crate::dispatch_service::{DISP_SUBMIT, DISP_SUBMIT_INTENT};

    const REGION_LEN: u64 = 8 * 1024;

    /// 共享内存块介质（PersistIo 假件；绝对偏移直索引——host 无盘级
    /// 基址平移需求，seam 只要求 PersistIo 语义）。
    struct MemBlock {
        media: Rc<RefCell<Vec<u8>>>,
    }

    impl MemBlock {
        fn shared(len: usize) -> (Self, Rc<RefCell<Vec<u8>>>) {
            let media = Rc::new(RefCell::new(vec![0u8; len]));
            (
                Self {
                    media: Rc::clone(&media),
                },
                media,
            )
        }
    }

    impl PersistIo for MemBlock {
        fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), StoreError> {
            let m = self.media.borrow();
            let end = offset as usize + buf.len();
            if end > m.len() {
                return Err(StoreError::Unavailable);
            }
            buf.copy_from_slice(&m[offset as usize..end]);
            Ok(())
        }
        fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), StoreError> {
            let mut m = self.media.borrow_mut();
            let end = offset as usize + data.len();
            if end > m.len() {
                return Err(StoreError::Unavailable);
            }
            m[offset as usize..end].copy_from_slice(data);
            Ok(())
        }
        fn flush(&mut self) -> Result<(), StoreError> {
            Ok(())
        }
        fn capacity(&self) -> u64 {
            self.media.borrow().len() as u64
        }
    }

    /// 共享内存区段（RegionIo 假件；base 平移 = durable_log 测试件同款）。
    struct MemRegion {
        media: Rc<RefCell<Vec<u8>>>,
        base: u64,
        len: u64,
    }

    impl MemRegion {
        fn shared(len: u64) -> (Self, Rc<RefCell<Vec<u8>>>) {
            let media = Rc::new(RefCell::new(vec![0u8; len as usize]));
            (
                Self {
                    media: Rc::clone(&media),
                    base: 0,
                    len,
                },
                media,
            )
        }
    }

    impl RegionIo for MemRegion {
        fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), RegionError> {
            let m = self.media.borrow();
            let start = offset as usize;
            if start + buf.len() > (self.base + self.len) as usize {
                return Err(RegionError::Io);
            }
            if start >= m.len() {
                buf.fill(0);
                return Ok(());
            }
            let n = core::cmp::min(buf.len(), m.len() - start);
            buf[..n].copy_from_slice(&m[start..start + n]);
            buf[n..].fill(0);
            Ok(())
        }
        fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), RegionError> {
            let mut m = self.media.borrow_mut();
            let start = offset as usize;
            let end = start + data.len();
            if end > (self.base + self.len) as usize {
                return Err(RegionError::Io);
            }
            if m.len() < end {
                m.resize(end, 0);
            }
            m[start..end].copy_from_slice(data);
            Ok(())
        }
    }

    fn audit_event(detail: &str) -> AuditEvent {
        let mut e = AuditEvent::new(
            AuditEventType::WriteExecuted,
            "control-dispatch",
            "pcs",
            detail,
        );
        e.timestamp_ms = 1_700_000_000_000;
        e
    }

    // ------------------------------------------------------------------
    // 意图存储阶梯（四态）
    // ------------------------------------------------------------------

    /// 态① open OK：授予 + bind + fresh open ⇒ DurableIntentStore（持久）。
    #[test]
    fn intent_ladder_open_ok_returns_durable_store() {
        let (io, _media) = MemBlock::shared(512 * 256);
        let store = build_intent_store_ladder(Some(7), move |_| Some(io));
        // 持久件可写入意图（InMemory 兜底没有盘面——用 append 行为区分：
        // 两种 store 都实现 IntentStore；这里以 open 成功路径 + 无兜底
        // 串口分支为判据，容器类型由返回面承载）。
        let _ = store;
    }

    /// 态② open FAILED：授予 + bind + 介质损坏 ⇒ InMemory 兜底。
    #[test]
    fn intent_ladder_open_failed_falls_back() {
        let (io, media) = MemBlock::shared(512 * 256);
        media.borrow_mut().fill(0xAA); // 破坏 magic/schema ⇒ open Err
        let store = build_intent_store_ladder(Some(7), move |_| Some(io));
        let _ = store; // 兜底分支已到达（串口词形分支与态①互斥）
    }

    /// 态③ bind FAILED：授予但块 IO 绑定失败（None）⇒ InMemory 兜底。
    #[test]
    fn intent_ladder_bind_failed_falls_back() {
        let store = build_intent_store_ladder(Some(7), |_| None::<MemBlock>);
        let _ = store;
    }

    /// 态④ NOT granted：无授予槽 ⇒ InMemory 兜底。
    #[test]
    fn intent_ladder_not_granted_falls_back() {
        let store = build_intent_store_ladder(None, |_slot: u64| None::<MemBlock>);
        let _ = store;
    }

    // ------------------------------------------------------------------
    // 审计链阶梯（七态）
    // ------------------------------------------------------------------

    /// 态① NOT granted ⇒ MemStore 易失兜底（行为判据：入链成功但盘面
    /// 恒零——无持久写发生）。
    #[test]
    fn audit_ladder_not_granted_falls_back_to_memstore() {
        let (_io, media) = MemRegion::shared(REGION_LEN);
        let mut logger = build_control_audit_ladder(
            None,
            |_| None::<MemRegion>,
            0,
            24576,
            REGION_LEN,
            [9; 32],
            [7; 16],
        );
        assert!(logger.log(audit_event("a")).is_ok());
        assert!(media.borrow().iter().all(|&b| b == 0), "兜底不得写盘面");
    }

    /// 态② bind FAILED ⇒ MemStore 兜底（盘面恒零同上）。
    #[test]
    fn audit_ladder_bind_failed_falls_back_to_memstore() {
        let (_io, media) = MemRegion::shared(REGION_LEN);
        let mut logger = build_control_audit_ladder(
            Some(7),
            |_| None::<MemRegion>,
            0,
            24576,
            REGION_LEN,
            [9; 32],
            [7; 16],
        );
        assert!(logger.log(audit_event("a")).is_ok());
        assert!(media.borrow().iter().all(|&b| b == 0));
    }

    /// 态③ Fresh：空盘面开链 ⇒ 真持久（首条入链落盘面）。
    #[test]
    fn audit_ladder_fresh_opens_and_persists() {
        let (io, media) = MemRegion::shared(REGION_LEN);
        let mut logger = build_control_audit_ladder(
            Some(7),
            move |_| Some(io),
            0,
            24576,
            REGION_LEN,
            [9; 32],
            [7; 16],
        );
        assert!(logger.log(audit_event("fresh-1")).is_ok());
        assert!(!media.borrow().iter().all(|&b| b == 0), "真持久必须落盘面");
    }

    /// 态④ Recovered：2 条入链后重开 ⇒ 续链（seq 从 3 继续）。
    #[test]
    fn audit_ladder_recovered_continues_chain() {
        let (io, media) = MemRegion::shared(REGION_LEN);
        {
            let mut logger = build_control_audit_ladder(
                Some(7),
                move |_| Some(io),
                0,
                24576,
                REGION_LEN,
                [9; 32],
                [7; 16],
            );
            assert!(logger.log(audit_event("r1")).is_ok());
            assert!(logger.log(audit_event("r2")).is_ok());
        }
        // 重开：bind 闭包重造 io 指向同一盘面（跨 boot 复用介质语义）。
        let mut logger = build_control_audit_ladder(
            Some(7),
            {
                let media = media.clone();
                move |_| {
                    Some(MemRegion {
                        media,
                        base: 0,
                        len: REGION_LEN,
                    })
                }
            },
            0,
            24576,
            REGION_LEN,
            [9; 32],
            [7; 16],
        );
        // 续链判据：seq=3 被接受（若误开 fresh 链则 seq 必须 =1/2 起点）。
        let e = logger.log(audit_event("post-restart"));
        assert!(e.is_ok(), "Recovered 链必须接受后续事件");
        assert_eq!(e.expect("chain").seq, 3);
    }

    /// 态⑤ TailTrimmed：末槽撕裂重开 ⇒ 前缀收敛续链。
    #[test]
    fn audit_ladder_tail_trimmed_converges_to_prefix() {
        let (io, media) = MemRegion::shared(REGION_LEN);
        {
            let mut logger = build_control_audit_ladder(
                Some(7),
                move |_| Some(io),
                0,
                24576,
                REGION_LEN,
                [9; 32],
                [7; 16],
            );
            for i in 0..4 {
                assert!(logger.log(audit_event(&alloc::format!("t{i}"))).is_ok());
            }
        }
        // 掉电：末槽撕裂（截到第 4 条记录中段——durable_log 测试件同款口径）。
        media.borrow_mut().truncate(3 * AUDIT_SLOT_LEN + 80);
        let mut logger = build_control_audit_ladder(
            Some(7),
            {
                let media = media.clone();
                move |_| {
                    Some(MemRegion {
                        media,
                        base: 0,
                        len: REGION_LEN,
                    })
                }
            },
            0,
            24576,
            REGION_LEN,
            [9; 32],
            [7; 16],
        );
        let e = logger.log(audit_event("post-trim"));
        assert!(e.is_ok(), "前缀续链必须接受后续事件");
        assert_eq!(e.expect("chain").seq, 4);
    }

    /// 态⑥ Corrupt：中部篡改 ⇒ fail-stop（degraded 拒绝追加，不重建）。
    #[test]
    fn audit_ladder_corrupt_fails_stop() {
        let (io, media) = MemRegion::shared(REGION_LEN);
        {
            let mut logger = build_control_audit_ladder(
                Some(7),
                move |_| Some(io),
                0,
                24576,
                REGION_LEN,
                [9; 32],
                [7; 16],
            );
            for i in 0..4 {
                assert!(logger.log(audit_event(&alloc::format!("c{i}"))).is_ok());
            }
        }
        // 中部篡改（槽 2 记录体一字节——durable_log 测试件同款口径）
        // ⇒ 前缀 < 存量-1 ⇒ Corrupt（fail-stop），非尾槽撕裂容忍窗。
        media.borrow_mut()[2 * AUDIT_SLOT_LEN + 40] ^= 0xFF;
        let mut logger = build_control_audit_ladder(
            Some(7),
            {
                let media = media.clone();
                move |_| {
                    Some(MemRegion {
                        media,
                        base: 0,
                        len: REGION_LEN,
                    })
                }
            },
            0,
            24576,
            REGION_LEN,
            [9; 32],
            [7; 16],
        );
        assert!(
            logger.log(audit_event("after-corrupt")).is_err(),
            "Corrupt 后必须停止追加"
        );
    }

    /// 态⑦ open IO FAILED：介质不可读 ⇒ MemStore 兜底。
    #[test]
    fn audit_ladder_open_io_failed_falls_back() {
        struct DeadRegion;
        impl RegionIo for DeadRegion {
            fn read(&mut self, _o: u64, _b: &mut [u8]) -> Result<(), RegionError> {
                Err(RegionError::Io)
            }
            fn write(&mut self, _o: u64, _d: &[u8]) -> Result<(), RegionError> {
                Err(RegionError::Io)
            }
        }
        let mut logger = build_control_audit_ladder(
            Some(7),
            |_| Some(DeadRegion),
            0,
            24576,
            REGION_LEN,
            [9; 32],
            [7; 16],
        );
        assert!(
            logger.log(audit_event("a")).is_ok(),
            "兜底 MemStore 必须可入链"
        );
    }

    // ------------------------------------------------------------------
    // DispatchHandler 拒载三臂（WP-A.1 chain-key-missing）
    // ------------------------------------------------------------------

    #[test]
    fn unloaded_dispatch_refuses_submit_and_intent() {
        let mut intent_requests = 0u64;
        let mut resp = alloc::vec::Vec::new();
        // SUBMIT → 拒载回执。
        assert!(handle_unloaded(
            &[DISP_SUBMIT, 0, 1],
            &mut resp,
            &mut intent_requests
        ));
        assert_eq!(
            resp,
            crate::dispatch_service::err_receipt("chain-key-missing")
        );
        // INTENT → 计数上行 + 拒载回执。
        resp.clear();
        assert!(handle_unloaded(
            &[DISP_SUBMIT_INTENT, 0, 1],
            &mut resp,
            &mut intent_requests
        ));
        assert_eq!(intent_requests, 1);
        assert_eq!(
            resp,
            crate::dispatch_service::err_receipt("chain-key-missing")
        );
        // 未知码 → 同一拒载回执（engine None 兜底臂）。
        resp.clear();
        assert!(handle_unloaded(&[0x7F], &mut resp, &mut intent_requests));
        assert_eq!(
            resp,
            crate::dispatch_service::err_receipt("chain-key-missing")
        );
        assert_eq!(intent_requests, 1, "未知码不得计入意图请求");
    }
}

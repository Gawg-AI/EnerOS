//! 蓝图 62 卷 62.2 WP-62.2.3（S2a）：采集交付半环单源（IngestionStage）。
//!
//! 单一事实：svc-modbus-tcp / svc-iec104 两 bin 的 `collect_and_report` +
//! `report_obs` 归一化 diff 仅 2 行注释（第六轮体检 S2a 坐实）——采集 tick
//! 之后的交付半环（tick → take_reconnects → checkpoint → drain → ACQ 行 →
//! 分批 INGEST）此前整段双抄且整体 `#[cfg(target_os = "none")]`（host 不可
//! 测）。本模块把半环下沉 host 可测的 crate 库面，bin 只保留协议差异点
//! （adapter 构造、地址过滤谓词、日志 tag）。
//!
//! 证据行词形冻结：ACQ / ACQ-ROUND / OBS-REJECT / OBS-CHECKPOINT FAIL /
//! OBS-ROUND / telemetry EP not granted 各行为 45–47/61 卷证据链依赖，
//! 词形经 `line_*` 纯函数单源 + host 快照测试逐字节锁定，bin 侧不再手写
//! 格式串。
//!
//! 分批/chunk 预算/响应解析/tel_epoch 递增全部内化于
//! [`TelemetryUplink::upload`]（epoch 私有，消费方无递增可能）；
//! 批次 wire 布局单源 agent-sdk `obs_batch_wire`（WP-62.2.2）。

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_agent_sdk::chunked::RawIpc;
use eneros_agent_sdk::endpoint::ChunkedEndpoint;
use eneros_agent_sdk::obs_batch_wire::{
    decode_obs_response, encode_obs_request_stream, FRAMES_PER_TRANSFER, FRAME_WIRE_LEN,
};
use eneros_ingestion_core::{IngestReject, IngestionModule, TickBudget};
use eneros_protocol_abstract::PointRegistry;

/// 采集轮 tick 预算（蓝图 61 Phase 2 基线值 256/16；禁改——词形与基线锚）。
pub const ACQ_TICK_BUDGET: TickBudget = TickBudget {
    max_events: 256,
    max_transactions: 16,
};

/// 单轮上行报告。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpsertReport {
    /// 发送（尝试）的帧总数（全批）。
    pub attempted: usize,
    /// 服务端累计接受帧数（仅 `all_ok` 时可信）。
    pub accepted: u32,
    /// 全部批次成功。
    pub all_ok: bool,
}

/// 遥测上行端（采集交付半环的 INGEST 段单源）。
///
/// 持 EIPC 客户端 + 私有分块 epoch + source_id；目标侧以
/// `Sel4IpcClient` 实例化（`RawIpc`），host 测试以 mock `RawIpc` 全链
/// 驱动。`client: None` = EP 未授予 → 上行 fail-closed（词形行保留）。
pub struct TelemetryUplink<C: RawIpc> {
    /// chunked 端点（epoch 递增单点 = [`ChunkedEndpoint::call`]；WP-62.2.4）。
    endpoint: Option<ChunkedEndpoint<C>>,
    source_id: u32,
    accepted_total: u64,
}

impl<C: RawIpc> TelemetryUplink<C> {
    pub fn new(client: Option<C>, source_id: u32) -> Self {
        Self {
            endpoint: client.map(ChunkedEndpoint::wrap),
            source_id,
            accepted_total: 0,
        }
    }

    /// 本服务来源 ID（重连登记/批次编码同源）。
    pub fn source_id(&self) -> u32 {
        self.source_id
    }

    /// INGEST 累计接受计数（观测面）。
    pub fn accepted_total(&self) -> u64 {
        self.accepted_total
    }

    /// 上行一帧流（n × 128B ObservationEvent wire）。
    ///
    /// 内化（原两 bin `report_obs` 双抄段）：分批（[`FRAMES_PER_TRANSFER`]
    /// 上限）、批次编码、chunk 传输、响应解析、tel_epoch 递增、失败即停批
    /// （break）语义。词形行：`OBS-ROUND sent/accepted`、`OBS-ROUND
    /// rejected {:02x?}`、`OBS-ROUND eipc={:?}`、`telemetry EP not granted`。
    pub fn upload(&mut self, frames: &[u8], log_tag: &str) -> UpsertReport {
        let attempted = frames.len() / FRAME_WIRE_LEN;
        if frames.is_empty() {
            return UpsertReport {
                attempted: 0,
                accepted: 0,
                all_ok: true,
            };
        }
        let Some(endpoint) = &mut self.endpoint else {
            eneros_runtime::println!("{}", line_telemetry_not_granted(log_tag));
            return UpsertReport {
                attempted,
                accepted: 0,
                all_ok: false,
            };
        };
        let mut accepted_total: u32 = 0;
        let mut all_ok = true;
        for batch in frames.chunks(FRAME_WIRE_LEN * FRAMES_PER_TRANSFER) {
            let Ok(req) = encode_obs_request_stream(self.source_id, batch) else {
                // 分批已保证 ≤ 上限且 128B 对齐——到达此处即结构破坏，
                // fail-closed 停批（不静默截断）。
                all_ok = false;
                eneros_runtime::println!("{}", line_obs_round_encode_reject(log_tag));
                break;
            };
            match endpoint.call("svc-telemetry", &req) {
                Ok(resp) => match decode_obs_response(&resp) {
                    Some((accepted, _rejected)) => accepted_total += accepted,
                    None => {
                        all_ok = false;
                        eneros_runtime::println!("{}", line_obs_round_rejected(log_tag, &resp));
                        break;
                    }
                },
                Err(e) => {
                    all_ok = false;
                    eneros_runtime::println!("{}", line_obs_round_eipc(log_tag, &e));
                    break;
                }
            }
        }
        if all_ok {
            self.accepted_total = self
                .accepted_total
                .saturating_add(u64::from(accepted_total));
            eneros_runtime::println!("{}", line_obs_round_ok(log_tag, attempted, accepted_total));
        }
        UpsertReport {
            attempted,
            accepted: accepted_total,
            all_ok,
        }
    }
}

/// 采集轮（交付半环编排序单源）：tick → take_reconnects → checkpoint →
/// drain → ACQ 行 → ACQ-ROUND/OBS-REJECT 行 → 分批上行。
///
/// 返回 `any_good`（本轮是否存在 q=good 事件——OP_EXECUTE 状态字节语义：
/// false → ST_UNAVAILABLE fail-closed 指示，原两 bin `collect_and_report`
/// 返回值同语义）。
pub fn run_acq_round<C: RawIpc>(
    module: &mut IngestionModule,
    uplink: &mut TelemetryUplink<C>,
    log_tag: &str,
    now_ns: u64,
) -> bool {
    // 1. 采集轮：周期调度 → 组读 → 准入 → 事件日志（背压/拒绝可见）。
    let report = module.tick(now_ns, ACQ_TICK_BUDGET);
    // 2. 对端重连登记（重连不换本地 source epoch——[07] §2.4）。
    module.take_reconnects(uplink.source_id());
    // 3. watermark 检查点（批量间隔持久化；save 失败显式——不静默）。
    if let Err(e) = module.checkpoint() {
        eneros_runtime::println!("{}", line_obs_checkpoint_fail(log_tag, &e));
    }
    // 4. 事件交付：drain → ACQ 证据行（同源 wire 解码）+ 分批上行。
    let frames = module.drain_frames();
    let mut any_good = false;
    let mut frame_buf: Vec<u8> = Vec::new();
    for wire in &frames {
        let Ok(ev) = eneros_telemetry_model::event::ObservationEvent::decode_v1(wire) else {
            continue;
        };
        let eng = match ev.value {
            eneros_telemetry_model::event::ObservationValue::F64(v) => v,
            eneros_telemetry_model::event::ObservationValue::I64(v) => v as f64,
            eneros_telemetry_model::event::ObservationValue::U64(v) => v as f64,
            _ => 0.0,
        };
        let bad = ev.quality_bits & eneros_telemetry_model::event::quality_bits::VALID == 0;
        if bad {
            eneros_runtime::println!("{}", line_acq_bad(log_tag, ev.point_id, ev.quality_bits));
        } else {
            any_good = true;
            eneros_runtime::println!("{}", line_acq_good(log_tag, ev.point_id, eng, ev.sequence));
        }
        frame_buf.extend_from_slice(wire);
    }
    if report.rejected > 0 || report.backpressured > 0 {
        eneros_runtime::println!(
            "{}",
            line_acq_round(
                log_tag,
                report.rejected,
                report.backpressured,
                report.rate_limited
            )
        );
        // 拒绝原因逐条可观测（[07] §2.5：一切丢弃可观测）。
        for (point, reason) in module.take_reject_log() {
            eneros_runtime::println!("{}", line_obs_reject(log_tag, point, &reason));
        }
    }
    uplink.upload(&frame_buf, log_tag);
    any_good
}

/// 装配序列单源（蓝图 63 卷 63.2 WP-63.2.4）：svc-modbus-tcp / svc-iec104
/// 两 bin 的 `'build:` 装配序列同构——descriptor 循环（[`descriptor_of`]
/// 工厂）→ module build → `open(own, epoch_floor)` → [`TelemetryUplink`]
/// → 自检 ACQ 轮 → 构造服务；任一步失败 → capability_shell 兜底
/// （fail-closed 终态）。
///
/// **谓词/adapter 是参数，序列是一份**——modbus（范围口径恒真）与
/// iec104（协议地址谓词）的"不与对方静默统一"差异由参数显式承载。
/// svc-mms 接入 IngestionModule 时（新业务能力，独立卷裁决）第三消费方
/// 走此入口。
///
/// 自检位置内化：[`run_acq_round`] 在服务构造**前**对 module/uplink 直呼
/// （原两 bin 经 io 字段调用——对象同一、副作用相同，ACQ/ACQ-ROUND/
/// OBS-ROUND 词形逐字节一致）。
#[allow(clippy::too_many_arguments)]
pub fn build_ingested_service<C, S, M, G>(
    log_tag: &'static str,
    disabled_row_hint: &str,
    sources: &[eneros_config_bundle::points::SourceRecord],
    source_protocol: eneros_config_bundle::points::PointProtocol,
    registry: eneros_protocol_abstract::VecPointRegistry,
    schema_of: S,
    build_module: M,
    telemetry: Option<C>,
    make_service: G,
) -> Box<dyn eneros_drv_svc::runtime::DriverService>
where
    C: RawIpc,
    S: Fn(&eneros_protocol_abstract::VecPointRegistry, u32) -> Option<u32>,
    M: FnOnce(
        &eneros_protocol_abstract::VecPointRegistry,
        &[(u32, eneros_ingestion_core::SourceDescriptor)],
        u32,
    ) -> Option<IngestionModule>,
    G: FnOnce(
        IngestionModule,
        u32,
        TelemetryUplink<C>,
        eneros_protocol_abstract::VecPointRegistry,
    ) -> Box<dyn eneros_drv_svc::runtime::DriverService>,
{
    // 1. 来源 descriptor 循环（descriptor_of 单源工厂，W3；schema_of 即
    //    两协议的过滤口径差异点）。
    let mut descriptors: alloc::vec::Vec<(u32, eneros_ingestion_core::SourceDescriptor)> =
        alloc::vec::Vec::new();
    let mut own_source_id: Option<u32> = None;
    for src in sources {
        if src.protocol == source_protocol {
            let mut ids = alloc::vec::Vec::new();
            registry.collect_ids(&mut ids);
            let descriptor =
                eneros_ingestion_core::descriptor_of(src, ids, |id| schema_of(&registry, *id));
            descriptors.push((src.source_id, descriptor));
            if own_source_id.is_none() {
                own_source_id = Some(src.source_id);
            }
        }
    }
    // 2. 'build 序列（任一失败 → None → capability_shell 兜底）。
    let built: Option<Box<dyn eneros_drv_svc::runtime::DriverService>> = 'build: {
        let Some(own) = own_source_id else {
            eneros_runtime::println!("[{log_tag}] OBS-SOURCE disabled ({disabled_row_hint})");
            break 'build None;
        };
        let Some(mut module) = build_module(&registry, &descriptors, own) else {
            eneros_runtime::println!("[{log_tag}] OBS-MODULE build FAIL");
            break 'build None;
        };
        // 开源（floor = boot 单调时刻；持久 boot counter = ADR-0047）。
        let epoch_floor = crate::now_ms();
        match module.open(own, epoch_floor) {
            Ok(epoch) => {
                eneros_runtime::println!(
                    "[{log_tag}] OBS-SOURCE id={} epoch={} (config-allocated)",
                    own,
                    epoch
                );
            }
            Err(e) => {
                eneros_runtime::println!("[{log_tag}] OBS-SOURCE open FAIL: {e:?}");
                break 'build None;
            }
        }
        let mut uplink = TelemetryUplink::new(telemetry, own);
        // 启动期自检：一轮真实采集（外部 peer 记录事务；串口打印 = L2
        // 互操作/45/46 采集证据行）。
        run_acq_round(&mut module, &mut uplink, log_tag, eneros_drv_svc::now_ns());
        break 'build Some(make_service(module, own, uplink, registry));
    };
    built.unwrap_or_else(crate::protocol_ladder::capability_shell)
}

/// 参考闭环 module 构造单源（WP-64.2.2：两 bin `build_module` 前置段
/// 孪生收编——sink 1024 / epoch store / `OperationalProfile::reference`
/// 构造 + descriptor 注册 + own 来源在场检查）。own 来源缺席 → `None`
/// （原 bin fail-closed 语义逐字节）。
pub fn new_reference_module(
    sources: &[(u32, eneros_ingestion_core::SourceDescriptor)],
    own_source_id: u32,
) -> Option<IngestionModule> {
    use eneros_ingestion_core::{InMemorySink, MemEpochStore, OperationalProfile};
    let mut module = IngestionModule::new(
        Box::new(InMemorySink::new(1024)),
        Box::new(MemEpochStore::default()),
        OperationalProfile::reference(),
    );
    for (_, d) in sources {
        module.register(d.clone());
    }
    sources
        .iter()
        .any(|(id, _)| *id == own_source_id)
        .then_some(module)
}

/// 协议组计划地址三字段（[`eneros_ingestion_core::PointPlan`] 的协议侧投影
/// ——各协议 bin 的唯一真实差异点：ProtocolAddress 反构造）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanAddr {
    /// 协议从站/设备地址（非 Modbus 协议 0）。
    pub slave_addr: u8,
    /// 协议寄存器/对象地址低位（非 Modbus 协议按 bin 语义填）。
    pub reg_addr: u16,
    /// 协议功能码（非 Modbus 协议 0）。
    pub func_code: u8,
}

/// 点计划装配单源（蓝图 64 卷 64.2 WP-64.2.2）：records → `PointPlan`。
///
/// 原 svc-modbus-tcp / svc-iec104 两 bin `build_module` 的 64 行逐字孪生
/// （注册表遍历 → 遥测点过滤 → 地址反构造 → 点表记录配对 → 15 字段
/// 默认值构造）唯一实现。**协议差异只经 `addr_of` 闭包与
/// `period_default_ms` 进入**：
/// - modbus：地址 = `ProtocolAddress::Modbus` 三字段；`period_default_ms=0`
///   （事件驱动点 period_ms=0 不轮询——原语义逐字节）；
/// - iec104：地址 = `ProtocolAddress::Iec104`（ioa→reg_addr，余 0）；
///   `period_default_ms=1_000`（[07] §2.3 事件驱动在调度面不可达 → 1s
///   drain 节奏缺省）。
///
/// 字段默认值 = 原孪生逐字节（phase 0 / deadline 2×period / Q1 + 不可抑制 /
/// deadband_rel 0 / timeout 1s / retries 2 / 身份与工程量来自映射绑定）。
///
/// `on_planned` 在**点表记录配对成功之后**（即该点确定进计划时）回调
/// （mapping 引用 + 地址投影）——iec104 bin 借此同步构建 adapter mappings
/// 表。**回调必须只在配对成功后触发**：原 bin 的 mapping 行构建就在配对
/// 之后，"地址匹配但缺记录"的点不得产生副作用（64.2 复查硬伤修正——
/// 侧放 addr_of 闭包会让该角落多出 mapping 行，行为漂移）。
pub fn plans_from_records(
    records: &[eneros_config_bundle::points::PointRecord],
    own_source: u32,
    registry: &dyn eneros_protocol_abstract::PointRegistry,
    addr_of: impl Fn(&eneros_protocol_abstract::mapping::ProtocolPointMapping) -> Option<PlanAddr>,
    period_default_ms: u64,
    mut on_planned: impl FnMut(&eneros_protocol_abstract::mapping::ProtocolPointMapping, PlanAddr),
) -> Vec<eneros_ingestion_core::PointPlan> {
    use eneros_ingestion_core::{EventQos, PointPlan};
    let mut plans = Vec::new();
    let mut ids = Vec::new();
    registry.collect_ids(&mut ids);
    for id in ids {
        let Some(m) = registry.resolve(id) else {
            continue;
        };
        // 遥测点（analog/digital）才进采集调度；控制/设点/计数点不采集。
        match m.data_type {
            eneros_upa_model::PointType::Analog | eneros_upa_model::PointType::Digital => {}
            _ => continue,
        }
        // 协议地址反构造（bin 闭包——非本协议地址的映射点跳过）。
        let Some(addr) = addr_of(m) else {
            continue;
        };
        // 调度字段来自点表记录（缺记录 fail-closed：无调度的点不进计划）。
        let Some(rec) = records.iter().find(|r| r.point_id == m.point_id) else {
            continue;
        };
        on_planned(m, addr);
        let period_ms = if rec.period_ms == 0 {
            period_default_ms
        } else {
            u64::from(rec.period_ms)
        };
        let period_ns = period_ms * 1_000_000;
        plans.push(PointPlan {
            source_id: own_source,
            point_id: m.point_id,
            device_id: m.device_id,
            // 周期来自点表（period_ms）；period_ms=0 → bin 缺省（modbus 保持
            // 0 = 事件驱动不轮询；iec104 → drain 节奏缺省）。
            period_ns,
            phase_offset_ns: 0,
            deadline_ns: period_ns.saturating_mul(2),
            // 参考闭环 QoS：Q1 + 不可抑制（控制资格点全量上报——与
            // Phase 0 基线一致；死区/QoS 分级在配置面扩展后生效）。
            qos: EventQos::Q1,
            suppressible: false,
            deadband_abs: rec.deadband,
            deadband_rel: 0.0,
            timeout_ns: 1_000_000_000,
            max_retries: 2,
            schema_id: m.identity.schema_id,
            schema_version: m.identity.schema_version,
            unit_code: m.identity.unit_code,
            asset_id: m.identity.asset_id,
            mapping_version: m.identity.mapping_generation,
            scale: m.scale,
            offset: m.offset,
            func_code: addr.func_code,
            reg_addr: addr.reg_addr,
            slave_addr: addr.slave_addr,
        });
    }
    plans
}

/// 遥测上行 EIPC 端点装配单源（WP-64.2.3：两 ingest bin 的 ~13 行
/// `eipc_resolve + Sel4IpcClient::new + ok()` 闭包归宿；**bound/NOT-granted
/// 词形归模块**——R-64.g：打印行逐字节不变，变的只是谁拥有拼出它的知识）。
///
/// 返回裸客户端（`TelemetryUplink::new` 的 `Option<C>` 形态——source_id
/// 由 [`build_ingested_service`] 的 own 决定，绑定时不可得；绑定时机保持在
/// 阶梯 run 之前 = 串口行序不变）。
#[cfg(any(target_os = "none", feature = "sel4"))]
pub fn bind_telemetry(service: &str) -> Option<eneros_agent_sdk::ipc_sel4::Sel4IpcClient> {
    let client = eneros_agent_sdk::eipc_resolve(service, "svc-telemetry").and_then(|(_, slot)| {
        eneros_agent_sdk::ipc_sel4::Sel4IpcClient::new(
            "telemetry-uplink",
            slot,
            eneros_drv_svc::now_ns,
        )
        .ok()
    });
    if client.is_some() {
        eneros_runtime::println!("{}", line_telemetry_bound(service));
    } else {
        eneros_runtime::println!("{}", line_telemetry_not_granted_ep(service));
    }
    client
}

// ============================================================================
// 词形单源（冻结证据行；host 快照测试逐字节锁定——bin 侧禁止手写格式串）
// ============================================================================
fn line_acq_good(tag: &str, point_id: u32, value: f64, sequence: u64) -> String {
    format!("[{tag}] ACQ point={point_id} value={value} q=good seq={sequence}")
}

fn line_acq_bad(tag: &str, point_id: u32, quality_bits: u16) -> String {
    format!("[{tag}] ACQ point={point_id} q=bad (quality={quality_bits:#06x})")
}

fn line_acq_round(tag: &str, rejected: u32, backpressured: u32, rate_limited: u32) -> String {
    format!("[{tag}] ACQ-ROUND rejected={rejected} backpressured={backpressured} (rate-limited={rate_limited})")
}

fn line_obs_reject(tag: &str, point: u32, reason: &IngestReject) -> String {
    format!("[{tag}] OBS-REJECT point={point} reason={reason:?}")
}

fn line_obs_checkpoint_fail(tag: &str, e: &IngestReject) -> String {
    format!("[{tag}] OBS-CHECKPOINT FAIL: {e:?}")
}

fn line_obs_round_ok(tag: &str, sent: usize, accepted: u32) -> String {
    format!("[{tag}] OBS-ROUND sent={sent} accepted={accepted}")
}

fn line_obs_round_encode_reject(tag: &str) -> String {
    format!("[{tag}] OBS-ROUND encode-reject —— fail-closed")
}

fn line_obs_round_rejected(tag: &str, resp: &[u8]) -> String {
    format!(
        "[{tag}] OBS-ROUND rejected {:02x?}",
        &resp[..resp.len().min(8)]
    )
}

fn line_obs_round_eipc(tag: &str, e: &eneros_agent_sdk::chunked::ChunkError) -> String {
    format!("[{tag}] OBS-ROUND eipc={e:?}")
}

fn line_telemetry_not_granted(tag: &str) -> String {
    format!("[{tag}] telemetry EP not granted —— 事件未上报（fail-closed）")
}

fn line_telemetry_bound(service: &str) -> String {
    format!("[{service}] telemetry EIPC bound (svc-telemetry)")
}

fn line_telemetry_not_granted_ep(service: &str) -> String {
    format!("[{service}] telemetry EIPC NOT granted")
}

// ============================================================================
// host 测试：词形快照（逐字节）+ uplink 全链（chunked 服务端 mock）+ 结构守卫
// ============================================================================

#[cfg(test)]
mod tests {
    use alloc::boxed::Box;

    use eneros_agent_sdk::chunked::{
        ack_bytes, encode_paged_header, is_pull_request, page_slice, ChunkedEvent, ChunkedReceiver,
        CHUNK_MAGIC_FINAL, CHUNK_MAGIC_HEADER,
    };
    use eneros_agent_sdk::obs_batch_wire::{decode_obs_request, encode_obs_response};
    use eneros_agent_sdk::AgentError;
    use eneros_ingestion_core::{InMemorySink, MemEpochStore, OperationalProfile};

    use super::*;

    /// 冻结词形对照（逐字节取自迁移前 svc-modbus-tcp.rs / svc-iec104.rs；
    /// 期望侧禁止引用 line_* helper——改一词形必须变红）。
    const TAG: &str = "svc-modbus-tcp";

    #[test]
    fn wordforms_are_byte_frozen() {
        assert_eq!(
            line_acq_good(TAG, 3, 230.5, 42),
            "[svc-modbus-tcp] ACQ point=3 value=230.5 q=good seq=42"
        );
        assert_eq!(
            line_acq_bad(TAG, 7, 0x2),
            "[svc-modbus-tcp] ACQ point=7 q=bad (quality=0x0002)"
        );
        assert_eq!(
            line_acq_round(TAG, 2, 1, 1),
            "[svc-modbus-tcp] ACQ-ROUND rejected=2 backpressured=1 (rate-limited=1)"
        );
        assert_eq!(
            line_obs_reject(TAG, 7, &IngestReject::RateExceeded { source_id: 2 }),
            "[svc-modbus-tcp] OBS-REJECT point=7 reason=RateExceeded { source_id: 2 }"
        );
        assert_eq!(
            line_obs_checkpoint_fail(TAG, &IngestReject::UnregisteredSource { source_id: 9 }),
            "[svc-modbus-tcp] OBS-CHECKPOINT FAIL: UnregisteredSource { source_id: 9 }"
        );
        assert_eq!(
            line_obs_round_ok(TAG, 12, 12),
            "[svc-modbus-tcp] OBS-ROUND sent=12 accepted=12"
        );
        assert_eq!(
            line_obs_round_encode_reject(TAG),
            "[svc-modbus-tcp] OBS-ROUND encode-reject —— fail-closed"
        );
        assert_eq!(
            line_obs_round_rejected(TAG, &[0xFF, 1, 2]),
            "[svc-modbus-tcp] OBS-ROUND rejected [ff, 01, 02]"
        );
        assert_eq!(
            line_obs_round_eipc(TAG, &eneros_agent_sdk::chunked::ChunkError::Transport),
            "[svc-modbus-tcp] OBS-ROUND eipc=Transport"
        );
        assert_eq!(
            line_telemetry_not_granted(TAG),
            "[svc-modbus-tcp] telemetry EP not granted —— 事件未上报（fail-closed）"
        );
        // WP-64.2.3：上行端点装配行（两 ingest bin 的闭包词形收编——逐字节
        // 取自迁移前 svc-modbus-tcp.rs / svc-iec104.rs）。
        assert_eq!(
            line_telemetry_bound(TAG),
            "[svc-modbus-tcp] telemetry EIPC bound (svc-telemetry)"
        );
        assert_eq!(
            line_telemetry_not_granted_ep(TAG),
            "[svc-modbus-tcp] telemetry EIPC NOT granted"
        );
        assert_eq!(
            line_telemetry_bound("svc-iec104"),
            "[svc-iec104] telemetry EIPC bound (svc-telemetry)"
        );
        // iec104 tag 同词形（仅 tag 段差异）。
        assert_eq!(
            line_acq_good("svc-iec104", 1, 0.0, 1),
            "[svc-iec104] ACQ point=1 value=0 q=good seq=1"
        );
    }

    /// chunked 服务端 mock（`RawIpc`）：Header→ACK / Data(send) 组装 /
    /// Final→obs 服务端语义→分页头 / 拉取→分页数据。fail=true 时服务端
    /// 回非 OK 首字节（客户端 decode → None → rejected 词形路径）。
    struct ChunkServer {
        rx: ChunkedReceiver,
        complete: Option<Vec<u8>>,
        pending_resp: Vec<u8>,
        fail: bool,
        /// Final 处理计数（停批断言用）。
        final_count: usize,
    }
    impl ChunkServer {
        fn new(fail: bool) -> Self {
            Self {
                rx: ChunkedReceiver::new(),
                complete: None,
                pending_resp: Vec::new(),
                fail,
                final_count: 0,
            }
        }
    }
    impl RawIpc for ChunkServer {
        fn request(
            &mut self,
            _target: &str,
            payload: &[u8],
            _timeout_ns: u64,
        ) -> Result<Vec<u8>, AgentError> {
            if is_pull_request(payload) {
                return Ok(page_slice(&self.pending_resp, payload[4]).to_vec());
            }
            if payload.len() == 4 && payload[0..4] == *CHUNK_MAGIC_FINAL {
                self.final_count += 1;
                let body = self.complete.clone().unwrap_or_default();
                // obs 服务端语义：解析批次 → 全收（fail 时回 1B 0xFF）。
                if self.fail {
                    self.pending_resp = alloc::vec![0xFF];
                } else {
                    let (_, frames) = decode_obs_request(&body).map_err(|_| AgentError::Timeout)?;
                    let count = (frames.len() / FRAME_WIRE_LEN) as u16;
                    self.pending_resp = encode_obs_response(count, 0).to_vec();
                }
                return Ok(encode_paged_header(self.pending_resp.len()).to_vec());
            }
            if payload.len() >= 4 && payload[0..4] == *CHUNK_MAGIC_HEADER {
                let _ = self.rx.on_message(payload, true);
                return Ok(ack_bytes().to_vec());
            }
            Err(AgentError::Timeout)
        }
        fn send(&mut self, _target: &str, payload: &[u8]) -> Result<(), AgentError> {
            if let ChunkedEvent::Complete(v) = self.rx.on_message(payload, false) {
                self.complete = Some(v);
            }
            Ok(())
        }
    }

    fn frames_buf(n: usize) -> Vec<u8> {
        let mut buf = Vec::new();
        for i in 0..n {
            let mut f = [0u8; FRAME_WIRE_LEN];
            f[0] = i as u8;
            buf.extend_from_slice(&f);
        }
        buf
    }

    #[test]
    fn upload_splits_batches_and_delivers_all_frames() {
        let mut up = TelemetryUplink::new(Some(ChunkServer::new(false)), 2);
        // 12 + 5 = 17 帧 → 两批（12 / 5），全部送达。
        let rep = up.upload(&frames_buf(17), TAG);
        assert!(rep.all_ok);
        assert_eq!(rep.attempted, 17);
        assert_eq!(rep.accepted, 17);
        assert_eq!(up.accepted_total(), 17);
    }

    #[test]
    fn upload_fail_closed_without_endpoint_and_reject_stops_batches() {
        // EP 未授予：fail-closed，accepted_total 不动。
        let mut up: TelemetryUplink<ChunkServer> = TelemetryUplink::new(None, 2);
        let rep = up.upload(&frames_buf(3), TAG);
        assert!(!rep.all_ok);
        assert_eq!(up.accepted_total(), 0);

        // 服务端拒绝（非 OK 响应）：停批（剩余批不发），all_ok=false。
        let mut up = TelemetryUplink::new(Some(ChunkServer::new(true)), 2);
        let rep = up.upload(&frames_buf(15), TAG);
        assert!(!rep.all_ok);
        assert_eq!(rep.accepted, 0);
        assert_eq!(
            up.endpoint.as_mut().unwrap().client_mut().final_count,
            1,
            "拒绝后停批"
        );
    }

    #[test]
    fn empty_round_is_noop() {
        let mut module = IngestionModule::new(
            Box::new(InMemorySink::new(64)),
            Box::new(MemEpochStore::default()),
            OperationalProfile::reference(),
        );
        let mut up = TelemetryUplink::new(Some(ChunkServer::new(false)), 2);
        // 空 module：round 正常完成（tick 0 事务、无帧、无上行）。
        let good = run_acq_round(&mut module, &mut up, TAG, 1_000);
        assert!(!good);
        assert_eq!(
            up.endpoint.as_mut().unwrap().client_mut().final_count,
            0,
            "无帧不发起传输"
        );
    }

    // ------------------------------------------------------------------------
    // 结构守卫（防再抄）：两 bin 不得再持有本地采集编排函数/手拼批次/chunked 直用。
    // 69.3 WP-69.3.3 M3 逐锁处置（执行卡 7.0 M3 表，实测 2026-09-07）：
    //   删 1 道——原 #4 `!src.contains("ChunkedClient::new")`：冗余副本。
    //     agent-sdk endpoint.rs `chunked_client_new_only_inside_agent_sdk`
    //     仓走守卫（67 WP-67.2.4）已全仓覆盖（crates/ 递归、白名单空、
    //     含尾括号全精度模式、变异②实弹咬合），本窄域锁删除后防御面不缩
    //     （M3 核于 2026-09-07：守卫 walk 含两 bin 文件）。
    //   边界加固 1 道——#10 `fetch_points`：主函数已降 `pub(crate)`
    //     （service_cfg.rs，bin 结构性不可达 E0603）；锁保留防孪生
    //     `fetch_points_registry`（svc-modbus-rtu 合法消费，无法降级）
    //     被族 B bin 用于二次拉表。
    //   余 10 道保留：#1/#2/#6 本地定义/标签禁令（bin 是独立 crate，无
    //     边界可挡本地新定义）；#3/#5 内存/算术字面（bin 对自有 Vec 有
    //     完全控制权）；#7 Z2 词形（println 字面无边界）；#8/#9 pub POD
    //     字段/pub 枚举被 adapter 合法消费（私有化=破坏读 API）；#11/#12
    //     pub 且跨 bin/crate 合法消费（降级必炸）——逐锁依据见执行卡。
    // ------------------------------------------------------------------------
    #[test]
    fn bins_no_longer_own_collection_orchestration() {
        let modbus = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/bin/svc-modbus-tcp.rs"
        ));
        let iec104 = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/bin/svc-iec104.rs"
        ));
        for (name, src) in [("svc-modbus-tcp", modbus), ("svc-iec104", iec104)] {
            assert!(
                !src.contains("fn collect_and_report"),
                "{name} 重新出现本地采集编排函数（S2a 禁止再抄）"
            );
            assert!(
                !src.contains("fn report_obs"),
                "{name} 重新出现本地上行编排函数（S2a 禁止再抄）"
            );
            assert!(
                !src.contains("push(4u8)"),
                "{name} 重新出现 OBS 批次手拼（S2b 禁止）"
            );
            // 69.3：原第 4 道锁 `!src.contains("ChunkedClient::new")` 已删
            // ——agent-sdk endpoint.rs 仓走守卫全仓覆盖本 bin（M3 核于
            // 2026-09-07，防御更强：白名单空 + 尾括号全精度 + 变异咬合）。
            assert!(
                !src.contains("saturating_add(1)"),
                "{name} 重新出现 epoch 手递增（S4 禁止）"
            );
            // 63.2 WP-63.2.4：装配序列进 lib——bin 不得再持有 'build 序列/
            // 自检调用/(descriptor 循环)。
            assert!(
                !src.contains("'build:"),
                "{name} 重新出现 'build 装配序列（63.2.4 禁止再抄，应走 \
                 build_ingested_service）"
            );
            assert!(
                !src.contains("OBS-SOURCE"),
                "{name} 重新出现 OBS-SOURCE 词形手写（63.2.4 禁止——词形归 \
                 build_ingested_service 单源）"
            );
            // 64.2 WP-64.2.2：点计划装配进 lib——bin 不得再持 PointPlan
            // 字面构造 / EventQos 字面（孪生 64 行的回归守卫）。
            assert!(
                !src.contains("PointPlan {"),
                "{name} 重新出现 PointPlan 字面构造（64.2.2 禁止——点计划装配归 \
                 plans_from_records 单源）"
            );
            assert!(
                !src.contains("EventQos::"),
                "{name} 重新出现 QoS 字面（64.2.2 禁止——默认值归单源）"
            );
            // 64.2 WP-64.2.3：二次 fetch / 端点手抄闭包删除——records 由
            // Ladder::Ready 携带；telemetry 装配归 bind_telemetry。
            // 69.3：主函数 fetch_points 已降 pub(crate)（bin 调用 = E0603
            // 编译失败，实测证据在执行卡）；锁保留防孪生
            // fetch_points_registry 面（svc-modbus-rtu 合法消费无法降级）。
            assert!(
                !src.contains("fetch_points"),
                "{name} 重新出现 fetch_points 直呼（64.2.3 禁止——records 由 Ready 携带）"
            );
            assert!(
                !src.contains("eipc_resolve"),
                "{name} 重新出现 EIPC 手工解析（64.2.3 禁止——telemetry 装配归 bind_telemetry；\
                 eipc_resolve 被其它 bin 合法消费无法降级，M3 #11）"
            );
            assert!(
                src.matches(concat!("Sel4IpcClient", "::new")).count() == 0,
                "{name} 重新出现 Sel4IpcClient 手工建端（64.2.3 禁止；跨 5+ \
                 crate 合法消费无法降级，M3 #12）"
            );
        }
    }

    // ------------------------------------------------------------------------
    // 63.2 WP-63.2.4：装配序列单源测试（mock 过 build_ingested_service 全
    // 序列，锁 descriptor 循环谓词 / make_service 契约 / 壳兜底三态）。
    // ------------------------------------------------------------------------

    use eneros_config_bundle::points::{PointProtocol, SourceRecord};
    use eneros_drv_svc::runtime::{DriverRequest, DriverResponse, DriverService};
    use eneros_protocol_abstract::mapping::ProtocolPointMapping;
    use eneros_protocol_abstract::{ProtocolAddress, VecPointRegistry};

    struct ProbeService {
        cap: Option<&'static str>,
    }
    impl DriverService for ProbeService {
        fn capability(&self) -> Option<&'static str> {
            self.cap
        }
        fn handle(&mut self, _req: &DriverRequest, _resp: &mut DriverResponse) {}
    }

    fn source_row(source_id: u32, protocol: PointProtocol) -> SourceRecord {
        SourceRecord {
            source_id,
            tenant_id: 1,
            site_id: 1,
            protocol,
            allow_control: false,
            require_integrity: false,
            point_min: 0,
            point_max: 0,
            max_events_per_s: 100,
        }
    }

    fn mapping(point_id: u32, addr: ProtocolAddress, schema_id: u32) -> ProtocolPointMapping {
        ProtocolPointMapping {
            point_id,
            device_id: 1,
            protocol_addr: addr,
            data_type: eneros_upa_model::PointType::Analog,
            scale: 1.0,
            offset: 0.0,
            identity: eneros_protocol_abstract::mapping::PointIdentityBinding {
                unit_code: 0,
                schema_id,
                schema_version: 1,
                asset_id: 0,
                mapping_generation: 1,
            },
        }
    }

    #[test]
    fn assembly_sequence_builds_service_via_make_closure() {
        let registry = VecPointRegistry::new(alloc::vec![mapping(
            1,
            ProtocolAddress::Modbus {
                slave_addr: 1,
                reg_addr: 0,
                func_code: 3,
            },
            7
        )]);
        let sources = alloc::vec![source_row(2, PointProtocol::ModbusTcp)];
        let built = core::cell::Cell::new(false);
        let svc = build_ingested_service(
            "svc-test",
            "no test [[source]] row",
            &sources,
            PointProtocol::ModbusTcp,
            registry,
            |_reg, _id| Some(7),
            |_reg, descriptors, own| {
                // descriptor 循环锁：本来源行被收集，schema 谓词结果进入
                // allowed_schemas。
                assert_eq!(own, 2);
                assert_eq!(descriptors.len(), 1);
                assert_eq!(descriptors[0].0, 2);
                assert_eq!(descriptors[0].1.allowed_schemas, alloc::vec![7]);
                let mut module = IngestionModule::new(
                    Box::new(InMemorySink::new(64)),
                    Box::new(MemEpochStore::default()),
                    OperationalProfile::reference(),
                );
                for (_, d) in descriptors {
                    module.register(d.clone());
                }
                Some(module)
            },
            None::<ChunkServer>,
            |_module, own, uplink, _registry| {
                // make_service 契约锁：own/uplink 同源（open→自检→构造的
                // 顺序由序列本体结构保证——open 失败即走壳，到不了此处）。
                assert_eq!(own, 2);
                assert_eq!(uplink.source_id(), 2);
                built.set(true);
                Box::new(ProbeService {
                    cap: Some("test-cap"),
                })
            },
        );
        assert!(built.get(), "make_service 必经（序列走通）");
        assert_eq!(svc.capability(), Some("test-cap"), "真实服务（非能力壳）");
    }

    #[test]
    fn assembly_sequence_falls_back_to_shell_on_missing_source_row() {
        let registry = VecPointRegistry::new(alloc::vec![]);
        let sources = alloc::vec![source_row(3, PointProtocol::Iec104)];
        let built = core::cell::Cell::new(false);
        let svc = build_ingested_service(
            "svc-test",
            "no test [[source]] row",
            &sources,
            PointProtocol::ModbusTcp,
            registry,
            |_reg, _id| None,
            |_reg, _d, _own| {
                built.set(true);
                None
            },
            None::<ChunkServer>,
            |_m, _o, _u, _r| {
                Box::new(ProbeService {
                    cap: Some("test-cap"),
                })
            },
        );
        assert!(!built.get(), "无本协议来源行 → 不进 build");
        assert_eq!(svc.capability(), None, "capability_shell 兜底（cap=None）");
    }

    #[test]
    fn assembly_sequence_falls_back_to_shell_on_module_build_fail() {
        let registry = VecPointRegistry::new(alloc::vec![]);
        let sources = alloc::vec![source_row(2, PointProtocol::ModbusTcp)];
        let built = core::cell::Cell::new(false);
        let svc = build_ingested_service(
            "svc-test",
            "no test [[source]] row",
            &sources,
            PointProtocol::ModbusTcp,
            registry,
            |_reg, _id| None,
            |_reg, _d, _own| {
                built.set(true);
                None
            },
            None::<ChunkServer>,
            |_m, _o, _u, _r| {
                Box::new(ProbeService {
                    cap: Some("test-cap"),
                })
            },
        );
        assert!(built.get(), "builder 被调用但失败");
        assert_eq!(svc.capability(), None, "module build FAIL → 壳兜底");
    }

    #[test]
    fn assembly_sequence_schema_predicate_is_parameterized() {
        // 谓词参数化锁：同一来源行，iec104 协议地址谓词（非 Iec104 地址
        // 的映射点被滤除——modbus/iec104 口径差异不静默统一）。
        let registry = VecPointRegistry::new(alloc::vec![
            mapping(
                1,
                ProtocolAddress::Modbus {
                    slave_addr: 1,
                    reg_addr: 0,
                    func_code: 3
                },
                7
            ),
            mapping(
                2,
                ProtocolAddress::Iec104 {
                    common_addr: 1,
                    ioa: 100,
                    type_id: 1,
                },
                9
            ),
        ]);
        let sources = alloc::vec![source_row(2, PointProtocol::Iec104)];
        let svc = build_ingested_service(
            "svc-test",
            "no test [[source]] row",
            &sources,
            PointProtocol::Iec104,
            registry,
            |reg, id| {
                reg.resolve(id).and_then(|m| match m.protocol_addr {
                    ProtocolAddress::Iec104 { .. } => Some(m.identity.schema_id),
                    _ => None,
                })
            },
            |_reg, descriptors, _own| {
                assert_eq!(
                    descriptors[0].1.allowed_schemas,
                    alloc::vec![9],
                    "modbus 地址映射点被协议地址谓词滤除（口径差异参数化保留）"
                );
                let mut module = IngestionModule::new(
                    Box::new(InMemorySink::new(64)),
                    Box::new(MemEpochStore::default()),
                    OperationalProfile::reference(),
                );
                for (_, d) in descriptors {
                    module.register(d.clone());
                }
                Some(module)
            },
            None::<ChunkServer>,
            |_m, _o, _u, _r| {
                Box::new(ProbeService {
                    cap: Some("test-cap"),
                })
            },
        );
        assert_eq!(svc.capability(), Some("test-cap"));
    }

    // ------------------------------------------------------------------------
    // WP-64.2.2：plans_from_records——64 行孪生的行为锁（15 字段默认值 +
    // 过滤/配对/周期缺省语义逐字段断言）。
    // ------------------------------------------------------------------------

    use eneros_config_bundle::points::PointRecord;
    use eneros_ingestion_core::{EventQos, PointPlan};
    use eneros_protocol_abstract::mapping::PointIdentityBinding;

    fn record(point_id: u32, period_ms: u32, deadband: f64) -> PointRecord {
        PointRecord {
            point_id,
            device_id: 1,
            protocol: PointProtocol::ModbusTcp,
            addr: Some(ProtocolAddress::Modbus {
                slave_addr: 3,
                reg_addr: 0x10,
                func_code: 4,
            }),
            data_type: eneros_upa_model::PointType::Analog,
            scale: 2.5,
            offset: -1.0,
            period_ms,
            deadband,
            range: None,
            identity: PointIdentityBinding {
                unit_code: 7,
                schema_id: 11,
                schema_version: 3,
                asset_id: 42,
                mapping_generation: 9,
            },
        }
    }

    fn modbus_addr_of(
        m: &eneros_protocol_abstract::mapping::ProtocolPointMapping,
    ) -> Option<PlanAddr> {
        match m.protocol_addr {
            ProtocolAddress::Modbus {
                slave_addr,
                reg_addr,
                func_code,
            } => Some(PlanAddr {
                slave_addr,
                reg_addr,
                func_code,
            }),
            _ => None,
        }
    }

    #[test]
    fn plans_from_records_locks_twin_field_defaults() {
        // 1 点（Analog + Modbus 地址 + 记录 period 5000/deadband 0.25）→
        // 全 15+ 字段值断言（= 原 build_module 孪生逐字节行为锁）。
        let rec = record(1, 5_000, 0.25);
        let registry = VecPointRegistry::new(alloc::vec![rec.to_mapping().expect("mapping")]);
        let plans = plans_from_records(&[rec], 6, &registry, modbus_addr_of, 0, |_, _| {});
        assert_eq!(plans.len(), 1);
        let p: PointPlan = plans[0];
        assert_eq!(p.source_id, 6);
        assert_eq!(p.point_id, 1);
        assert_eq!(p.device_id, 1);
        assert_eq!(p.period_ns, 5_000 * 1_000_000);
        assert_eq!(p.phase_offset_ns, 0);
        assert_eq!(p.deadline_ns, 2 * 5_000 * 1_000_000);
        assert_eq!(p.qos, EventQos::Q1);
        assert!(!p.suppressible);
        assert_eq!(p.deadband_abs, 0.25);
        assert_eq!(p.deadband_rel, 0.0);
        assert_eq!(p.timeout_ns, 1_000_000_000);
        assert_eq!(p.max_retries, 2);
        assert_eq!(p.schema_id, 11);
        assert_eq!(p.schema_version, 3);
        assert_eq!(p.unit_code, 7);
        assert_eq!(p.asset_id, 42);
        assert_eq!(p.mapping_version, 9);
        assert_eq!(p.scale, 2.5);
        assert_eq!(p.offset, -1.0);
        assert_eq!(p.func_code, 4);
        assert_eq!(p.reg_addr, 0x10);
        assert_eq!(p.slave_addr, 3);
    }

    #[test]
    fn plans_from_records_filters_and_pairs_like_bins() {
        // 非 Analog 点 / 非本协议地址 / 缺点表记录 → 全跳过（原孪生
        // continue 三连）；period_ms=0 → period_default_ms（iec104 缺省面）。
        let analog = record(1, 5_000, 0.1);
        let mut control_point = record(2, 5_000, 0.1);
        control_point.data_type = eneros_upa_model::PointType::Setpoint;
        let mut other_proto = record(3, 5_000, 0.1);
        other_proto.addr = Some(ProtocolAddress::Iec104 {
            common_addr: 1,
            ioa: 9,
            type_id: 1,
        });
        let mut no_record = record(4, 5_000, 0.1);
        no_record.point_id = 99; // 注册表有映射、records 无配对记录。
        let mut event_driven = record(5, 0, 0.1); // period_ms=0。
        event_driven.point_id = 5;
        let registry = VecPointRegistry::new(alloc::vec![
            analog.to_mapping().unwrap(),
            control_point.to_mapping().unwrap(),
            other_proto.to_mapping().unwrap(),
            no_record.to_mapping().unwrap(),
            event_driven.to_mapping().unwrap(),
        ]);
        let records = [analog, control_point, other_proto, event_driven.clone()];
        let plans = plans_from_records(&records, 2, &registry, modbus_addr_of, 1_000, |_, _| {});
        // 只有 analog（1）与 event_driven（5，period 0 → 缺省 1000ms）进计划。
        assert_eq!(
            plans.iter().map(|p| p.point_id).collect::<Vec<_>>(),
            alloc::vec![1, 5],
            "控制点/他协议地址点/缺记录点全跳过（孪生过滤序）"
        );
        assert_eq!(plans[1].period_ns, 1_000 * 1_000_000, "period 0 → 缺省");
        // modbus 缺省 = 0：事件驱动点不轮询（原 modbus bin 语义逐字节）。
        let plans_m =
            plans_from_records(&[event_driven], 2, &registry, modbus_addr_of, 0, |_, _| {});
        assert_eq!(
            plans_m[0].period_ns, 0,
            "modbus 面 period 0 保持 0（不轮询）"
        );
    }

    /// 64.2 复查硬伤回归锁：地址匹配但**缺点表记录**的点不得触发
    /// on_planned（原 iec104 bin 的 mappings 构建在配对成功后——回调
    /// 语义必须与之一致，否则该角落多出 adapter mapping 行）。
    #[test]
    fn on_planned_fires_only_after_record_pairing() {
        let paired = record(1, 5_000, 0.1);
        let mut unpaired = record(2, 5_000, 0.1);
        unpaired.point_id = 77; // 注册表有映射、records 无配对记录。
        let registry = VecPointRegistry::new(alloc::vec![
            paired.to_mapping().unwrap(),
            unpaired.to_mapping().unwrap(),
        ]);
        let planned: core::cell::RefCell<Vec<u32>> = core::cell::RefCell::new(Vec::new());
        let plans = plans_from_records(&[paired], 2, &registry, modbus_addr_of, 0, |m, _addr| {
            planned.borrow_mut().push(m.point_id)
        });
        assert_eq!(plans.len(), 1);
        assert_eq!(
            planned.into_inner(),
            alloc::vec![1],
            "缺记录点（77）不得触发 on_planned"
        );
    }
}

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
            assert!(
                !src.contains("ChunkedClient::new"),
                "{name} 重新出现 chunked 直用（S4 收敛面，应走 uplink/ChunkedEndpoint）"
            );
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
}

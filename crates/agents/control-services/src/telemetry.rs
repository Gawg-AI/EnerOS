//! 蓝图 40 WP-40.2：TelemetryService（svc-telemetry 服务逻辑）。
//!
//! - 有界样本缓冲（去重：同点同序号拒绝；断线 epoch 推进后旧消息拒绝）；
//! - 快照：原子冻结覆盖 + provenance（经 `TelemetryTrustEvaluator`——
//!   覆盖/新鲜度/质量/范围/重放/完整性全校验）；
//! - 输出只能是 `TrustedTelemetry` 或结构化拒绝（零"健康默认值"）。
//!
//! 服务请求 wire（定长，版本化）：
//! - `TEL_INGEST`：样本批次（逐样本 8 字段定长）→ ack；
//! - `TEL_SNAPSHOT`：评估当前缓冲 → `TrustedTelemetry` wire 或拒绝码；
//! - `TEL_ADVANCE_EPOCH`：断线重连推进（旧消息拒绝）。

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::vec;
use alloc::vec::Vec;

/// 69.3 T2：来源身份装载走 ConfigEntrySource trait 面（bin = ChunkedEndpoint
/// impl；host 测试 = 内存假件——签名收窄，见 [`TelemetryService::
/// ensure_source_identity`]）。cfg 与方法体同门（fetch_catalog 口径）。
#[cfg(any(test, feature = "sel4"))]
use eneros_agent_sdk::config_gateway::ConfigEntrySource;
/// 蓝图 61 v0.314.27：观测事件影子批次（V1 128B 帧 × n；影子轨零控制消费）。
/// 蓝图 62 卷 62.2（WP-62.2.2）：批次 wire（布局/编码/解码/分批预算）收归
/// agent-sdk `obs_batch_wire` 单一事实源，本 crate 与协议服务共同消费；
/// 原 `OBS_REQ_HEADER_LEN`/`OBS_RESP_LEN` 本地常量随实现内转正而退役。
pub use eneros_agent_sdk::obs_batch_wire::{
    decode_obs_request, encode_obs_response, FRAME_WIRE_LEN as OBS_FRAME_WIRE_LEN,
    OPCODE_TEL_INGEST_OBS as TEL_INGEST_OBS,
};
/// 45.5：58B 定长样本 wire 收归 agent-sdk `sample_wire`（单一事实；
/// root/svc-telemetry/协议服务共同消费）。此处 re-export 保持既有调用面。
pub use eneros_agent_sdk::sample_wire::{
    decode_sample, encode_sample, integrity_code, quality_code, SAMPLE_WIRE_LEN,
};
/// 请求码（蓝图 67 WP-67.3 单源收编：常量本体只在 agent-sdk `tel_wire`
/// ——此处 re-export 别名保持 crate 内分发表与既有调用面零改动；本文件
/// 禁止再以裸 const 定义请求码（自守卫测试执法，见文件尾））。
pub use eneros_agent_sdk::tel_wire::{
    TEL_REQ_ADVANCE_EPOCH as TEL_ADVANCE_EPOCH, TEL_REQ_INGEST as TEL_INGEST,
    TEL_REQ_SNAPSHOT as TEL_SNAPSHOT,
};
use eneros_control_dispatch::FeedbackPointMapping;
use eneros_safety_validator::{
    EvaluatorConfig, TelemetrySample, TelemetryTrustEvaluator, TrustRejection, TrustedTelemetry,
};
use eneros_telemetry_model::event::ObservationEvent;
/// 样本缓冲上限（有界）。
pub const MAX_BUFFERED_SAMPLES: usize = 256;
/// 评估器配置（服务装配期注入；覆盖点由部署配置决定）。
/// 单源 = `eneros_decision_contract::CONTROL_HORIZON_MS`（ADR-0054:34–35：
/// 快照产出后 30s 是安全事实，不为 4×15s 配置放宽——演示时域收敛为
/// 4×7s=28s ≤ 30s。2026-09-18 R10 复查订正：上一轮 60s 违约已回退）。
/// 重导出仅为既有调用点兼容。
pub const DEFAULT_CONTROL_HORIZON_MS: u64 = eneros_decision_contract::CONTROL_HORIZON_MS;

/// 快照响应（TrustedTelemetry wire）。
pub const SNAPSHOT_OK: u8 = 0;
pub const SNAPSHOT_REJECTED: u8 = 1;

/// 状态绑定单一事实（点 1-4 = 电压/电流/频率/SOC；评估器配置与
/// 冻结快照 → 决策输入派生共用——蓝图 70.8.1 两侧同点，禁止分叉）。
pub const STATE_BINDINGS: eneros_safety_validator::StateBindings =
    eneros_safety_validator::StateBindings {
        voltage: 1,
        current: 2,
        frequency: 3,
        soc: 4,
    };

/// 输入新鲜度**策略窗**（蓝图 [08] §9：**待实测冻结**的设计输入，不是
/// 性能承诺）。**不是**输入可否用于控制的唯一判据——载荷自身的控制
/// 有效期（`TrustedTelemetry::valid_until_ms` = 产刻 + 控制窗 30s
/// ＝ADR-0054 安全事实，下游 dispatch 以同一字段裁决）是第一上界；
/// 本窗只能更紧，不得更松（放宽到超过载荷有效期只会把"输入过期"变成
/// dispatch 的 `TelemetryExpired`，EA 按 `DispatchRejected` 记账并烧错误
/// 预算）。2026-09-18 R10 复查订正：控制窗恢复 30s 后，本窗从 60s 对齐
/// 收敛到 30s（不得宽于载荷有效期）。
pub const DEFAULT_INPUT_FRESHNESS_MS: u64 = 30_000;

/// 快照输入拒绝（结构化；调用方必须零决策、零新写）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotInputReject {
    /// 无已冻结快照（冷启动/窗口未到/事件面不可用）。
    NoFrozenSnapshot,
    /// 快照不完整（缺必需点）或身份未绑定。
    Incomplete,
    /// 快照超出输入新鲜度窗。
    Stale { age_ms: u64 },
    /// 保留背压（消费者落后，容量压力未释放）。
    Backpressure,
}

/// 冻结快照 → 可信输入（蓝图 70.8.1 唯一派生点）。
///
/// 身份段（snapshot_id/provenance_root/cut/produced/目录版本与 digest）
/// 全部取自快照本体；144B 遥测载荷由**同一快照**的观测派生
/// （`samples` 全点 + `SystemState` 四字段）——两侧同源，不允许调用方
/// 自行拼接。快照不完整时仍产出（完整度码如实标注），由输入侧拒绝。
pub fn input_from_snapshot(
    snap: &eneros_snapshot_service::TrustedSnapshot,
    catalog_version: u64,
    catalog_digest8: [u8; 8],
    horizon_ms: u64,
) -> eneros_agent_sdk::snap_input_wire::TrustedInput {
    use eneros_agent_sdk::snap_input_wire::{SnapshotFacts, TrustedInput, COMPLETE, INCOMPLETE};
    use eneros_telemetry_model::event::ObservationValue;
    let mut samples = alloc::collections::BTreeMap::new();
    let mut max_epoch = 0u64;
    let mut max_mapping = 0u64;
    for o in &snap.observations {
        if let ObservationValue::F64(v) = o.event.value {
            if v.is_finite() {
                samples.insert(o.event.point_id, v);
            }
            max_epoch = max_epoch.max(o.event.source_epoch);
            max_mapping = max_mapping.max(o.event.mapping_version);
        }
    }
    let b = STATE_BINDINGS;
    let state = eneros_safety_validator::SystemState {
        voltage_v: samples.get(&b.voltage).copied().unwrap_or(f64::NAN),
        current_a: samples.get(&b.current).copied().unwrap_or(f64::NAN),
        frequency_hz: samples.get(&b.frequency).copied().unwrap_or(f64::NAN),
        soc_pct: samples.get(&b.soc).copied().unwrap_or(f64::NAN),
        timestamp_ms: snap.cut_time_ns / 1_000_000,
    };
    let produced_ms = snap.produced_at_ns / 1_000_000;
    let trusted = TrustedTelemetry {
        state,
        coverage: eneros_control_dispatch::feedback::safety_coverage(),
        valid_until_ms: produced_ms.saturating_add(horizon_ms),
        // 身份摘要 = 快照 provenance root（比样本 SM3 更强：覆盖来源/序/
        // 事件摘要，且与证书引用同一根）。
        provenance_digest: snap.provenance_root,
        generated_at_ms: produced_ms,
        source_epoch: max_epoch,
        mapping_version: max_mapping,
        samples,
    };
    let payload = encode_trusted(&trusted);
    let mut telemetry = [0u8; eneros_agent_sdk::snap_input_wire::SNAP_INPUT_TELEMETRY_LEN];
    let n = core::cmp::min(telemetry.len(), payload.len());
    telemetry[..n].copy_from_slice(&payload[..n]);
    let c = &snap.completeness;
    let facts = SnapshotFacts {
        snapshot_id: snap.snapshot_id,
        provenance_root: snap.provenance_root,
        cut_time_ns: snap.cut_time_ns,
        produced_at_ns: snap.produced_at_ns,
        catalog_version,
        catalog_digest8,
        accepted: u16::try_from(snap.observations.len()).unwrap_or(u16::MAX),
        missing: u16::try_from(c.missing.len()).unwrap_or(u16::MAX),
        completeness: if c.is_complete() {
            COMPLETE
        } else {
            INCOMPLETE
        },
    };
    TrustedInput::new(facts, telemetry)
}

/// 69.3 WP-69.3.2 T1：safe 评估器配置**单源构造器**（原 svc-telemetry bin
/// `:36-52` 装配初值与 `:217-230` 生产重建两处双抄收编——五项恒同字段
/// （coverage/bindings/max_skew_ms/require_integrity/control_horizon_ms）
/// 单点于此，两处刻意差异字段（mapping_version/allowed_sources）入参：
/// 装配初值 = (1, vec![1])（root 合成设施兼容轨），生产重建 =
/// (generation, vec![])（配置覆盖）。产出与两处原字面量逐字段相同
/// （t_evalcfg_single_source 冻结对照）。
pub fn evaluator_config(mapping_version: u64, allowed_sources: Vec<u32>) -> EvaluatorConfig {
    EvaluatorConfig {
        coverage: eneros_control_dispatch::feedback::safety_coverage(),
        bindings: STATE_BINDINGS,
        mapping_version,
        max_skew_ms: 1000,
        allowed_sources,
        require_integrity: true,
        control_horizon_ms: DEFAULT_CONTROL_HORIZON_MS,
    }
}

/// 69.3 WP-69.3.2 T3：SNAPSHOT 拒绝判定行**词形单源**（原 svc-telemetry
/// bin SNAPSHOT 分支收编，对齐 protocol_ladder `line_*` 模式；词形字面
/// 逐字不变，Z2——t_tel_snapshot_wordform_byte_identical 冻结对照）。
pub fn line_snapshot_rejected(
    e: &TrustRejection,
    ingested: u64,
    now_ms: u64,
    first_age: u64,
) -> alloc::string::String {
    alloc::format!(
        "[svc-telemetry] SNAPSHOT rejected: {e:?} (ingested={ingested} now={now_ms} first_age={first_age})"
    )
}

/// TelemetryService 服务逻辑（host 可测；target 经 EIPC 循环调用）。
pub struct TelemetryService {
    evaluator: TelemetryTrustEvaluator,
    /// 有界样本缓冲（FIFO；满时拒绝新样本——不静默丢）。
    buffer: VecDeque<TelemetrySample>,
    /// 已收样本计数（观测）。
    pub ingested: u64,
    /// 被拒样本计数（去重/满）。
    pub rejected: u64,
    /// WP-2：历史落盘接缝（None=未装配，行为与旧版完全一致）。
    history: Option<Box<dyn TelemetryHistorySink>>,
    /// WP-2：成功写出的历史批次数（观测）。
    pub history_written: u64,
    /// WP-2：历史缓冲满被 DropOldest 丢弃的记录数（观测，不静默）。
    pub history_dropped: u64,
    /// WP-2：sink 写出失败的批次数（观测；不影响 ingest ack）。
    pub history_errors: u64,
    /// SPEC-C：判定行词形由装配介质决定（Ring=既有行为字节不变，
    /// transcript 基线连续性；Disk=TSDB-DISK-* 三态行）。
    history_media: HistoryMedia,
    /// SPEC-C：批次序号（Disk 判定行 batch=<seq>；每次非空 flush 递增）。
    history_batch_seq: u64,
    /// SPEC-C：已落 REJECTED 判定行的拒绝计数高水位（增量行防重复刷屏）。
    rejected_reported: u64,
    /// 蓝图 61：观测事件影子（None = 未装配——旧 bundle 无来源表时如实
    /// 降级；影子轨零控制消费，见 obs_shadow 模块文档）。
    pub obs: Option<crate::obs_shadow::ObsShadow>,
    /// 蓝图 68 卷 68.4：OBS 事件面接缝（None = 未装配——行为与未接线
    /// 版本一致）。失败语义与历史接缝同律：计数/串口行可见，**绝不反向
    /// 影响 ingest ack**（采集与事件面是两条独立生死线）。
    event_face: Option<Box<dyn crate::event_face::ObsEventFace>>,
    /// 蓝图 70.8：输入新鲜度窗（冻结快照 produced_at → now；超龄拒绝）。
    input_freshness_ms: u64,
}

/// SPEC-C：历史介质标签（随 sink 装配指定；影响判定行词形与统计语义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryMedia {
    /// 内存环（缺省；沿用 TSDB-COMMIT/COMMIT-FAIL 两行基线）。
    Ring,
    /// 磁盘 TSDB 区段（D-TSDB-2 方案 A；TSDB-DISK-COMMIT/DROPPED/
    /// REJECTED 三态行）。
    Disk,
}

impl TelemetryService {
    /// 构造（评估器配置）。
    pub fn new(config: EvaluatorConfig) -> Self {
        Self {
            evaluator: TelemetryTrustEvaluator::new(config),
            buffer: VecDeque::new(),
            ingested: 0,
            rejected: 0,
            history: None,
            history_written: 0,
            history_dropped: 0,
            history_errors: 0,
            history_media: HistoryMedia::Ring,
            history_batch_seq: 0,
            rejected_reported: 0,
            obs: None,
            event_face: None,
            input_freshness_ms: DEFAULT_INPUT_FRESHNESS_MS,
        }
    }

    /// 蓝图 61：装配观测影子路径（bin 启动期一次；descriptor 集来自
    /// config `[[source]]` 编译，评估器配置与生产同源注入）。
    pub fn enable_obs_shadow(
        &mut self,
        descriptors: alloc::vec::Vec<eneros_ingestion_core::SourceDescriptor>,
        eval_config: EvaluatorConfig,
    ) {
        self.obs = Some(crate::obs_shadow::ObsShadow::new(descriptors, eval_config));
    }

    /// 蓝图 61 v0.314.27：配置驱动的来源身份生效（惰性——首个 OBS 批次
    /// 到达时由 handler 调用一次）。
    ///
    /// 生产评估器以**同一 EvaluatorConfig** 重建（allowed_sources/mapping_
    /// version 换为配置事实：注册来源 id 集合 + bundle generation），并
    /// 同时装配影子路径。启动早期（配置未装载）评估器仍以装配参数运行
    /// ——qemu-test 合成设施（source=1）与旧 bundle 兼容轨不受影响。
    pub fn apply_source_identity(
        &mut self,
        descriptors: alloc::vec::Vec<eneros_ingestion_core::SourceDescriptor>,
        mut eval_config: EvaluatorConfig,
    ) {
        eval_config.allowed_sources = descriptors
            .iter()
            .map(|d| d.source_id)
            .collect::<alloc::vec::Vec<_>>();
        // 重建生产评估器（启动窗口调用：评估器无积累状态——restore 空）。
        self.evaluator = TelemetryTrustEvaluator::new(eval_config.clone());
        self.enable_obs_shadow(descriptors, eval_config);
    }

    /// 69.3 WP-69.3.2 T2：惰性来源身份装载**单源**（原 svc-telemetry bin
    /// `ensure_source_identity` 96 行收编——落点订正见执行卡 7.1-2：该函数
    /// 依赖 agent-sdk/control-services，ingestion-core 依赖方向不可达，且
    /// 函数零 SourceRegistry 直呼，实为 TelemetryService 装配编排，落本
    /// 库与 [`TelemetryService::apply_source_identity`] 同域）。
    ///
    /// 签名收窄（卷册要求）：bin 局部状态 → 参数/返回值——config 经
    /// [`ConfigEntrySource`] trait 面（bin 传 `ChunkedEndpoint` 的
    /// Option<&mut T>；host 测试传内存假件，免 chunked 状态机）；now_ms
    /// 入参（catalog 事件面模块时基；调用点 handle() 已持有，时钟同源）。
    ///
    /// 返回 true = 身份已装载（一次成功后短路——obs 在场即 true）；
    /// false = 未装载（下一轮 OBS 重试；期间 legacy 兼容轨以装配参数
    /// 运行）。词形逐字保持（Z2：OBS-CFG/CATALOG-*/OBS-CFG loaded 三组）。
    /// cfg 与 fetch_catalog 同口径（catalog 装配域，dispatch_assembly 先例）。
    #[cfg(any(test, feature = "sel4"))]
    pub fn ensure_source_identity<T: ConfigEntrySource>(
        &mut self,
        config: Option<&mut T>,
        now_ms: u64,
    ) -> bool {
        if self.obs.is_some() {
            return true;
        }
        let Some(config) = config else {
            return false;
        };
        let cfg_id = eneros_config_bundle::ConfigId::from_static("points");
        // generation（GET_GENERATION；失败 = 0 → 世代未定，观测不满足控制
        // 资格——fail-closed）。蓝图 67 WP-67.2.3：endpoint trait 面。
        let generation = config.config_generation().unwrap_or(0);
        let Some(payload) = config.config_entry("points").and_then(|resp| {
            eneros_agent_sdk::config_client::decode_response_body(&resp)
                .ok()
                .map(|b| b.to_vec())
        }) else {
            eneros_runtime::println!(
                "[svc-telemetry] OBS-CFG points fetch FAIL (retry next round)"
            );
            return false;
        };
        let Ok(table) = eneros_config_bundle::points::decode_points(&payload, &cfg_id) else {
            eneros_runtime::println!("[svc-telemetry] OBS-CFG points decode FAIL");
            return false;
        };
        if table.sources.is_empty() {
            eneros_runtime::println!("[svc-telemetry] OBS-CFG no [[source]] rows (legacy bundle)");
            return false;
        }
        let descriptors = crate::obs_shadow::descriptors_from_config(&table.sources, &table.points);
        // 生产评估器重建（T1 单源构造器；allowed_sources/mapping_version
        // 换为配置事实：注册来源 id 集合覆盖 + bundle generation）。
        let mut applied_config = evaluator_config(generation, Vec::new());
        // 蓝图 61 v0.314.33：目录装载（安全覆盖/反馈槽单一事实源）。目录
        // 不可得（legacy bundle）→ 以装配参数覆盖运行（兼容轨；生产
        // bundle 必含 catalog 条目）。
        if let Some(cat) = crate::catalog_gateway::fetch_catalog(config) {
            applied_config.coverage = crate::catalog_gateway::coverage_config(&cat.repo);
            let d = cat.repo.digest();
            eneros_runtime::println!(
                "[svc-telemetry] CATALOG-ACTIVE gen={} digest={:02x}{:02x}{:02x}{:02x}",
                cat.generation,
                d[0],
                d[1],
                d[2],
                d[3]
            );
            eneros_runtime::println!(
                "[svc-telemetry] CATALOG-COVERAGE points={}",
                applied_config.coverage.points.len()
            );
            // 蓝图 68 卷 68.4：catalog 交事件面（SnapshotModule + 快照
            // 政策构建；repo 所有权移交，coverage/digest 已先取）。
            self.set_event_face_catalog(cat.repo, now_ms);
        } else {
            eneros_runtime::println!(
                "[svc-telemetry] CATALOG-MISSING —— coverage 走装配参数（legacy 轨，fail-closed 输入不变）"
            );
        }
        self.apply_source_identity(descriptors, applied_config);
        eneros_runtime::println!(
            "[svc-telemetry] OBS-CFG loaded sources={} generation={}",
            table.sources.len(),
            generation
        );
        true
    }

    /// 蓝图 61：OBS 影子批次准入（TEL_INGEST_OBS 服务端；响应
    /// [OK][accepted u16][rejected u16]）。
    pub fn obs_ingest(&mut self, source_id: u32, frames: &[u8], now_ms: u64) -> Vec<u8> {
        // 首段借用（obs 准入）作用域化——生产 upsert 需要独立 self
        //（[07] §2.6.5 生产轨切换）。
        let (accepted, rejected, converted, accepted_events) = {
            let Some(obs) = self.obs.as_mut() else {
                // 影子未装配 = 配置无来源表 → 显式拒绝（不静默吞帧）。
                return alloc::vec![0xFF];
            };
            let mut converted: Vec<TelemetrySample> = Vec::new();
            let mut accepted_events: Vec<ObservationEvent> = Vec::new();
            let (accepted, rejected) = obs.ingest_frames(
                source_id,
                frames,
                now_ms,
                &mut converted,
                &mut accepted_events,
            );
            (accepted, rejected, converted, accepted_events)
        };
        // 蓝图 68 卷 68.4：接受事件本体交事件面（StoreSink 追加 +
        // SnapshotModule 投影；obs 借用域外——事件面持独立 self）。
        // 失败语义与历史接缝同律：只计数/打点，绝不反向影响 ack。
        if let Some(face) = self.event_face.as_mut() {
            face.on_obs_batch(&accepted_events, now_ms);
        }
        // 蓝图 61 v0.314.30（[07] §2.6.5）：telemetry 侧改为 ObservationEvent
        // consumer——OBS 是生产轨。转换样本逐点 upsert 进生产缓冲（同点
        // 新样本替换旧样本；**多源不互相 clear**——旧 INGEST 全量替换语义
        // 只保留在 legacy 迁移 reader 路径）。
        // 蓝图 68.2（F13）：生产评估器 per-source epoch 自管（t1/t2/t4 行
        // 为锁）——原"钉扎→restore(epoch,0,清态) 同步"删除。该同步是
        // "最后钉扎者赢"毒化机制本体：每服务 open() floor 各取 boot
        // now_ms 必不相等，后钉者把评估器单全局 epoch 重置，先钉源样本
        // 全拒（agent=OK 59→0）。
        for sample in converted.iter() {
            self.upsert_sample(sample);
        }
        // 蓝图 68 卷 68.3（WP-68.3.4 装配补全）：批次边界落盘与 legacy
        // TEL_INGEST 同语义——OBS 是生产轨（61 v0.314.30 起），历史落盘
        // 只挂 legacy 路径的旧装配在生产窗里永不触发（vol68-2/vol68-3
        // 实弹：telemetry-ingest=0 全程）。按**本轮接受集**落盘（非整
        // 缓冲——buffer 是 SNAPSHOT 存续面，flush 不清；见
        // commit_history_batch 注记），且在响应前执行：ACK 到达请求方时
        // 批次已过历史接缝（TsdbHistorySink"后方 ACK"语义对 OBS 轨同等
        // 成立）。
        let batch: Vec<HistoryRecord> = converted
            .iter()
            .map(|s| HistoryRecord {
                timestamp_ms: s.measured_at_ms,
                device_id: s.source,
                point: s.point,
                value: s.value,
                quality: quality_code(s.quality),
            })
            .collect();
        self.commit_history_batch(&batch);
        // 蓝图 62 卷 62.2（WP-62.2.2）：响应编码单源 obs_batch_wire
        //（[OK][accepted u16][rejected u16] 布局逐字节不变）。
        encode_obs_response(accepted as u16, rejected as u16).to_vec()
    }

    /// 装配历史落盘接缝（WP-2；服务装配期调用一次；介质=Ring 基线词形）。
    pub fn set_history_sink(&mut self, sink: Box<dyn TelemetryHistorySink>) {
        self.history = Some(sink);
        self.history_media = HistoryMedia::Ring;
    }

    /// SPEC-C：装配磁盘历史接缝（介质=Disk，判定行切换 TSDB-DISK-* 三态）。
    pub fn set_history_sink_with_media(
        &mut self,
        sink: Box<dyn TelemetryHistorySink>,
        media: HistoryMedia,
    ) {
        self.history = Some(sink);
        self.history_media = media;
    }

    /// 蓝图 68 卷 68.4：装配 OBS 事件面接缝（bin 启动期一次；None =
    /// 未装配零开销直通）。
    pub fn set_event_face(&mut self, face: Box<dyn crate::event_face::ObsEventFace>) {
        self.event_face = Some(face);
    }

    /// 蓝图 68 卷 68.4：catalog 到达（首个 OBS 批次惰性装载）后交给事件
    /// 面——SnapshotModule 与快照政策的构建输入（未装配事件面时零操作）。
    pub fn set_event_face_catalog(
        &mut self,
        repo: eneros_semantic_catalog::CatalogRepository,
        now_ms: u64,
    ) {
        if let Some(face) = self.event_face.as_mut() {
            face.set_snapshot_catalog(repo, now_ms);
        }
    }

    /// 批次边界 flush：把当前缓冲（即本轮 ingest 接受集）写往历史接缝。
    ///
    /// 失败语义 fail-visible：错误只计数、不吞掉、也**不反向影响**
    /// ingest 的 ack——采集与历史是两条独立生死线。无 sink 时为零开销
    /// 直通（与未接线版本行为一致）。
    ///
    /// SPEC-C 判定行词形（介质分形）：
    /// - Ring（基线，w7/w10 transcript 连续性）：`TSDB-COMMIT batch items=` /
    ///   `TSDB-COMMIT-FAIL errors=`——逐字节不变；
    /// - Disk（D-TSDB-2 方案 A）：三态 `TSDB-DISK-COMMIT batch=<seq> items=<n>`
    ///   / `TSDB-DISK-DROPPED n=`（磁盘写失败批）/ `TSDB-DISK-REJECTED n=`
    ///   （本轮 ingest 拒绝样本增量）。
    fn flush_history(&mut self) {
        if self.history.is_none() {
            return;
        }
        let batch: Vec<HistoryRecord> = self
            .buffer
            .iter()
            .map(|s| HistoryRecord {
                timestamp_ms: s.measured_at_ms,
                device_id: s.source,
                point: s.point,
                value: s.value,
                quality: quality_code(s.quality),
            })
            .collect();
        self.commit_history_batch(&batch);
    }

    /// 批次提交（flush 的落盘+判定行段；68.3 WP-68.3.4 装配补全抽出——
    /// OBS 生产轨按**本轮接受集**落盘而非整缓冲：buffer 是 SNAPSHOT 的
    /// 当前样本存续面（flush 不清），整缓冲重写会造成同批历史重复）。
    fn commit_history_batch(&mut self, batch: &[HistoryRecord]) {
        let Some(sink) = self.history.as_mut() else {
            return;
        };
        // REJECTED 三态：本轮 ingest 拒绝增量（decode 级拒绝不计入
        // self.rejected，走既有 0xFF 错误帧路径）。
        let rejected_now = self.rejected - self.rejected_reported;
        if rejected_now > 0 && self.history_media == HistoryMedia::Disk {
            eneros_runtime::println!("[svc-telemetry] TSDB-DISK-REJECTED n={}", rejected_now);
        }
        self.rejected_reported = self.rejected;

        if batch.is_empty() {
            return;
        }
        self.history_batch_seq += 1;
        match sink.write_batch(batch) {
            Ok(()) => {
                self.history_written += 1;
                match self.history_media {
                    HistoryMedia::Ring => {
                        // SPEC-GAP-CLOSURE WP-2/C2：QEMU 串口证据行
                        // （grep 连续性纪律）——基线词形原样保留。
                        eneros_runtime::println!(
                            "[svc-telemetry] TSDB-COMMIT batch items={}",
                            batch.len()
                        );
                    }
                    HistoryMedia::Disk => {
                        eneros_runtime::println!(
                            "[svc-telemetry] TSDB-DISK-COMMIT batch={} items={}",
                            self.history_batch_seq,
                            batch.len()
                        );
                    }
                }
            }
            Err(()) => {
                self.history_errors += 1;
                match self.history_media {
                    HistoryMedia::Ring => {
                        eneros_runtime::println!(
                            "[svc-telemetry] TSDB-COMMIT-FAIL errors={}",
                            self.history_errors
                        );
                    }
                    HistoryMedia::Disk => {
                        // 写失败批内记录未落库（热环可能仍持有）——显式计数。
                        self.history_dropped += batch.len() as u64;
                        eneros_runtime::println!(
                            "[svc-telemetry] TSDB-DISK-DROPPED n={}",
                            batch.len()
                        );
                    }
                }
            }
        }
        // DropOldest 计数由具体缓冲实现内部累计后经 here 汇报：
        // 为避免 trait 面膨胀，丢弃统计下沉到 HistoryBuffer 自身字段，
        // 服务侧只计批次级成败（见模块尾设计注记）。
    }

    /// 缓冲首样本 age（诊断：now - measured_at；空 → None）。
    pub fn first_sample_age_ms(&self, now_ms: u64) -> Option<u64> {
        self.buffer
            .front()
            .map(|s| now_ms.saturating_sub(s.measured_at_ms))
    }

    /// 已见最大 per-source epoch（观测面；68.2 起多源语义）。
    pub fn epoch(&self) -> u64 {
        self.evaluator.epoch()
    }

    /// 断线重连处理（TEL_ADVANCE_EPOCH 请求路径；蓝图 68.2 语义收缩：
    /// epoch 推进已由采集侧 `open_source` per-source 分配 + 消费侧
    /// per-source max 吸收承载，本请求降级为清缓冲——丢弃在途旧样本；
    /// wire opcode 保留兼容，生产零发送方）。
    pub fn advance_epoch(&mut self) {
        self.buffer.clear();
    }

    /// 接收一批样本（去重：同点同序号拒绝；满 → 拒绝并计数）。
    pub fn ingest(&mut self, samples: &[TelemetrySample]) -> usize {
        let mut accepted = 0usize;
        for s in samples {
            // 去重：缓冲中已存在同点同序号。
            if self
                .buffer
                .iter()
                .any(|b| b.point == s.point && b.sequence == s.sequence)
            {
                self.rejected += 1;
                continue;
            }
            if self.buffer.len() >= MAX_BUFFERED_SAMPLES {
                self.rejected += 1;
                continue;
            }
            self.buffer.push_back(*s);
            accepted += 1;
        }
        self.ingested += accepted as u64;
        accepted
    }

    /// 清空样本缓冲（新一轮采集周期开始——旧周期样本不参与评估；
    /// QEMU 实弹：多轮样本累积导致同点重复 Replay / 跨周期 skew
    /// ClockNotMonotonic，快照必须原子冻结单周期样本）。
    pub fn clear(&mut self) {
        self.buffer.clear();
    }

    /// 蓝图 61 v0.314.30：OBS 生产轨逐点 upsert。
    ///
    /// 同点 → 新样本替换旧样本（最新事实单点有效）；新点 → 追加；
    /// 缓冲满且为新点 → 淘汰最旧并计数（DropOldest 可观测，不静默）。
    /// 每点保留一份最新样本 = 「单周期单点原子」与「多源不互相 clear」
    /// 的交集语义。
    fn upsert_sample(&mut self, sample: &TelemetrySample) {
        if let Some(slot) = self.buffer.iter_mut().find(|b| b.point == sample.point) {
            *slot = *sample;
            self.ingested += 1;
            return;
        }
        if self.buffer.len() >= MAX_BUFFERED_SAMPLES {
            self.buffer.pop_front();
            self.rejected += 1;
        }
        self.buffer.push_back(*sample);
        self.ingested += 1;
    }

    /// 评估当前缓冲（快照原子冻结：评估器在拷贝上执行，不因并发 ingest
    /// 变化——单线程服务循环内天然原子；provenance 覆盖全部样本）。
    ///
    /// 蓝图 70.8：本路径自此为**差分/迁移轨**（`TEL_REQ_SNAPSHOT`）——
    /// 产物仍带 provenance_digest，但**没有** snapshot_id/目录代次，
    /// 生产决策输入不再消费它（走 [`Self::snapshot_input`]）。
    pub fn snapshot(&mut self, now_ms: u64) -> Result<TrustedTelemetry, TrustRejection> {
        let samples: Vec<TelemetrySample> = self.buffer.iter().copied().collect();
        let t = self.evaluator.evaluate(&samples, now_ms)?;
        // 蓝图 61：生产 verdict 成立后做影子对照（零控制消费——影子
        // 结果只进计数/串口行，不改变本返回值与任何下游）。
        if let Some(obs) = self.obs.as_mut() {
            obs.compare_production(&t);
        }
        Ok(t)
    }

    /// 蓝图 70.8 WP-70.8.1：**可信快照输入**（生产决策输入唯一入口）。
    ///
    /// 身份与载荷都取自事件面最近一次成功构建的冻结快照；不完整、超龄
    /// 或保留背压一律拒绝（零决策、零新写——绝不回退评估器伪造身份）。
    pub fn snapshot_input(
        &mut self,
        now_ms: u64,
    ) -> Result<eneros_agent_sdk::snap_input_wire::TrustedInput, SnapshotInputReject> {
        let Some(input) = self.event_face.as_ref().and_then(|f| f.current_input()) else {
            return Err(SnapshotInputReject::NoFrozenSnapshot);
        };
        if !input.facts().is_complete() {
            return Err(SnapshotInputReject::Incomplete);
        }
        let produced_ms = input.facts().produced_at_ns / 1_000_000;
        let age_ms = now_ms.saturating_sub(produced_ms);
        // **载荷自身的控制有效期是第一上界**（单源 = `valid_until_ms`：
        // 下游 dispatch 以同一字段裁决 `TrustedTelemetry::is_valid_at`）。
        // 超出它的输入在此必须拒绝——否则 dispatch 以 `TelemetryExpired`
        // 拒绝，EA 按 `DispatchRejected` 记账（连续 3 次 → EscalateToHuman
        // + 冷却），把"输入已过期"伪装成"决策被拒"并烧错误预算。
        // 策略窗（`input_freshness_ms`）是**额外**（只可更紧）的上界——
        // 蓝图 §9 的"输入新鲜度"意图由此承担，不得替代载荷有效期。
        let Some(payload) = decode_trusted(input.telemetry()) else {
            // 载荷不可解 = 状态不可信（构造缺陷/介质损坏）——不放行。
            return Err(SnapshotInputReject::Incomplete);
        };
        if now_ms > payload.valid_until_ms || age_ms > self.input_freshness_ms {
            return Err(SnapshotInputReject::Stale { age_ms });
        }
        if self
            .event_face
            .as_ref()
            .map(|f| f.retention_blocked())
            .unwrap_or(false)
        {
            return Err(SnapshotInputReject::Backpressure);
        }
        Ok(input)
    }

    /// 蓝图 70.8.2/70.9.1：按 ID 取回已冻结快照 blob（certifier 独立读
    /// 可信输入的通道；身份不匹配 → None）。
    pub fn snapshot_blob(&mut self, id: &[u8; 16]) -> Option<Vec<u8>> {
        self.event_face.as_mut().and_then(|f| f.snapshot_blob(id))
    }

    /// 蓝图 70.8.2：登记计划输入保留持有（被证书/执行证据引用的记录
    /// 不得先删）。
    pub fn hold_input_offset(&mut self, partition: u32, offset: u64) {
        if let Some(f) = self.event_face.as_mut() {
            f.hold_offset(partition, offset);
        }
    }

    /// ADR-0054:29–30：快照保留钉（被持有计划证书引用的冻结快照在持有
    /// 期内不得被剪枝回收；事件面缺失/容量满 → false = fail-closed 面）。
    pub fn pin_snapshot(&mut self, id: &[u8; 16]) -> bool {
        self.event_face
            .as_mut()
            .map(|f| f.pin_snapshot(id))
            .unwrap_or(false)
    }

    /// ADR-0054:29–30：解除快照保留钉（计划终态且无未决后；幂等）。
    pub fn release_snapshot(&mut self, id: &[u8; 16]) -> bool {
        self.event_face
            .as_mut()
            .map(|f| f.release_snapshot(id))
            .unwrap_or(false)
    }
}

// ============================================================================
// SPEC-GAP-CLOSURE-2026-08-26 · WP-2：历史落盘接缝（host 波）
//
// 磁盘 TSDB（eneros-tsdb）绑定 eneros_fs::Lfs × BlockDevice，而目标镜像中
// 块句柄仅由 svc-storage 持有（41 卷移交先例）——svc-telemetry 的磁盘挂载
// 架构属 owner 裁决项 D-TSDB-2，本波不擅自开辟访问路径。落地形状：
// - [`TelemetryHistorySink`]：批次写接缝（磁盘适配器将来即插即用）；
// - [`HistoryBuffer`]：缺省内存环回放缓冲（容量守恒、DropOldest 可观测），
//   字段与 tsdb 的 TimeSeriesPoint 对齐以便后续直映射。
// 内存纪律（R2）：缺省容量 512 条 ×32B ≈16KiB ≪ 服务堆预算；扩容 MUST
// 先出静态内存审计表。
// ============================================================================

/// 缺省历史容量（条）。
pub const DEFAULT_HISTORY_CAP: usize = 512;

/// 单条历史记录（plain-data；字段语义对齐 eneros-tsdb TimeSeriesPoint）。
///
/// SPEC-C：新增 `quality` 四档码——历史必须如实存品质，Bad→Good 的
/// 有损抹平是伪造。记录体积 24→32 B（对齐填充），热环容量不变，
/// 红线断言（≤32KiB）与审计表 §一#3 同步复核。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HistoryRecord {
    /// 测量时刻（单调毫秒，取样本 measured_at_ms）。
    pub timestamp_ms: u64,
    /// 来源实例 ID（取样本 source；磁盘适配器映射 DeviceId 用）。
    pub device_id: u32,
    /// 统一点 ID。
    pub point: u32,
    /// 采样值。
    pub value: f64,
    /// 品质码（0=Good / 1=Suspect / 2=Bad / 3=Unknown；单一来源 =
    /// agent-sdk `sample_wire::quality_code`，本模块不再自持映射）。
    pub quality: u8,
}

/// 历史落盘接缝：每轮 TEL_INGEST 接受后调用一次 `write_batch`。
///
/// 实现方约定：失败返回 `Err(())` 即可（服务侧计数），不得 panic、不得
/// 阻塞采集路径。磁盘适配器（D-TSDB-2 后）与本内存环均实现本 trait。
pub trait TelemetryHistorySink {
    // 二元失败是接缝契约（服务侧只计成败、不解释原因）；错误类型留空是
    // 刻意为之，非占位——历史路径不向采集链回传细节。
    #[allow(clippy::result_unit_err)]
    fn write_batch(&mut self, batch: &[HistoryRecord]) -> Result<(), ()>;
}

/// 有界历史缓冲（内存环；满时 DropOldest + 计数——不静默，语义对齐
/// drivers/tsdb ring.rs / 22 §2.W3.3）。
pub struct HistoryBuffer {
    buf: VecDeque<HistoryRecord>,
    cap: usize,
    /// 满'后丢弃的记录总数（指标源数据）。
    pub dropped: u64,
}

impl HistoryBuffer {
    /// 新建（cap=0 回退 1——不允许不可用缓冲，同 tsdb ring 纪律）。
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            buf: VecDeque::new(),
            dropped: 0,
        }
    }

    /// 当前记录数。
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// 读回最近 n 条（时间升序；n > len 时给全量）。重启持久性依赖
    /// D-TSDB-2 落地前的进程内排障/对账通道。
    pub fn last(&self, n: usize) -> Vec<HistoryRecord> {
        let start = self.buf.len().saturating_sub(n);
        self.buf.iter().skip(start).copied().collect()
    }
}

impl TelemetryHistorySink for HistoryBuffer {
    fn write_batch(&mut self, batch: &[HistoryRecord]) -> Result<(), ()> {
        for r in batch {
            if self.buf.len() >= self.cap {
                self.buf.pop_front();
                self.dropped += 1;
            }
            self.buf.push_back(*r);
        }
        Ok(())
    }
}

/// TrustedTelemetry → wire（定长 144B：state 4×8 + ts 8 + valid 8 + digest
/// 32 + generated 8 + epoch 8 + mapping 8 + 反馈样本 5×8）。
///
/// 反馈槽键 = [`PROD_FEEDBACK_MAPPING`]（槽序 = 字段声明序，单源；
/// SPEC-SERIES-2026-08-29/R4 WP-R4.1——历史硬编码 5/4/1/6/6 已收编，
/// 需要非生产映射的测试走 [`encode_trusted_with`]）。
pub const TRUSTED_WIRE_LEN: usize = 144;

pub fn encode_trusted(t: &TrustedTelemetry) -> Vec<u8> {
    encode_trusted_with(t, &eneros_control_dispatch::PROD_FEEDBACK_MAPPING)
}

/// 按给定反馈映射编码（槽序 = 映射字段声明序：grid_up, soc, dc_bus_v,
/// overcurrent, overtemp）。
pub fn encode_trusted_with(t: &TrustedTelemetry, m: &FeedbackPointMapping) -> Vec<u8> {
    let mut out = Vec::with_capacity(TRUSTED_WIRE_LEN);
    out.extend_from_slice(&t.state.voltage_v.to_le_bytes());
    out.extend_from_slice(&t.state.current_a.to_le_bytes());
    out.extend_from_slice(&t.state.frequency_hz.to_le_bytes());
    out.extend_from_slice(&t.state.soc_pct.to_le_bytes());
    out.extend_from_slice(&t.state.timestamp_ms.to_le_bytes());
    out.extend_from_slice(&t.valid_until_ms.to_le_bytes());
    out.extend_from_slice(&t.provenance_digest);
    out.extend_from_slice(&t.generated_at_ms.to_le_bytes());
    out.extend_from_slice(&t.source_epoch.to_le_bytes());
    out.extend_from_slice(&t.mapping_version.to_le_bytes());
    // 反馈样本按映射点位键取值（缺失 → NaN = 未覆盖，链按不可信裁决——
    // 服务装配以覆盖为准）。
    out.extend_from_slice(
        &t.samples
            .get(&m.grid_up)
            .copied()
            .unwrap_or(f64::NAN)
            .to_le_bytes(),
    );
    out.extend_from_slice(
        &t.samples
            .get(&m.soc)
            .copied()
            .unwrap_or(f64::NAN)
            .to_le_bytes(),
    );
    out.extend_from_slice(
        &t.samples
            .get(&m.dc_bus_v)
            .copied()
            .unwrap_or(f64::NAN)
            .to_le_bytes(),
    );
    out.extend_from_slice(
        &t.samples
            .get(&m.overcurrent)
            .copied()
            .unwrap_or(f64::NAN)
            .to_le_bytes(),
    );
    out.extend_from_slice(
        &t.samples
            .get(&m.overtemp)
            .copied()
            .unwrap_or(f64::NAN)
            .to_le_bytes(),
    );
    out
}

/// wire → TrustedTelemetry（长度不符 → None）。
///
/// 反馈槽按 [`PROD_FEEDBACK_MAPPING`] 键独立回填——键相异时 oc/ot 各自
/// 保留（体检 A3 修复：历史实现 ot 无条件覆盖 oc 的键）。61 复查矫正后
/// 生产键 = 6/7（独立过温点），别名不复存在。非生产映射走
/// [`decode_trusted_with`]。
pub fn decode_trusted(buf: &[u8]) -> Option<TrustedTelemetry> {
    decode_trusted_with(buf, &eneros_control_dispatch::PROD_FEEDBACK_MAPPING)
}

/// 按给定反馈映射解码（槽序 = 映射字段声明序，与 [`encode_trusted_with`] 对偶）。
pub fn decode_trusted_with(buf: &[u8], m: &FeedbackPointMapping) -> Option<TrustedTelemetry> {
    use eneros_safety_validator::RequiredPointCoverage;
    if buf.len() != TRUSTED_WIRE_LEN {
        return None;
    }
    let mut off = 0usize;
    let take8 = |off: &mut usize| -> Option<f64> {
        let v = f64::from_le_bytes(buf[*off..*off + 8].try_into().ok()?);
        *off += 8;
        Some(v)
    };
    let voltage = take8(&mut off)?;
    let current = take8(&mut off)?;
    let freq = take8(&mut off)?;
    let soc = take8(&mut off)?;
    let ts = u64::from_le_bytes(buf[off..off + 8].try_into().ok()?);
    off += 8;
    let valid_until = u64::from_le_bytes(buf[off..off + 8].try_into().ok()?);
    off += 8;
    let mut digest = [0u8; 32];
    digest.copy_from_slice(&buf[off..off + 32]);
    off += 32;
    let generated = u64::from_le_bytes(buf[off..off + 8].try_into().ok()?);
    off += 8;
    let epoch = u64::from_le_bytes(buf[off..off + 8].try_into().ok()?);
    off += 8;
    let mapping = u64::from_le_bytes(buf[off..off + 8].try_into().ok()?);
    off += 8;
    // 反馈样本（NaN = 未覆盖）——按映射键独立回填。
    let mut samples = alloc::collections::BTreeMap::new();
    let read_fb = |off: &mut usize| -> Option<f64> {
        let v = f64::from_le_bytes(buf[*off..*off + 8].try_into().ok()?);
        *off += 8;
        Some(v)
    };
    let grid = read_fb(&mut off)?;
    let soc_val = read_fb(&mut off)?;
    let dc = read_fb(&mut off)?;
    let oc = read_fb(&mut off)?;
    let ot = read_fb(&mut off)?;
    for (p, v) in [
        (m.grid_up, grid),
        (m.soc, soc_val),
        (m.dc_bus_v, dc),
        (m.overcurrent, oc),
        (m.overtemp, ot),
    ] {
        if v.is_finite() {
            samples.insert(p, v);
        }
    }
    Some(TrustedTelemetry {
        state: eneros_safety_validator::SystemState::new(voltage, current, freq, soc, ts),
        coverage: RequiredPointCoverage::empty(),
        valid_until_ms: valid_until,
        provenance_digest: digest,
        generated_at_ms: generated,
        source_epoch: epoch,
        mapping_version: mapping,
        samples,
    })
}

/// 服务请求处理（EIPC handler 入口）。
pub fn handle_request(svc: &mut TelemetryService, req: &[u8], now_ms: u64) -> Vec<u8> {
    let Some(&code) = req.first() else {
        return vec![0xFF]; // 空请求 → 明确错误
    };
    match code {
        TEL_INGEST => {
            let body = &req[1..];
            let mut samples = Vec::new();
            let mut ok = true;
            for chunk in body.chunks(SAMPLE_WIRE_LEN) {
                match decode_sample(chunk) {
                    Some(s) => samples.push(s),
                    None => {
                        ok = false;
                        break;
                    }
                }
            }
            if !ok {
                return vec![eneros_agent_sdk::tel_wire::TEL_RESP_ERR];
            }
            // INGEST = 新采集周期全量替换（QEMU 实弹：累积多轮样本导致
            // 快照评估同点重复 Replay/跨周期 ClockNotMonotonic）。
            svc.clear();
            let accepted = svc.ingest(&samples);
            // 蓝图 66 WP-66.5.4：accepted ≤255 为结构性事实（chunked
            // 逻辑上限 1664B / 样本 58B → 单批 ≤28，tel_wire 模块 doc
            // 登记核查结论）——无静默回绕风险，debug_assert 锁事实。
            debug_assert!(accepted <= 255, "accepted 越单批结构性上限（≤28）");
            // WP-2（SPEC-GAP-CLOSURE）：批次边界落历史——磁盘 TSDB 适配器
            // 待 D-TSDB-2 架构裁决（块句柄现仅 svc-storage 持有），当前以
            // HistoryBuffer 内存环为缺省实现。
            svc.flush_history();
            eneros_agent_sdk::tel_wire::encode_ingest_ack(accepted as u8)
        }
        TEL_SNAPSHOT => match svc.snapshot(now_ms) {
            Ok(t) => {
                let mut out = vec![SNAPSHOT_OK];
                out.extend_from_slice(&encode_trusted(&t));
                out
            }
            Err(e) => {
                // 69.3 T3：拒绝判定行收编（原 svc-telemetry bin SNAPSHOT
                // twin 分支——词形逐字不变，bin 分支删除改走本单源路径）。
                let age = svc.first_sample_age_ms(now_ms).unwrap_or(u64::MAX);
                eneros_runtime::println!(
                    "{}",
                    line_snapshot_rejected(&e, svc.ingested, now_ms, age)
                );
                vec![SNAPSHOT_REJECTED]
            }
        },
        TEL_ADVANCE_EPOCH => {
            svc.advance_epoch();
            vec![0]
        }
        // 蓝图 70.8 WP-70.8.1：可信快照输入（生产决策输入唯一入口）。
        eneros_agent_sdk::tel_wire::TEL_REQ_SNAPSHOT_INPUT => match svc.snapshot_input(now_ms) {
            Ok(input) => {
                let mut out = vec![SNAPSHOT_OK];
                out.extend_from_slice(&input.encode());
                out
            }
            Err(e) => {
                let code = match e {
                    SnapshotInputReject::NoFrozenSnapshot => {
                        eneros_agent_sdk::snap_input_wire::SNAP_REJECT_NO_FROZEN
                    }
                    SnapshotInputReject::Incomplete => {
                        eneros_agent_sdk::snap_input_wire::SNAP_REJECT_INCOMPLETE
                    }
                    SnapshotInputReject::Stale { .. } => {
                        eneros_agent_sdk::snap_input_wire::SNAP_REJECT_STALE
                    }
                    SnapshotInputReject::Backpressure => {
                        eneros_agent_sdk::snap_input_wire::SNAP_REJECT_BACKPRESSURE
                    }
                };
                eneros_runtime::println!(
                    "[svc-telemetry] SNAPSHOT-INPUT-REJECT reason={} now={}",
                    eneros_agent_sdk::snap_input_wire::reject_word(code),
                    now_ms
                );
                eneros_agent_sdk::snap_input_wire::encode_reject(code)
            }
        },
        // 蓝图 70.8.2/70.9.1：已冻结快照 blob（certifier 独立读取通道）。
        // 蓝图 76 WP-76.2.7：`[op, id16]` 帧解析单源 =
        // `tel_wire::parse_snap_request`（三臂共用长度门与 id 偏移；
        // 应答形状各臂保形不并入）。
        eneros_agent_sdk::tel_wire::TEL_REQ_SNAPSHOT_BLOB => {
            let Some(id) = eneros_agent_sdk::tel_wire::parse_snap_request(req) else {
                return vec![eneros_agent_sdk::tel_wire::TEL_RESP_ERR];
            };
            match svc.snapshot_blob(&id) {
                Some(bytes) => {
                    let mut out = vec![SNAPSHOT_OK];
                    out.extend_from_slice(&bytes);
                    out
                }
                None => vec![SNAPSHOT_REJECTED],
            }
        }
        // ADR-0054:29–30：快照保留钉（dispatch 持有面）。请求 [码, id16]；
        // hold 失败 = [TEL_RESP_ERR]（调用方 fail-closed 拒绝新计划准入）。
        eneros_agent_sdk::tel_wire::TEL_REQ_SNAP_HOLD => {
            let Some(id) = eneros_agent_sdk::tel_wire::parse_snap_request(req) else {
                return vec![eneros_agent_sdk::tel_wire::TEL_RESP_ERR];
            };
            if svc.pin_snapshot(&id) {
                vec![eneros_agent_sdk::tel_wire::TEL_RESP_OK]
            } else {
                vec![eneros_agent_sdk::tel_wire::TEL_RESP_ERR]
            }
        }
        eneros_agent_sdk::tel_wire::TEL_REQ_SNAP_RELEASE => {
            let Some(id) = eneros_agent_sdk::tel_wire::parse_snap_request(req) else {
                return vec![eneros_agent_sdk::tel_wire::TEL_RESP_ERR];
            };
            let _ = svc.release_snapshot(&id);
            vec![eneros_agent_sdk::tel_wire::TEL_RESP_OK]
        }
        TEL_INGEST_OBS => {
            // 蓝图 61 v0.314.27；蓝图 62 卷 62.2（WP-62.2.2）：请求解析
            // 单源 agent-sdk obs_batch_wire（布局校验失败 → 0xFF 拒绝语义不变）。
            match decode_obs_request(req) {
                Ok((source_id, frames)) => svc.obs_ingest(source_id, frames, now_ms),
                Err(_) => vec![eneros_agent_sdk::tel_wire::TEL_RESP_ERR],
            }
        }
        _ => vec![eneros_agent_sdk::tel_wire::TEL_RESP_ERR],
    }
}

#[cfg(test)]
mod tests {
    use eneros_safety_validator::{
        EvaluatorConfig, IntegrityStatus, RequiredPoint, RequiredPointCoverage, SampleQuality,
        StateBindings,
    };

    use super::*;

    /// 事件面替身：只承载 `snapshot_input` 依赖的两个读面（当前冻结输入 +
    /// 保留背压）——其余面（OBS/目录/blob/持有）在本测为无操作。
    struct StubFace {
        input: Option<eneros_agent_sdk::snap_input_wire::TrustedInput>,
        blocked: bool,
    }

    impl crate::event_face::ObsEventFace for StubFace {
        fn on_obs_batch(
            &mut self,
            _events: &[eneros_telemetry_model::event::ObservationEvent],
            _now_ms: u64,
        ) {
        }
        fn set_snapshot_catalog(
            &mut self,
            _repo: eneros_semantic_catalog::CatalogRepository,
            _now_ms: u64,
        ) {
        }
        fn current_input(&self) -> Option<eneros_agent_sdk::snap_input_wire::TrustedInput> {
            self.input
        }
        fn snapshot_blob(&mut self, _id: &[u8; 16]) -> Option<Vec<u8>> {
            None
        }
        fn hold_offset(&mut self, _partition: u32, _offset: u64) {}
        fn pin_snapshot(&mut self, _id: &[u8; 16]) -> bool {
            false
        }
        fn release_snapshot(&mut self, _id: &[u8; 16]) -> bool {
            false
        }
        fn retention_blocked(&self) -> bool {
            self.blocked
        }
    }

    /// 冻结输入构造（payload 有效期显式给定——单源 = `valid_until_ms`）。
    fn frozen_input(
        produced_ms: u64,
        valid_until_ms: u64,
    ) -> eneros_agent_sdk::snap_input_wire::TrustedInput {
        use eneros_agent_sdk::snap_input_wire::{
            SnapshotFacts, TrustedInput, COMPLETE, SNAP_INPUT_TELEMETRY_LEN,
        };
        let mut t = crate::ai_worker::tests_helpers::minimal_trusted_at(produced_ms);
        t.valid_until_ms = valid_until_ms;
        let payload = encode_trusted(&t);
        let mut telemetry = [0u8; SNAP_INPUT_TELEMETRY_LEN];
        telemetry.copy_from_slice(&payload);
        TrustedInput::new(
            SnapshotFacts {
                snapshot_id: [0x5C; 16],
                provenance_root: t.provenance_digest,
                cut_time_ns: produced_ms.saturating_mul(1_000_000),
                produced_at_ns: produced_ms.saturating_mul(1_000_000),
                catalog_version: 1,
                catalog_digest8: [1, 2, 3, 4, 5, 6, 7, 8],
                accepted: 7,
                missing: 0,
                completeness: COMPLETE,
            },
            telemetry,
        )
    }

    fn service_with_input(
        input: eneros_agent_sdk::snap_input_wire::TrustedInput,
    ) -> TelemetryService {
        let mut svc = service();
        svc.set_event_face(alloc::boxed::Box::new(StubFace {
            input: Some(input),
            blocked: false,
        }));
        svc
    }

    /// 复查修复：**输入新鲜度必须以载荷自身的控制有效期为准**（单源 =
    /// `TrustedTelemetry::valid_until_ms`，dispatch 端以同一字段裁决）。
    ///
    /// 修复前：策略窗宽于载荷有效期 → 超龄输入在此被受理，而后由
    /// dispatch 以 `TelemetryExpired` 拒绝，EA 按 `DispatchRejected`
    /// 记账（连续 3 次 → EscalateToHuman + 30s 冷却，见
    /// `TickPolicy::record`）——把"输入已过期"伪装成"决策被拒"并烧掉
    /// 错误预算。修复后：同一形态在输入面即以 `Stale` 诚实拒绝（
    /// `NoTrustedInput` 路径：零推理、零新写、不计失败）。
    ///
    /// 2026-09-18 R10 复查：控制窗恢复 30s（ADR-0054:34–35），策略窗
    /// 对齐 30s——超 30s 龄的输入必须在此拒绝。
    ///
    /// 变异靶：新鲜度判定只看策略窗（忽略载荷有效期）→ ② 变绿 → 本测红。
    #[test]
    #[allow(clippy::disallowed_macros)] // 负例断言用 panic!（仓例先例）
    fn snapshot_input_rejects_beyond_payload_validity() {
        let produced = 1_700_000_000_000u64;
        let valid_until = produced + DEFAULT_CONTROL_HORIZON_MS; // 控制窗内有效
        let mut svc = service_with_input(frozen_input(produced, valid_until));

        // ① 载荷仍有效（龄 25s < 30s 控制窗 = ADR-0054 安全事实）→ 受理。
        assert!(svc.snapshot_input(produced + 25_000).is_ok());

        // ② 载荷有效期比策略窗更紧时必须先拒（两侧上界都生效：
        // min(策略窗 30s, 载荷 20s)——"载荷更紧"由显式短有效期载荷承载）。
        let mut tighter = service_with_input(frozen_input(produced, produced + 20_000));
        match tighter.snapshot_input(produced + 25_000) {
            Err(SnapshotInputReject::Stale { age_ms }) => assert_eq!(age_ms, 25_000),
            other => panic!("超出载荷有效期必须 Stale（不得受理给 dispatch 拒）: {other:?}"),
        }
        // ②b 载荷 30s 窗自身到期（龄 40s > 30s ≥ 策略窗）→ 同样 Stale。
        match svc.snapshot_input(produced + 40_000) {
            Err(SnapshotInputReject::Stale { age_ms }) => assert_eq!(age_ms, 40_000),
            other => panic!("超出载荷有效期必须 Stale: {other:?}"),
        }

        // ③ 策略窗更紧时同样拒绝（两侧上界都生效：min(策略窗, 载荷有效期)）。
        let mut narrow = service_with_input(frozen_input(produced, produced + 120_000));
        narrow.input_freshness_ms = 10_000;
        match narrow.snapshot_input(produced + 20_000) {
            Err(SnapshotInputReject::Stale { .. }) => {}
            other => panic!("策略窗内超出必须 Stale: {other:?}"),
        }
    }

    fn service() -> TelemetryService {
        TelemetryService::new(EvaluatorConfig {
            coverage: RequiredPointCoverage {
                points: vec![
                    RequiredPoint {
                        point: 1,
                        min: 0.0,
                        max: 1000.0,
                        max_age_ms: 5000,
                        unit: eneros_safety_validator::PointUnit::Volt,
                    },
                    RequiredPoint {
                        point: 2,
                        min: -1000.0,
                        max: 1000.0,
                        max_age_ms: 5000,
                        unit: eneros_safety_validator::PointUnit::Ampere,
                    },
                    RequiredPoint {
                        point: 3,
                        min: 45.0,
                        max: 55.0,
                        max_age_ms: 5000,
                        unit: eneros_safety_validator::PointUnit::Hertz,
                    },
                    RequiredPoint {
                        point: 4,
                        min: 0.0,
                        max: 1.0,
                        max_age_ms: 5000,
                        unit: eneros_safety_validator::PointUnit::Ratio,
                    },
                    RequiredPoint {
                        point: 5,
                        min: 0.0,
                        max: 1.0,
                        max_age_ms: 5000,
                        unit: eneros_safety_validator::PointUnit::Other,
                    },
                    RequiredPoint {
                        point: 6,
                        min: 0.0,
                        max: 1.0,
                        max_age_ms: 5000,
                        unit: eneros_safety_validator::PointUnit::Other,
                    },
                ],
            },
            bindings: StateBindings {
                voltage: 1,
                current: 2,
                frequency: 3,
                soc: 4,
            },
            mapping_version: 1,
            max_skew_ms: 1000,
            allowed_sources: vec![1],
            require_integrity: true,
            control_horizon_ms: 30_000,
        })
    }

    /// 复查矫正测试装配：coverage = 6 点 + 独立过温点 7（镜像生产
    /// `safety_coverage()` 7 点新现实；评估器只放行必需点）。
    fn service_with_point7() -> TelemetryService {
        // coverage 重建（evaluator 私有）——以完整 config 新建服务。
        TelemetryService::new(EvaluatorConfig {
            coverage: RequiredPointCoverage {
                points: vec![
                    rp(1, eneros_safety_validator::PointUnit::Volt),
                    rp(2, eneros_safety_validator::PointUnit::Ampere),
                    rp(3, eneros_safety_validator::PointUnit::Hertz),
                    rp(4, eneros_safety_validator::PointUnit::Ratio),
                    rp(5, eneros_safety_validator::PointUnit::Other),
                    rp(6, eneros_safety_validator::PointUnit::Other),
                    rp(7, eneros_safety_validator::PointUnit::Other),
                ],
            },
            bindings: StateBindings {
                voltage: 1,
                current: 2,
                frequency: 3,
                soc: 4,
            },
            mapping_version: 1,
            max_skew_ms: 1000,
            allowed_sources: vec![1],
            require_integrity: true,
            control_horizon_ms: 30_000,
        })
    }

    fn rp(point: u32, unit: eneros_safety_validator::PointUnit) -> RequiredPoint {
        let max = match point {
            1 | 2 => 1000.0,
            3 => 55.0,
            _ => 1.0,
        };
        RequiredPoint {
            point,
            min: 0.0,
            max,
            max_age_ms: 5000,
            unit,
        }
    }

    fn good(point: u32, value: f64, seq: u64, now: u64) -> TelemetrySample {
        TelemetrySample {
            point,
            value,
            source: 1,
            source_epoch: 0,
            measured_at_ms: now - 50,
            received_at_ms: now,
            quality: SampleQuality::Good,
            sequence: seq,
            integrity: IntegrityStatus::Verified,
            mapping_version: 1,
        }
    }

    fn full(now: u64) -> Vec<TelemetrySample> {
        vec![
            good(1, 380.0, 1, now),
            good(2, 10.0, 1, now),
            good(3, 50.0, 1, now),
            good(4, 0.6, 1, now),
            good(5, 1.0, 1, now),
            good(6, 0.0, 1, now),
        ]
    }

    #[test]
    fn ingest_snapshot_roundtrip() {
        let mut svc = service();
        let now = 10_000;
        let accepted = svc.ingest(&full(now));
        assert_eq!(accepted, 6);
        assert_eq!(svc.ingested, 6);
        let t = svc.snapshot(now).expect("trusted");
        assert_eq!(t.state.voltage_v, 380.0);
        assert_eq!(t.state.soc_pct, 0.6);
        assert!(t.is_valid_at(now + 29_999));
    }

    #[test]
    fn duplicate_sample_rejected() {
        let mut svc = service();
        let now = 10_000;
        svc.ingest(&full(now));
        // 同点同序号重放 → 拒绝（去重）。
        let dup = vec![good(1, 999.0, 1, now)];
        assert_eq!(svc.ingest(&dup), 0);
        assert_eq!(svc.rejected, 1);
    }

    #[test]
    fn epoch_lifecycle_per_source_and_advance_buffer_only() {
        // 68.2 per-source 语义（替代旧 advance_epoch 推进全局 epoch）：
        // 同源前跳（重连新 epoch）放行且推进观测面；旧 epoch = 同源回退
        // 结构化拒绝；advance_epoch 收缩为清缓冲（在途旧样本丢弃——
        // legacy INGEST 缓冲为追加式，轮间以此隔离接收时刻基线）。
        let mut svc = service();
        let now = 10_000;
        svc.ingest(&full(now));
        let _ = svc.snapshot(now).expect("首评（提交 per-source max=0）");
        assert_eq!(svc.epoch(), 0);
        // 清缓冲 + 同源前跳（断线重连新 epoch=1）接受。
        svc.advance_epoch();
        let mut s = full(20_000);
        for x in s.iter_mut() {
            x.source_epoch = 1;
            x.sequence = 2;
        }
        svc.ingest(&s);
        let t = svc.snapshot(20_000).expect("新 epoch 放行");
        assert_eq!(t.source_epoch, 1);
        assert_eq!(svc.epoch(), 1);
        // 清缓冲 + 旧 epoch 样本（同源回退）→ 评估拒绝（结构化）。
        svc.advance_epoch();
        let mut old = full(30_000);
        for x in old.iter_mut() {
            x.sequence = 100;
        }
        svc.ingest(&old);
        assert!(matches!(
            svc.snapshot(30_000),
            Err(TrustRejection::SourceEpochMismatch { .. })
        ));
        // advance_epoch 收缩面：清缓冲（在途旧样本丢弃）→ 无样本拒绝。
        svc.advance_epoch();
        assert!(matches!(
            svc.snapshot(40_000),
            Err(TrustRejection::NoSamples)
        ));
    }

    #[test]
    fn sample_wire_roundtrip() {
        let s = good(7, 12.5, 3, 1000);
        let buf = encode_sample(&s);
        let d = decode_sample(&buf).expect("decode");
        assert_eq!(d.point, 7);
        assert_eq!(d.value, 12.5);
        assert_eq!(d.sequence, 3);
        assert_eq!(d.quality, SampleQuality::Good);
        assert_eq!(d.integrity, IntegrityStatus::Verified);
    }

    #[test]
    fn trusted_wire_roundtrip() {
        let mut svc = service();
        let now = 10_000;
        svc.ingest(&full(now));
        let t = svc.snapshot(now).expect("trusted");
        let buf = encode_trusted(&t);
        let d = decode_trusted(&buf).expect("decode");
        assert_eq!(d.state.voltage_v, 380.0);
        assert_eq!(d.valid_until_ms, t.valid_until_ms);
        assert_eq!(d.provenance_digest, t.provenance_digest);
    }

    /// SPEC-R4 WP-R4.1 判据 C1：体检 A3 称"oc/ot 独立往返今天没有任何
    /// 测试能表达它"——映射驱动编解码后可表达。相异键映射下五个反馈
    /// 槽位独立往返保留（历史实现 ot 无条件覆盖 oc 的键，此场景必丢）。
    #[test]
    fn feedback_slots_roundtrip_independent() {
        let m = FeedbackPointMapping {
            grid_up: 5,
            soc: 4,
            dc_bus_v: 1,
            overcurrent: 8,
            overtemp: 9,
        };
        let t = TrustedTelemetry {
            state: eneros_safety_validator::SystemState::new(380.0, 10.0, 50.0, 60.0, 9_000),
            coverage: RequiredPointCoverage::empty(),
            valid_until_ms: 11_000,
            provenance_digest: [0x5A; 32],
            generated_at_ms: 9_500,
            source_epoch: 3,
            mapping_version: 1,
            samples: {
                let mut s = alloc::collections::BTreeMap::new();
                s.insert(5u32, 1.0);
                s.insert(4, 0.6);
                s.insert(1, 400.0);
                s.insert(8, 1.0);
                s.insert(9, 0.0);
                s
            },
        };
        let buf = encode_trusted_with(&t, &m);
        assert_eq!(buf.len(), TRUSTED_WIRE_LEN);
        let d = decode_trusted_with(&buf, &m).expect("decode");
        assert_eq!(d.samples.get(&8), Some(&1.0), "overcurrent 槽独立保留");
        assert_eq!(d.samples.get(&9), Some(&0.0), "overtemp 槽独立保留");
        assert_eq!(d.samples.get(&5), Some(&1.0));
        assert_eq!(d.samples.get(&4), Some(&0.6));
        assert_eq!(d.samples.get(&1), Some(&400.0));
    }

    /// SPEC-R4 WP-R4.1 判据：生产映射值冻结（5/4/1/6/7）。61 复查矫正：
    /// oc/ot 独立点位（点 6 过流 / 点 7 过温 = peer 0x0105/0x0106），
    /// wire 上两槽各自承载、独立往返；缺失/不可信仍走 HZ-THERM-001
    /// 拒充（fail-closed 不变）。
    #[test]
    fn prod_mapping_slots_independent_on_wire() {
        let m = eneros_control_dispatch::PROD_FEEDBACK_MAPPING;
        assert_eq!(m.grid_up, 5);
        assert_eq!(m.soc, 4);
        assert_eq!(m.dc_bus_v, 1);
        assert_eq!(m.overcurrent, 6);
        assert_eq!(m.overtemp, 7, "overtemp 必须绑定独立过温点 7（0x0106）");
        assert_ne!(
            m.overcurrent, m.overtemp,
            "过流/过温不得再共享点位（A3 别名已解除）"
        );
        // 本测试专用装配：coverage 镜像生产 safety_coverage()（7 点，含
        // 独立过温点 7）——评估器忽略非必需点，点 7 必须在覆盖表内才能
        // 进入受信快照（生产由 svc-telemetry safety_coverage() 保证）。
        let mut svc = service_with_point7();
        let now = 10_000;
        let mut samples = full(now);
        samples.push(good(7, 1.0, 1, now)); // 独立过温告警置位
        svc.ingest(&samples);
        let t = svc.snapshot(now).expect("trusted");
        let buf = encode_trusted(&t);
        // 反馈段自偏移 104 起：oc 槽 [128..136] 与 ot 槽 [136..144] 独立——
        // 分别承载点 6（0.0）与点 7（1.0）。
        assert_ne!(
            buf[128..136],
            buf[136..144],
            "oc/ot 槽独立承载（A3 别名已解除）"
        );
        let d = decode_trusted(&buf).expect("decode");
        assert_eq!(d.samples.get(&6), Some(&0.0), "过流值经 oc 槽往返保留");
        assert_eq!(d.samples.get(&7), Some(&1.0), "过温值经 ot 槽独立往返");
    }

    #[test]
    fn request_handler_routes() {
        let mut svc = service();
        let now = 10_000;
        // INGEST。
        let mut req = vec![TEL_INGEST];
        for s in full(now) {
            req.extend_from_slice(&encode_sample(&s));
        }
        let r = handle_request(&mut svc, &req, now);
        assert_eq!(r, vec![0, 6]);
        // SNAPSHOT。
        let r = handle_request(&mut svc, &[TEL_SNAPSHOT], now);
        assert_eq!(r[0], SNAPSHOT_OK);
        assert_eq!(r.len(), 1 + TRUSTED_WIRE_LEN);
        // 空请求 → 错误码。
        assert_eq!(handle_request(&mut svc, &[], now), vec![0xFF]);
    }

    // ===== SPEC-GAP-CLOSURE-2026-08-26 · WP-2 host 波判据 =====

    use super::{HistoryBuffer, HistoryRecord, TelemetryHistorySink, DEFAULT_HISTORY_CAP};

    /// 构造样本（field 布局对齐 safety-validator::TelemetrySample）。
    fn sample(point: u32, value: f64, ts_ms: u64) -> TelemetrySample {
        TelemetrySample {
            point,
            value,
            source: 7,
            source_epoch: 0,
            measured_at_ms: ts_ms,
            received_at_ms: ts_ms + 1,
            quality: eneros_safety_validator::SampleQuality::Good,
            sequence: 0,
            integrity: eneros_safety_validator::IntegrityStatus::Verified,
            mapping_version: 0,
        }
    }

    fn rec(id: u64) -> HistoryRecord {
        HistoryRecord {
            timestamp_ms: id * 100,
            device_id: 7,
            point: id as u32,
            value: id as f64,
            quality: 0,
        }
    }

    /// WP-2/C1-a：批次边界 flush——ingest 后当前缓冲整批写往 sink，
    /// 字段逐项映射正确（point/value/measured_at→timestamp/source→device）。
    #[test]
    fn wp2_ingest_flush_routes_batch_to_sink() {
        use alloc::rc::Rc;
        use core::cell::RefCell;

        #[derive(Default)]
        struct ProbeSink(Rc<RefCell<Vec<HistoryRecord>>>);
        impl TelemetryHistorySink for ProbeSink {
            fn write_batch(&mut self, b: &[HistoryRecord]) -> Result<(), ()> {
                self.0.borrow_mut().extend_from_slice(b);
                Ok(())
            }
        }

        let captured = Rc::new(RefCell::new(Vec::new()));
        let mut svc = service();
        svc.set_history_sink(Box::new(ProbeSink(captured.clone())));
        let batch = [
            sample(1, 11.5, 1_000),
            sample(4, 0.9, 1_100),
            sample(5, 22.0, 1_200),
        ];
        svc.clear();
        let accepted = svc.ingest(&batch);
        assert_eq!(accepted, 3);
        svc.flush_history();

        assert_eq!(svc.history_written, 1);
        assert_eq!(svc.history_errors, 0);
        let got = captured.borrow().clone();
        assert_eq!(got.len(), 3);
        let want: Vec<(u64, u32, u32, u64)> =
            [(1_000, 7, 1, 11), (1_100, 7, 4, 12), (1_200, 7, 5, 22)]
                .iter()
                .map(|&(ts, dev, pt, _)| (ts, dev, pt, pt as u64))
                .collect();
        for (r, &(ts, dev, pt, _)) in got.iter().zip(want.iter()) {
            assert_eq!((r.timestamp_ms, r.device_id, r.point), (ts, dev, pt));
        }
        assert_eq!(got[0].value, 11.5);
        assert_eq!(got[1].value, 0.9);
        assert_eq!(got[2].value, 22.0);
    }

    /// WP-2/C1-b：ring 满 → DropOldest + 计数可见；读回升序一致。
    #[test]
    fn wp2_history_ring_dropoldest_visible_and_readback_consistent() {
        let mut ring = HistoryBuffer::new(4);
        for i in 1..=6u64 {
            let _ = ring.write_batch(&[rec(i)]);
        }
        assert_eq!(ring.len(), 4);
        assert_eq!(ring.dropped, 2, "DropOldest 必须计数（不静默）");
        let got = ring.last(10);
        let want: Vec<u32> = (3..=6).map(|v| v as u32).collect();
        assert_eq!(
            got.iter().map(|r| r.point).collect::<Vec<_>>(),
            want,
            "读回 = 最近 4 条且时间升序"
        );
        // cap=0 回退 1（不允许不可用缓冲）。
        let mut zero = HistoryBuffer::new(0);
        zero.write_batch(&[rec(1)]).unwrap();
        assert_eq!(zero.len(), 1);
    }

    /// SPEC-C（WP-C.1/C2）：Disk 介质的判定行三态与批次序号可观测——
    /// 判定行文本在 target 串口/broker transcript 上验证（R9）；host 侧
    /// 锁定驱动其内容的内部事实：seq 单调、DROPPED 计数等于写失败批长
    /// 累计、REJECTED 增量只报一次。
    #[test]
    fn spec_c_disk_media_line_facts() {
        // 构造写失败 sink（模拟磁盘不可达）：链路层已保证热环先写成功，
        // 磁盘结果如实上抛由服务侧落 DROPPED 行并累计 history_dropped。
        struct FailSink;
        impl TelemetryHistorySink for FailSink {
            fn write_batch(&mut self, _b: &[HistoryRecord]) -> Result<(), ()> {
                Err(())
            }
        }
        let mut svc = service();
        svc.set_history_sink_with_media(Box::new(FailSink), HistoryMedia::Disk);
        let now = 10_000;
        svc.clear();
        svc.ingest(&[sample(1, 11.0, now - 10)]);
        svc.flush_history();
        assert_eq!(svc.history_errors, 1);
        assert_eq!(svc.history_dropped, 1, "DROPPED n= 批内记录数口径");
        assert_eq!(svc.history_batch_seq, 1, "batch=<seq> 每次非空 flush 递增");
        // 再一轮失败：seq 继续单调；DROPPED 累计。
        svc.clear();
        svc.ingest(&[sample(2, 12.0, now), sample(3, 13.0, now)]);
        svc.flush_history();
        assert_eq!(svc.history_batch_seq, 2);
        assert_eq!(svc.history_dropped, 3);
        // REJECTED 行增量语义：上一轮的拒绝不被重复报告（此处间接通过
        // 字段稳定表达；target 侧以 transcript 文本判定三态齐备）。
        let rejected_before = svc.rejected;
        svc.clear();
        svc.ingest(&[sample(4, 14.0, now + 1)]);
        svc.flush_history();
        assert!(svc.rejected >= rejected_before);

        // Ring 介质基线行为回归：既有两行词形对应的内部量不变——
        // dropped 不被复用（Ring 失败不计入 history_dropped）。
        let mut ring_svc = service();
        ring_svc.set_history_sink(Box::new(FailSink));
        ring_svc.clear();
        ring_svc.ingest(&[sample(5, 15.0, now + 2)]);
        ring_svc.flush_history();
        assert_eq!(ring_svc.history_errors, 1);
        assert_eq!(
            ring_svc.history_dropped, 0,
            "Ring 介质不占用 DISK-DROPPED 口径"
        );
        // seq 对两介质统一单调（Ring 基线行不含该字段，无兼容影响）。
        assert_eq!(ring_svc.history_batch_seq, 1);
    }

    /// SPEC-C：HistoryRecord 质量码直通（品质如实存档，无 Bad→Good 抹平）。
    #[test]
    fn spec_c_quality_code_mapping() {
        // 单一来源 = agent-sdk sample_wire::quality_code（本模块不再自持副本）。
        use eneros_agent_sdk::sample_wire::quality_code;
        use eneros_safety_validator::QualityReason;
        assert_eq!(quality_code(SampleQuality::Good), 0);
        assert_eq!(
            quality_code(SampleQuality::Suspect(QualityReason::Replay)),
            1
        );
        assert_eq!(quality_code(SampleQuality::Bad(QualityReason::Replay)), 2);
        assert_eq!(quality_code(SampleQuality::Unknown), 3);
        // 体积红线复核（quality 扩字段后仍守 R2）。
        assert!(
            DEFAULT_HISTORY_CAP * core::mem::size_of::<HistoryRecord>() <= 32 * 1024,
            "热环 512×{}B 必须 ≤32KiB",
            core::mem::size_of::<HistoryRecord>()
        );
    }

    /// WP-2/C1-c：sink 失败 fail-visible——错误计数、ingest ack 不受影响、
    /// 不 panic。
    #[test]
    fn wp2_sink_error_counted_never_blocks_ingest() {
        struct FailSink;
        impl TelemetryHistorySink for FailSink {
            fn write_batch(&mut self, _b: &[HistoryRecord]) -> Result<(), ()> {
                Err(())
            }
        }
        let mut svc = service();
        svc.set_history_sink(Box::new(FailSink));
        svc.clear();
        svc.ingest(&[sample(1, 5.0, 100)]);
        svc.flush_history();
        assert_eq!(svc.history_errors, 1);
        assert_eq!(svc.history_written, 0);
        // 未装配 sink 时零开销直通（行为与未接线版本一致）。
        let mut plain = service();
        plain.clear();
        plain.ingest(&[sample(1, 5.0, 100)]);
        plain.flush_history();
        assert_eq!(plain.history_written, 0);
        assert_eq!(plain.history_errors, 0);
        assert!(
            DEFAULT_HISTORY_CAP * core::mem::size_of::<HistoryRecord>() <= 32 * 1024,
            "缺省内存环必须守 R2 预算（≤32KiB）；扩容先出静态审计表"
        );
    }

    /// 蓝图 67 WP-67.3 自守卫（照 tel_wire.rs include_str! 先例）：本文件
    /// 禁止再以裸 const 定义请求码——单源在 agent-sdk
    /// tel_wire，此处只允许 use re-export 别名（`TEL_INGEST_OBS` 是 use
    /// 别名不是 const，不受影响）。
    #[test]
    fn no_bare_tel_const_definitions_in_module() {
        let src = include_str!("telemetry.rs");
        let banned = concat!("pub const ", "TEL_");
        assert!(
            !src.contains(banned),
            "请求码裸常量回归（单源 = agent-sdk tel_wire，蓝图 67 WP-67.3）"
        );
    }

    // --------------------------------------------------------------------
    // 69.3 WP-69.3.2：T1/T2/T3 红测（执行卡 7.2；变异验证见执行卡）
    // --------------------------------------------------------------------

    /// t_evalcfg_single_source：T1 构造器产出与原 bin 两处字面量**逐字段
    /// 相同**（装配初值形 = svc-telemetry.rs 原 :36-52；生产重建形 = 原
    /// :217-230。期望侧手抄冻结（F-37 同律），改构造器任一字段必红）。
    #[test]
    fn t_evalcfg_single_source() {
        // 装配初值形（兼容轨：mapping_version=1，合成源 1 在册）。
        let boot = evaluator_config(1, alloc::vec![1]);
        assert_eq!(boot.mapping_version, 1);
        assert_eq!(boot.allowed_sources, alloc::vec![1]);
        // 生产重建形（mapping_version = bundle generation，注册集合覆盖）。
        let prod = evaluator_config(0x1234, alloc::vec![]);
        assert_eq!(prod.mapping_version, 0x1234);
        assert_eq!(prod.allowed_sources, Vec::<u32>::new());
        // 两形五项恒同字段（与原 bin 字面逐项相同）。
        for cfg in [&boot, &prod] {
            let b = &cfg.bindings;
            assert_eq!((b.voltage, b.current, b.frequency, b.soc), (1u32, 2, 3, 4));
            assert_eq!(cfg.max_skew_ms, 1000);
            assert!(cfg.require_integrity);
            // 控制窗单源 = ADR-0054:34–35（30s 安全事实；时域收敛非窗放宽）。
            assert_eq!(cfg.control_horizon_ms, DEFAULT_CONTROL_HORIZON_MS);
            assert_eq!(
                cfg.control_horizon_ms,
                eneros_decision_contract::CONTROL_HORIZON_MS
            );
            // coverage 同源 = control-dispatch feedback 单源（非空）。
            assert!(!cfg.coverage.points.is_empty());
        }
    }

    /// t_tel_snapshot_wordform_byte_identical：T3 词形冻结对照（期望侧
    /// 手抄 = 原 svc-telemetry bin SNAPSHOT 分支 println 字面；改一词段
    /// 必红——Z2）。
    #[test]
    fn t_tel_snapshot_wordform_byte_identical() {
        assert_eq!(
            line_snapshot_rejected(&TrustRejection::NoSamples, 0, 199785, u64::MAX),
            alloc::format!(
                "[svc-telemetry] SNAPSHOT rejected: {:?} (ingested={} now={} first_age={})",
                TrustRejection::NoSamples,
                0,
                199785,
                u64::MAX
            )
        );
        // 冻结字面（69.1 出口窗串口实采形态；NoSamples Debug 词形在场）。
        assert_eq!(
            line_snapshot_rejected(&TrustRejection::NoSamples, 0, 199785, u64::MAX),
            "[svc-telemetry] SNAPSHOT rejected: NoSamples (ingested=0 now=199785 \
             first_age=18446744073709551615)"
        );
    }

    /// t_snapshot_reject_via_handle_request：SNAPSHOT 臂收编后，空服务
    /// 快照请求经 handle_request 单源路径回 SNAPSHOT_REJECTED（bin 分支
    /// 删除的响应字节等价锁）。
    #[test]
    fn t_snapshot_reject_via_handle_request() {
        let mut svc = service();
        let resp = handle_request(&mut svc, &[TEL_SNAPSHOT], 100);
        assert_eq!(resp, alloc::vec![SNAPSHOT_REJECTED]);
    }

    /// t_source_registration_order_preserved（T2 红测；执行卡 7.1 判据
    /// ③不适用登记——本函数不触 SourceRegistry）：① descriptors 转换序
    /// = `[[source]]` 表序（7→3）；② 未注册来源事件只进 quarantine，
    /// 生产 evaluator 零消费（ingested 不动 + 快照仍 NoSamples）；
    /// ③ 取数失败 → false（下一轮重试）。变异：交换表序/放行未注册
    /// 事件均必红。
    #[test]
    fn t_source_registration_order_preserved() {
        use eneros_config_bundle::points::decode_points;
        use eneros_config_bundle::schema::points as pts_schema;
        use eneros_config_bundle::ConfigId;

        // [[source]] 表：7 = ModbusTcp，3 = Iec104（行 26B 布局 =
        // toml_compile::compile_sources 单源；rate ≥1 必填）。
        let source_row = |id: u32, tenant: u32, tag: u8| {
            let mut r = Vec::new();
            r.extend_from_slice(&id.to_le_bytes());
            r.extend_from_slice(&tenant.to_le_bytes());
            r.extend_from_slice(&1u32.to_le_bytes()); // site_id
            r.push(tag);
            r.push(pts_schema::SOURCE_FLAG_REQUIRE_INTEGRITY);
            r.extend_from_slice(&0u32.to_le_bytes()); // point_min（0 = 全域）
            r.extend_from_slice(&0u32.to_le_bytes()); // point_max（0 = 无界）
            r.extend_from_slice(&100u32.to_le_bytes()); // max_events_per_s
            r
        };
        let mut sources = Vec::new();
        sources.extend_from_slice(&2u32.to_le_bytes());
        sources.extend_from_slice(&source_row(7, 10, pts_schema::PROTO_MODBUS_TCP));
        sources.extend_from_slice(&source_row(3, 20, pts_schema::PROTO_IEC104));
        // 点记录（v1 布局 = schema::points 注释序；2 点，全域命中两源）。
        let point_row = |pid: u32, tag: u8| {
            let mut r = Vec::new();
            r.extend_from_slice(&pid.to_le_bytes());
            r.extend_from_slice(&1u16.to_le_bytes()); // device_id
            r.push(tag);
            r.push(0); // PointType::Analog
            r.extend_from_slice(&1.0f64.to_le_bytes()); // scale
            r.extend_from_slice(&0.0f64.to_le_bytes()); // offset
            r.extend_from_slice(&1000u32.to_le_bytes()); // period_ms
            r.extend_from_slice(&0.0f64.to_le_bytes()); // deadband
            r.push(0); // no range
            match tag {
                pts_schema::PROTO_MODBUS_TCP => {
                    r.extend_from_slice(&4u16.to_le_bytes()); // addr_len
                    r.extend_from_slice(&[1, 0, 0, 0]); // slave,reg,func
                }
                _ => {
                    r.extend_from_slice(&5u16.to_le_bytes()); // addr_len
                    r.extend_from_slice(&[1, 0, 100, 0, 13]); // common,ioa,type
                }
            }
            r
        };
        let mut table = Vec::new();
        table.extend_from_slice(&2u32.to_le_bytes());
        table.extend_from_slice(&point_row(1001, pts_schema::PROTO_MODBUS_TCP));
        table.extend_from_slice(&point_row(1002, pts_schema::PROTO_IEC104));
        let payload = eneros_config_bundle::payload::encode_payload([
            (pts_schema::FIELD_POINTS, &table[..]),
            (pts_schema::FIELD_SOURCES, &sources[..]),
        ]);
        let decoded =
            decode_points(&payload, &ConfigId::from_static("points")).expect("夹具表必须可解码");
        assert_eq!(decoded.sources.len(), 2);

        // ① 转换序 = 表序（7→3；交换表序必红）。
        let descriptors =
            crate::obs_shadow::descriptors_from_config(&decoded.sources, &decoded.points);
        let order: alloc::vec::Vec<u32> = descriptors.iter().map(|d| d.source_id).collect();
        assert_eq!(order, alloc::vec![7, 3], "注册序 = [[source]] 表序");

        // 装配 + 内存 config 假件（generation 42 + points 载荷；无
        // catalog 条目 → CATALOG-MISSING 兼容轨）。
        struct FakeSrc {
            gen: u64,
            points: Option<alloc::vec::Vec<u8>>,
        }
        impl ConfigEntrySource for FakeSrc {
            fn config_entry(&mut self, id: &str) -> Option<alloc::vec::Vec<u8>> {
                let body = match id {
                    "points" => self.points.as_ref()?,
                    _ => return None,
                };
                let mut out = alloc::vec::Vec::new();
                out.extend_from_slice(&(body.len() as u32).to_le_bytes());
                out.extend_from_slice(body);
                Some(out)
            }
            fn config_generation(&mut self) -> Option<u64> {
                Some(self.gen)
            }
        }
        let mut src = FakeSrc {
            gen: 42,
            points: Some(payload),
        };
        let mut svc = service();
        // 装载成功：注册两源 + generation 生效；二次调用短路。
        assert!(svc.ensure_source_identity(Some(&mut src), 1_000));
        assert!(svc.ensure_source_identity(Some(&mut src), 1_000), "短路");
        {
            let obs = svc.obs.as_ref().expect("装载后 obs 在场");
            assert!(obs.is_registered(7) && obs.is_registered(3));
        }

        // ② 未注册来源（id=9）事件 → 只进 quarantine；生产 evaluator
        // 零消费（ingested 不动 + 快照仍 NoSamples 拒绝）。
        let ev = eneros_telemetry_model::event::ObservationEvent {
            source_id: 9,
            source_epoch: 5,
            sequence: 1,
            ..make_min_event()
        };
        let mut frames = ev.encode_v1().to_vec();
        // 同批第二帧 = 已注册源 7 的合法事件（对照：不进 quarantine）。
        frames.extend_from_slice(
            &eneros_telemetry_model::event::ObservationEvent {
                source_id: 7,
                sequence: 1,
                ..make_min_event()
            }
            .encode_v1(),
        );
        let resp = svc.obs_ingest(7, &frames, 2_000);
        assert_eq!(resp[0], 0, "批次 ack OK");
        {
            let obs = svc.obs.as_ref().expect("obs 在场");
            assert_eq!(obs.quarantined, 1, "未注册源事件必须进 quarantine");
            assert_eq!(obs.quarantine_len(), 1);
            assert!(obs.is_registered(7));
            assert!(!obs.is_registered(9));
        }
        // 未注册事件零消费：批内 2 帧只 registered 源的 1 帧进生产轨
        //（若未注册事件泄漏，ingested = 2）。
        assert_eq!(svc.ingested, 1, "仅注册源样本进生产轨");

        // ③ 取数失败（points 缺席）→ false（下一轮重试；legacy 轨续行）。
        let mut empty_src = FakeSrc {
            gen: 42,
            points: None,
        };
        let mut fresh = service();
        assert!(!fresh.ensure_source_identity(Some(&mut empty_src), 1_000));
        assert!(fresh.obs.is_none(), "失败不半装载");
    }

    /// T2 夹具最小事件（ingestion-core make_min_event 同款布局；source
    /// 域按用例覆盖）。
    fn make_min_event() -> eneros_telemetry_model::event::ObservationEvent {
        eneros_telemetry_model::event::ObservationEvent {
            tenant_id: 1,
            site_id: 1,
            source_id: 2,
            source_epoch: 5,
            sequence: 10,
            point_id: 1,
            value: eneros_telemetry_model::event::ObservationValue::F64(1.0),
            event_time_ns: 1,
            ingest_time_ns: 1,
            quality_bits: 0,
            integrity: eneros_telemetry_model::event::IntegrityTag::Unverified,
            flags: 0,
            schema_id: 0,
            schema_version: 0,
            unit_code: 0,
            asset_id: 0,
            mapping_version: 0,
            device_id: 0,
            clock_id: 1,
        }
    }

    // ========================================================================
    // C3（2026-09-24）：钉面 17B wire 双面锁（客户端编码器 ↔ 本服务
    // handle_request 解码）。迁移前客户端（dispatch_assembly cfg(none)
    // TelemetrySnapRetainer）与服务端各自手抄布局——漂移只在 QEMU 可见；
    // 现客户端编码单源 = dispatch_wire，本测锁两侧一致性。
    // ========================================================================

    /// 钉面成功替身（pin/release 恒 true——双面锁只关心 wire 布局与
    /// 应答形状，不关心容量语义）。
    struct PinOkFace;

    impl crate::event_face::ObsEventFace for PinOkFace {
        fn on_obs_batch(
            &mut self,
            _events: &[eneros_telemetry_model::event::ObservationEvent],
            _now_ms: u64,
        ) {
        }
        fn set_snapshot_catalog(
            &mut self,
            _repo: eneros_semantic_catalog::CatalogRepository,
            _now_ms: u64,
        ) {
        }
        fn current_input(&self) -> Option<eneros_agent_sdk::snap_input_wire::TrustedInput> {
            None
        }
        fn snapshot_blob(&mut self, _id: &[u8; 16]) -> Option<Vec<u8>> {
            None
        }
        fn hold_offset(&mut self, _partition: u32, _offset: u64) {}
        fn pin_snapshot(&mut self, _id: &[u8; 16]) -> bool {
            true
        }
        fn release_snapshot(&mut self, _id: &[u8; 16]) -> bool {
            true
        }
        fn retention_blocked(&self) -> bool {
            false
        }
    }

    /// 双面锁：客户端 `dispatch_wire::snap_*_request`（`SNAP_REQ_LEN`B =
    /// [op, id×16]）↔ 服务端 `handle_request` 的 TEL_REQ_SNAP_HOLD/_RELEASE
    /// 臂（解帧单源 = `tel_wire::parse_snap_request`，WP-76.2.7）；应答
    /// `[TEL_RESP_OK]` ↔ 客户端 `snap_resp_ok`。任一侧布局漂移（长度/首
    /// 字节/切片偏移）本测即红。
    #[test]
    fn snap_wire_dual_side_lock() {
        use eneros_agent_sdk::tel_wire::{TEL_REQ_SNAP_HOLD, TEL_RESP_ERR, TEL_RESP_OK};
        let id = [0x5Au8; 16];
        let mut svc = service();
        svc.set_event_face(alloc::boxed::Box::new(PinOkFace));
        let now = 10_000;

        // hold：客户端编码 → 服务端接纳 → OK 应答 → 客户端判真。
        let hold = crate::dispatch_wire::snap_hold_request(id);
        assert_eq!(hold.len(), 17, "客户端 17B 布局");
        let resp = handle_request(&mut svc, &hold, now);
        assert_eq!(resp, vec![TEL_RESP_OK], "服务端按同一布局接纳并回 OK");
        assert!(crate::dispatch_wire::snap_resp_ok(&resp));

        // release：同一布局，恒 OK（幂等语义）。
        let release = crate::dispatch_wire::snap_release_request(id);
        assert_eq!(release.len(), 17);
        let resp = handle_request(&mut svc, &release, now);
        assert_eq!(resp, vec![TEL_RESP_OK]);
        assert!(crate::dispatch_wire::snap_resp_ok(&resp));

        // 负例①：非 17B（缺 id）→ 服务端 ERR（服务端半边的布局判据）。
        let resp = handle_request(&mut svc, &[TEL_REQ_SNAP_HOLD], now);
        assert_eq!(resp, vec![TEL_RESP_ERR], "非 17B 必须拒绝");
        assert!(!crate::dispatch_wire::snap_resp_ok(&resp));

        // 负例②：pin 面失败（容量满/事件面缺失）→ ERR → 客户端判假
        //（fail-closed：engine 拒绝新准入的入口判据）。
        let mut no_face = service(); // 未 set_event_face → pin_snapshot=false
        let resp = handle_request(&mut no_face, &hold, now);
        assert_eq!(resp, vec![TEL_RESP_ERR]);
        assert!(!crate::dispatch_wire::snap_resp_ok(&resp));
    }
}

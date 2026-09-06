//! 蓝图 40 WP-40.1：可信遥测（`TrustedTelemetry`）——控制输入的信任边界。
//!
//! 采集链的原始样本（[`TelemetrySample`]）必须经 [`TelemetryTrustEvaluator`]
//! 逐项校验后才可进入决策/控制链：
//!
//! 1. required points 全覆盖；
//! 2. 时间单调与逐点新鲜度（`max_age_ms` → Stale；max skew 判据 68.2 退役——同源内聚检查在异期点位缓冲下结构性假阳）；
//! 3. source 白名单与 source epoch（断线重连后旧 epoch 拒绝）；
//! 4. 质量（Good 才可用）、范围、单位（经映射版本强制）、NaN/Inf；
//! 5. sequence/replay（重复/乱序拒绝）；
//! 6. 多源冲突（同点多源严格拒绝，不猜优先级）；
//! 7. 设备/点映射版本（mapping version 不匹配拒绝）；
//! 8. integrity/authentication（要求 Verified 时未验证样本拒绝）。
//!
//! 输出只能是 [`TrustedTelemetry`] 或结构化 [`TrustRejection`]——缺数据时
//! 允许维持最后安全输出/进入 safe mode，但**不得**制造"正常默认状态"
//! 继续发新控制（P0-09 修复：`SystemState` 已无 `Default`，见 [`super::state`]）。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use eneros_upa_model::point::PointId;

use super::state::SystemState;

/// 样本质量（蓝图 40 模型；命名避免与已废弃的 3 态 `Quality` 混淆，
/// 且与 upa-model `PointQuality` 为不同层——本类型是可信评估层的结论）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleQuality {
    /// 好品质（唯一可进入控制链的品质）。
    Good,
    /// 可疑（存在风险但未定论）——控制输入拒绝。
    Suspect(QualityReason),
    /// 坏品质——拒绝。
    Bad(QualityReason),
    /// 未评估/未知——拒绝。
    Unknown,
}

impl SampleQuality {
    /// 是否可用于控制（仅 `Good`）。
    pub fn is_usable(&self) -> bool {
        matches!(self, SampleQuality::Good)
    }
}

/// 质量降级原因（可枚举、可审计）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityReason {
    /// 超过最大新鲜度窗口。
    Stale,
    /// 超出量程。
    OutOfRange,
    /// 非有限值（NaN/Inf）。
    NotFinite,
    /// 重放/乱序。
    Replay,
    /// 多源冲突。
    SourceConflict,
    /// 点映射版本过期。
    MappingStale,
    /// 量纲不符（由映射版本契约强制）。
    BadUnit,
    /// 完整性/认证失败。
    IntegrityFailure,
    /// 来源能力不足。
    CapabilityMissing,
    /// 来源 epoch 不匹配（断线重连）。
    SourceEpochMismatch,
}

/// 样本完整性状态（采集链认证结果；`require_integrity` 时仅 `Verified`
/// 可进入控制链）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrityStatus {
    /// 已认证（采集链 HMAC/签名/会话认证通过）。
    Verified,
    /// 未认证（无认证链覆盖）。
    Unverified,
    /// 不可验证（认证机制缺失/降级）。
    Unverifiable,
}

/// 采样值单位（映射版本契约的一部分——点必须按声明单位上报）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointUnit {
    Volt,
    Ampere,
    Hertz,
    /// 无量纲比例（SOC 0.0~1.0）。
    Ratio,
    Percent,
    Watt,
    Kilowatt,
    Other,
}

/// 单点采集样本（原始输入；由 TelemetryService 从 39 ProtocolPlugin/
/// DriverServiceHandle 订阅构造）。
#[derive(Debug, Clone, Copy)]
pub struct TelemetrySample {
    /// 统一点 ID（upa-model）。
    pub point: PointId,
    /// 采样值（声明单位，见 RequiredPoint.unit）。
    pub value: f64,
    /// 来源 ID（采集链实例）。
    pub source: u32,
    /// 来源 epoch（采集链断线重连计数；旧 epoch 样本拒绝）。
    pub source_epoch: u64,
    /// 测量时刻（单调毫秒）。
    pub measured_at_ms: u64,
    /// 接收时刻（单调毫秒；用于单调性检查）。
    pub received_at_ms: u64,
    /// 质量（采集链上报 + 评估层结论；控制输入要求 Good）。
    pub quality: SampleQuality,
    /// 序号（每点单调递增；重放/乱序拒绝）。
    pub sequence: u64,
    /// 完整性状态。
    pub integrity: IntegrityStatus,
    /// 采集链引用的设备/点映射版本（评估器 mapping_version 比对）。
    pub mapping_version: u64,
}

/// 必需点（覆盖声明 + 校验参数）。
#[derive(Debug, Clone)]
pub struct RequiredPoint {
    /// 点 ID。
    pub point: PointId,
    /// 量程下限（含）。
    pub min: f64,
    /// 量程上限（含）。
    pub max: f64,
    /// 最大新鲜度（ms）——`now - measured_at <= max_age_ms` 才可用。
    pub max_age_ms: u64,
    /// 声明单位。
    pub unit: PointUnit,
}

/// 必需点覆盖集合。
#[derive(Debug, Clone)]
pub struct RequiredPointCoverage {
    /// 必需点列表（全有才产生 TrustedTelemetry）。
    pub points: Vec<RequiredPoint>,
}

impl RequiredPointCoverage {
    /// 空覆盖（永不通过——fail-closed）。
    pub fn empty() -> Self {
        Self { points: Vec::new() }
    }

    /// 查找覆盖声明（返回 None = 未覆盖）。
    pub fn find(&self, point: PointId) -> Option<&RequiredPoint> {
        self.points.iter().find(|p| p.point == point)
    }
}

/// 点 → SystemState 字段绑定（评估器用受信任样本构造 `SystemState`）。
#[derive(Debug, Clone, Copy)]
pub struct StateBindings {
    /// 母线电压点。
    pub voltage: PointId,
    /// 母线电流点。
    pub current: PointId,
    /// 系统频率点。
    pub frequency: PointId,
    /// SOC 点。
    pub soc: PointId,
}

/// 评估器配置。
#[derive(Debug, Clone)]
pub struct EvaluatorConfig {
    /// 必需点覆盖。
    pub coverage: RequiredPointCoverage,
    /// SystemState 字段绑定。
    pub bindings: StateBindings,
    /// 设备/点映射版本（评估器创建时注入；样本侧不匹配拒绝）。
    pub mapping_version: u64,
    /// 接收时刻最大偏差。**68.2 起退役**（字段保留装配兼容）：原"同快照
    /// 内聚"检查在缓冲评估形态（每点最新替身）下测的是点位周期差而非
    /// 数据病理，误拒多源异期点位（68.2 执行卡 §D.3）；新鲜度由逐点
    /// max_age 承载。构造面保留传值、判据不再消费。
    pub max_skew_ms: u64,
    /// 允许的来源白名单（空 = 拒绝一切来源）。
    pub allowed_sources: Vec<u32>,
    /// 是否要求完整性 Verified。
    pub require_integrity: bool,
    /// 控制时域（ms）——`TrustedTelemetry.valid_until = now + horizon`。
    pub control_horizon_ms: u64,
}

/// 可信遥测（唯一可进入决策/控制链的状态载体）。
///
/// `state` 只由评估器从受信任样本构造；`provenance_digest` 为样本集合的
/// SM3 摘要（篡改可检出）；`valid_until_ms` 定义控制时域，执行时刻
/// `now > valid_until_ms` 必须重新评估（WP-40.6 步骤 2）。
#[derive(Debug, Clone)]
pub struct TrustedTelemetry {
    /// 电气状态（由受信任样本构造，非默认值）。
    pub state: SystemState,
    /// 本次评估覆盖的点集合。
    pub coverage: RequiredPointCoverage,
    /// 控制时域截止（单调毫秒）。
    pub valid_until_ms: u64,
    /// 样本集合 provenance 摘要（SM3）。
    pub provenance_digest: [u8; 32],
    /// 评估生成时刻（单调毫秒）。
    pub generated_at_ms: u64,
    /// 来源 epoch（68.2：本次评估样本的最大 per-source epoch；多源快照
    /// 下各源 epoch 不再相等，单值 = max——观测面语义）。
    pub source_epoch: u64,
    /// 映射版本（本次评估使用的版本）。
    pub mapping_version: u64,
    /// 全部受信任样本值（点 → 值；dispatch 层据此构造反馈包/按点校验）。
    pub samples: BTreeMap<PointId, f64>,
}

impl TrustedTelemetry {
    /// 执行时刻是否仍在控制时域内（`now <= valid_until_ms`）。
    pub fn is_valid_at(&self, now_ms: u64) -> bool {
        now_ms <= self.valid_until_ms
    }
}

/// 结构化拒绝（缺数据/坏质量/过期/重放等——绝不用"健康默认值"掩盖）。
#[derive(Debug, Clone, PartialEq)]
pub enum TrustRejection {
    /// 无样本。
    NoSamples,
    /// 必需点缺失。
    MissingPoints { missing: Vec<PointId> },
    /// 样本过期。
    Stale {
        point: PointId,
        age_ms: u64,
        max_age_ms: u64,
    },
    /// 质量不可用。
    BadQuality {
        point: PointId,
        quality: SampleQuality,
    },
    /// 非有限值。
    NotFinite { point: PointId },
    /// 越界。
    OutOfRange {
        point: PointId,
        value: f64,
        min: f64,
        max: f64,
    },
    /// 重放/乱序。
    Replay {
        point: PointId,
        sequence: u64,
        last_sequence: u64,
    },
    /// 多源冲突（同点多源，严格拒绝）。
    SourceConflict { point: PointId, sources: Vec<u32> },
    /// 来源不在白名单。
    UnknownSource { point: PointId, source: u32 },
    /// 来源 epoch 不匹配（断线重连后旧 epoch 消息）。
    SourceEpochMismatch {
        point: PointId,
        epoch: u64,
        expected: u64,
    },
    /// 映射版本不匹配。
    MappingStale {
        point: PointId,
        mapping_version: u64,
        expected: u64,
    },
    /// 完整性未验证。
    IntegrityFailure {
        point: PointId,
        status: IntegrityStatus,
    },
    /// 接收时刻倒退/超偏差（时钟不单调）。
    ClockNotMonotonic {
        received_at_ms: u64,
        last_received_at_ms: u64,
    },
}

/// per-source epoch 表容量 = 配置面来源注册上限（config-bundle
/// `points::MAX_SOURCES`，schema.rs；两处仅注释耦合、无跨 crate 依赖——
/// 容量漂移由 commit 防御性饱和兜底，不放大判据）。
const SOURCE_EPOCH_TABLE_CAP: usize = 64;

/// per-source epoch 表（蓝图 68.2：source_id → 已见最大 epoch；定长槽 +
/// 线性查——来源数有配置上限，no_std 零动态分配。替代原单一全局 `epoch`：
/// 每服务 `open()` 的 floor 各取 boot 单调时刻必不相等（F13），全局单值
/// 语义下"最后钉扎者赢"，异源互毒）。
#[derive(Debug, Clone)]
struct SourceEpochTable {
    slots: [Option<(u32, u64)>; SOURCE_EPOCH_TABLE_CAP],
}

impl SourceEpochTable {
    const fn new() -> Self {
        Self {
            slots: [None; SOURCE_EPOCH_TABLE_CAP],
        }
    }

    /// 判据（68.2 唯一实现）：未见源放行；`epoch ≥ seen` 放行（前跳 =
    /// 重启新 epoch，per-source 取 max）；`< seen` = 同源回退拒绝
    /// （`Err(已见最大值)`——防真回拨/旧 epoch 重放）。异源互不干扰。
    fn judge(&self, source: u32, epoch: u64) -> Result<(), u64> {
        for slot in &self.slots {
            if let Some((s, seen)) = *slot {
                if s == source {
                    return if epoch < seen { Err(seen) } else { Ok(()) };
                }
            }
        }
        Ok(())
    }

    /// 提交（快照评估全过后调用；失败路径零突变）：首见入槽 / 已见取 max。
    fn commit(&mut self, source: u32, epoch: u64) {
        for slot in &mut self.slots {
            match slot {
                None => {
                    *slot = Some((source, epoch));
                    return;
                }
                Some((s, seen)) if *s == source => {
                    if epoch > *seen {
                        *seen = epoch;
                    }
                    return;
                }
                _ => {}
            }
        }
        // 槽满 = 来源数超配置上限（≤MAX_SOURCES 由 config 编译面保证；
        // 防御性饱和——不记录即视为未见，判据不放大）。
    }

    /// 已见最大 epoch（各源 max 的最大值；观测面）。
    fn max_seen(&self) -> u64 {
        self.slots
            .iter()
            .flatten()
            .map(|&(_, e)| e)
            .max()
            .unwrap_or(0)
    }
}

/// 可信遥测评估器（WP-40.1 验证器）。
///
/// 有状态：跨快照记住每点最后序号与最后接收时刻（重启后经
/// `TelemetryService` 持久化恢复——服务层职责，见蓝图 40 WP-40.2）。
#[derive(Debug, Clone)]
pub struct TelemetryTrustEvaluator {
    config: EvaluatorConfig,
    /// 每点最后接受序号，按 (来源, 点) 记账并携带 epoch 空间戳（68.2：
    /// 同源同 epoch 才比序号；同源换新 epoch 后旧基线自动失效——adapter
    /// 重启序号回卷不再误拒，也无需全局清态牵连他源）。
    last_sequences: BTreeMap<(u32, PointId), (u64, u64)>,
    /// 跨快照最后接收时刻基线，**按来源记账**（68.2 实弹订正：多源各自
    /// 节奏，全局基线在混合缓冲下互踩——A 源新批抬高基线后 B 源同轮较旧
    /// 批必判 ClockNotMonotonic，快照永久被打死，QEMU 窗 agent=OK 60→1；
    /// 单调性是同源性质，判据同源化）。
    last_received_at_ms: BTreeMap<u32, u64>,
    /// per-source epoch 表（68.2 起替代单全局 epoch）。
    epochs: SourceEpochTable,
}

impl TelemetryTrustEvaluator {
    /// 创建评估器。
    pub fn new(config: EvaluatorConfig) -> Self {
        Self {
            config,
            last_sequences: BTreeMap::new(),
            last_received_at_ms: BTreeMap::new(),
            epochs: SourceEpochTable::new(),
        }
    }

    /// 已见最大 per-source epoch（观测面；68.2 起多源语义——单值 =
    /// 各源 max 的最大值）。
    pub fn epoch(&self) -> u64 {
        self.epochs.max_seen()
    }

    /// 批内是否已含全部必需点（68.2 影子对照逐批预检：多源上行后单批
    /// 只含单源点位，对全量覆盖面评估结构性 MissingPoints，无对照意义
    /// ——调用方跳过该批评估，不产影子拒绝/分歧计数）。
    pub fn covers_coverage(&self, samples: &[TelemetrySample]) -> bool {
        self.config
            .coverage
            .points
            .iter()
            .all(|req| samples.iter().any(|s| s.point == req.point))
    }

    /// 从持久化状态恢复（重启后防旧消息被误接受；68.2 per-source 形态）：
    /// `epochs` = (source, 已见最大 epoch) 表种子；序号基线按
    /// (source, point) → (epoch, seq) 注入。
    pub fn restore(
        &mut self,
        epochs: &[(u32, u64)],
        last_received_at_ms: u64,
        last_sequences: BTreeMap<(u32, PointId), (u64, u64)>,
    ) {
        for &(source, epoch) in epochs {
            self.epochs.commit(source, epoch);
            // 恢复面：已列来源的接收基线统一置该值（持久化恢复无逐源
            // 接收时刻可考时的一致下界）。
            self.last_received_at_ms.insert(source, last_received_at_ms);
        }
        self.last_sequences = last_sequences;
    }

    /// 评估样本集合：全部通过 → `TrustedTelemetry`；任何一项不通过 →
    /// 结构化拒绝（零部分通过）。
    pub fn evaluate(
        &mut self,
        samples: &[TelemetrySample],
        now_ms: u64,
    ) -> Result<TrustedTelemetry, TrustRejection> {
        if samples.is_empty() {
            return Err(TrustRejection::NoSamples);
        }

        // 1. 接收时刻单调性（68.2 实弹订正：判据同源化——跨快照基线按
        // 来源比。多源各自节奏，全局基线在混合缓冲下为永假阳：A 源新批
        // 抬高全局基线 → B 源同轮较旧批必拒；同源异期点位的新旧替身散布
        // 亦然。QEMU 窗 ClockNotMonotonic 数千次、agent=OK 60→1/7 实锤）。
        // 跨源/跨点时间散布由逐点 Stale(max_age) 各自兜底，安全语义不
        // 放松。原 max_skew 同快照内聚检查随同退役：缓冲评估形态是
        // "每点最新替身"，同源散布 = 点位周期差（数据病理由 Stale/重放
        // 判据承载），max_skew_ms 字段保留装配兼容（见字段 doc）。
        // 基线提交取本快照该源 **min**（68.2 实弹订正四）：基线收敛到
        // "该源最老仍在替样本"——异期点位下 max 提交会让慢点位老替身必然
        // 落后基线（二轮后残余 1.9 万次误拒的根源）；min 语义下重替身
        // = 基线本身（等值放行），而严格早于上轮最早样本的**新到达**仍
        // 判回拨（检查的独占职责不变）。
        let mut per_src_min: BTreeMap<u32, u64> = BTreeMap::new();
        for s in samples {
            if let Some(&base) = self.last_received_at_ms.get(&s.source) {
                if s.received_at_ms < base {
                    return Err(TrustRejection::ClockNotMonotonic {
                        received_at_ms: s.received_at_ms,
                        last_received_at_ms: base,
                    });
                }
            }
            per_src_min
                .entry(s.source)
                .and_modify(|m| {
                    if s.received_at_ms < *m {
                        *m = s.received_at_ms;
                    }
                })
                .or_insert(s.received_at_ms);
        }

        // 2. 覆盖检查：必需点是否全部出现且唯一（多源冲突严格拒绝）。
        let mut missing = Vec::new();
        let mut by_point: BTreeMap<PointId, &TelemetrySample> = BTreeMap::new();
        for s in samples {
            if self.config.coverage.find(s.point).is_none() {
                // 非必需点：评估层忽略（采集侧自证由上层负责）；不参与控制。
                continue;
            }
            match by_point.get(&s.point) {
                None => {
                    by_point.insert(s.point, s);
                }
                Some(prev) if prev.source != s.source => {
                    return Err(TrustRejection::SourceConflict {
                        point: s.point,
                        sources: alloc::vec![prev.source, s.source],
                    });
                }
                Some(_) => {
                    // 同源重复样本：视为重放（快照应每点一值）。
                    return Err(TrustRejection::Replay {
                        point: s.point,
                        sequence: s.sequence,
                        last_sequence: self
                            .last_sequences
                            .get(&(s.source, s.point))
                            .map_or(s.sequence.saturating_sub(1), |&(_, ls)| ls),
                    });
                }
            }
        }
        for req in &self.config.coverage.points {
            if !by_point.contains_key(&req.point) {
                missing.push(req.point);
            }
        }
        if !missing.is_empty() {
            return Err(TrustRejection::MissingPoints { missing });
        }

        // 3. 逐点校验。
        let mut values: BTreeMap<PointId, f64> = BTreeMap::new();
        for req in &self.config.coverage.points {
            let s = by_point[&req.point];
            // 来源白名单。
            if !self.config.allowed_sources.contains(&s.source) {
                return Err(TrustRejection::UnknownSource {
                    point: req.point,
                    source: s.source,
                });
            }
            // 来源 epoch（68.2 per-source 表：同源回退拒绝（防真回拨/旧
            // epoch 重放）、异源互不重置（消灭"最后钉扎者赢"）、前跳放行
            // （重启新 epoch，per-source 取 max））。
            if let Err(seen) = self.epochs.judge(s.source, s.source_epoch) {
                return Err(TrustRejection::SourceEpochMismatch {
                    point: req.point,
                    epoch: s.source_epoch,
                    expected: seen,
                });
            }
            // 映射版本。
            if s.mapping_version != self.config.mapping_version {
                return Err(TrustRejection::MappingStale {
                    point: req.point,
                    mapping_version: s.mapping_version,
                    expected: self.config.mapping_version,
                });
            }
            // 质量。
            if !s.quality.is_usable() {
                return Err(TrustRejection::BadQuality {
                    point: req.point,
                    quality: s.quality,
                });
            }
            // 完整性。
            if self.config.require_integrity && s.integrity != IntegrityStatus::Verified {
                return Err(TrustRejection::IntegrityFailure {
                    point: req.point,
                    status: s.integrity,
                });
            }
            // 有限值。
            if !s.value.is_finite() {
                return Err(TrustRejection::NotFinite { point: req.point });
            }
            // 量程。
            if s.value < req.min || s.value > req.max {
                return Err(TrustRejection::OutOfRange {
                    point: req.point,
                    value: s.value,
                    min: req.min,
                    max: req.max,
                });
            }
            // 新鲜度。
            let age = now_ms.saturating_sub(s.measured_at_ms);
            if age > req.max_age_ms {
                return Err(TrustRejection::Stale {
                    point: req.point,
                    age_ms: age,
                    max_age_ms: req.max_age_ms,
                });
            }
            // 序号（重放/乱序）——按 (来源, 点) 记账，epoch 空间隔离
            //（68.2：同源换新 epoch 后旧基线自动失效，重启序号回卷不误拒）。
            // 68.2 实弹订正二：判严格回退（<）而非 `<=`——快照评估是缓冲
            // 只读投影，多源节奏下他源刷新会触发未变点位的**等值重替身**
            //（单源时代被"按源摄取门"掩盖）：等值 = 同一样本幂等重评，非
            // 重放。等值重放由上游严线拦截（sink event_id 去重 +
            // WatermarkTracker::check 序列 `<=` + 旧轨缓冲门 (point,seq)
            // 去重）——评估层此处独占职责 = 回退。
            if let Some(&(last_epoch, last_seq)) = self.last_sequences.get(&(s.source, req.point)) {
                if last_epoch == s.source_epoch && s.sequence < last_seq {
                    return Err(TrustRejection::Replay {
                        point: req.point,
                        sequence: s.sequence,
                        last_sequence: last_seq,
                    });
                }
            }
            values.insert(req.point, s.value);
        }

        // 4. 构造 SystemState（只从受信任样本——无默认值路径）。
        let state = SystemState::new(
            values[&self.config.bindings.voltage],
            values[&self.config.bindings.current],
            values[&self.config.bindings.frequency],
            values[&self.config.bindings.soc],
            now_ms,
        );

        // 5. provenance 摘要：SM3 覆盖全部样本的规范编码（定长、无歧义）。
        // 68.2：第三参 = 本次快照样本最大 per-source epoch（多源下
        // "所有样本同一 epoch"不再成立；确定性不变）。
        let snap_max_epoch = samples.iter().map(|s| s.source_epoch).max().unwrap_or(0);
        let digest = provenance_digest(samples, self.config.mapping_version, snap_max_epoch);

        // 6. 推进状态（全部判据通过后——失败路径零突变；epoch 表与
        // 接收基线同纪律）。接收基线按来源取本快照该源 min（见步骤 1
        // 订正四——收敛到最老仍在替样本，重替身等值放行）。
        for (&src, &mn) in &per_src_min {
            self.last_received_at_ms.insert(src, mn);
        }
        for (point, s) in &by_point {
            self.last_sequences
                .insert((s.source, *point), (s.source_epoch, s.sequence));
            self.epochs.commit(s.source, s.source_epoch);
        }

        Ok(TrustedTelemetry {
            state,
            coverage: self.config.coverage.clone(),
            valid_until_ms: now_ms.saturating_add(self.config.control_horizon_ms),
            provenance_digest: digest,
            generated_at_ms: now_ms,
            source_epoch: snap_max_epoch,
            mapping_version: self.config.mapping_version,
            samples: values,
        })
    }
}

/// 样本携带的映射版本（采集链在发送时打上点映射版本戳，评估器比对）。
///
/// 说明：多版本映射演进时由采集链按点映射表填充；当前产品点映射为 v1。
///
/// 规范编码（定长字段、全小端、无长度前缀歧义）→ SM3。
pub fn provenance_digest(
    samples: &[TelemetrySample],
    mapping_version: u64,
    epoch: u64,
) -> [u8; 32] {
    let mut buf = alloc::vec![0u8; 8 + 8 + 56 * samples.len()];
    buf[0..8].copy_from_slice(&mapping_version.to_le_bytes());
    buf[8..16].copy_from_slice(&epoch.to_le_bytes());
    for (i, s) in samples.iter().enumerate() {
        let base = 16 + i * 56;
        buf[base..base + 4].copy_from_slice(&s.point.to_le_bytes());
        buf[base + 4..base + 12].copy_from_slice(&s.value.to_le_bytes());
        buf[base + 12..base + 16].copy_from_slice(&s.source.to_le_bytes());
        buf[base + 16..base + 24].copy_from_slice(&s.source_epoch.to_le_bytes());
        buf[base + 24..base + 32].copy_from_slice(&s.measured_at_ms.to_le_bytes());
        buf[base + 32..base + 40].copy_from_slice(&s.sequence.to_le_bytes());
        buf[base + 40] = quality_code(s.quality);
        buf[base + 41] = integrity_code(s.integrity);
        buf[base + 48..base + 56].copy_from_slice(&s.mapping_version.to_le_bytes());
    }
    eneros_crypto::sm3_hash(&buf)
}

fn quality_code(q: SampleQuality) -> u8 {
    match q {
        SampleQuality::Good => 0,
        SampleQuality::Suspect(_) => 1,
        SampleQuality::Bad(_) => 2,
        SampleQuality::Unknown => 3,
    }
}

fn integrity_code(i: IntegrityStatus) -> u8 {
    match i {
        IntegrityStatus::Verified => 0,
        IntegrityStatus::Unverified => 1,
        IntegrityStatus::Unverifiable => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good(
        point: PointId,
        value: f64,
        sequence: u64,
        measured_at_ms: u64,
        received_at_ms: u64,
    ) -> TelemetrySample {
        TelemetrySample {
            point,
            value,
            source: 1,
            source_epoch: 0,
            measured_at_ms,
            received_at_ms,
            quality: SampleQuality::Good,
            sequence,
            integrity: IntegrityStatus::Verified,
            mapping_version: 1,
        }
    }

    fn evaluator() -> TelemetryTrustEvaluator {
        TelemetryTrustEvaluator::new(EvaluatorConfig {
            coverage: RequiredPointCoverage {
                points: alloc::vec![
                    RequiredPoint {
                        point: 1,
                        min: 0.0,
                        max: 1000.0,
                        max_age_ms: 5000,
                        unit: PointUnit::Volt
                    },
                    RequiredPoint {
                        point: 2,
                        min: -1000.0,
                        max: 1000.0,
                        max_age_ms: 5000,
                        unit: PointUnit::Ampere
                    },
                    RequiredPoint {
                        point: 3,
                        min: 45.0,
                        max: 55.0,
                        max_age_ms: 5000,
                        unit: PointUnit::Hertz
                    },
                    RequiredPoint {
                        point: 4,
                        min: 0.0,
                        max: 1.0,
                        max_age_ms: 5000,
                        unit: PointUnit::Ratio
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
            allowed_sources: alloc::vec![1],
            require_integrity: true,
            control_horizon_ms: 30_000,
        })
    }

    fn snapshot(now: u64) -> Vec<TelemetrySample> {
        alloc::vec![
            good(1, 380.0, 1, now - 100, now),
            good(2, 10.0, 1, now - 100, now),
            good(3, 50.0, 1, now - 100, now),
            good(4, 0.6, 1, now - 100, now),
        ]
    }

    #[test]
    fn evaluate_trusted_snapshot_builds_state() {
        let mut ev = evaluator();
        let t = ev.evaluate(&snapshot(10_000), 10_000).expect("trusted");
        assert_eq!(t.state.voltage_v, 380.0);
        assert_eq!(t.state.current_a, 10.0);
        assert_eq!(t.state.frequency_hz, 50.0);
        assert_eq!(t.state.soc_pct, 0.6);
        assert_eq!(t.valid_until_ms, 40_000);
        assert_eq!(t.provenance_digest.len(), 32);
        assert!(t.is_valid_at(39_999));
        assert!(!t.is_valid_at(40_001));
        // 摘要随值变化（篡改可检出）。
        let mut tampered = snapshot(10_000);
        tampered[0].value = 381.0;
        for s in tampered.iter_mut() {
            s.sequence = 2;
        }
        let t2 = ev.evaluate(&tampered, 10_000).expect("trusted");
        assert_ne!(t.provenance_digest, t2.provenance_digest);
        // 68.2：等值重替身 = 幂等重评（缓冲只读投影）→ 放行。
        let _ = ev.evaluate(&tampered, 10_000).expect("幂等重评");
        // 序号严格回退必须拒绝（评估器独占职责）。
        let mut regressed = tampered.clone();
        for s in regressed.iter_mut() {
            s.sequence = 1;
        }
        assert!(matches!(
            ev.evaluate(&regressed, 10_000).unwrap_err(),
            TrustRejection::Replay { .. }
        ));
    }

    #[test]
    fn no_samples_rejected() {
        let mut ev = evaluator();
        assert!(matches!(
            ev.evaluate(&[], 10_000),
            Err(TrustRejection::NoSamples)
        ));
    }

    #[test]
    fn missing_point_rejected() {
        let mut ev = evaluator();
        let mut s = snapshot(10_000);
        s.retain(|x| x.point != 4);
        let err = ev.evaluate(&s, 10_000).unwrap_err();
        assert!(
            matches!(err, TrustRejection::MissingPoints { missing } if missing == alloc::vec![4])
        );
    }

    #[test]
    fn stale_rejected() {
        let mut ev = evaluator();
        let mut s = snapshot(10_000);
        s[0].measured_at_ms = 1_000; // age 9000 > 5000
        assert!(matches!(
            ev.evaluate(&s, 10_000).unwrap_err(),
            TrustRejection::Stale { point: 1, .. }
        ));
    }

    #[test]
    fn bad_quality_rejected() {
        let mut ev = evaluator();
        let mut s = snapshot(10_000);
        s[2].quality = SampleQuality::Bad(QualityReason::IntegrityFailure);
        assert!(matches!(
            ev.evaluate(&s, 10_000).unwrap_err(),
            TrustRejection::BadQuality { point: 3, .. }
        ));
    }

    #[test]
    fn nan_inf_rejected() {
        let mut ev = evaluator();
        let mut s = snapshot(10_000);
        s[1].value = f64::NAN;
        assert!(matches!(
            ev.evaluate(&s, 10_000).unwrap_err(),
            TrustRejection::NotFinite { point: 2 }
        ));
        let mut s2 = snapshot(10_000);
        s2[3].value = f64::INFINITY;
        assert!(matches!(
            ev.evaluate(&s2, 10_000).unwrap_err(),
            TrustRejection::NotFinite { point: 4 }
        ));
    }

    #[test]
    fn out_of_range_rejected() {
        let mut ev = evaluator();
        let mut s = snapshot(10_000);
        s[3].value = 1.5;
        assert!(matches!(
            ev.evaluate(&s, 10_000).unwrap_err(),
            TrustRejection::OutOfRange { point: 4, .. }
        ));
    }

    #[test]
    fn replay_rejected() {
        let mut ev = evaluator();
        let _ = ev.evaluate(&snapshot(10_000), 10_000).expect("first");
        // 等值重替身幂等放行（68.2 实弹订正二）。
        let _ = ev.evaluate(&snapshot(10_000), 11_000).expect("幂等重评");
        // 序号严格回退（旧消息）拒绝。
        let mut regressed = snapshot(12_000);
        for x in regressed.iter_mut() {
            x.sequence = 0;
        }
        let err = ev.evaluate(&regressed, 12_000).unwrap_err();
        assert!(matches!(err, TrustRejection::Replay { point: 1, .. }));
        // 序号递增但接收时刻倒退 → 时钟不单调。
        let mut s = snapshot(11_000);
        for x in s.iter_mut() {
            x.sequence = 2;
            x.received_at_ms = 9_000;
        }
        assert!(matches!(
            ev.evaluate(&s, 11_000).unwrap_err(),
            TrustRejection::ClockNotMonotonic { .. }
        ));
    }

    #[test]
    fn source_conflict_rejected() {
        let mut ev = evaluator();
        let mut s = snapshot(10_000);
        let mut dup = good(1, 381.0, 1, 9_900, 10_000);
        dup.source = 2;
        s.push(dup);
        assert!(matches!(
            ev.evaluate(&s, 10_000).unwrap_err(),
            TrustRejection::SourceConflict { point: 1, .. }
        ));
    }

    #[test]
    fn unknown_source_rejected() {
        let mut ev = evaluator();
        let mut s = snapshot(10_000);
        s[0].source = 99;
        assert!(matches!(
            ev.evaluate(&s, 10_000).unwrap_err(),
            TrustRejection::UnknownSource { point: 1, .. }
        ));
    }

    #[test]
    fn stale_epoch_rejected_after_reconnect() {
        // 68.2 per-source 语义：断线重连由采集侧分配更高 epoch 表达（
        // open_source 严格递增），消费侧 max 吸收（前跳放行）；旧 epoch
        // 样本 = 同源回退，结构化拒绝。
        let mut ev = evaluator();
        let _ = ev.evaluate(&snapshot(10_000), 10_000).expect("epoch 0");
        // 断线重连：同源新 epoch（前跳）接受。
        let mut s = snapshot(20_000);
        for x in s.iter_mut() {
            x.source_epoch = 1;
        }
        let t = ev.evaluate(&s, 20_000).expect("epoch 1 trusted");
        assert_eq!(t.source_epoch, 1);
        // 重启重放形态：旧 epoch 样本必须拒绝（不误接受旧消息）。
        let err = ev.evaluate(&snapshot(30_000), 30_000).unwrap_err();
        assert!(matches!(
            err,
            TrustRejection::SourceEpochMismatch { point: 1, .. }
        ));
    }

    #[test]
    fn integrity_required_rejected() {
        let mut ev = evaluator();
        let mut s = snapshot(10_000);
        s[0].integrity = IntegrityStatus::Unverified;
        assert!(matches!(
            ev.evaluate(&s, 10_000).unwrap_err(),
            TrustRejection::IntegrityFailure { point: 1, .. }
        ));
    }

    #[test]
    fn restore_state_rejects_old_epoch() {
        let mut ev = evaluator();
        ev.restore(&[(1, 5)], 100_000, BTreeMap::new());
        let err = ev.evaluate(&snapshot(200_000), 200_000).unwrap_err();
        assert!(matches!(
            err,
            TrustRejection::SourceEpochMismatch {
                epoch: 0,
                expected: 5,
                ..
            }
        ));
        let mut s = snapshot(200_000);
        for x in s.iter_mut() {
            x.source_epoch = 5;
        }
        let _ = ev.evaluate(&s, 200_000).expect("restored epoch accepted");
    }

    /// t3（WP-68.2.1）：同源 epoch 回退仍拒绝（防修复扩大化——per-source
    /// 归位只消灭异源互毒，不放松同源单调性）。t1/t2 绿后仍须绿。
    #[test]
    fn t3_same_source_regression_still_quarantined() {
        let mut ev = dual_evaluator();
        // 源 A epoch=100 建立基线。
        let _ = ev
            .evaluate(&source_snapshot(1, 100, 0, 10_000), 10_000)
            .expect("A epoch=100 首评");
        // 同源回退（99 < 100）→ SourceEpochMismatch（隔离语义维持）。
        let err = ev
            .evaluate(&source_snapshot(1, 99, 200, 11_000), 11_000)
            .unwrap_err();
        assert!(matches!(
            err,
            TrustRejection::SourceEpochMismatch { expected: 100, .. }
        ));
        // 同源等值/前跳照常放行。
        let _ = ev
            .evaluate(&source_snapshot(1, 100, 200, 12_000), 12_000)
            .expect("等值放行");
        let _ = ev
            .evaluate(&source_snapshot(1, 101, 300, 13_000), 13_000)
            .expect("前跳放行（重启新 epoch）");
    }

    /// t5（WP-68.2.1）：per-source 表三态（未见/推进/回退）单元锁 +
    /// 容量（= MAX_SOURCES=64）。
    #[test]
    fn t5_evaluator_multi_source_state() {
        let mut t = SourceEpochTable::new();
        // 未见 → 放行。
        assert_eq!(t.judge(1, 100), Ok(()));
        assert_eq!(t.judge(2, 200), Ok(()));
        t.commit(1, 100);
        t.commit(2, 200);
        // 同源等值/前跳放行；commit 只升不降。
        assert_eq!(t.judge(1, 100), Ok(()));
        assert_eq!(t.judge(1, 300), Ok(()));
        t.commit(1, 50);
        assert_eq!(t.judge(1, 99), Err(100), "commit(50) 不得拉低已见 max=100");
        // 同源回退拒绝（载荷 = 已见最大值）。
        assert_eq!(t.judge(2, 199), Err(200));
        // 异源互不重置：A 回退判拒不影响 B 独立推进。
        t.commit(2, 201);
        assert_eq!(t.judge(2, 201), Ok(()));
        assert_eq!(t.judge(1, 99), Err(100));
        // 容量：64 源全容（config-bundle points::MAX_SOURCES 上限）。
        for s in 3u32..=64 {
            t.commit(s, u64::from(s));
        }
        assert_eq!(t.judge(64, 64), Ok(()));
        assert_eq!(t.judge(64, 63), Err(64));
    }

    #[test]
    fn required_point_unit_documented_in_coverage() {
        let ev = evaluator();
        let soc = ev.config.coverage.find(4).expect("soc covered");
        assert_eq!(soc.unit, PointUnit::Ratio);
    }

    // ---- 蓝图 68.2 红测群（WP-68.2.1；F13 每服务 floor 不等 + 消费侧
    // 单全局 epoch = "最后钉扎者赢"毒化的评估器级行为锁）----

    /// 双源评估器（8 点全覆盖，来源白名单 [1,2]）。
    fn dual_evaluator() -> TelemetryTrustEvaluator {
        TelemetryTrustEvaluator::new(EvaluatorConfig {
            coverage: RequiredPointCoverage {
                points: (1u32..=8)
                    .map(|p| RequiredPoint {
                        point: p,
                        min: 0.0,
                        max: 1000.0,
                        max_age_ms: 5000,
                        unit: PointUnit::Kilowatt,
                    })
                    .collect(),
            },
            bindings: StateBindings {
                voltage: 1,
                current: 2,
                frequency: 3,
                soc: 4,
            },
            mapping_version: 1,
            max_skew_ms: 1000,
            allowed_sources: alloc::vec![1, 2],
            require_integrity: true,
            control_horizon_ms: 30_000,
        })
    }

    /// 单源满快照（8 点；seq = 序号基，received 单调递增由调用方保证）。
    fn source_snapshot(source: u32, epoch: u64, seq: u64, received: u64) -> Vec<TelemetrySample> {
        (1u32..=8)
            .map(|p| TelemetrySample {
                point: p,
                value: 100.0 + f64::from(p),
                source,
                source_epoch: epoch,
                measured_at_ms: received - 50,
                received_at_ms: received,
                quality: SampleQuality::Good,
                sequence: seq + u64::from(p),
                integrity: IntegrityStatus::Verified,
                mapping_version: 1,
            })
            .collect()
    }

    /// t1（WP-68.2.1）：双源交错投递互不毒化——源 B（高 epoch）到达后，
    /// 源 A（低 epoch）样本评估必须照常全过。现状（单全局 epoch）红。
    #[test]
    fn t1_dual_source_no_cross_poison() {
        let mut ev = dual_evaluator();
        // 源 A（epoch=100）首评。
        let _ = ev
            .evaluate(&source_snapshot(1, 100, 0, 10_000), 10_000)
            .expect("源 A 首评");
        // 源 B（epoch=200）到达。
        let _ = ev
            .evaluate(&source_snapshot(2, 200, 0, 11_000), 11_000)
            .expect("源 B 首评");
        // B 到达后 A 仍全过（毒化点本体）；交错第二轮对称。
        let _ = ev
            .evaluate(&source_snapshot(1, 100, 100, 12_000), 12_000)
            .expect("B 到达后 A 不被毒化");
        let _ = ev
            .evaluate(&source_snapshot(2, 200, 100, 13_000), 13_000)
            .expect("A 再达后 B 不被毒化");
    }

    /// t2（WP-68.2.1）：启动序翻转（B 先开 A 后开）结论与 t1 对称——毒化
    /// 方向不随启动序翻转（现状红）。
    #[test]
    fn t2_boot_order_independent() {
        let mut ev = dual_evaluator();
        let _ = ev
            .evaluate(&source_snapshot(2, 200, 0, 10_000), 10_000)
            .expect("源 B 先开");
        let _ = ev
            .evaluate(&source_snapshot(1, 100, 0, 11_000), 11_000)
            .expect("源 A 后开不毒化");
        let _ = ev
            .evaluate(&source_snapshot(2, 200, 100, 12_000), 12_000)
            .expect("B 续过");
        let _ = ev
            .evaluate(&source_snapshot(1, 100, 100, 13_000), 13_000)
            .expect("A 续过");
    }

    /// 点位区间快照（[lo, hi]；供跨源混合构造——同点异源在快照内属
    /// SourceConflict，混合用例须点位互斥）。
    fn span(
        source: u32,
        epoch: u64,
        seq: u64,
        received: u64,
        lo: u32,
        hi: u32,
    ) -> Vec<TelemetrySample> {
        (lo..=hi)
            .map(|p| TelemetrySample {
                point: p,
                value: 100.0 + f64::from(p),
                source,
                source_epoch: epoch,
                measured_at_ms: received - 50,
                received_at_ms: received,
                quality: SampleQuality::Good,
                sequence: seq + u64::from(p),
                integrity: IntegrityStatus::Verified,
                mapping_version: 1,
            })
            .collect()
    }

    /// t7（68.2 实弹订正锁）：接收时刻判据同源化。QEMU 实弹抓到的次生
    /// 缺陷——全局接收基线 + 跨源 max_skew 在混合缓冲下永假阳
    /// （ClockNotMonotonic 数千次、agent=OK 60→1）：A 源新批抬高全局
    /// 基线后 B 源同轮较旧批必拒。同源化后跨源交错放行；同源失聚/同源
    /// 跨快照回退仍拒（安全语义不放松）。
    #[test]
    fn t7_dual_source_receipt_independence() {
        let mut ev = dual_evaluator();
        // A 源全量首评建立基线 10_000。
        let _ = ev
            .evaluate(&source_snapshot(1, 100, 0, 10_000), 10_000)
            .expect("A 全量首评");
        // 混合快照（点位互斥拼全量）：A 点 1-4 @11_000 + B 点 5-8 @9_500
        // （B 接收早于 A 历史基线——交错形态；且同快照跨源相距
        // 1_500ms > max_skew=1_000）。全局基线/skew 下必炸
        // ClockNotMonotonic；同源化后放行。
        let mut mixed = span(1, 100, 100, 11_000, 1, 4);
        mixed.extend(span(2, 200, 0, 9_500, 5, 8));
        let _ = ev.evaluate(&mixed, 11_000).expect("跨源接收交错不互拒");
        // 同源异期散布同样放行（68.2 实弹订正三：缓冲评估形态 = 每点最新
        // 替身，同源散布 = 点位周期差，非数据病理；原 max_skew 同快照内聚
        // 检查退役，新鲜度由逐点 Stale 承载）——A 源内 3_000ms 散布 +
        // B 正常，整体放行。
        let mut spread = span(1, 100, 200, 12_000, 1, 4);
        for x in spread.iter_mut().skip(2) {
            x.received_at_ms = 15_000;
        }
        spread.extend(span(2, 200, 100, 12_000, 5, 8));
        let _ = ev.evaluate(&spread, 15_000).expect("同源异期散布不误拒");
        // 订正四锁：异期替身的重替身幂等放行——基线提交取该源 min（收敛
        // 到最老仍在替样本），慢点位老替身不得被基线追赶打死（第三轮
        // QEMU 残余 1.9 万次 ClockNotMonotonic 误拒、agent=OK 33<60 的
        // 根源；max 提交形态下本断言必炸）。
        let _ = ev.evaluate(&spread, 16_000).expect("异期替身重替身幂等");
        // 同源跨快照回退仍拒（A 全量 @9_000 < A 基线 11_000/15_000；单调
        // 性检查先于覆盖面，结构化 ClockNotMonotonic）。
        assert!(matches!(
            ev.evaluate(&source_snapshot(1, 100, 300, 9_000), 16_000)
                .unwrap_err(),
            TrustRejection::ClockNotMonotonic { .. }
        ));
    }
}

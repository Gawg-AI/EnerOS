//! 快照构建器（[08] §3.3 一致性算法 1-8）：冻结 cut 与版本组 → 选择 →
//! 校验 → 冲突裁决 → 派生 → 完整度 → provenance 根。

use alloc::vec::Vec;

use eneros_semantic_catalog::{SemanticCatalog, VersionSet};
use eneros_telemetry_model::event::{IntegrityTag, ObservationEvent};
use sha2::{Digest, Sha256};

use crate::conflict::{Conflict, ConflictResolver};
use crate::policy::SnapshotPolicy;
use crate::projector::StateProjector;
use crate::watermark::SnapshotWatermark;
use crate::{
    CompletenessReport, DerivedValue, NormalizedObservation, RejectReason, SnapshotRejected,
    SnapshotRequest, TrustedSnapshot,
};

/// 域串（与 [02] §5 冻结；任何改变必须升 schema 版本）。
const SNAPSHOT_ROOT_DOMAIN: &[u8] = b"eneros.snapshot.v1";
const SNAPSHOT_ID_DOMAIN: &[u8] = b"eneros.snapshot-id.v1";

/// 派生公式注册表（id/version 随值记录；[08] §3.3 步骤 6）。
#[derive(Debug, Clone, Copy)]
pub enum Formula {
    /// 百分比 → 无量纲比例（SOC 0..100 → 0..1）。
    PercentToRatio = 1,
}

/// 构建错误（全部结构化：写明缺什么/为什么）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotBuilderError {
    Rejected(SnapshotRejected),
}

impl From<SnapshotRejected> for SnapshotBuilderError {
    fn from(r: SnapshotRejected) -> Self {
        SnapshotBuilderError::Rejected(r)
    }
}

/// 构建器（无状态；全部确定性——同输入同 root）。
#[derive(Debug, Clone, Default)]
pub struct SnapshotBuilder;

impl SnapshotBuilder {
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        &self,
        request: SnapshotRequest,
        policy: &SnapshotPolicy,
        now_ns: u64,
        catalog: &dyn SemanticCatalog,
        projector: &StateProjector,
        watermarks: &SnapshotWatermark,
        resolver: &ConflictResolver,
    ) -> Result<TrustedSnapshot, SnapshotBuilderError> {
        let version_set = catalog.version_set();
        // 1. 固定目录版本组（policy 与目录同代激活）。
        if policy.catalog_version != version_set.contract_version {
            return Err(SnapshotRejected {
                purpose_id: request.purpose_id,
                reason: RejectReason::CatalogVersionMismatch,
                keys: Vec::new(),
                watermark_deficit: Vec::new(),
            }
            .into());
        }
        let cut = request.as_of_ns;
        // 2. watermark：迟到缺口（deficit 由 module 预检；此处复验）。
        let deficit = watermarks.deficit_before(cut, policy.allowed_lateness_ns);
        if !deficit.is_empty() {
            return Err(SnapshotRejected {
                purpose_id: request.purpose_id,
                reason: RejectReason::WatermarkNotReached,
                keys: Vec::new(),
                watermark_deficit: deficit,
            }
            .into());
        }

        let mut accepted: Vec<NormalizedObservation> = Vec::new();
        let mut missing = Vec::new();
        let mut stale = Vec::new();
        let mut bad = Vec::new();
        let mut unknown_source = Vec::new();
        let mut conflicts: Vec<Conflict> = Vec::new();
        let mut substituted = Vec::new();
        let mut events_by_point: Vec<(u32, ObservationEvent)> = Vec::new();

        // 3-5. 每 required key：选择 `event_time <= cut` 的最新合格事件。
        for &point in policy.required.iter() {
            let candidates: Vec<ObservationEvent> = projector
                .latest_by_source(point)
                .copied()
                .filter(|e| e.event_time_ns <= cut)
                .collect();
            if candidates.is_empty() {
                missing.push(point);
                continue;
            }
            // 来源白名单（[02] §4.2：未注册来源只能进 quarantine）——
            // 白名单外来源参与候选 → 拒绝本点（fail-closed 逐 key）。
            if let Some(allow) = policy.allowed_sources.as_ref() {
                if candidates.iter().any(|e| !allow.contains(&e.source_id)) {
                    unknown_source.push(point);
                    bad.push(point); // 完整度如实归类（bad 集含具体键）
                    continue;
                }
            }
            // 冲突裁决（多源同点；裁决证据进 conflicts）。
            let (chosen, was_conflict) = match resolver.resolve(point, &candidates) {
                Some((c, ev)) => {
                    if matches!(c.decision, crate::conflict::ConflictDecision::Unresolved) {
                        // 不可裁决 → conflict 计数（完整度/拒绝按 conflict）；
                        // 不参与本点取值（[08] §3.3 步骤 5）。
                        conflicts.push(c);
                        continue;
                    }
                    conflicts.push(c);
                    (ev, true)
                }
                None => (
                    *candidates
                        .iter()
                        .max_by_key(|e| (e.event_time_ns, e.sequence))
                        .unwrap(),
                    false,
                ),
            };
            // 逐项校验（freshness/quality/integrity/unit/mapping/asset/
            // topology——[08] §3.3 步骤 4）。
            let key_result =
                validate_observation(point, &chosen, cut, now_ns, policy, catalog, version_set);
            match key_result {
                Ok(ev) => {
                    if was_conflict {
                        if let Some(c) = conflicts.iter_mut().find(|c| c.point_id == point) {
                            c.decision = crate::conflict::ConflictDecision::Accepted {
                                source: ev.source_id,
                                sequence: ev.sequence,
                            };
                        }
                    }
                    accepted.push(ev.into());
                    events_by_point.push((point, ev));
                }
                Err((reason, _key)) => match reason {
                    // 可替代场景：按 policy 替代（[08] §3.4）。
                    RejectReason::MissingKeys | RejectReason::StaleKeys
                        if policy.substitution.is_some() =>
                    {
                        if let Some(ev) =
                            substitute_event(point, policy, cut, projector, catalog, version_set)
                        {
                            substituted.push(point);
                            accepted.push(ev.into());
                            events_by_point.push((point, ev));
                        } else {
                            missing.push(point);
                        }
                    }
                    RejectReason::StaleKeys => stale.push(point),
                    RejectReason::BadQuality => bad.push(point),
                    other => {
                        missing.push(point);
                        let _ = other;
                    }
                },
            }
        }

        // 6. 派生量（公式注册表；确定性）。
        let mut derived: Vec<DerivedValue> = Vec::new();
        for (_point, ev) in events_by_point.iter() {
            if let Some(v) = derive(Formula::PercentToRatio, *ev) {
                derived.push(v);
            }
        }

        // 7-8. 完整度 + provenance root + snapshot_id。
        let completeness = CompletenessReport {
            required: policy.required.clone(),
            accepted: events_by_point.iter().map(|(p, _)| *p).collect(),
            missing: missing.clone(),
            stale: stale.clone(),
            bad: bad.clone(),
            conflict: conflicts
                .iter()
                .filter(|c| matches!(c.decision, crate::conflict::ConflictDecision::Unresolved))
                .map(|c| c.point_id)
                .collect(),
            substituted: substituted.clone(),
        };
        // required 缺失（且非降级 purpose）→ 结构化拒绝。
        let incomplete = !completeness.is_complete();
        if incomplete
            && !policy.degraded_ok
            && (!missing.is_empty()
                || !stale.is_empty()
                || !bad.is_empty()
                || !unknown_source.is_empty()
                || !completeness.conflict.is_empty())
        {
            let reason = if !missing.is_empty() {
                RejectReason::MissingKeys
            } else if !stale.is_empty() {
                RejectReason::StaleKeys
            } else if !unknown_source.is_empty() {
                RejectReason::UnknownSource
            } else if !bad.is_empty() {
                RejectReason::BadQuality
            } else {
                RejectReason::UnresolvedConflict
            };
            let mut keys = missing.clone();
            keys.extend(stale.iter().copied());
            keys.extend(bad.iter().copied());
            keys.extend(unknown_source.iter().copied());
            keys.extend(completeness.conflict.iter().copied());
            return Err(SnapshotRejected {
                purpose_id: request.purpose_id,
                reason,
                keys,
                watermark_deficit: Vec::new(),
            }
            .into());
        }

        let watermarks_out = watermarks.watermarks();
        let provenance_root = provenance_root(
            version_set,
            request.purpose_id,
            cut,
            &events_by_point,
            &watermarks_out,
            &conflicts,
        );
        let mut id_buf = [0u8; 16];
        {
            let mut h = Sha256::new();
            h.update(SNAPSHOT_ID_DOMAIN);
            h.update(provenance_root);
            h.update(now_ns.to_le_bytes());
            let out = h.finalize();
            id_buf.copy_from_slice(&out[..16]);
        }
        Ok(TrustedSnapshot {
            snapshot_id: id_buf,
            tenant_id: request.tenant_id,
            site_id: request.site_id,
            cut_time_ns: cut,
            produced_at_ns: now_ns,
            policy_version: policy.policy_version,
            version_set,
            observations: accepted,
            source_watermarks: watermarks_out,
            completeness,
            conflicts,
            derived,
            provenance_root,
        })
    }
}

/// 观测校验：freshness/quality/integrity/unit/mapping/asset/topology。
/// 返回 `Ok(事件)` 或 `(拒绝原因, 点)`。
fn validate_observation(
    point: u32,
    ev: &ObservationEvent,
    cut: u64,
    _now_ns: u64,
    policy: &SnapshotPolicy,
    catalog: &dyn SemanticCatalog,
    version_set: VersionSet,
) -> Result<ObservationEvent, (RejectReason, u32)> {
    // freshness：cut - event_time <= max_age。
    if cut.saturating_sub(ev.event_time_ns) > policy.max_age_ns {
        return Err((RejectReason::StaleKeys, point));
    }
    // quality：控制热路径最低质量（valid 且无 invalid/blocked/overflow）。
    if !eneros_telemetry_model::event::quality_bits::is_control_eligible(ev.quality_bits) {
        return Err((RejectReason::BadQuality, point));
    }
    // integrity：policy 要求 Authenticated。
    if policy.require_integrity && ev.integrity != IntegrityTag::Authenticated {
        return Err((RejectReason::BadQuality, point));
    }
    // 映射世代逐事件校验：事件 mapping_version ≠ 版本组 → 不合格
    // （[08] §2.7 门：映射版本混搭不得进入控制）。
    if ev.mapping_version != version_set.mapping_version {
        return Err((RejectReason::MissingKeys, point));
    }
    // unit/mapping/asset：目录解析（未知 point → MissingKeys 类）。
    let key = eneros_semantic_catalog::ObservationKey {
        tenant_id: ev.tenant_id,
        site_id: ev.site_id,
        source_id: ev.source_id,
        device_id: ev.device_id,
        point_id: ev.point_id,
        mapping_version: ev.mapping_version,
    };
    match catalog.resolve_observation(&key, version_set) {
        Ok(sem) if sem.control_eligible() || ev.asset_id != 0 => {}
        Ok(_) => return Err((RejectReason::BadQuality, point)),
        Err(e) => {
            // 版本/未知点全部按缺失处理（fail-closed，[08] §2.7 门：
            // 单位和资产缺失事件不能进入控制）。
            let _ = e;
            return Err((RejectReason::MissingKeys, point));
        }
    }
    // topology：站点拓扑视图可得（版本不符 → 视为缺失）。
    if catalog
        .topology_view(ev.site_id, version_set.topology_version)
        .is_err()
    {
        return Err((RejectReason::MissingKeys, point));
    }
    Ok(*ev)
}

/// 替代值（[08] §3.4）：来源优先级 + 最大持续时长 + 用途白名单；
/// 禁止 0/默认/无限续期。
fn substitute_event(
    point: u32,
    policy: &SnapshotPolicy,
    cut: u64,
    projector: &StateProjector,
    catalog: &dyn SemanticCatalog,
    version_set: VersionSet,
) -> Option<ObservationEvent> {
    let rule = policy.substitution.as_ref()?;
    if !rule.allowed_purposes.contains(&policy.purpose_id) {
        return None;
    }
    let mut candidates: Vec<ObservationEvent> = projector
        .latest_by_source(point)
        .copied()
        .filter(|e| e.event_time_ns <= cut)
        .collect();
    // 优先级排序（priority[0] 最高）。
    candidates.sort_by_key(|e| {
        rule.source_priority
            .iter()
            .position(|s| *s == e.source_id)
            .unwrap_or(usize::MAX)
    });
    for ev in candidates.iter() {
        let age = cut.saturating_sub(ev.event_time_ns);
        if age > rule.max_duration_ns {
            continue;
        }
        // 替代事件仍须过目录身份（来源优先级已显式白名单）。
        let key = eneros_semantic_catalog::ObservationKey {
            tenant_id: ev.tenant_id,
            site_id: ev.site_id,
            source_id: ev.source_id,
            device_id: ev.device_id,
            point_id: ev.point_id,
            mapping_version: ev.mapping_version,
        };
        if catalog.resolve_observation(&key, version_set).is_err() {
            continue;
        }
        return Some(*ev);
    }
    None
}

/// 派生计算（公式 1：Percent → Ratio；单位码 13 → 18）。
fn derive(formula: Formula, ev: ObservationEvent) -> Option<DerivedValue> {
    match formula {
        Formula::PercentToRatio => {
            use eneros_telemetry_model::unit_catalog;
            let unit_percent = unit_catalog::code_of(eneros_telemetry_model::PhysicalUnit::Percent);
            if ev.unit_code != unit_percent {
                return None;
            }
            let ObservationEvent { value, .. } = ev;
            let f = match value {
                eneros_telemetry_model::event::ObservationValue::F64(v) => v,
                _ => return None,
            };
            if !f.is_finite() {
                return None;
            }
            Some(DerivedValue {
                point_id: ev.point_id,
                formula_id: Formula::PercentToRatio as u32,
                formula_version: 1,
                value: f / 100.0,
                unit_code: unit_catalog::code_of(
                    eneros_telemetry_model::PhysicalUnit::Dimensionless,
                ),
            })
        }
    }
}

/// provenance 根（[02] §5 冻结算法；Canonical 全定长小端）。
pub(crate) fn provenance_root(
    version_set: VersionSet,
    purpose_id: u32,
    cut_time_ns: u64,
    events: &[(u32, ObservationEvent)],
    watermarks: &[crate::SourceWatermark],
    conflicts: &[Conflict],
) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(SNAPSHOT_ROOT_DOMAIN);
    // Canonical(VersionSet)：6 × u64。
    for v in [
        version_set.generation,
        version_set.mapping_version,
        version_set.topology_version,
        version_set.asset_version,
        version_set.contract_version,
        version_set.safety_version,
    ] {
        h.update(v.to_le_bytes());
    }
    h.update(purpose_id.to_le_bytes());
    h.update(cut_time_ns.to_le_bytes());
    // sorted(key,event_id)：按 (point, source) 排序定长条目。
    let mut sorted: Vec<(u32, u32, [u8; 16])> = events
        .iter()
        .map(|(p, e)| (*p, e.source_id, e.event_id()))
        .collect();
    sorted.sort();
    for (p, s, id) in sorted.iter() {
        h.update(p.to_le_bytes());
        h.update(s.to_le_bytes());
        h.update(id);
    }
    // sorted(watermark)：按 source_id。
    let mut ws: Vec<&crate::SourceWatermark> = watermarks.iter().collect();
    ws.sort_by_key(|w| w.source_id);
    for w in ws.iter() {
        h.update(w.source_id.to_le_bytes());
        h.update(w.max_event_time_ns.to_le_bytes());
        h.update(w.max_ingest_time_ns.to_le_bytes());
        h.update(w.epoch.to_le_bytes());
        h.update(w.sequence.to_le_bytes());
    }
    // sorted(conflict_decision_digest)：按 point。
    let mut cs: Vec<&Conflict> = conflicts.iter().collect();
    cs.sort_by_key(|c| c.point_id);
    for c in cs.iter() {
        h.update(c.point_id.to_le_bytes());
        match c.decision {
            crate::conflict::ConflictDecision::Accepted { source, sequence } => {
                h.update([1u8]);
                h.update(source.to_le_bytes());
                h.update(sequence.to_le_bytes());
            }
            crate::conflict::ConflictDecision::Unresolved => {
                h.update([0u8]);
                h.update([0u8; 12]);
            }
        }
    }
    h.finalize().into()
}

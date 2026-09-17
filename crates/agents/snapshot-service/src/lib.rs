//! EnerOS 蓝图 61 v0.314.34 —— SnapshotModule。
//!
//! 唯一不变量：**每次决策只消费 `TrustedSnapshot`**——一个 event-time
//! 一致切面，明确列出完整、缺失、过期、冲突、替代和 provenance
//! （[08] §3.1）。决策代码不直接读「当前 map」；`SnapshotModule::build`
//! 是唯一入口（[02] §5）。
//!
//! 组件（[08] §3.2）：StateProjector（最近合格事件投影）、SnapshotWatermark
//! （每 source 最大 event/ingest 时间、epoch、sequence）、
//! SnapshotPolicyRegistry（按 purpose 的 required/optional/freshness/
//! quality/integrity/substitution）、ConflictResolver（确定性优先/多数/
//! 主备 + 裁决证据）、SnapshotBuilder（冻结 cut 与 VersionSet、provenance
//! 根）、SnapshotCatalog（按 snapshot_id 查元数据与 blob 偏移）。
//!
//! # 机制状态（蓝图 68 卷 68.4 WP-68.4.2 解冻翻转——原总册 ID SM-deferred）
//!
//! **SnapshotModule 生产入口已接电**（svc-telemetry 快照面：catalog 经
//! svc-config 惰性装载 → 政策 required=安全覆盖点 → 8 批窗口构建 →
//! blob 落 DATA_SNAP_* LFS 卷；装配在 control-services::event_face，
//! 快照触发源 = OBS 事件流）。certifier 生产消费仍未接线（68.5 后
//! certifier 接电卷；可达性已验证——certifier tests/integration.rs
//! wp6844 冒烟）。解冻条件="68.3 块面 + 68.4 接线完成"（Z8），证据 =
//! evidence/68/vol68-4/ 双窗 QEMU 日志。
//!
//! 依赖纪律（[16] §3.4）：禁止依赖 LLM、solver、control dispatch。
//! 跨 binary 必须走版本化分页或 BlobRef（[02] §5）；本 crate 提供
//! 定长规范编码 [`wire`]，分页承载由传输层（agent-sdk chunked）负责。

#![no_std]
// 63.3 WP-63.3.3：crate 级 blanket allow 已撤（R-63.b）——机制面死项
// 逐项门控/allow + 登记注释（总册 ID SM-deferred），交付态不再整体静默。

extern crate alloc;

use alloc::boxed::Box;

pub mod builder;
pub mod c1;
pub mod conflict;
pub mod consumer;
pub mod policy;
pub mod projection;
pub mod projector;
pub mod store;
pub mod watermark;
pub mod wire;

use alloc::vec::Vec;
use core::fmt;

pub use builder::{SnapshotBuilder, SnapshotBuilderError};
pub use conflict::{Conflict, ConflictDecision, ConflictResolver};
use eneros_semantic_catalog::{CatalogError, VersionSet};
use eneros_telemetry_model::event::ObservationEvent;
pub use policy::{SnapshotPolicy, SnapshotPolicyRegistry, SubstitutionRule};
pub use projector::{PointKey, ProjectorCapacity, StateProjector};
pub use store::{SnapshotCatalog, SnapshotMeta};
pub use watermark::{SnapshotWatermark, SourceWatermark};
pub use wire::{decode_snapshot_v1, encode_snapshot_v1};

/// 快照请求（[02] §5 / [08] §3.3 步骤 1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotRequest {
    pub tenant_id: u32,
    pub site_id: u32,
    pub purpose_id: u32,
    /// 一致性切面时刻（event-time 语义）。
    pub as_of_ns: u64,
    /// 等待 watermark 的最长时长（超出后按 policy 决定降级/拒绝）。
    pub max_wait_ns: u64,
    pub policy_version: u64,
}

/// 归一化观测（快照内的观测行；字段与 [`ObservationEvent`] 同型——
/// 单位码/映射世代/asset 已在接入门规范化）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NormalizedObservation {
    pub event: ObservationEvent,
}

impl From<ObservationEvent> for NormalizedObservation {
    fn from(event: ObservationEvent) -> Self {
        Self { event }
    }
}

/// 完整度报告（[02] §5：必须列出计数与具体键；「部分数据也返回 Ok」
/// 不允许）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletenessReport {
    pub required: Vec<u32>,
    pub accepted: Vec<u32>,
    pub missing: Vec<u32>,
    pub stale: Vec<u32>,
    pub bad: Vec<u32>,
    pub conflict: Vec<u32>,
    pub substituted: Vec<u32>,
}

impl CompletenessReport {
    pub fn is_complete(&self) -> bool {
        self.missing.is_empty()
            && self.stale.is_empty()
            && self.bad.is_empty()
            && self.conflict.is_empty()
    }
}

/// 派生量（步骤 6：由合格观测按公式计算；公式 id/version 随值记录）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DerivedValue {
    pub point_id: u32,
    pub formula_id: u32,
    pub formula_version: u16,
    pub value: f64,
    pub unit_code: u16,
}

/// 信任快照（[02] §5 字段集）。
#[derive(Debug, Clone, PartialEq)]
pub struct TrustedSnapshot {
    pub snapshot_id: [u8; 16],
    pub tenant_id: u32,
    pub site_id: u32,
    pub cut_time_ns: u64,
    pub produced_at_ns: u64,
    pub policy_version: u64,
    pub version_set: VersionSet,
    pub observations: Vec<NormalizedObservation>,
    pub source_watermarks: Vec<SourceWatermark>,
    pub completeness: CompletenessReport,
    pub conflicts: Vec<Conflict>,
    pub derived: Vec<DerivedValue>,
    pub provenance_root: [u8; 32],
}

/// 结构化拒绝（必须给出缺什么/为什么；不允许健康默认值）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotRejected {
    pub purpose_id: u32,
    pub reason: RejectReason,
    pub keys: Vec<u32>,
    pub watermark_deficit: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    NoSamples,
    NoPolicy,
    WatermarkNotReached,
    MissingKeys,
    StaleKeys,
    BadQuality,
    UnknownSource,
    CatalogVersionMismatch,
    UnknownUnit,
    UnresolvedConflict,
    RequiredSubstitutionDenied,
}

impl fmt::Display for RejectReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl TrustedSnapshot {
    /// Read a battery's SOC through its catalog binding, never through a shared slot.
    /// Snapshot construction supplies freshness; rejected/ambiguous inputs stay rejected.
    pub fn asset_soc(
        &self,
        catalog: &dyn eneros_semantic_catalog::SemanticCatalog,
        asset_id: u64,
    ) -> Option<f64> {
        use eneros_telemetry_model::{
            event::{quality_bits, ObservationValue},
            unit_catalog, PhysicalUnit,
        };
        if self.version_set != catalog.version_set() {
            return None;
        }
        let point = catalog.asset_soc_point(asset_id)?;
        let c = &self.completeness;
        if [&c.missing, &c.stale, &c.bad, &c.conflict, &c.substituted]
            .iter()
            .any(|v| v.contains(&point))
        {
            return None;
        }
        let mut rows = self
            .observations
            .iter()
            .filter(|o| o.event.point_id == point);
        let ev = &rows.next()?.event;
        if rows.next().is_some()
            || ev.asset_id != asset_id
            || ev.mapping_version != self.version_set.mapping_version
            || ev.unit_code != unit_catalog::code_of(PhysicalUnit::Dimensionless)
            || ev.quality_bits != quality_bits::VALID
        {
            return None;
        }
        match ev.value {
            ObservationValue::F64(v) if v.is_finite() && (0.0..=1.0).contains(&v) => Some(v),
            _ => None,
        }
    }
}

/// 快照模块装配体（深模块门面；外部只调 [`build`] 与 [`apply_events`]）。
pub struct SnapshotModule {
    catalog: Box<dyn eneros_semantic_catalog::SemanticCatalog + 'static>,
    projector: StateProjector,
    watermarks: SnapshotWatermark,
    policies: SnapshotPolicyRegistry,
    resolver: ConflictResolver,
    builder: SnapshotBuilder,
    store: SnapshotCatalog,
    /// 投影逐点最新事件（decision 投影用；BTreeMap 有界）。
    latest: alloc::collections::BTreeMap<u32, ObservationEvent>,
}

/// build 错误（catalog 层错误透传）。
#[derive(Debug)]
pub enum SnapshotError {
    Catalog(CatalogError),
    Rejected(SnapshotRejected),
    Store(alloc::string::String),
}

impl From<CatalogError> for SnapshotError {
    fn from(e: CatalogError) -> Self {
        SnapshotError::Catalog(e)
    }
}

impl core::fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SnapshotError::Catalog(e) => write!(f, "catalog:{e}"),
            SnapshotError::Rejected(r) => write!(f, "rejected:{:?}:{:?}", r.reason, r.keys),
            SnapshotError::Store(s) => write!(f, "store:{s}"),
        }
    }
}

impl SnapshotModule {
    pub fn new(
        catalog: Box<dyn eneros_semantic_catalog::SemanticCatalog + 'static>,
        projector_cap: ProjectorCapacity,
        policies: SnapshotPolicyRegistry,
        resolver: ConflictResolver,
        now_ns: u64,
    ) -> Self {
        let watermarks = SnapshotWatermark::new(now_ns);
        Self {
            catalog,
            projector: StateProjector::new(projector_cap),
            watermarks,
            policies,
            resolver,
            builder: SnapshotBuilder,
            store: SnapshotCatalog::default(),
            latest: alloc::collections::BTreeMap::new(),
        }
    }

    /// 事件流入（ingestion 或 event-store 重放调用；每次流入更新
    /// watermark 与投影）。
    pub fn apply_event(&mut self, ev: ObservationEvent) {
        self.watermarks.observe(
            ev.source_id,
            ev.event_time_ns,
            ev.ingest_time_ns,
            ev.source_epoch,
            ev.sequence,
        );
        self.projector.observe(ev);
        self.latest.insert(ev.point_id, ev);
    }

    /// 批量流入（重放/批次）。
    pub fn apply_events(&mut self, events: &[ObservationEvent]) {
        for ev in events {
            self.apply_event(*ev);
        }
    }

    /// 快照构建唯一入口（[08] §3.3 一致性算法 1-8）。
    pub fn build(
        &mut self,
        request: SnapshotRequest,
        now_ns: u64,
    ) -> Result<TrustedSnapshot, SnapshotError> {
        let policy = self.policies.get(request.purpose_id).ok_or_else(|| {
            SnapshotError::Rejected(SnapshotRejected {
                purpose_id: request.purpose_id,
                reason: RejectReason::NoPolicy,
                keys: Vec::new(),
                watermark_deficit: Vec::new(),
            })
        })?;
        // 2. 等待 required source watermark（同步模块：水位已到 = 通过；
        // 未到 → 若允许等待窗口已耗尽 → watermarks 记 deficit）。
        let deficit = self
            .watermarks
            .deficit_before(request.as_of_ns, policy.allowed_lateness_ns);
        if !deficit.is_empty() && request.max_wait_ns == 0 {
            return Err(SnapshotError::Rejected(SnapshotRejected {
                purpose_id: request.purpose_id,
                reason: RejectReason::WatermarkNotReached,
                keys: Vec::new(),
                watermark_deficit: deficit,
            }));
        }
        self.builder
            .build(
                request,
                policy,
                now_ns,
                self.catalog.as_ref(),
                &self.projector,
                &self.watermarks,
                &self.resolver,
            )
            .map_err(|e| match e {
                builder::SnapshotBuilderError::Rejected(r) => SnapshotError::Rejected(r),
            })
    }

    /// 快照目录（元数据查询；`None` = 该 snapshot_id 未见）。
    pub fn meta(&self, snapshot_id: &[u8; 16]) -> Option<&SnapshotMeta> {
        self.store.meta(snapshot_id)
    }

    /// 最近合格事件（decision 投影输入；`None` = 无该点事件）。
    pub fn latest_event(&self, point_id: u32) -> Option<&ObservationEvent> {
        self.latest.get(&point_id)
    }

    /// 来源水位数（C1/观测）。
    pub fn watermark_count(&self) -> usize {
        self.watermarks.len()
    }

    /// 投影点数（C1/观测）。
    pub fn projected_points(&self) -> usize {
        self.projector.len()
    }

    /// 水位拒绝计数（重放/回拨/旧 epoch——可观测不静默）。
    pub fn watermark_rejected(&self) -> u64 {
        self.watermarks.rejected
    }

    /// 已装载的政策版本（快照携带；catalog 切换后旧 policy 失效）。
    pub fn policy_version(&self, purpose_id: u32) -> Option<u64> {
        self.policies.get(purpose_id).map(|p| p.policy_version)
    }
}

/// 便捷构造：catalog 仓库持有（无堆 trait 对象时用 Box）。
pub fn module_from_catalog(
    catalog: eneros_semantic_catalog::CatalogRepository,
    projector_cap: ProjectorCapacity,
    policies: SnapshotPolicyRegistry,
    resolver: ConflictResolver,
    now_ns: u64,
) -> SnapshotModule {
    SnapshotModule::new(Box::new(catalog), projector_cap, policies, resolver, now_ns)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]
    use super::*;

    /// 目录：semantic-catalog 测试同构最小 payload——直接依赖其测试辅助
    /// 太深；此处用编译面（dev-dep）加载生产 catalog.toml。
    pub(crate) fn prod_catalog() -> eneros_semantic_catalog::CatalogRepository {
        let toml = include_str!("../../../../configs/catalog.toml");
        let compiled =
            eneros_config_bundle::toml_compile::compile_catalog(toml).expect("compile catalog");
        eneros_semantic_catalog::open(&compiled.payload).expect("decode catalog")
    }

    /// 生产形态 7 点观测（点 1-7；epoch=1 生产形态；事件时间=now-50ns）。
    pub(crate) fn obs7(
        now_ns: u64,
        seq0: u64,
        source: u32,
        values: &[f64],
    ) -> Vec<ObservationEvent> {
        obs7_multi(now_ns, values.to_vec(), seq0, source)
    }

    /// 生产形态 7 点观测（多值版；供外部差分测试复用——控制服务
    /// `snapshot_diff` 消费）。
    pub fn obs7_multi(
        now_ns: u64,
        values: Vec<f64>,
        seq0: u64,
        source: u32,
    ) -> Vec<ObservationEvent> {
        use eneros_telemetry_model::event::{IntegrityTag, ObservationValue};
        use eneros_telemetry_model::unit_catalog;
        let units = [
            unit_catalog::code_of(eneros_telemetry_model::PhysicalUnit::Volts),
            unit_catalog::code_of(eneros_telemetry_model::PhysicalUnit::Amperes),
            unit_catalog::code_of(eneros_telemetry_model::PhysicalUnit::Hertz),
            unit_catalog::code_of(eneros_telemetry_model::PhysicalUnit::Dimensionless),
            unit_catalog::code_of(eneros_telemetry_model::PhysicalUnit::Dimensionless),
            unit_catalog::code_of(eneros_telemetry_model::PhysicalUnit::Dimensionless),
            unit_catalog::code_of(eneros_telemetry_model::PhysicalUnit::Dimensionless),
        ];
        (1..=7)
            .zip(values.iter().copied())
            .map(|(point, v)| ObservationEvent {
                tenant_id: 1,
                site_id: 1,
                source_id: source,
                source_epoch: 1,
                sequence: seq0 + point as u64,
                event_time_ns: now_ns.saturating_sub(50),
                ingest_time_ns: now_ns.saturating_sub(40),
                asset_id: 1,
                device_id: 2,
                unit_code: units[(point - 1) as usize],
                point_id: point,
                schema_id: 1,
                schema_version: 1,
                quality_bits: eneros_telemetry_model::event::quality_bits::VALID,
                mapping_version: 1,
                value: ObservationValue::F64(v),
                integrity: IntegrityTag::Authenticated,
                clock_id: 1,
                flags: 0,
            })
            .collect()
    }

    #[test]
    fn module_build_healthy_snapshot() {
        use eneros_semantic_catalog::SemanticCatalog;
        let catalog = prod_catalog();
        let vs = catalog.version_set();
        let mut policies = SnapshotPolicyRegistry::new();
        policies.register(test_policy(vs.contract_version));
        let mut m = module_from_catalog(
            catalog,
            ProjectorCapacity::default(),
            policies,
            ConflictResolver::new(conflict::ConflictRule::PrimaryBackup),
            10_000,
        );
        let now = 1_000_000_000u64;
        m.apply_events(&obs7(now, 100, 2, &[400.0, 50.0, 50.0, 0.6, 1.0, 0.0, 0.0]));
        let snap = m
            .build(
                SnapshotRequest {
                    tenant_id: 1,
                    site_id: 1,
                    purpose_id: 1,
                    as_of_ns: now,
                    max_wait_ns: 0,
                    policy_version: vs.contract_version,
                },
                now,
            )
            .unwrap();
        assert_eq!(snap.observations.len(), 7);
        assert!(snap.completeness.is_complete());
        assert_eq!(snap.observations[0].event.point_id, 1);
        assert_eq!(snap.provenance_root.len(), 32);
        // 重新构建 → 相同 provenance（确定性，[08] §3.5 重放）。
        let snap2 = m
            .build(
                SnapshotRequest {
                    tenant_id: 1,
                    site_id: 1,
                    purpose_id: 1,
                    as_of_ns: now,
                    max_wait_ns: 0,
                    policy_version: vs.contract_version,
                },
                now,
            )
            .unwrap();
        assert_eq!(snap.provenance_root, snap2.provenance_root);
    }

    /// 蓝图 70 R10：optional 点语义兑现（builder 5.5 段）——可选点合格
    /// 时入快照（认证器 readback_ok presence 判据的数据面），缺席时不影响
    /// 完整性（快照照常构建）；required 集行为不变。
    #[test]
    fn optional_point_included_when_present_and_skipped_when_absent() {
        use eneros_semantic_catalog::SemanticCatalog;
        let catalog = prod_catalog();
        let vs = catalog.version_set();
        let mk_policies = |optional: &[u32]| {
            let mut policies = SnapshotPolicyRegistry::new();
            let mut policy = test_policy(vs.contract_version);
            policy.optional = optional.to_vec();
            policies.register(policy);
            policies
        };
        let now = 1_000_000_000u64;
        // ① 可选点在流：入快照。
        let mut m = module_from_catalog(
            catalog.clone(),
            ProjectorCapacity::default(),
            mk_policies(&[1001]),
            ConflictResolver::new(conflict::ConflictRule::PrimaryBackup),
            10_000,
        );
        m.apply_events(&obs7(now, 100, 2, &[400.0, 50.0, 50.0, 0.6, 1.0, 0.0, 0.0]));
        let mut kw = obs7(now, 200, 2, &[0.0; 7]);
        kw.truncate(0);
        // 构造点 1001 事件（kW；asset 2 回读点形态）。
        let mut ev1001 = obs7(now, 300, 2, &[0.0; 7]).pop().unwrap();
        ev1001.point_id = 1001;
        m.apply_events(&alloc::vec![ev1001]);
        let snap = m
            .build(
                SnapshotRequest {
                    tenant_id: 1,
                    site_id: 1,
                    purpose_id: 1,
                    as_of_ns: now,
                    max_wait_ns: 0,
                    policy_version: vs.contract_version,
                },
                now,
            )
            .expect("可选点在场不影响完整性");
        let pts: alloc::collections::BTreeSet<u32> =
            snap.observations.iter().map(|o| o.event.point_id).collect();
        assert!(
            pts.contains(&1001),
            "可选回读点必须入快照（readback_ok 面）"
        );
        assert_eq!(snap.observations.len(), 8);
        assert!(snap.completeness.is_complete());
        // ② 可选点缺席：快照照常构建（缺 1001 不入 missing）。
        let mut m2 = module_from_catalog(
            catalog,
            ProjectorCapacity::default(),
            mk_policies(&[]),
            ConflictResolver::new(conflict::ConflictRule::PrimaryBackup),
            10_000,
        );
        m2.apply_events(&obs7(now, 100, 2, &[400.0, 50.0, 50.0, 0.6, 1.0, 0.0, 0.0]));
        let snap2 = m2
            .build(
                SnapshotRequest {
                    tenant_id: 1,
                    site_id: 1,
                    purpose_id: 1,
                    as_of_ns: now,
                    max_wait_ns: 0,
                    policy_version: vs.contract_version,
                },
                now,
            )
            .expect("可选点缺席不得拒绝快照");
        assert_eq!(snap2.observations.len(), 7);
        assert!(snap2.completeness.is_complete());
        assert!(!snap2.completeness.missing.contains(&1001));
        let _ = kw;
    }

    pub(crate) fn test_policy(contract_version: u64) -> SnapshotPolicy {
        SnapshotPolicy {
            purpose_id: 1,
            policy_version: 1,
            required: alloc::vec![1u32, 2, 3, 4, 5, 6, 7],
            optional: Vec::new(),
            allowed_lateness_ns: 10_000,
            max_age_ns: 500_000,
            min_quality_bits_ok: true,
            require_integrity: true,
            degraded_ok: false,
            substitution: None,
            allowed_sources: None,
            catalog_version: contract_version,
        }
    }

    #[test]
    fn missing_key_rejected_with_keys() {
        use eneros_semantic_catalog::SemanticCatalog;
        let catalog = prod_catalog();
        let vs = catalog.version_set();
        let mut policies = SnapshotPolicyRegistry::new();
        policies.register(test_policy(vs.contract_version));
        let mut m = module_from_catalog(
            catalog,
            ProjectorCapacity::default(),
            policies,
            ConflictResolver::new(conflict::ConflictRule::PrimaryBackup),
            10_000,
        );
        let now = 1_000_000_000u64;
        let mut evs = obs7(now, 100, 2, &[400.0, 50.0, 50.0, 0.6, 1.0, 0.0, 0.0]);
        evs.retain(|e| e.point_id != 4); // 缺 SOC
        m.apply_events(&evs);
        let err = m
            .build(
                SnapshotRequest {
                    tenant_id: 1,
                    site_id: 1,
                    purpose_id: 1,
                    as_of_ns: now,
                    max_wait_ns: 0,
                    policy_version: vs.contract_version,
                },
                now,
            )
            .unwrap_err();
        match err {
            SnapshotError::Rejected(r) => {
                assert_eq!(r.reason, RejectReason::MissingKeys);
                assert!(r.keys.contains(&4));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }
}

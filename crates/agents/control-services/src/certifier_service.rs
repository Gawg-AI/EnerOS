//! 蓝图 70.9 WP-70.9.1/70.9.2/70.9.3——**生产独立认证来源**装配。
//!
//! # 责任
//!
//! 引擎（svc-control-dispatch）只持有 [`CertificateSource`] 接缝；本模块
//! 是它的生产实现：
//!
//! 1. 解码计划 → 读 `snapshot_id`；
//! 2. **独立**向可信来源取冻结快照 blob（[`SnapshotBlobSource`]；生产 =
//!    svc-telemetry 的 `TEL_REQ_SNAPSHOT_BLOB` 通道），解码 `TrustedSnapshot`；
//! 3. 由目录装配 `ResourceRegistry`（铭牌 → 包络基座）并逐资产
//!    `evaluate`（快照身份 + 观测紧缩写进包络）；
//! 4. 目录派生目标文档（合同功率/爬坡 + 覆盖 SOC 窗）与电池参数
//!    （铭牌容量/效率 × 快照 SOC）；
//! 5. `CertifierModule::certify`（独立重算 → 证书 + 逐约束证据 + MAC）。
//!
//! 任一步失败 → 结构化拒绝（零证书、零投递）。生成者（planner/启发式/
//! 远端）**没有**进入本模块的路径。
//!
//! # 密钥（WP-70.9.3 登记）
//!
//! signing key 由控制链注入材派生（域分隔 `eneros.certifier-key.v1`），
//! 与信封密钥不复用同一 key 字节；独立 KMS 条目（生产密钥分离）是蓝图
//! §10 已登记的外部欠账，本卷以域分隔派生提供可用密钥并在执行卡登记。

use alloc::string::String;
use alloc::vec::Vec;

use eneros_control_dispatch::certify::{CertificateSource, CertifierReply, CertifyReject};
use eneros_decision_contract::CandidatePlan;
use eneros_semantic_catalog::CatalogRepository;

/// 冻结快照 blob 来源（生产 = svc-telemetry；host 测试 = 内存假件）。
pub trait SnapshotBlobSource {
    /// 按快照 ID 取回 `encode_snapshot_v1` 字节。
    fn blob(&mut self, snapshot_id: &[u8; 16]) -> Result<Vec<u8>, String>;
}

/// 域分隔派生：控制链注入材 → 认证 signing key。
pub const CERTIFIER_KEY_DOMAIN: &[u8] = b"eneros.certifier-key.v1";

/// 由注入材派生认证 key（32B；`key_id` 固定 1，轮换走
/// [`eneros_certifier::proof::CertifierKeyRing::rotate`]）。
pub fn certifier_key_from_material(material: &[u8]) -> [u8; 32] {
    eneros_decision_contract::keyed::hmac_sha256(material, CERTIFIER_KEY_DOMAIN)
}

/// 生产认证来源（目录 + 快照 blob + key 环）。
pub struct TrustedInputCertifier<B: SnapshotBlobSource> {
    blobs: B,
    catalog: CatalogRepository,
    ring: eneros_certifier::proof::CertifierKeyRing,
    tolerance_profile_id: u32,
    /// 已签发证书数（观测）。
    pub certified: u64,
    /// 拒绝数（观测；含不可达）。
    pub rejected: u64,
    /// 最近一次拒绝原因（诊断；空 = 无）。
    pub last_reject: String,
}

impl<B: SnapshotBlobSource> TrustedInputCertifier<B> {
    /// 装配（目录 + blob 来源 + key 材）。
    pub fn new(
        blobs: B,
        catalog: CatalogRepository,
        key_material: &[u8],
        tolerance_profile_id: u32,
    ) -> Self {
        let key = certifier_key_from_material(key_material);
        let ring = eneros_certifier::proof::CertifierKeyRing::new(1, &key);
        Self {
            blobs,
            catalog,
            ring,
            tolerance_profile_id,
            certified: 0,
            rejected: 0,
            last_reject: String::new(),
        }
    }

    /// 验方 key 环（引擎验真用；与发放侧同一 ring 语义）。
    pub fn ring(&self) -> &eneros_certifier::proof::CertifierKeyRing {
        &self.ring
    }

    /// 轮换 signing key（N-1 验签窗口由 ring 承载）。
    pub fn rotate(&mut self, new_key_id: u16, key_material: &[u8]) -> Result<(), String> {
        let key = certifier_key_from_material(key_material);
        self.ring
            .rotate(new_key_id, &key)
            .map_err(|e| alloc::format!("{e:?}"))
    }

    /// 退役 key（立即失效）。
    pub fn retire(&mut self, key_id: u16) -> bool {
        self.ring.retire(key_id)
    }
}

impl<B: SnapshotBlobSource + 'static> CertificateSource for TrustedInputCertifier<B> {
    /// 验真用环 = 发放侧同一 ring（rotate/retire 立即对执行准入生效）。
    fn verifier_ring(&self) -> Option<&eneros_certifier::proof::CertifierKeyRing> {
        Some(&self.ring)
    }

    fn certify(&mut self, plan_wire: &[u8], now_ms: u64) -> Result<CertifierReply, CertifyReject> {
        let reject = |s: String, this: &mut Self| -> CertifyReject {
            this.rejected = this.rejected.saturating_add(1);
            this.last_reject = s.clone();
            CertifyReject::Rejected(s)
        };
        let plan = match CandidatePlan::decode_v1(plan_wire) {
            Ok(p) => p,
            Err(e) => return Err(reject(alloc::format!("plan-wire:{e:?}"), self)),
        };
        // 1. 独立读取冻结快照（不信请求方随附的任何状态）。
        let blob = match self.blobs.blob(&plan.snapshot_id().0) {
            Ok(b) => b,
            Err(e) => {
                self.rejected = self.rejected.saturating_add(1);
                self.last_reject = alloc::format!("snapshot:{e}");
                return Err(CertifyReject::Unavailable(alloc::format!("snapshot:{e}")));
            }
        };
        let snapshot = match eneros_snapshot_service::decode_snapshot_v1(&blob) {
            Ok(s) => s,
            Err(e) => return Err(reject(alloc::format!("snapshot-decode:{e:?}"), self)),
        };
        // 2. 快照身份复核（防"另一代 blob"）。
        if snapshot.snapshot_id != plan.snapshot_id().0 {
            return Err(reject(String::from("snapshot-id-mismatch"), self));
        }
        // 3. 目录派生上下文（铭牌 → 包络/电池/硬约束）。
        let (inputs, registry) = match eneros_certifier::context::context_inputs_from_catalog(
            &plan,
            &snapshot,
            &self.catalog,
        ) {
            Ok(v) => v,
            Err(e) => return Err(reject(alloc::format!("context:{e:?}"), self)),
        };
        let _ = registry;
        let ctx = eneros_certifier::EvalContext {
            snapshot: &snapshot,
            spec: &inputs.spec,
            envelopes: &inputs.envelopes,
            batteries: &inputs.batteries,
            horizon_step_ns: inputs.horizon_step_ns,
            horizon_step_count: inputs.horizon_step_count,
            horizon_start_ms: inputs.horizon_start_ms,
            grid_asset_id: inputs.grid_asset_id,
            tolerance_profile_id: self.tolerance_profile_id,
            constraint_catalog_version: inputs.catalog_version,
        };
        // 4. 独立重算 + 出证（唯一发放点）。
        let outcome =
            match eneros_certifier::CertifierModule::certify(&ctx, &plan, now_ms, &self.ring) {
                Ok(o) => o,
                Err(e) => return Err(reject(alloc::format!("certify:{e:?}"), self)),
            };
        self.certified = self.certified.saturating_add(1);
        eneros_runtime::println!(
            "[svc-control-dispatch] CERT-OUT constraints={} key={}/{}",
            outcome.records.len(),
            outcome.key_envelope.key_id,
            outcome.key_envelope.key_version
        );
        Ok(CertifierReply {
            certificate: outcome.certificate.encode_v1(),
            key_id: outcome.key_envelope.key_id,
            key_version: outcome.key_envelope.key_version,
            mac: outcome.key_envelope.mac,
        })
    }
}

/// EIPC 冻结快照来源（生产：svc-telemetry `TEL_REQ_SNAPSHOT_BLOB`）。
///
/// 仅在目标构建可达（seL4 EIPC 客户端）；host 装配返回 `None` → 认证
/// 来源缺省 = fail-closed（生产路径不静默降级）。
#[cfg(all(target_os = "none", feature = "sel4"))]
pub struct EipcBlobSource {
    endpoint: alloc::boxed::Box<
        eneros_agent_sdk::endpoint::ChunkedEndpoint<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
    >,
}

#[cfg(all(target_os = "none", feature = "sel4"))]
impl SnapshotBlobSource for EipcBlobSource {
    fn blob(&mut self, snapshot_id: &[u8; 16]) -> Result<Vec<u8>, String> {
        let mut req = alloc::vec::Vec::with_capacity(17);
        req.push(eneros_agent_sdk::tel_wire::TEL_REQ_SNAPSHOT_BLOB);
        req.extend_from_slice(snapshot_id);
        let resp = self
            .endpoint
            .call("svc-telemetry", &req)
            .map_err(|e| alloc::format!("eipc:{e:?}"))?;
        if resp.first() != Some(&eneros_agent_sdk::tel_wire::TEL_RESP_OK) {
            return Err(String::from("snapshot-rejected"));
        }
        Ok(resp[1..].to_vec())
    }
}

/// 生产装配入口（EIPC 授予缺失 → `None`：调用方装 `NoCertifier`）。
///
/// 与 `DispatchHandler::new` 的快照端点装配同源（`bind_cap` + 授予槽）。
#[cfg(all(target_os = "none", feature = "sel4"))]
pub fn eipc_blob_source(mask: u64) -> Option<EipcBlobSource> {
    eipc_blob_source_for("svc-control-dispatch", mask)
}

/// Bind a named service to the same frozen snapshot protocol.
#[cfg(all(target_os = "none", feature = "sel4"))]
pub fn eipc_blob_source_for(client: &str, mask: u64) -> Option<EipcBlobSource> {
    let slot = eneros_agent_sdk::eipc_granted_slot(client, mask, "svc-telemetry")?;
    let endpoint = eneros_agent_sdk::endpoint::ChunkedEndpoint::bind_cap(client, slot)?;
    Some(EipcBlobSource {
        endpoint: alloc::boxed::Box::new(endpoint),
    })
}

/// 引擎侧 key 环装配（验真用；与发放侧同源派生）。
pub fn verifier_ring(key_material: &[u8]) -> eneros_certifier::proof::CertifierKeyRing {
    let key = certifier_key_from_material(key_material);
    eneros_certifier::proof::CertifierKeyRing::new(1, &key)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)] // 测试内判定 panic（clippy.toml 测试模块豁免）

    use super::*;

    struct MemBlobs {
        blob: Vec<u8>,
        id: [u8; 16],
    }
    impl SnapshotBlobSource for MemBlobs {
        fn blob(&mut self, snapshot_id: &[u8; 16]) -> Result<Vec<u8>, String> {
            if snapshot_id == &self.id {
                Ok(self.blob.clone())
            } else {
                Err(String::from("unknown-snapshot"))
            }
        }
    }

    fn catalog() -> CatalogRepository {
        let toml = include_str!("../../../../configs/catalog.toml");
        let compiled = eneros_config_bundle::toml_compile::compile_catalog(toml).unwrap();
        eneros_semantic_catalog::open(&compiled.payload).unwrap()
    }

    /// 域分隔派生：同材同值、不同材不同值、非零。
    #[test]
    fn key_derivation_is_domain_separated() {
        let k = certifier_key_from_material(b"chain-material");
        assert_eq!(k, certifier_key_from_material(b"chain-material"));
        assert_ne!(k, certifier_key_from_material(b"chain-material2"));
        assert_ne!(k, [0u8; 32]);
        assert_ne!(
            k,
            eneros_decision_contract::keyed::hmac_sha256(b"chain-material", b"other-domain"),
            "域分隔必须改变输出"
        );
    }

    /// 未知快照 → 不可达（零证书；调用方零投递）。
    #[test]
    fn unknown_snapshot_is_unavailable() {
        let catalog = catalog();
        let mut c = TrustedInputCertifier::new(
            MemBlobs {
                blob: alloc::vec::Vec::new(),
                id: [9u8; 16],
            },
            catalog,
            b"chain-material",
            2,
        );
        // 计划构造：走 adapter（身份绑定）。
        use eneros_control_dispatch::adapter::{proposal_to_candidate_bound, CandidateInputs};
        let p = eneros_dual_brain::ProposedControlPlan::single_entry(
            String::from("cert-t1"),
            eneros_dual_brain::DecisionPath::Fast,
            -30.0,
            0.5,
            eneros_energy_lp_model::result::SolveStatus::Optimal,
            60,
            1_700_000_000_000,
            Some([0xAB; 32]),
        );
        let inputs =
            CandidateInputs::from_snapshot([3u8; 16], 1, 1_700_000_000_000, 60_000_000_000, 1);
        let plan = proposal_to_candidate_bound(&p, &inputs, (0, 0, 0, 0), 0).unwrap();
        match c.certify(&plan.encode_v1(), 1_700_000_000_000) {
            Err(CertifyReject::Unavailable(_)) => {}
            other => panic!("unexpected: {other:?}"),
        }
        assert_eq!(c.certified, 0);
        assert_eq!(c.rejected, 1);
    }

    // ========================================================================
    // 端到端：目录 + 冻结快照 + 绑定计划 → 证书 → 引擎验真
    // ========================================================================

    fn fixture_snapshot(produced_at_ns: u64) -> eneros_snapshot_service::TrustedSnapshot {
        use eneros_telemetry_model::event::{quality_bits, IntegrityTag, ObservationValue};
        use eneros_telemetry_model::unit_catalog;
        use eneros_telemetry_model::PhysicalUnit;
        let vs = eneros_semantic_catalog::SemanticCatalog::version_set(&catalog());
        let values: [(u32, f64, PhysicalUnit); 8] = [
            (1, 400.0, PhysicalUnit::Volts),
            (2, 50.0, PhysicalUnit::Amperes),
            (3, 50.0, PhysicalUnit::Hertz),
            (4, 0.6, PhysicalUnit::Dimensionless),
            (5, 1.0, PhysicalUnit::Dimensionless),
            (6, 0.0, PhysicalUnit::Dimensionless),
            (7, 0.0, PhysicalUnit::Dimensionless),
            (1001, 0.0, PhysicalUnit::Kilowatts),
        ];
        let observations: Vec<eneros_snapshot_service::NormalizedObservation> = values
            .iter()
            .map(|&(point_id, v, unit)| {
                eneros_telemetry_model::event::ObservationEvent {
                    tenant_id: 1,
                    site_id: 1,
                    source_id: 2,
                    source_epoch: 1,
                    sequence: u64::from(point_id),
                    event_time_ns: produced_at_ns.saturating_sub(1_000),
                    ingest_time_ns: produced_at_ns,
                    asset_id: 1,
                    device_id: 2,
                    unit_code: unit_catalog::code_of(unit),
                    point_id,
                    schema_id: 1,
                    schema_version: 1,
                    quality_bits: quality_bits::VALID,
                    mapping_version: 1,
                    value: ObservationValue::F64(v),
                    integrity: IntegrityTag::Authenticated,
                    clock_id: 1,
                    flags: 0,
                }
                .into()
            })
            .collect();
        eneros_snapshot_service::TrustedSnapshot {
            snapshot_id: [0x5A; 16],
            tenant_id: 1,
            site_id: 1,
            cut_time_ns: produced_at_ns,
            produced_at_ns,
            policy_version: 1,
            version_set: vs,
            observations,
            source_watermarks: Vec::new(),
            completeness: eneros_snapshot_service::CompletenessReport {
                required: alloc::vec![1, 2, 3, 4, 5, 6, 7],
                accepted: alloc::vec![1, 2, 3, 4, 5, 6, 7],
                missing: Vec::new(),
                stale: Vec::new(),
                bad: Vec::new(),
                conflict: Vec::new(),
                substituted: Vec::new(),
            },
            conflicts: Vec::new(),
            derived: Vec::new(),
            provenance_root: [0x3D; 32],
        }
    }

    fn bound_plan_for(
        snapshot_id: [u8; 16],
        catalog_version: u64,
        produced_at_ns: u64,
    ) -> CandidatePlan {
        use eneros_control_dispatch::adapter::{proposal_to_candidate_bound, CandidateInputs};
        let horizon_ms = produced_at_ns / 1_000_000;
        let p = eneros_dual_brain::ProposedControlPlan::single_entry(
            String::from("cert-e2e-1"),
            eneros_dual_brain::DecisionPath::Fast,
            -30.0,
            0.6,
            eneros_energy_lp_model::result::SolveStatus::Optimal,
            60,
            horizon_ms,
            Some([0xAB; 32]),
        );
        let inputs = CandidateInputs::from_snapshot(
            snapshot_id,
            catalog_version,
            horizon_ms,
            60_000_000_000,
            1,
        );
        // 资产绑定在服务入口解析（未绑定 0 → dispatch 默认资产）；本测
        // 直接调认证器，故此处显式绑定目录中的电池资产（asset 1）。
        proposal_to_candidate_bound(&p, &inputs, (1, 2, 10, 0), 6).expect("bound plan")
    }

    /// 正例：目录 + 冻结快照 → 独立认证证书 → 引擎验真通过（约束记录
    /// 非零、plan 绑定一致、MAC 过）。
    #[test]
    fn trusted_certification_end_to_end_and_engine_verifies() {
        let produced_at_ns = 1_700_000_000_000u64 * 1_000_000;
        let snap = fixture_snapshot(produced_at_ns);
        let blob = eneros_snapshot_service::encode_snapshot_v1(&snap);
        let mut c = TrustedInputCertifier::new(
            MemBlobs {
                blob,
                id: snap.snapshot_id,
            },
            catalog(),
            b"chain-material",
            2,
        );
        let plan = bound_plan_for(snap.snapshot_id, 1, produced_at_ns);
        let wire = plan.encode_v1();
        let reply = c.certify(&wire, 1_700_000_000_000).expect("证书签发");
        assert_eq!(c.certified, 1);
        assert_eq!(reply.key_id, 1);
        // 引擎侧验真（与生产同一判据）。
        let cert = eneros_control_dispatch::certify::verify_reply(
            &plan,
            &wire,
            &reply,
            &verifier_ring(b"chain-material"),
            1_700_000_000_000,
        )
        .expect("验真通过");
        assert!(
            cert.hard_constraint_count() > 0,
            "独立重算约束记录必须非零（过渡发放面已删除）"
        );
        assert_eq!(cert.snapshot_id().0, snap.snapshot_id, "证书绑定快照身份");

        // 负例①：错 key（验方环不含发放 key）→ 零投递。
        let bad = eneros_control_dispatch::certify::verify_reply(
            &plan,
            &wire,
            &reply,
            &verifier_ring(b"other-material"),
            1_700_000_000_000,
        );
        assert!(bad.is_err(), "错 key 必须拒绝");
        // 负例②：MAC 篡改。
        let mut tampered = reply.clone();
        tampered.mac[0] ^= 0xFF;
        assert!(eneros_control_dispatch::certify::verify_reply(
            &plan,
            &wire,
            &tampered,
            &verifier_ring(b"chain-material"),
            1_700_000_000_000
        )
        .is_err());
        // 负例③：证书过期。
        assert!(eneros_control_dispatch::certify::verify_reply(
            &plan,
            &wire,
            &reply,
            &verifier_ring(b"chain-material"),
            1_700_000_000_000 + 10_000_000
        )
        .is_err());
        // 负例④：计划换一个（跨代）→ 绑定不符。
        let other = bound_plan_for([0x77; 16], 1, produced_at_ns);
        assert!(eneros_control_dispatch::certify::verify_reply(
            &other,
            &other.encode_v1(),
            &reply,
            &verifier_ring(b"chain-material"),
            1_700_000_000_000
        )
        .is_err());
    }

    /// 负例：跨代快照（计划绑定另一快照 ID）→ 认证拒绝（零证书）。
    #[test]
    fn certification_rejects_cross_generation_plan() {
        let produced_at_ns = 1_700_000_000_000u64 * 1_000_000;
        let snap = fixture_snapshot(produced_at_ns);
        let blob = eneros_snapshot_service::encode_snapshot_v1(&snap);
        let c = TrustedInputCertifier::new(
            MemBlobs {
                blob,
                id: snap.snapshot_id,
            },
            catalog(),
            b"chain-material",
            2,
        );
        let plan = bound_plan_for([0x66; 16], 1, produced_at_ns);
        // 来源"拿到了一代 blob"，但计划绑定的是另一代 → 身份复核必须拒。
        // （来源按 ID 找不到时不达即可达测试见 unknown_snapshot_is_unavailable；
        // 本测覆盖"存在但不同代"这一更危险的形态。）
        struct AnyBlob(Vec<u8>);
        impl SnapshotBlobSource for AnyBlob {
            fn blob(&mut self, _id: &[u8; 16]) -> Result<Vec<u8>, String> {
                Ok(self.0.clone())
            }
        }
        let mut c2 = TrustedInputCertifier::new(
            AnyBlob(eneros_snapshot_service::encode_snapshot_v1(&snap)),
            catalog(),
            b"chain-material",
            2,
        );
        match c2.certify(&plan.encode_v1(), 1_700_000_000_000) {
            Err(CertifyReject::Rejected(ref s)) if s.contains("snapshot-id-mismatch") => {}
            other => panic!("unexpected: {other:?}"),
        }
        assert_eq!(c2.certified, 0, "跨代计划不得出证");
        let _ = c;
    }

    /// 蓝图 70.9.2 硬约束的**非空洞性**（复查红测，双面）：SOC 窗来自
    /// 铭牌 `soc_min/soc_max`（生产目录 0.05/0.95），因此
    /// ① 顶出窗顶的计划必须被独立重算拒绝（MaxSoc 违约，constraint_kind 2）；
    /// ② **同一个计划**在把窗放宽到 [0,1] 的目录下必须出证——证明①的拒绝
    /// 来自铭牌窗而不是别的约束（否则原实现拿量程覆盖 [0,1] 当 SOC 窗，
    /// MinSoc/MaxSoc 恒真，本条硬约束形同虚设）。
    ///
    /// 变异靶：SOC 窗改回覆盖表（或删掉 MaxSoc 入 spec）→ ①变绿（越窗
    /// 计划被接受出证）→ 本测红。
    #[test]
    fn certification_soc_window_is_non_vacuous() {
        let produced_at_ns = 1_700_000_000_000u64 * 1_000_000;
        let horizon_ms = produced_at_ns / 1_000_000;
        // 初始 94 kWh；充电 -2 kW × 3600s × η0.95 = +1.9 kWh → 95.9 kWh：
        // 越铭牌窗顶（95）但远低于满容量（100）。
        let plan_for = |snap_id: [u8; 16]| {
            use eneros_control_dispatch::adapter::{proposal_to_candidate_bound, CandidateInputs};
            let p = eneros_dual_brain::ProposedControlPlan::single_entry(
                String::from("cert-soc-window"),
                eneros_dual_brain::DecisionPath::Fast,
                -2.0,
                0.94,
                eneros_energy_lp_model::result::SolveStatus::Optimal,
                3_600,
                horizon_ms,
                Some([0xAB; 32]),
            );
            let inputs =
                CandidateInputs::from_snapshot(snap_id, 1, horizon_ms, 3_600_000_000_000, 1);
            proposal_to_candidate_bound(&p, &inputs, (1, 2, 10, 0), 6).expect("bound")
        };
        let mut snap = fixture_snapshot(produced_at_ns);
        set_soc(&mut snap, 0.94);
        let blob = eneros_snapshot_service::encode_snapshot_v1(&snap);
        let plan = plan_for(snap.snapshot_id);

        // ① 生产窗（0.05/0.95）→ MaxSoc 违约拒绝。
        let mut strict = TrustedInputCertifier::new(
            MemBlobs {
                blob: blob.clone(),
                id: snap.snapshot_id,
            },
            catalog_with_soc_window("0.05", "0.95"),
            b"chain-material",
            2,
        );
        match strict.certify(&plan.encode_v1(), horizon_ms) {
            Err(CertifyReject::Rejected(ref s)) => {
                assert!(
                    s.contains("constraint_kind: 2") || s.contains("MaxSoc"),
                    "越铭牌窗顶必须记 MaxSoc 违约（kind=2）: {s}"
                );
            }
            other => panic!("越窗计划不得出证: {other:?}"),
        }
        assert_eq!(strict.certified, 0);

        // ② 同计划 + 放宽窗 [0,1] → 出证（证明①的拒绝来自铭牌窗）。
        let mut wide = TrustedInputCertifier::new(
            MemBlobs {
                blob,
                id: snap.snapshot_id,
            },
            catalog_with_soc_window("0.0", "1.0"),
            b"chain-material",
            2,
        );
        wide.certify(&plan.encode_v1(), horizon_ms)
            .expect("放宽窗后同计划必须出证（否则①的拒绝理由不是 SOC 窗）");
        assert_eq!(wide.certified, 1);
    }

    /// 目录爬坡变体（测试构造）：铭牌两行 + 合同 `ramp_kw_per_s` 一并替换。
    fn catalog_with_ramps(nameplate: &str, contract: Option<&str>) -> CatalogRepository {
        let toml = include_str!("../../../../configs/catalog.toml");
        let mut patched = toml
            .replace(
                "ramp_up_kw_s = 3.3333",
                &alloc::format!("ramp_up_kw_s = {nameplate}"),
            )
            .replace(
                "ramp_down_kw_s = 3.3333",
                &alloc::format!("ramp_down_kw_s = {nameplate}"),
            );
        if let Some(c) = contract {
            patched = patched.replace(
                "ramp_kw_per_s = 3.3333",
                &alloc::format!("ramp_kw_per_s = {c}"),
            );
        }
        let compiled = eneros_config_bundle::toml_compile::compile_catalog(&patched)
            .expect("patched catalog compiles");
        eneros_semantic_catalog::open(&compiled.payload).expect("decode")
    }

    /// 两时段计划（步长 10s）：步 1 静置、步 2 放电 60 kW ⇒ 步间爬坡
    /// 6.0 kW/s（**严格位于合同 3.3333 与铭牌 10.0 之间**）。用于验证
    /// **合同爬坡**（运行限值）在认证面生效——铭牌（物理能力）可宽于合同。
    ///
    /// 步长取 10s 而非 60s：包络有效性 = 快照产 + 30s（`base_env`），
    /// 步起始时刻必须落在包络窗内——跨 30s 的多步计划当前无法被认证
    /// （架构面登记：见执行卡"计划时域 vs 包络生命周期"）。
    fn two_step_ramp_plan(snap_id: [u8; 16], produced_at_ns: u64) -> CandidatePlan {
        use eneros_control_dispatch::adapter::CandidateInputs;
        use eneros_decision_contract::{Assignment, CandidatePlanParts, PlanId, SnapshotId};
        let horizon_ms = produced_at_ns / 1_000_000;
        let inputs = CandidateInputs::from_snapshot(snap_id, 1, horizon_ms, 10_000_000_000, 2);
        let assignments = alloc::vec![
            Assignment {
                asset_id: 1,
                device_id: 2,
                point_id: 10,
                unit_code: 6,
                power_kw: 0.0,
                step_index: 1,
            },
            Assignment {
                asset_id: 1,
                device_id: 2,
                point_id: 10,
                unit_code: 6,
                power_kw: 60.0,
                step_index: 2,
            },
        ];
        CandidatePlan::from_parts(CandidatePlanParts {
            id: PlanId::new([0x31; 16]).expect("id"),
            snapshot_id: SnapshotId(snap_id),
            objective_digest: inputs.objective_digest,
            resource_catalog_version: 1,
            horizon_start_ms: horizon_ms,
            step_ns: 10_000_000_000,
            step_count: 2,
            solver_status: eneros_decision_contract::SolveStatusKind::Optimal,
            objective_value: 0.0,
            relative_gap: 0.0,
            max_primal_residual: 0.0,
            elapsed_ms: 1,
            model_digest: [0x11; 32],
            input_digest: [0x22; 32],
            assignments,
            issued_at_ms: horizon_ms,
            ttl_ms: 3_600_000,
        })
        .expect("two-step plan")
    }

    /// 复查修复：**合同爬坡（运行限值）在认证面生效**，双面对照——
    /// ① 铭牌宽（10 kW/s）而合同窄（3.3333 kW/s）：8.333 kW/s 的步间爬坡
    ///    必须被独立重算拒绝（RampUp，kind=5）；
    /// ② 同一计划在「铭牌=合同=10」的目录下出证——证明①的拒绝来自合同
    ///    收紧项而不是别的约束。
    ///
    /// 变异靶：`ResourceRegistry::evaluate` 不把合同 `ramp_kw_per_s` 求交
    /// 进包络（只收紧功率窗）→ ①变绿（越合同爬坡被出证）→ 本测红；而执行
    /// 侧 `ramp-limit`（同合同值）会随后拒绝该命令 = 证书证据与执行不一致。
    #[test]
    fn certification_rejects_ramp_beyond_contract_not_nameplate() {
        let produced_at_ns = 1_700_000_000_000u64 * 1_000_000;
        let snap = fixture_snapshot(produced_at_ns);
        let blob = eneros_snapshot_service::encode_snapshot_v1(&snap);
        let plan = two_step_ramp_plan(snap.snapshot_id, produced_at_ns);
        let now_ms = produced_at_ns / 1_000_000;

        // ① 铭牌 10 / 合同 3.3333 → 拒（越合同爬坡）。
        let mut narrow_contract = TrustedInputCertifier::new(
            MemBlobs {
                blob: blob.clone(),
                id: snap.snapshot_id,
            },
            catalog_with_ramps("10.0", Some("3.3333")),
            b"chain-material",
            2,
        );
        match narrow_contract.certify(&plan.encode_v1(), now_ms) {
            Err(CertifyReject::Rejected(ref s)) => assert!(
                s.contains("constraint_kind: 5") || s.contains("RampUp"),
                "越合同爬坡必须记 RampUp 违约（kind=5）: {s}"
            ),
            other => panic!("越合同爬坡不得出证: {other:?}"),
        }
        assert_eq!(narrow_contract.certified, 0);

        // ② 铭牌 10 / 合同 10 → 同计划出证（证明①的拒绝来自合同收紧）。
        let mut wide_contract = TrustedInputCertifier::new(
            MemBlobs {
                blob,
                id: snap.snapshot_id,
            },
            catalog_with_ramps("10.0", Some("10.0")),
            b"chain-material",
            2,
        );
        wide_contract
            .certify(&plan.encode_v1(), now_ms)
            .expect("合同放宽后同计划必须出证（否则①的拒绝理由不是合同爬坡）");
        assert_eq!(wide_contract.certified, 1);
    }

    /// 目录 SOC 窗变体（测试构造；只替换铭牌窗两行）。
    fn catalog_with_soc_window(soc_min: &str, soc_max: &str) -> CatalogRepository {
        let toml = include_str!("../../../../configs/catalog.toml");
        let patched = toml
            .replace("soc_min = 0.05", &alloc::format!("soc_min = {soc_min}"))
            .replace("soc_max = 0.95", &alloc::format!("soc_max = {soc_max}"));
        let compiled = eneros_config_bundle::toml_compile::compile_catalog(&patched)
            .expect("patched catalog compiles");
        eneros_semantic_catalog::open(&compiled.payload).expect("decode")
    }

    /// 快照 SOC 观测改写（负例构造用）。
    fn set_soc(snap: &mut eneros_snapshot_service::TrustedSnapshot, soc: f64) {
        use eneros_telemetry_model::event::ObservationValue;
        for o in snap.observations.iter_mut() {
            if o.event.point_id == 4 {
                o.event.value = ObservationValue::F64(soc);
            }
        }
        snap.derived.clear();
    }

    /// 轮换/退役语义（WP-70.9.3）：退役后同 key_id 不可再验。
    #[test]
    fn rotate_and_retire_lifecycle() {
        let catalog = catalog();
        let mut c = TrustedInputCertifier::new(
            MemBlobs {
                blob: alloc::vec::Vec::new(),
                id: [9u8; 16],
            },
            catalog,
            b"chain-material",
            2,
        );
        assert_eq!(c.ring().active().map(|(id, ..)| id), Some(1));
        c.rotate(2, b"next-material").expect("rotate");
        assert_eq!(c.ring().active().map(|(id, ..)| id), Some(2));
        // 前代仍在 N-1 窗口内（旧证书可验）。
        assert!(c.ring().key(1).is_some());
        assert!(c.retire(1), "退役前代");
        assert!(c.ring().key(1).is_none(), "退役立即失效");
    }

    /// WP-70.9.3 生命周期**接到执行准入**：认证者自带的环是验真权威，
    /// 退役立即让已签发证书在验真面失效（不是"下次重启才失效"）。
    ///
    /// 变异靶：验真改用引擎侧独立拷贝环（即不读来源环）→ 退役后旧证书
    /// 仍验过 → 本测红。
    #[test]
    fn retired_key_invalidates_issued_certificate_at_verify() {
        let produced_at_ns = 1_700_000_000_000u64 * 1_000_000;
        let snap = fixture_snapshot(produced_at_ns);
        let blob = eneros_snapshot_service::encode_snapshot_v1(&snap);
        let mut c = TrustedInputCertifier::new(
            MemBlobs {
                blob,
                id: snap.snapshot_id,
            },
            catalog(),
            b"chain-material",
            2,
        );
        let plan = bound_plan_for(snap.snapshot_id, 1, produced_at_ns);
        let wire = plan.encode_v1();
        let reply = c.certify(&wire, 1_700_000_000_000).expect("出证");
        // 验真以来源自带的环为权威 → 通过。
        let ring = c.verifier_ring().expect("来源必须自带环");
        eneros_control_dispatch::certify::verify_reply(
            &plan,
            &wire,
            &reply,
            ring,
            1_700_000_000_000,
        )
        .expect("退役前可通过");
        // 退役发放 key → 同一证书在验真面立即失效。
        assert!(c.retire(1), "退役");
        let ring = c.verifier_ring().expect("来源必须自带环");
        assert!(
            eneros_control_dispatch::certify::verify_reply(
                &plan,
                &wire,
                &reply,
                ring,
                1_700_000_000_000
            )
            .is_err(),
            "退役后同证书必须零投递（退役立即失效，需接到验方）"
        );
    }
}

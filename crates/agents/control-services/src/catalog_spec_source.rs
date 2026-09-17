//! 蓝图 70.10 WP-70.10.2——**目录派生结构化计划输入源**（生产实现）。
//!
//! host 侧 planner 不得再凭 host 自持 fixture 建模（那是 70.10 明确要拆掉
//! 的形态）；请求携带的**目标侧事实**由本模块从同代目录装配：
//!
//! | 请求字段 | 目录来源 |
//! |---|---|
//! | `capacity_kwh`/`eta_c`/`eta_d`/`soc_min`/`soc_max`/功率窗/爬坡 | 铭牌（`nameplate`） |
//! | `p_min_kw`/`p_max_kw` 最终值 | 铭牌 ∩ 控制合同（合同是收紧项） |
//! | `device_id`/`point_id`/`unit_code` | 控制点表绑定 + 控制合同 |
//! | `initial_kwh` | 铭牌容量 × 快照 SOC（每 tick 现算） |
//! | 价格序列 | 注入的报价（`prices_per_kwh`；缺省 = 无价格信号时拒绝发请求） |
//!
//! 缺铭牌/缺合同的资产**不进请求**（不可控——与 registry 可控池同一判据），
//! 且请求资产集合为空时返回 `None`（不发出无输入源的结构化请求）。

use alloc::string::String;
use alloc::vec::Vec;

use eneros_intent_contract::peer::PeerAssetSpec;

use crate::remote_brain::{PlanSpecSource, PlanSpecs};

/// 目录派生输入源（同代目录 + 注入的报价/时域）。
pub struct CatalogPlanSpecSource {
    repo: eneros_semantic_catalog::CatalogRepository,
    prices_per_kwh: Vec<f64>,
    objective_kind: String,
    step_ns: u64,
    step_count: u32,
    /// 本机可接受的功率幅值上界（kW；ai-peer `max_power_kw`——dispatch
    /// 守卫的值域）。请求包络必须先收进预算内（见 `specs`）。
    budget_kw: f64,
    blobs: alloc::boxed::Box<dyn crate::certifier_service::SnapshotBlobSource>,
    bindings: Vec<(u64, u32, u32)>,
}

impl CatalogPlanSpecSource {
    /// 装配（目录派生的版本组是契约面单源；价格/时域为站点侧注入）。
    pub fn new(
        repo: eneros_semantic_catalog::CatalogRepository,
        prices_per_kwh: Vec<f64>,
        objective_kind: &str,
        step_ns: u64,
        step_count: u32,
        budget_kw: f64,
        blobs: alloc::boxed::Box<dyn crate::certifier_service::SnapshotBlobSource>,
    ) -> Self {
        Self {
            repo,
            prices_per_kwh,
            objective_kind: String::from(objective_kind),
            step_ns,
            step_count,
            budget_kw,
            blobs,
            bindings: Vec::new(),
        }
    }

    /// Same point-table control bindings used by dispatch. Missing or ambiguous
    /// bindings disable planning; observation gateway IDs are never write targets.
    pub fn with_control_bindings(mut self, bindings: Vec<(u64, u32, u32)>) -> Self {
        self.bindings = bindings;
        self
    }

    /// 目录合同版本（与快照身份携带的代次比对用）。
    pub fn contract_version(&self) -> u64 {
        use eneros_semantic_catalog::SemanticCatalog as _;
        self.repo.version_set().contract_version
    }
}

impl PlanSpecSource for CatalogPlanSpecSource {
    fn specs(
        &mut self,
        facts: &eneros_agent_sdk::snap_input_wire::SnapshotFacts,
        soc_pct: f64,
    ) -> Option<PlanSpecs> {
        use eneros_semantic_catalog::SemanticCatalog as _;
        // 代次不符 = 目录与快照不同代 ⇒ 不发请求（fail-closed；宁可不决策）。
        if self.repo.version_set().contract_version != facts.catalog_version {
            eneros_runtime::println!(
                "[svc-ai-worker] PLAN-SPEC-SKIP reason=catalog-generation-mismatch plan={} facts={}",
                self.repo.version_set().contract_version,
                facts.catalog_version
            );
            return None;
        }
        if !soc_pct.is_finite() || self.prices_per_kwh.len() != self.step_count as usize {
            eneros_runtime::println!(
                "[svc-ai-worker] PLAN-SPEC-SKIP reason=bad-soc-or-prices soc={soc_pct} prices={} steps={}",
                self.prices_per_kwh.len(),
                self.step_count
            );
            return None;
        }
        let blob = self.blobs.blob(&facts.snapshot_id).ok()?;
        let snapshot = eneros_snapshot_service::decode_snapshot_v1(&blob).ok()?;
        if snapshot.snapshot_id != facts.snapshot_id
            || snapshot.provenance_root != facts.provenance_root
            || snapshot.cut_time_ns != facts.cut_time_ns
            || snapshot.produced_at_ns != facts.produced_at_ns
            || !snapshot.completeness.is_complete()
        {
            return None;
        }
        let vs = self.repo.version_set();
        let mut assets: Vec<PeerAssetSpec> = Vec::new();
        let mut grid_asset_id = 0u64;
        for asset_id in self.repo.asset_ids() {
            let Some(np) = self.repo.nameplate(asset_id) else {
                continue; // 无铭牌 = 不可控
            };
            let Some(contract) = self
                .repo
                .asset_control_contract(asset_id, vs.contract_version)
            else {
                continue; // 无合同 = 不可控
            };
            let mut bound = self.bindings.iter().filter(|(id, _, _)| *id == asset_id);
            let &(_, device, point) = bound.next()?;
            if bound.next().is_some() || point != contract.point_id || device == 0 {
                return None;
            }
            let device = u16::try_from(device).ok()?;
            // 合同是收紧项：功率窗 = 铭牌 ∩ 合同 ∩ **本机预算**。
            //
            // 预算（ai-peer `max_power_kw`）是 dispatch 守卫的值域：请求
            // 包络若宽于预算，host LP 的最优解必然越预算 → host 按"越预算
            // 即拒（契约无钳制通路）"拒绝应答 → 目标端每周期超时降级 =
            // v2 通道自阻塞（复查修复）。**收窄请求**是正当的（请求事实
            // 必须落在本机可执行域内）；**应答**越预算仍按契约拒绝，
            // 不引入钳制通路。
            let budget = if self.budget_kw.is_finite() && self.budget_kw > 0.0 {
                self.budget_kw
            } else {
                // 预算不可用（非有限/≤0）= 装配错误 → 不发请求（fail-closed）。
                eneros_runtime::println!(
                    "[svc-ai-worker] PLAN-SPEC-SKIP reason=budget-unusable budget={}",
                    self.budget_kw
                );
                return None;
            };
            let p_min = np.p_min_kw.max(contract.min).max(-budget);
            let p_max = np.p_max_kw.min(contract.max).min(budget);
            if p_min > p_max {
                continue; // 交空 = 不可控（含预算比功率窗更紧到空的情形）
            }
            let ramp = np
                .ramp_up_kw_s
                .min(np.ramp_down_kw_s)
                .min(contract.ramp_kw_per_s);
            let soc = snapshot.asset_soc(&self.repo, asset_id)?;
            assets.push(PeerAssetSpec {
                asset_id,
                capacity_kwh: np.capacity_kwh,
                eta_c: np.eta_c,
                eta_d: np.eta_d,
                initial_kwh: np.capacity_kwh * soc,
                soc_min: np.soc_min,
                soc_max: np.soc_max,
                p_min_kw: p_min,
                p_max_kw: p_max,
                ramp_up_kw_s: ramp,
                ramp_down_kw_s: ramp,
                device_id: device,
                point_id: contract.point_id,
                unit_code: contract.unit_code,
            });
        }
        // 并网点（站点级约束锚；无铭牌/无合同，仅作 PCC 参考）。
        for asset_id in self.repo.asset_ids() {
            if self
                .repo
                .asset(asset_id, vs.asset_version)
                .map(|a| {
                    a.kind == eneros_config_bundle::schema::catalog_kinds::ASSET_GRID_CONNECTION
                })
                .unwrap_or(false)
            {
                grid_asset_id = asset_id;
                break;
            }
        }
        if assets.is_empty() {
            eneros_runtime::println!(
                "[svc-ai-worker] PLAN-SPEC-SKIP reason=no-controllable-assets（不发结构化请求）"
            );
            return None;
        }
        // 分配条数上限（与契约上限同源判据：资产 × 时域）。
        if assets.len() as u64 * u64::from(self.step_count)
            > eneros_intent_contract::peer::PEER_PLAN_MAX_ASSIGNMENTS as u64
        {
            eneros_runtime::println!(
                "[svc-ai-worker] PLAN-SPEC-SKIP reason=plan-over-cap assets={} steps={}",
                assets.len(),
                self.step_count
            );
            return None;
        }
        Some(PlanSpecs {
            assets,
            grid_asset_id,
            objective_kind: self.objective_kind.clone(),
            prices_per_kwh: self.prices_per_kwh.clone(),
            step_ns: self.step_ns,
            step_count: self.step_count,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use eneros_agent_sdk::snap_input_wire::{SnapshotFacts, COMPLETE};

    use super::*;

    fn repo() -> eneros_semantic_catalog::CatalogRepository {
        let toml = include_str!("../../../../configs/catalog.toml");
        let compiled = eneros_config_bundle::toml_compile::compile_catalog(toml).expect("compile");
        eneros_semantic_catalog::open(&compiled.payload).expect("decode")
    }

    struct Blobs(eneros_snapshot_service::TrustedSnapshot);
    impl crate::certifier_service::SnapshotBlobSource for Blobs {
        fn blob(&mut self, _: &[u8; 16]) -> Result<Vec<u8>, String> {
            Ok(eneros_snapshot_service::encode_snapshot_v1(&self.0))
        }
    }
    fn blobs() -> alloc::boxed::Box<dyn crate::certifier_service::SnapshotBlobSource> {
        use eneros_semantic_catalog::SemanticCatalog;
        let f = facts(1);
        let observations = eneros_agent_sdk::synth_feed::SYNTH_FEED
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let mut e = eneros_agent_sdk::synth_feed::synth_event(p, 0, i, f.produced_at_ns);
                if e.point_id == 4 {
                    e.value = eneros_telemetry_model::event::ObservationValue::F64(0.6);
                }
                e.into()
            })
            .collect();
        alloc::boxed::Box::new(Blobs(eneros_snapshot_service::TrustedSnapshot {
            snapshot_id: f.snapshot_id,
            tenant_id: 1,
            site_id: 1,
            cut_time_ns: f.cut_time_ns,
            produced_at_ns: f.produced_at_ns,
            policy_version: 1,
            version_set: repo().version_set(),
            observations,
            source_watermarks: vec![],
            conflicts: vec![],
            derived: vec![],
            completeness: eneros_snapshot_service::CompletenessReport {
                required: vec![],
                accepted: vec![],
                missing: vec![],
                stale: vec![],
                bad: vec![],
                conflict: vec![],
                substituted: vec![],
            },
            provenance_root: f.provenance_root,
        }))
    }

    fn facts(catalog_version: u64) -> SnapshotFacts {
        SnapshotFacts {
            snapshot_id: [0x5A; 16],
            provenance_root: [0x3D; 32],
            cut_time_ns: 1_700_000_000_000_000_000,
            produced_at_ns: 1_700_000_000_000_000_000,
            catalog_version,
            catalog_digest8: [1; 8],
            accepted: 7,
            missing: 0,
            completeness: COMPLETE,
        }
    }

    /// 目录 → 规格：铭牌 + 合同求交（功率窗/爬坡）、绑定来自资产与合同、
    /// 初始能量 = 容量 × SOC；并网点识别。蓝图 70 R10：双资产（PCS-1 +
    /// PCS-2）均进请求。
    #[test]
    fn specs_derive_from_catalog_nameplate_and_contract() {
        let mut src = CatalogPlanSpecSource::new(
            repo(),
            vec![0.5, 1.5],
            "max_revenue",
            60_000_000_000,
            2,
            1_000.0,
            blobs(),
        )
        .with_control_bindings(vec![(1, 1, 10), (2, 8, 1009)]);
        let specs = src.specs(&facts(1), 0.6).expect("规格");
        assert_eq!(specs.assets.len(), 2, "双电池资产均有铭牌+合同");
        // asset 1（PCS-1，100 kWh）。
        let a = &specs.assets[0];
        assert_eq!(a.asset_id, 1);
        assert_eq!(a.capacity_kwh, 100.0);
        assert!((a.initial_kwh - 60.0).abs() < 1e-9, "初始能量 = 容量 × SOC");
        assert_eq!(a.soc_min, 0.05);
        assert_eq!(a.soc_max, 0.95);
        assert_eq!(a.p_min_kw, -500.0);
        assert_eq!(a.p_max_kw, 500.0);
        assert_eq!(
            a.device_id, 1,
            "写目标必须来自点表，不能使用观测网关设备 ID"
        );
        assert_eq!(a.point_id, 10, "点绑定来自控制合同");
        assert_eq!(
            a.unit_code,
            eneros_telemetry_model::unit_catalog::code_of(
                eneros_telemetry_model::PhysicalUnit::Kilowatts
            )
        );
        // asset 2（PCS-2，50 kWh；蓝图 70 R10 多资产）。
        let b = &specs.assets[1];
        assert_eq!(b.asset_id, 2);
        assert_eq!(b.capacity_kwh, 50.0);
        assert!((b.initial_kwh - 15.0).abs() < 1e-9);
        assert_eq!(b.p_min_kw, -500.0);
        assert_eq!(b.p_max_kw, 500.0);
        assert_eq!(b.device_id, 8, "设备绑定来自控制点表");
        assert_eq!(b.point_id, 1009, "点绑定来自控制合同");
        assert_eq!(specs.grid_asset_id, 10, "并网点识别");
    }

    /// 代次不符 / 无价格 / SOC 非法 → 不发请求（None）。
    /// 变异靶：去掉代次判据 → 跨代规格被发出（host 拿旧目录建模）→ 本测红。
    #[test]
    fn specs_fail_closed_on_generation_or_input_mismatch() {
        let mut src = CatalogPlanSpecSource::new(
            repo(),
            vec![0.5, 1.5],
            "max_revenue",
            60_000_000_000,
            2,
            1_000.0,
            blobs(),
        )
        .with_control_bindings(vec![(1, 1, 10), (2, 8, 1009)]);
        assert!(src.specs(&facts(2), 0.6).is_none(), "跨代目录必须拒");
        assert!(
            src.specs(&facts(1), f64::NAN).is_none(),
            "非有限 SOC 必须拒"
        );
        let mut bad_prices = CatalogPlanSpecSource::new(
            repo(),
            vec![0.5],
            "max_revenue",
            60_000_000_000,
            2,
            1_000.0,
            blobs(),
        )
        .with_control_bindings(vec![(1, 1, 10), (2, 8, 1009)]);
        assert!(
            bad_prices.specs(&facts(1), 0.6).is_none(),
            "价格长度不符必须拒"
        );
    }

    /// 复查修复：**预算收窄请求包络**（ai-peer `max_power_kw` = dispatch
    /// 守卫值域）。预算宽于功率窗时不变；窄于功率窗时请求窗 = ±预算
    /// （否则 host LP 最优解必越预算 → host 按"越预算即拒"不应答 →
    /// 目标每周期超时降级 = v2 通道自阻塞）；预算不可用（≤0/非有限）
    /// → 不发请求（fail-closed）。
    ///
    /// 变异靶：去掉预算收窄 → 本测红（p_max 仍为 500）。
    #[test]
    fn specs_clamp_window_to_local_budget() {
        let mut src = CatalogPlanSpecSource::new(
            repo(),
            vec![0.5, 1.5],
            "max_revenue",
            60_000_000_000,
            2,
            5.0,
            blobs(),
        )
        .with_control_bindings(vec![(1, 1, 10), (2, 8, 1009)]);
        let specs = src.specs(&facts(1), 0.6).expect("规格");
        assert_eq!(specs.assets[0].p_min_kw, -5.0, "请求窗收进预算");
        assert_eq!(specs.assets[0].p_max_kw, 5.0);
        // 预算不可用 ⇒ 无输入源（不发请求）。
        let mut bad = CatalogPlanSpecSource::new(
            repo(),
            vec![0.5, 1.5],
            "max_revenue",
            60_000_000_000,
            2,
            0.0,
            blobs(),
        )
        .with_control_bindings(vec![(1, 1, 10), (2, 8, 1009)]);
        assert!(bad.specs(&facts(1), 0.6).is_none(), "预算 ≤0 必须拒");
    }
    #[test]
    fn missing_or_ambiguous_control_binding_never_falls_back_to_gateway() {
        for bindings in [
            vec![],
            vec![(1, 1, 999), (2, 8, 1009)],
            vec![(1, 1, 10), (1, 2, 10), (2, 8, 1009)],
        ] {
            let mut src = CatalogPlanSpecSource::new(
                repo(),
                vec![1.0],
                "max_revenue",
                1_000_000_000,
                1,
                50.0,
                blobs(),
            )
            .with_control_bindings(bindings);
            assert!(src.specs(&facts(1), 0.6).is_none());
        }
    }
}
